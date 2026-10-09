use crate::database::McpServerRecord;
use keyring::Entry;
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, CONTENT_TYPE};
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use url::Url;
mod owned;
pub(crate) use owned::execute_owned;
pub(crate) use owned::execute_owned_readonly;

const CREDENTIAL_SERVICE: &str = "com.fox.agent.mcp";
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";
#[cfg(not(test))]
const TIMEOUT: Duration = Duration::from_secs(15);
#[cfg(test)]
const TIMEOUT: Duration = Duration::from_secs(2);
const MAX_TOOLS: usize = 200;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

type SharedClient = Arc<Mutex<ExtensionClient>>;

struct PooledClient {
    fingerprint: u64,
    client: SharedClient,
}

#[derive(Default)]
struct ConnectionPool {
    clients: HashMap<String, PooledClient>,
}

static CONNECTION_POOL: OnceLock<Mutex<ConnectionPool>> = OnceLock::new();

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
        .map_err(|error| error.to_string())?;
    invalidate_connection(server_id);
    Ok(())
}

pub fn clear_environment(server_id: &str) -> Result<(), String> {
    let result = match credential_entry(server_id)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    };
    invalidate_connection(server_id);
    result
}

pub fn invalidate_connection(server_id: &str) {
    if let Some(pool) = CONNECTION_POOL.get() {
        if let Ok(mut pool) = pool.lock() {
            pool.clients.remove(server_id);
        }
    }
}

pub fn shutdown_connections() {
    if let Some(pool) = CONNECTION_POOL.get() {
        if let Ok(mut pool) = pool.lock() {
            pool.clients.clear();
        }
    }
}

pub fn list_tools(server: &McpServerRecord) -> Result<Vec<Value>, String> {
    if server.id == crate::office::SERVER_ID {
        if !server.enabled { return Err("Office 连接器已停用".to_owned()); }
        crate::office::verify_binary(std::path::Path::new(&server.command))?;
        return Ok(crate::office::tool_definitions());
    }
    with_client(server, |client| client.list_tools()).and_then(normalize_tools)
}

pub fn call_tool(server: &McpServerRecord, tool: &str, arguments: &Value) -> Result<Value, String> {
    if server.id == crate::office::SERVER_ID {
        return Err("Office 连接器必须经过项目权限适配器调用".to_owned());
    }
    let tools = list_tools(server)?;
    if !tools
        .iter()
        .any(|item| item.get("name").and_then(Value::as_str) == Some(tool))
    {
        return Err(format!("扩展源未声明工具 {tool}"));
    }
    with_client(server, |client| client.call_tool(tool, arguments))
}

fn with_client<T>(
    server: &McpServerRecord,
    operation: impl FnOnce(&mut ExtensionClient) -> Result<T, String>,
) -> Result<T, String> {
    if !server.enabled {
        return Err("扩展源已停用".to_owned());
    }
    let fingerprint = server_fingerprint(server);
    let pool = CONNECTION_POOL.get_or_init(|| Mutex::new(ConnectionPool::default()));
    let shared = {
        let mut pool = pool.lock().map_err(|_| "扩展连接池锁已损坏".to_owned())?;
        let replace = pool
            .clients
            .get(&server.id)
            .is_none_or(|entry| entry.fingerprint != fingerprint);
        if replace {
            let client = Arc::new(Mutex::new(ExtensionClient::connect(server)?));
            pool.clients.insert(
                server.id.clone(),
                PooledClient {
                    fingerprint,
                    client,
                },
            );
        }
        pool.clients
            .get(&server.id)
            .map(|entry| Arc::clone(&entry.client))
            .ok_or_else(|| "扩展连接创建失败".to_owned())?
    };
    let result = shared
        .lock()
        .map_err(|_| "扩展连接锁已损坏".to_owned())
        .and_then(|mut client| operation(&mut client));
    if result.is_err() {
        invalidate_connection(&server.id);
    }
    result
}

pub(crate) fn server_fingerprint(server: &McpServerRecord) -> u64 {
    let mut hasher = DefaultHasher::new();
    server.id.hash(&mut hasher);
    server.transport.hash(&mut hasher);
    server.command.hash(&mut hasher);
    server.args.hash(&mut hasher);
    server.endpoint_url.hash(&mut hasher);
    server.definition.hash(&mut hasher);
    if let Some(mut credentials) = load_environment(&server.id) {
        let mut entries = credentials.drain().collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        entries.hash(&mut hasher);
    }
    hasher.finish()
}

enum ExtensionClient {
    Stdio(McpSession),
    StreamableHttp(HttpMcpSession),
    OpenApi(crate::mcp_openapi::OpenApiConnector),
}

impl ExtensionClient {
    fn connect(server: &McpServerRecord) -> Result<Self, String> {
        match server.transport.as_str() {
            "stdio" => Ok(Self::Stdio(McpSession::start(server)?)),
            "streamable_http" => Ok(Self::StreamableHttp(HttpMcpSession::start(server)?)),
            "openapi" => Ok(Self::OpenApi(crate::mcp_openapi::OpenApiConnector::start(
                server,
            )?)),
            other => Err(format!("不支持的扩展传输类型: {other}")),
        }
    }

    fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        match self {
            Self::Stdio(session) => {
                session.ensure_initialized()?;
                tools_from_result(session.request("tools/list", json!({}))?)
            }
            Self::StreamableHttp(session) => {
                session.ensure_initialized()?;
                tools_from_result(session.request("tools/list", json!({}))?)
            }
            Self::OpenApi(connector) => connector.list_tools(),
        }
    }

    fn call_tool(&mut self, tool: &str, arguments: &Value) -> Result<Value, String> {
        match self {
            Self::Stdio(session) => {
                session.ensure_initialized()?;
                session.request(
                    "tools/call",
                    json!({ "name": tool, "arguments": arguments }),
                )
            }
            Self::StreamableHttp(session) => {
                session.ensure_initialized()?;
                session.request(
                    "tools/call",
                    json!({ "name": tool, "arguments": arguments }),
                )
            }
            Self::OpenApi(connector) => connector.call_tool(tool, arguments),
        }
    }
}

fn tools_from_result(result: Value) -> Result<Vec<Value>, String> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| "MCP tools/list 响应缺少 tools 数组".to_owned())
}

fn normalize_tools(tools: Vec<Value>) -> Result<Vec<Value>, String> {
    if tools.len() > MAX_TOOLS {
        return Err(format!("扩展源工具数量超过上限 {MAX_TOOLS}"));
    }
    let mut names = HashSet::new();
    tools
        .into_iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "扩展工具缺少名称".to_owned())?;
            if !names.insert(name.to_owned()) {
                return Err(format!("扩展源声明了重复工具名 {name}"));
            }
            let schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({ "type": "object" }));
            validate_input_schema(name, &schema)?;
            Ok(json!({
                "name": name,
                "description": tool.get("description").and_then(Value::as_str).unwrap_or_default(),
                "inputSchema": schema,
                "annotations": {"readOnlyHint": tool["annotations"]["readOnlyHint"] == true,
                    "destructiveHint": tool["annotations"]["destructiveHint"] != false},
            }))
        })
        .collect()
}

struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    receiver: mpsc::Receiver<Result<String, String>>,
    next_id: u64,
    initialized: bool,
}

impl McpSession {
    fn start(server: &McpServerRecord) -> Result<Self, String> {
        if server.command.trim().is_empty() {
            return Err("stdio MCP 缺少启动命令".to_owned());
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
            .stderr(Stdio::null())
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
            initialized: false,
        })
    }

    fn ensure_initialized(&mut self) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
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
        self.notify("notifications/initialized", json!({}))?;
        self.initialized = true;
        Ok(())
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
                .map_err(|error| format!("MCP 返回了无效 JSON: {error}"))?;
            if let Some(result) = extract_rpc_response(&value, &json!(id))? {
                return Ok(result);
            }
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

struct HttpMcpSession {
    endpoint: Url,
    client: Client,
    credential_headers: HeaderMap,
    next_id: u64,
    initialized: bool,
    protocol_version: String,
    session_id: Option<String>,
    execution_deadline: Option<std::time::Instant>,
    cancellation: Option<crate::kernel::CancellationToken>,
}

impl HttpMcpSession {
    fn start(server: &McpServerRecord) -> Result<Self, String> {
        Ok(Self {
            endpoint: parse_endpoint(server.endpoint_url.as_deref(), "HTTP MCP")?,
            client: http_client()?,
            credential_headers: credential_headers(&server.id)?,
            next_id: 1,
            initialized: false,
            protocol_version: MCP_PROTOCOL_VERSION.to_owned(),
            session_id: None,
            execution_deadline: None,
            cancellation: None,
        })
    }

    fn ensure_initialized(&mut self) -> Result<(), String> {
        if self.initialized {
            return Ok(());
        }
        let response = self.request(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "Fox", "version": env!("CARGO_PKG_VERSION") },
            }),
        )?;
        self.protocol_version = response
            .get("protocolVersion")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| "HTTP MCP initialize 响应缺少 protocolVersion".to_owned())?
            .to_owned();
        self.initialized = true;
        self.notify("notifications/initialized", json!({}))?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        let body = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        let response = self.send(&body)?;
        if matches!(
            response.status(),
            StatusCode::ACCEPTED | StatusCode::NO_CONTENT
        ) {
            return Ok(());
        }
        let status = response.status();
        let bytes = read_limited(response)?;
        status
            .is_success()
            .then_some(())
            .ok_or_else(|| http_status_error("HTTP MCP 通知", status, &bytes))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let body = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        let response = self.send(&body)?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let bytes = read_limited(response)?;
        if !status.is_success() {
            return Err(http_status_error("HTTP MCP 请求", status, &bytes));
        }
        let values = if content_type.contains("text/event-stream") {
            parse_sse_messages(&bytes)?
        } else {
            match serde_json::from_slice::<Value>(&bytes)
                .map_err(|error| format!("HTTP MCP 返回了无效 JSON: {error}"))?
            {
                Value::Array(values) => values,
                value => vec![value],
            }
        };
        for value in values {
            if let Some(result) = extract_rpc_response(&value, &json!(id))? {
                return Ok(result);
            }
        }
        Err(format!("HTTP MCP 请求 {method} 未返回对应的 JSON-RPC 响应"))
    }

    fn send(&mut self, body: &Value) -> Result<Response, String> {
        if let Some(token) = &self.cancellation { token.check()?; }
        let timeout = self.execution_deadline.map(|deadline| deadline.checked_duration_since(std::time::Instant::now())
            .ok_or_else(|| "MCP execution budget exceeded".to_owned())).transpose()?.unwrap_or(TIMEOUT).min(TIMEOUT);
        let mut request = self
            .client
            .post(self.endpoint.clone())
            .timeout(timeout)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .headers(self.credential_headers.clone());
        if self.initialized {
            request = request.header("MCP-Protocol-Version", &self.protocol_version);
        }
        if let Some(session_id) = &self.session_id {
            request = request.header("Mcp-Session-Id", session_id);
        }
        let response = request
            .json(body)
            .send()
            .map_err(|error| error.to_string())?;
        if response.status().is_redirection() {
            return Err("HTTP MCP 禁止自动重定向；请配置最终端点".to_owned());
        }
        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.trim().is_empty())
        {
            self.session_id = Some(session_id.to_owned());
        }
        Ok(response)
    }
}

pub(crate) fn parse_endpoint(value: Option<&str>, label: &str) -> Result<Url, String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{label} 缺少端点 URL"))?;
    if value.contains('{') || value.contains('}') {
        return Err(format!("{label} 端点不支持未解析的 URL 模板"));
    }
    let endpoint = Url::parse(value).map_err(|error| format!("{label} URL 无效: {error}"))?;
    if !matches!(endpoint.scheme(), "http" | "https") {
        return Err(format!("{label} 只允许 http/https URL"));
    }
    if !endpoint.username().is_empty() || endpoint.password().is_some() {
        return Err(format!("{label} URL 不得内嵌凭证"));
    }
    Ok(endpoint)
}

pub(crate) fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

pub(crate) fn credential_headers(server_id: &str) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in load_environment(server_id).unwrap_or_default() {
        let name = safe_header_name(&name)?;
        let value = HeaderValue::from_str(&value)
            .map_err(|error| format!("HTTP 凭证 Header 值无效: {error}"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

pub(crate) fn safe_header_name(value: &str) -> Result<HeaderName, String> {
    let name = HeaderName::from_bytes(value.trim().as_bytes())
        .map_err(|error| format!("HTTP Header 名称 {value} 无效: {error}"))?;
    if matches!(
        name.as_str(),
        "host"
            | "content-length"
            | "content-type"
            | "accept"
            | "connection"
            | "transfer-encoding"
            | "mcp-session-id"
            | "mcp-protocol-version"
    ) {
        return Err(format!(
            "HTTP Header {} 由 Fox 管理，不能覆盖",
            name.as_str()
        ));
    }
    Ok(name)
}

pub(crate) fn read_limited(mut response: Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err("HTTP 响应超过 10 MB 上限".to_owned());
    }
    let mut bytes = Vec::new();
    response
        .by_ref()
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err("HTTP 响应超过 10 MB 上限".to_owned());
    }
    Ok(bytes)
}

fn parse_sse_messages(bytes: &[u8]) -> Result<Vec<Value>, String> {
    let text = String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("MCP SSE 不是有效 UTF-8: {error}"))?;
    let mut messages = Vec::new();
    let mut data = Vec::new();
    fn flush(data: &mut Vec<String>, messages: &mut Vec<Value>) -> Result<(), String> {
        if data.is_empty() {
            return Ok(());
        }
        let payload = data.join("\n");
        data.clear();
        if payload.trim() == "[DONE]" {
            return Ok(());
        }
        match serde_json::from_str::<Value>(&payload)
            .map_err(|error| format!("MCP SSE data 不是有效 JSON: {error}"))?
        {
            Value::Array(values) => messages.extend(values),
            value => messages.push(value),
        }
        Ok(())
    }
    for raw_line in text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.is_empty() {
            flush(&mut data, &mut messages)?;
        } else if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value).to_owned());
        }
    }
    flush(&mut data, &mut messages)?;
    Ok(messages)
}

fn extract_rpc_response(value: &Value, expected_id: &Value) -> Result<Option<Value>, String> {
    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
        || value.get("id") != Some(expected_id)
    {
        return Ok(None);
    }
    if let Some(error) = value.get("error") {
        return Err(format!("MCP 远端错误: {error}"));
    }
    value
        .get("result")
        .cloned()
        .map(Some)
        .ok_or_else(|| "MCP 响应缺少 result".to_owned())
}

pub(crate) fn http_status_error(label: &str, status: StatusCode, body: &[u8]) -> String {
    let preview = String::from_utf8_lossy(body)
        .chars()
        .take(1024)
        .collect::<String>();
    format!("{label}失败（HTTP {}）: {preview}", status.as_u16())
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
        .ok_or_else(|| format!("扩展工具 {tool_name} 的 inputSchema 必须是对象"))?;
    if object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("object")
        != "object"
    {
        return Err(format!(
            "扩展工具 {tool_name} 的 inputSchema 顶层类型必须是 object"
        ));
    }
    if object
        .get("properties")
        .is_some_and(|value| !value.is_object())
    {
        return Err(format!(
            "扩展工具 {tool_name} 的 inputSchema.properties 必须是对象"
        ));
    }
    if object
        .get("required")
        .is_some_and(|value| !value.is_array())
    {
        return Err(format!(
            "扩展工具 {tool_name} 的 inputSchema.required 必须是数组"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
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
            transport: "stdio".to_owned(),
            endpoint_url: None,
            definition: None,
            enabled: true,
            status: "unknown".to_owned(),
            credential_configured: false,
            last_error: None,
            last_checked_at: None,
            last_latency_ms: None,
            tool_count: None,
            consecutive_failures: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn http_server(endpoint: String) -> McpServerRecord {
        McpServerRecord {
            id: "http-mcp-test".to_owned(),
            name: "HTTP MCP".to_owned(),
            command: String::new(),
            args: Vec::new(),
            transport: "streamable_http".to_owned(),
            endpoint_url: Some(endpoint),
            definition: None,
            enabled: true,
            status: "unknown".to_owned(),
            credential_configured: false,
            last_error: None,
            last_checked_at: None,
            last_latency_ms: None,
            tool_count: None,
            consecutive_failures: 0,
            created_at: 0,
            updated_at: 0,
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> (HashMap<String, String>, Value) {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 1024];
        let header_end = loop {
            let count = stream.read(&mut buffer).expect("read request");
            assert!(count > 0, "request closed before headers");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let header_text = String::from_utf8_lossy(&bytes[..header_end]);
        let headers = header_text
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_owned()))
            .collect::<HashMap<_, _>>();
        let content_length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let count = stream.read(&mut buffer).expect("read body");
            assert!(count > 0, "request closed before body");
            bytes.extend_from_slice(&buffer[..count]);
        }
        let body = serde_json::from_slice(&bytes[header_end..header_end + content_length])
            .expect("parse request JSON");
        (headers, body)
    }

    fn write_http_response(
        stream: &mut TcpStream,
        status: &str,
        content_type: &str,
        body: &str,
        session: bool,
    ) {
        let session_header = if session {
            "Mcp-Session-Id: fox-session-1\r\n"
        } else {
            ""
        };
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n{session_header}Connection: close\r\n\r\n{body}",
            body.len()
        )
        .expect("write response");
        stream.flush().expect("flush response");
    }

    #[test]
    fn lists_and_calls_tools_through_one_persistent_stdio_session() {
        let server = server("valid");
        let tools = list_tools(&server).expect("tools/list should succeed");
        assert_eq!(tools[0]["name"], "echo");
        let result = call_tool(&server, "echo", &json!({ "text": "hello" }))
            .expect("tools/call should succeed");
        assert_eq!(result["content"][0]["text"], "hello");
        invalidate_connection(&server.id);
    }

    #[test]
    fn kernel_owned_mcp_validates_arguments_and_never_reuses_pooled_sessions() {
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("owned-mcp").unwrap();
        let token = registry.run_token("owned-mcp").unwrap();
        let server = server("valid");
        let result = execute_owned(&server, Some(("echo", &json!({"text":"owned 中文"}))), &token, Duration::from_secs(5)).unwrap();
        assert_eq!(result["content"][0]["text"], "owned 中文");
        assert!(execute_owned(&server, Some(("echo", &json!({"text":42}))), &token, Duration::from_secs(5)).is_err());
        assert!(execute_owned(&server, Some(("missing", &json!({}))), &token, Duration::from_secs(5)).is_err());
    }

    #[test]
    fn reconciliation_connector_queries_require_live_explicit_readonly_schema() {
        let registry=crate::kernel::CancellationRegistry::default();
        registry.register_run("query").unwrap();let token=registry.run_token("query").unwrap();
        let result=execute_owned_readonly(&server("readonly"),Some(("echo",&json!({"text":"receipt-46"}))),&token,Duration::from_secs(5)).unwrap();
        assert_eq!(result["content"][0]["text"],"receipt-46");
        let error=execute_owned_readonly(&server("valid"),Some(("echo",&json!({"text":"must-not-execute"}))),&token,Duration::from_secs(5)).unwrap_err();
        assert!(error.contains("只读查询"));
        assert!(execute_owned_readonly(&server("readonly"),Some(("echo",&json!({"text":42}))),&token,Duration::from_secs(5)).is_err());
    }

    #[test]
    fn kernel_owned_mcp_cancellation_stops_waiting_and_reaps_its_server() {
        use crate::kernel::CancellationPort;
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("cancel-mcp").unwrap();
        let token = registry.run_token("cancel-mcp").unwrap();
        let cancel = registry.clone();
        let sender = thread::spawn(move || { thread::sleep(Duration::from_millis(150)); cancel.request_run_cancel("cancel-mcp"); });
        let start = std::time::Instant::now();
        assert!(execute_owned(&server("timeout"), None, &token, Duration::from_secs(10)).is_err());
        assert!(start.elapsed() < Duration::from_secs(5));
        sender.join().unwrap();
    }

    #[test]
    fn rejects_invalid_schema_and_unknown_tools() {
        let invalid = server("invalid-schema");
        assert!(list_tools(&invalid)
            .expect_err("schema must fail")
            .contains("顶层类型必须是 object"));
        let valid = server("valid");
        assert!(call_tool(&valid, "missing", &json!({}))
            .expect_err("unknown tools must fail")
            .contains("未声明工具 missing"));
        invalidate_connection(&valid.id);
    }

    #[test]
    fn reports_protocol_timeouts_without_leaving_a_process_running() {
        let server = server("timeout");
        assert!(list_tools(&server)
            .expect_err("request must time out")
            .contains("超时"));
    }

    #[test]
    fn parses_sse_rpc_responses_and_rejects_reserved_headers() {
        let values = parse_sse_messages(
            b"event: message\nid: 1\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
        )
        .expect("parse SSE");
        assert_eq!(
            extract_rpc_response(&values[0], &json!(1)).expect("extract"),
            Some(json!({ "ok": true }))
        );
        assert!(safe_header_name("Host").is_err());
    }

    #[test]
    fn negotiates_and_reuses_streamable_http_session_with_sse_tool_result() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let server_thread = thread::spawn(move || {
            let mut observations = Vec::new();
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().expect("accept request");
                let (headers, body) = read_http_request(&mut stream);
                let method = body
                    .get("method")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                observations.push((
                    method.to_owned(),
                    headers.get("mcp-session-id").cloned(),
                    headers.get("mcp-protocol-version").cloned(),
                ));
                match method {
                    "initialize" => write_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &json!({
                            "jsonrpc": "2.0", "id": body["id"],
                            "result": { "protocolVersion": MCP_PROTOCOL_VERSION, "capabilities": { "tools": {} }, "serverInfo": { "name": "test", "version": "1" } }
                        }).to_string(),
                        true,
                    ),
                    "notifications/initialized" => write_http_response(
                        &mut stream,
                        "202 Accepted",
                        "application/json",
                        "",
                        false,
                    ),
                    "tools/list" => write_http_response(
                        &mut stream,
                        "200 OK",
                        "application/json",
                        &json!({
                            "jsonrpc": "2.0", "id": body["id"],
                            "result": { "tools": [{ "name": "echo", "description": "Echo", "inputSchema": { "type": "object", "properties": { "text": { "type": "string" } } } }] }
                        }).to_string(),
                        false,
                    ),
                    "tools/call" => write_http_response(
                        &mut stream,
                        "200 OK",
                        "text/event-stream",
                        &format!(
                            "event: message\ndata: {}\n\n",
                            json!({
                                "jsonrpc": "2.0", "id": body["id"],
                                "result": { "content": [{ "type": "text", "text": body["params"]["arguments"]["text"] }] }
                            })
                        ),
                        false,
                    ),
                    other => panic!("unexpected method {other}"),
                }
            }
            observations
        });
        let server = http_server(endpoint);
        assert_eq!(list_tools(&server).unwrap()[0]["name"], "echo");
        let result = call_tool(&server, "echo", &json!({ "text": "hello-http" })).unwrap();
        assert_eq!(result["content"][0]["text"], "hello-http");
        invalidate_connection(&server.id);
        let observations = server_thread.join().expect("join server");
        assert_eq!(observations[0].0, "initialize");
        assert!(observations[0].1.is_none());
        for (_, session, version) in observations.iter().skip(1) {
            assert_eq!(session.as_deref(), Some("fox-session-1"));
            assert_eq!(version.as_deref(), Some(MCP_PROTOCOL_VERSION));
        }
    }
}
