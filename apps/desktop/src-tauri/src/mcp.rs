use crate::database::McpServerRecord;
use keyring::Entry;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const CREDENTIAL_SERVICE: &str = "com.fox.agent.mcp";
#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_TOOLS: usize = 200;

pub fn environment_configured(server_id: &str) -> bool {
    load_environment(server_id).is_some()
}

pub fn save_environment(
    server_id: &str,
    environment: &HashMap<String, String>,
) -> Result<(), String> {
    let value = serde_json::to_string(environment).map_err(|error| error.to_string())?;
    credential_entry(server_id)?
        .set_password(&value)
        .map_err(|error| error.to_string())
}

pub fn clear_environment(server_id: &str) -> Result<(), String> {
    match credential_entry(server_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

pub fn list_tools(server: &McpServerRecord) -> Result<Vec<Value>, String> {
    let mut session = McpSession::start(server)?;
    session.initialize()?;
    let response = session.request("tools/list", json!({}))?;
    let tools = response
        .get("tools")
        .and_then(Value::as_array)
        .ok_or_else(|| "MCP tools/list 响应缺少 tools 数组".to_owned())?;
    if tools.len() > MAX_TOOLS {
        return Err(format!("MCP 工具数量超过 {MAX_TOOLS} 项限制"));
    }
    tools
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "MCP 工具缺少名称".to_owned())?;
            let schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            validate_input_schema(name, &schema)?;
            Ok(json!({
                "name": name,
                "description": tool.get("description").and_then(Value::as_str).unwrap_or_default(),
                "inputSchema": schema,
            }))
        })
        .collect()
}

pub fn call_tool(server: &McpServerRecord, tool: &str, arguments: &Value) -> Result<Value, String> {
    let tools = list_tools(server)?;
    if !tools
        .iter()
        .any(|item| item.get("name").and_then(Value::as_str) == Some(tool))
    {
        return Err(format!("MCP Server 未声明工具 {tool}"));
    }
    let mut session = McpSession::start(server)?;
    session.initialize()?;
    session.request(
        "tools/call",
        json!({ "name": tool, "arguments": arguments }),
    )
}

struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    receiver: mpsc::Receiver<Result<String, String>>,
    next_id: u64,
}

impl McpSession {
    fn start(server: &McpServerRecord) -> Result<Self, String> {
        if !server.enabled {
            return Err("MCP Server 已停用".to_owned());
        }
        let mut command = Command::new(&server.command);
        command.args(&server.args);
        if let Some(environment) = load_environment(&server.id) {
            command.envs(environment);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动 MCP Server: {error}"))?;
        let stdin = child.stdin.take().ok_or("MCP stdin 不可用")?;
        let stdout = child.stdout.take().ok_or("MCP stdout 不可用")?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) if !line.trim().is_empty() => {
                        if sender.send(Ok(line)).is_err() {
                            return;
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        let _ = sender.send(Err(error.to_string()));
                        return;
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            receiver,
            next_id: 1,
        })
    }

    fn initialize(&mut self) -> Result<(), String> {
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "Fox", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        if response
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err("MCP initialize 响应缺少 protocolVersion".to_owned());
        }
        self.notify("notifications/initialized", json!({}))
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let line = self
                .receiver
                .recv_timeout(TIMEOUT)
                .map_err(|_| format!("MCP 请求 {method} 超时"))??;
            let value: Value = serde_json::from_str(&line)
                .map_err(|error| format!("MCP 输出不是有效 JSON: {error}"))?;
            if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                continue;
            }
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(format!("MCP 请求失败: {error}"));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| "MCP 响应缺少 result".to_owned());
        }
    }

    fn write(&mut self, value: &Value) -> Result<(), String> {
        let line = serde_json::to_string(value).map_err(|error| error.to_string())?;
        self.stdin
            .write_all(format!("{line}\n").as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|error| error.to_string())
    }
}

impl Drop for McpSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn load_environment(server_id: &str) -> Option<HashMap<String, String>> {
    credential_entry(server_id)
        .and_then(|entry| entry.get_password().map_err(|error| error.to_string()))
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
}

fn credential_entry(server_id: &str) -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, server_id).map_err(|error| error.to_string())
}

fn validate_input_schema(tool_name: &str, schema: &Value) -> Result<(), String> {
    let object = schema
        .as_object()
        .ok_or_else(|| format!("MCP 工具 {tool_name} 的 inputSchema 不是对象"))?;
    if object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object")
        != "object"
    {
        return Err(format!(
            "MCP 工具 {tool_name} 的 inputSchema 必须声明 object 类型"
        ));
    }
    if object
        .get("properties")
        .is_some_and(|value| !value.is_object())
    {
        return Err(format!(
            "MCP 工具 {tool_name} 的 inputSchema.properties 不是对象"
        ));
    }
    if object
        .get("required")
        .is_some_and(|value| !value.is_array())
    {
        return Err(format!(
            "MCP 工具 {tool_name} 的 inputSchema.required 不是数组"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn server(mode: &str) -> McpServerRecord {
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("fake-mcp-server.mjs");
        McpServerRecord {
            id: format!("fake-{mode}"),
            name: "Fake MCP".to_owned(),
            command: "node".to_owned(),
            args: vec![script.to_string_lossy().into_owned(), mode.to_owned()],
            enabled: true,
            status: "unknown".to_owned(),
            credential_configured: false,
            last_error: None,
            last_checked_at: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn lists_and_calls_tools_through_stdio_json_rpc() {
        let tools = list_tools(&server("valid")).expect("tools/list should succeed");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "echo");
        let result = call_tool(&server("valid"), "echo", &json!({ "text": "hello" }))
            .expect("tools/call should succeed");
        assert_eq!(result["content"][0]["text"], "hello");
    }

    #[test]
    fn rejects_invalid_tool_input_schema() {
        let error = list_tools(&server("invalid-schema")).expect_err("schema must fail");
        assert!(error.contains("object 类型"));
    }

    #[test]
    fn rejects_unknown_tools_before_calling_the_server() {
        let error = call_tool(&server("valid"), "missing", &json!({}))
            .expect_err("unknown tools must fail");
        assert!(error.contains("未声明工具 missing"));
    }

    #[test]
    fn reports_protocol_timeouts_without_leaving_a_process_running() {
        let error = list_tools(&server("timeout")).expect_err("request must time out");
        assert!(error.contains("超时"));
    }
}
