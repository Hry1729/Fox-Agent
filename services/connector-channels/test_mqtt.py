# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = ["amqtt==0.11.3", "octop-gateway==1.0.4", "mcp>=1.12,<2", "uvicorn>=0.30,<1"]
# ///
"""Exercise the Fox gateway with a loopback broker and a real MQTT device client."""
import asyncio
import json
import socket

from amqtt.broker import Broker
from amqtt.client import MQTTClient
import httpx
from mcp import ClientSession
from mcp.client.streamable_http import streamablehttp_client
import uvicorn
from gateway import application


async def main():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    broker = Broker({"listeners": {"default": {"type": "tcp", "bind": f"127.0.0.1:{port}"}},
                     "plugins": {"amqtt.plugins.authentication.AnonymousAuthPlugin": {"allow_anonymous": True}}})
    await broker.start()
    device = MQTTClient()
    service = uvicorn.Server(uvicorn.Config(application("mqtt", {"host": "127.0.0.1", "port": port, "tls_enabled": False},
                                                  "fixture-token"), host="127.0.0.1", port=0, log_level="warning", lifespan="on"))
    task = asyncio.create_task(service.serve())
    try:
        while not service.started:
            if task.done():
                await task
                raise RuntimeError("网关启动失败")
            await asyncio.sleep(0.05)
        endpoint = "http://127.0.0.1:" + str(service.servers[0].sockets[0].getsockname()[1]) + "/mcp"
        async with httpx.AsyncClient() as http:
            assert (await http.get(endpoint)).status_code == 401
        await device.connect(f"mqtt://127.0.0.1:{port}/")
        await device.subscribe([("devices/fixture/out", 1)])
        async with streamablehttp_client(endpoint, headers={"Authorization": "Bearer fixture-token"}) as (reader, writer, _):
            async with ClientSession(reader, writer) as session:
                await session.initialize()
                assert {tool.name for tool in (await session.list_tools()).tools} == {"recent_messages", "reply"}
                await device.publish("devices/fixture/in", b"local test", qos=1)
                for _ in range(50):
                    result = await session.call_tool("recent_messages", {})
                    records = result.structuredContent["result"]
                    if records:
                        break
                    await asyncio.sleep(0.1)
                assert records[0]["text"] == "local test", json.dumps(records)
                result = await session.call_tool("reply", {"message_id": records[0]["id"], "text": "local reply"})
                assert not result.isError
                reply = await asyncio.wait_for(device.deliver_message(), 5)
                assert reply.data == b"local reply"
        print("MQTT 收件、MCP 鉴权、按原会话回复通过")
    finally:
        await device.disconnect()
        service.should_exit = True
        await task
        await broker.shutdown()


if __name__ == "__main__":
    asyncio.run(asyncio.wait_for(main(), 30))
