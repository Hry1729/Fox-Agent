from pathlib import Path
import subprocess


def list_tools():
    return [{"name": "travel_search", "description": "搜索飞猪机票、酒店和景点",
             "annotations": {"readOnlyHint": True, "destructiveHint": False},
             "inputSchema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}}]


def call_tool(creds, name, args):
    if name != "travel_search":
        raise ValueError("未知工具")
    entry = Path(creds["flyai_entry"])
    if not entry.is_absolute() or not entry.is_file():
        raise ValueError("需要指定已安装的飞猪 CLI 文件")
    if entry.suffix.lower() in (".js", ".mjs", ".cjs"):
        command = ["node", str(entry)]
    elif entry.suffix.lower() == ".exe" or (not entry.suffix and entry.name == "flyai"):
        command = [str(entry)]
    else:
        raise ValueError("请选择飞猪 CLI 的程序或 JavaScript 入口文件")
    result = subprocess.run([*command, "fliggy-fast-search", "--query", args["query"]],
                            capture_output=True, encoding="utf-8", errors="replace", timeout=60, check=True)
    return result.stdout
