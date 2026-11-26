# /// script
# requires-python = ">=3.10"
# dependencies = ["httpx==0.28.1"]
# ///
"""Fox stdio host for the vendored Octop platform adapters."""
import importlib
import json
import os
import sys

ADAPTERS = {
    "qq-mail": "qq_mail", "netease-mail": "qq_mail", "ima": "tencent_ima",
    "tencent-news": "tencent_news", "weread": "wechat_reading", "qqmusic": "qq_music",
    "ctrip": "ctrip_wendao", "meituan": "meituan_travel", "baidu-maps": "baidu_map", "weknora": "weknora",
    "zendesk": "zendesk", "whatsapp": "whatsapp_twilio", "fliggy": "fliggy_cli",
}

READ_ONLY = {
    "search_place", "plan_direction", "get_weather", "search_music", "list_charts", "get_chart_detail",
    "get_playlist_detail", "listening_report", "list_notebooks", "list_notes", "get_note", "search_notes",
    "list_knowledge_bases", "list_addable_knowledge_bases", "get_knowledge_base", "list_knowledge", "search_knowledge",
    "get_media", "search_news", "list_bookshelf", "get_book_notes", "search", "read_document",
    "search_emails", "read_email",
}

def main():
    sys.stdin.reconfigure(encoding="utf-8")
    sys.stdout.reconfigure(encoding="utf-8")
    kind = sys.argv[1]
    adapter = importlib.import_module("adapters." + ADAPTERS[kind])
    creds = {key: os.environ["FOX_" + key.upper()] for key in ("api_key", "client_id", "base_url", "email", "password",
             "subdomain", "token", "phone", "account_sid", "flyai_entry")
             if "FOX_" + key.upper() in os.environ}
    if kind in ("qq-mail", "netease-mail"):
        domain = creds["email"].rsplit("@", 1)[-1].lower()
        if domain not in ({"qq.com", "foxmail.com"} if kind == "qq-mail" else {"163.com", "126.com", "yeah.net"}):
            raise ValueError("邮箱账号与所选服务不匹配")
        host = "qq.com" if kind == "qq-mail" else domain
        creds.update(imap_host="imap." + host, smtp_host="smtp." + host)
    tools = adapter.list_tools()
    for tool in tools:
        if tool["name"] in READ_ONLY:
            tool["annotations"] = {"readOnlyHint": True, "destructiveHint": False}
    for line in sys.stdin:
        message = json.loads(line)
        if "id" not in message:
            continue
        reply = {"jsonrpc": "2.0", "id": message["id"]}
        method = message.get("method")
        if method == "initialize":
            reply["result"] = {"protocolVersion": "2025-03-26", "capabilities": {"tools": {}},
                               "serverInfo": {"name": "fox-" + kind, "version": "1.0"}}
        elif method == "ping":
            reply["result"] = {}
        elif method == "tools/list":
            reply["result"] = {"tools": tools}
        elif method == "tools/call":
            try:
                params = message["params"]
                result = adapter.call_tool(creds, params["name"], params.get("arguments", {}))
                reply["result"] = {"content": [{"type": "text", "text": result}]}
            except Exception as error:
                # SDK exceptions may include a request URL with a vendor credential.
                reply["result"] = {"isError": True, "content": [{"type": "text", "text": "调用失败，请核对账号、权限和参数 (" + type(error).__name__ + ")"}]}
        else:
            reply["error"] = {"code": -32601, "message": "Method not found"}
        print(json.dumps(reply, ensure_ascii=False), flush=True)

if __name__ == "__main__":
    main()
