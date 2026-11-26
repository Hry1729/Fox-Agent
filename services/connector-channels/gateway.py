# /// script
# requires-python = ">=3.12"
# dependencies = ["octop-gateway==1.0.4", "mcp>=1.12,<2", "uvicorn>=0.30,<1"]
# ///
"""One loopback MCP endpoint for one platform bot."""
import argparse
from collections import deque
from contextlib import asynccontextmanager
import json
import os
import secrets
import uuid

from mcp.server.fastmcp import FastMCP
from mcp.types import ToolAnnotations
from octop_gateway import ChannelManager, MediaBackend, MessageEvent
from starlette.responses import JSONResponse
import uvicorn


class TextOnly(MediaBackend):
    async def save(self, data, key):
        pass

    async def read(self, key):
        raise FileNotFoundError(key)

    async def exists(self, key):
        return False


def application(platform, config, token):
    inbox = deque(maxlen=200)

    async def receive(message):
        if message.channel_subject and message.text:
            inbox.append((uuid.uuid4().hex, message.timestamp, message.text, message.channel_subject))
        yield MessageEvent.completed()

    manager = ChannelManager(processor=receive, media_backend=TextOnly())

    @asynccontextmanager
    async def connection(_server):
        await manager.start()
        try:
            await manager.add_channel(channel_type=platform, config=config, channel_id=platform)
            yield {}
        finally:
            await manager.stop()

    if not token.strip():
        raise ValueError("需要为 Fox 消息网关设置访问令牌")
    server = FastMCP("Fox " + platform, json_response=True)

    @server.tool(annotations=ToolAnnotations(readOnlyHint=True, destructiveHint=False))
    def recent_messages(limit: int = 20) -> list[dict]:
        """列出网关本次运行期间收到的文本，最新消息排在前面。"""
        return [{"id": key, "time": timestamp, "from": subject.display_name or subject.subject_id, "text": text}
                for key, timestamp, text, subject in reversed(list(inbox)[-max(1, min(limit, 200)):])]

    @server.tool(annotations=ToolAnnotations(readOnlyHint=False, destructiveHint=False, idempotentHint=False))
    async def reply(message_id: str, text: str) -> str:
        """回复指定收件记录所在的原会话。"""
        subject = next((record[3] for record in inbox if record[0] == message_id), None)
        if subject is None or not text.strip():
            raise ValueError("需要有效的收件记录和回复文本")
        await manager.push_text(platform, subject, text)
        return "已调用平台回复接口"

    # A FastMCP lifespan runs per MCP session. Keep the IM subscription alive
    # for the host lifetime, including intervals with no connected Fox session.
    inner = server.streamable_http_app()
    previous = inner.router.lifespan_context

    @asynccontextmanager
    async def lifespan(app):
        async with connection(server), previous(app):
            yield

    inner.router.lifespan_context = lifespan

    async def authenticated(scope, recv, send):
        if scope["type"] == "http" and not secrets.compare_digest(
                dict(scope["headers"]).get(b"authorization", b""), b"Bearer " + token.encode("ascii")):
            return await JSONResponse({"error": "unauthorized"}, status_code=401)(scope, recv, send)
        await inner(scope, recv, send)

    return authenticated


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("platform", choices=("qq", "weixin", "dingtalk", "feishu", "wecom", "telegram", "discord", "yuanbao", "mqtt"))
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    app = application(args.platform, json.loads(os.environ["FOX_CHANNEL_CONFIG"]), os.environ["FOX_CHANNEL_TOKEN"])
    uvicorn.run(app, host="127.0.0.1", port=args.port, access_log=False, lifespan="on")
