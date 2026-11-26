import json
import re
import httpx


def list_tools():
    return [
        {"name": "inbox", "description": "读取企业 WhatsApp 号码最近收到的 20 条消息",
         "annotations": {"readOnlyHint": True, "destructiveHint": False}, "inputSchema": {"type": "object", "properties": {}}},
        {"name": "reply", "description": "在服务窗口内回复指定的 WhatsApp 联系人",
         "annotations": {"readOnlyHint": False, "idempotentHint": False}, "inputSchema": {"type": "object",
         "properties": {"phone": {"type": "string", "description": "带国际区号的号码"}, "text": {"type": "string"}},
         "required": ["phone", "text"]}},
    ]


def address(number):
    if not re.fullmatch(r"\+[1-9][0-9]{5,14}", number):
        raise ValueError("请填写带国际区号的 WhatsApp 号码")
    return "whatsapp:" + number


def call_tool(creds, name, args):
    sid = creds["account_sid"]
    if not re.fullmatch(r"AC[0-9a-fA-F]{32}", sid):
        raise ValueError("Twilio Account SID 无效")
    url = "https://api.twilio.com/2010-04-01/Accounts/" + sid + "/Messages.json"
    sender = address(creds["phone"])
    auth = (sid, creds["token"])
    if name == "inbox":
        response = httpx.get(url, params={"To": sender, "PageSize": 20}, auth=auth)
    elif name == "reply":
        response = httpx.post(url, data={"From": sender, "To": address(args["phone"]), "Body": args["text"]}, auth=auth)
    else:
        raise ValueError("未知工具")
    response.raise_for_status()
    data = response.json()
    if name == "reply":
        data = {"sid": data["sid"], "status": data["status"]}
    else:
        data = [{key: row.get(key) for key in ("sid", "from", "body", "date_sent")} for row in data["messages"]]
    return json.dumps(data, ensure_ascii=False)
