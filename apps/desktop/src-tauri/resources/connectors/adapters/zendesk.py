import json
import re
import httpx


def list_tools():
    return [{"name": "search_tickets", "description": "在 Zendesk 中搜索工单",
             "annotations": {"readOnlyHint": True, "destructiveHint": False},
             "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}]


def call_tool(creds, name, args):
    if name != "search_tickets" or not re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9-]{0,62}", creds["subdomain"]):
        raise ValueError("工单工具或 Zendesk 子域无效")
    response = httpx.get("https://" + creds["subdomain"] + ".zendesk.com/api/v2/search.json",
                         params={"query": "type:ticket " + args["query"], "per_page": 20},
                         headers={"Authorization": "Bearer " + creds["token"]})
    response.raise_for_status()
    return json.dumps(response.json(), ensure_ascii=False)
