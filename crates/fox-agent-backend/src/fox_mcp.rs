//! Plan C3: the Fox tool MCP bridge for the external Agents (pi / Codex / dsh).
//!
//! External Agents reach a small set of Fox Host tools over MCP:
//! `present_files` (delivery cards), `list_knowledge_bases` /
//! `search_knowledge` / `read_knowledge_document` (the conversation's
//! knowledge bases) and `ask_user` (a question card). Every call goes back to
//! the Host ([`FoxToolHost`]), which runs it through the same entry as a
//! PiNative call (expert tool scope, lifecycle hooks, the approval engine,
//! the Host tool-call record) and returns its result; the bridge only speaks
//! MCP and applies the one result ceiling every tool result shares
//! ([`MAX_TOOL_RESULT_BYTES`], the same 64 KiB head + tail cut as the
//! PiNative runtime's TB-2 cap).
//!
//! Transport (plan C3: "stdio or loopback with a one-time token"):
//!
//! ```text
//! Agent --stdio (MCP, newline-delimited JSON-RPC)--> relay (node fox-mcp-relay.mjs)
//!       --TCP 127.0.0.1:<port>, first line {"foxMcpToken": "..."}--> McpBridge (Host)
//! pi extension (fox-mcp-tools.ts) --same TCP + token line--> McpBridge
//! ```
//!
//! MCP stdio is the one transport every Agent must support (ACP: "All Agents
//! MUST support this transport"), so Codex and dsh get a stdio server in ACP
//! `session/new` `mcpServers`; the stdio process is a dumb byte relay, all
//! protocol handling is here. pi-acp stores `mcpServers` but does not wire
//! them to pi, so pi gets the same bridge through a pi extension instead.
//!
//! Token: 32 random bytes, issued for ONE launch of ONE conversation's Agent
//! process and revoked when that process stops (the launch's `on_closed`).
//! A token is never reused across launches; connections still open on a
//! revoked token are closed. Several connections on the same live token are
//! allowed (an Agent may restart its MCP client, pi-acp restarts pi on
//! `session/load`), each bound to the token's conversation; nothing from the
//! wire can name another conversation.
//!
//! Failures are errors, never a fallback: an unknown token, a call before
//! `initialize`, an unknown tool or a Host refusal is reported as such.

use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::port::AgentBackend;

/// Name of the MCP server in the Agents' tool lists (Codex: chosen so a
/// user's own Codex config is unlikely to shadow it, see
/// [`crate::backends::codex::HOST_MCP_SERVER_NAME`]).
pub const SERVER_NAME: &str = crate::backends::codex::HOST_MCP_SERVER_NAME;
/// `serverInfo.name` in the MCP `initialize` answer.
pub const SERVER_INFO_NAME: &str = "fox-host";
/// MCP protocol revisions this bridge answers (newest first).
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// The one ceiling for a tool result's text (bytes of UTF-8), the same as the
/// PiNative runtime (`tool-view.mjs` `MAX_MODEL_TOOL_RESULT_BYTES`).
pub const MAX_TOOL_RESULT_BYTES: usize = 64 * 1024;
const HEAD_BYTES: usize = 44 * 1024;
const TAIL_BYTES: usize = 16 * 1024;
/// Longest accepted wire line (a request); longer lines close the connection.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
/// The handshake line must arrive within this time.
pub const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Environment variables the relay and the pi extension read.
pub const ENDPOINT_ENV: &str = "FOX_MCP_ENDPOINT";
pub const TOKEN_ENV: &str = "FOX_MCP_TOKEN";
/// pi only: path of the Fox tool extension the gated pi loads with `-e`.
pub const PI_EXTENSION_ENV: &str = "FOX_PI_MCP_EXTENSION";

/// The relay the stdio server runs (embedded, written to the Host state
/// directory by [`write_assets`]).
pub const RELAY_SCRIPT: &str = include_str!("../runtime-assets/fox-mcp/fox-mcp-relay.mjs");
pub const RELAY_FILE: &str = "fox-mcp-relay.mjs";
/// The pi extension that registers the bridge's tools in pi.
pub const PI_EXTENSION_SCRIPT: &str = include_str!("../runtime-assets/fox-mcp/fox-mcp-tools.ts");
pub const PI_EXTENSION_FILE: &str = "fox-mcp-tools.ts";

/// JSON-RPC / MCP error codes the bridge returns.
pub mod rpc_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    /// A request other than `initialize` / `ping` before `initialize`.
    pub const NOT_INITIALIZED: i64 = -32002;
    /// The handshake token is unknown, revoked or malformed.
    pub const TOKEN_REFUSED: i64 = -32001;
}

/// The tools the bridge offers, in `tools/list` order.
pub const TOOL_NAMES: [&str; 5] = ["present_files", "list_knowledge_bases", "search_knowledge", "read_knowledge_document", "ask_user"];

fn knowledge_reference_schema() -> Value {
    json!({
        "anyOf": [
            { "type": "object", "properties": { "source": { "const": "local" }, "id": { "type": "string" }, "revision": { "type": "string" } }, "required": ["source", "id"] },
            { "type": "object", "properties": { "source": { "const": "remote" }, "connectionId": { "type": "string" }, "id": { "type": "string" }, "revision": { "type": "string" } }, "required": ["source", "connectionId", "id"] }
        ]
    })
}

/// MCP `tools/list` entries. Descriptions and schemas follow the PiNative
/// definitions (`services/agent-runtime/src/host-tools.mjs`) so a model sees
/// the same contract on every backend.
pub fn tool_definitions() -> Vec<Value> {
    let reference = knowledge_reference_schema();
    let references = json!({ "type": "array", "items": reference.clone(), "minItems": 1 });
    vec![
        json!({
            "name": "present_files",
            "title": "Present final files",
            "description": "Declare the files you are actually handing to the user as this turn's deliverables. Call this once, after the files exist, naming only files you really produced or updated for this request; the user sees exactly these as file cards under your reply. Pass paths relative to the authorized project (or absolute inside it). Each file must already exist and be a regular file - this tool does not create, convert or repair anything. It does not check, grade or verify file content. Prefer the one or two files the user actually wants; at most 8 per call. A path that does not exist, is a directory, or falls outside the project is rejected with a tool error, and then no card is shown for this call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "files": {
                        "type": "array", "minItems": 1, "maxItems": 8,
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "Path of an existing file, relative to the authorized project folder or absolute inside it." },
                                "description": { "type": "string", "maxLength": 200, "description": "Optional one-line note shown under the file name. Plain text, no markdown." }
                            },
                            "required": ["path"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["files"],
                "additionalProperties": false
            }
        }),
        json!({
            "name": "list_knowledge_bases",
            "title": "List knowledge bases",
            "description": "List knowledge bases explicitly enabled for the current Fox conversation, including their local or remote source. Pass the returned reference objects to search_knowledge / read_knowledge_document.",
            "inputSchema": { "type": "object", "properties": { "target": reference.clone(), "targets": references.clone() } }
        }),
        json!({
            "name": "search_knowledge",
            "title": "Search knowledge",
            "description": "Search the conversation's knowledge bases. First call list_knowledge_bases, then pass its reference objects in targets. query should contain concise domain keywords.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "knowledgeBaseId": { "type": "string" },
                    "target": reference.clone(),
                    "targets": references,
                    "query": { "type": "string" },
                    "topK": { "type": "integer", "minimum": 1 },
                    "documentIds": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
                },
                "required": ["query"]
            }
        }),
        json!({
            "name": "read_knowledge_document",
            "title": "Read knowledge document",
            "description": "Read parsed text of an enabled knowledge document. documentId must be a real file_id or documentId returned by search_knowledge, never the knowledge-base ID. Use target for the same knowledge reference as the search.",
            "inputSchema": {
                "type": "object",
                "properties": { "knowledgeBaseId": { "type": "string" }, "target": reference, "documentId": { "type": "string" }, "chunkId": { "type": "string" } },
                "required": ["documentId"]
            }
        }),
        json!({
            "name": "ask_user",
            "title": "Ask the user",
            "description": "Ask the user one to four short questions when a missing choice materially changes the result. Fox shows them as a question card. After calling this, end your turn immediately without further actions; the user's answers arrive as the next user message.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array", "minItems": 1, "maxItems": 4,
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": { "type": "string", "minLength": 1, "maxLength": 300 },
                                "options": { "type": "array", "maxItems": 8, "items": { "type": "string", "minLength": 1, "maxLength": 120 } },
                                "multiSelect": { "type": "boolean" },
                                "allowOther": { "type": "boolean" }
                            },
                            "required": ["question"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["questions"],
                "additionalProperties": false
            }
        }),
    ]
}

/// Who a tool call is for. Fixed by the token, never by the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallContext {
    pub conversation_id: String,
    pub backend: AgentBackend,
    /// Host-assigned id of this call (unique per bridge).
    pub tool_call_id: String,
}

/// A tool call the Host refused or that failed. `code` is a stable,
/// machine-readable code (`[code] message` is what the model reads).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolError {
    pub code: String,
    pub message: String,
}

impl ToolError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into() }
    }
}

/// The Host side of the bridge. Implemented by the desktop: runs the tool
/// through the Host's own entry (authority, approval engine, records) and
/// returns its model-facing result (text or JSON).
pub trait FoxToolHost: Send + Sync + 'static {
    fn call_tool(&self, context: ToolCallContext, tool: String, arguments: Value) -> BoxFuture<'static, Result<Value, ToolError>>;
}

/// Cut `text` to [`MAX_TOOL_RESULT_BYTES`] the way the PiNative runtime does
/// (head + tail, with a notice of what was left out). `true` when cut.
pub fn cap_result_text(text: &str) -> (String, bool) {
    if text.len() <= MAX_TOOL_RESULT_BYTES {
        return (text.to_owned(), false);
    }
    let head = utf8_prefix(text, HEAD_BYTES);
    let tail = utf8_suffix(text, TAIL_BYTES);
    let omitted = text.len() - head.len() - tail.len();
    let notice = format!(
        "[Fox 工具结果上限 {MAX_TOOL_RESULT_BYTES} 字节：原文约 {} 字节，这里保留开头 {} 与结尾 {} 字节，中间省略约 {omitted} 字节。省略的部分已截断，无法取回；请用更精确的参数重新查询，不要凭省略内容编造。]",
        text.len(),
        head.len(),
        tail.len()
    );
    (format!("{head}\n\n{notice}\n\n{tail}"), true)
}

fn utf8_prefix(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn utf8_suffix(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// The MCP `tools/call` result of a Host answer.
pub fn call_result(outcome: Result<Value, ToolError>) -> Value {
    let (text, is_error) = match outcome {
        Ok(Value::String(text)) => (text, false),
        Ok(value) => (serde_json::to_string(&value).unwrap_or_default(), false),
        Err(error) => (format!("[{}] {}", error.code, error.message), true),
    };
    let (text, capped) = cap_result_text(&text);
    let mut result = json!({ "content": [{ "type": "text", "text": text }], "isError": is_error });
    if capped {
        result["_meta"] = json!({ "foxResultCapped": true });
    }
    result
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// One MCP connection's state (bound to a token's conversation).
#[derive(Debug, Clone)]
pub struct McpSession {
    pub conversation_id: String,
    pub backend: AgentBackend,
    pub initialized: bool,
}

/// What a message asks the connection to do.
pub enum Dispatch {
    /// Nothing to answer (a notification).
    None,
    /// Answer now.
    Reply(Value),
    /// A `tools/call`: run it on the Host, then answer with [`call_result`].
    Call { id: Value, tool: String, arguments: Value },
}

/// Decide one incoming JSON-RPC message (no I/O; the protocol in one place).
pub fn dispatch(session: &mut McpSession, message: &Value) -> Dispatch {
    let Some(object) = message.as_object() else {
        return Dispatch::Reply(rpc_error(Value::Null, rpc_code::INVALID_REQUEST, "expected a JSON-RPC object"));
    };
    let id = object.get("id").cloned();
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        // A response from the client (the bridge sends no requests): ignore.
        return match id {
            Some(_) if object.contains_key("result") || object.contains_key("error") => Dispatch::None,
            Some(id) => Dispatch::Reply(rpc_error(id, rpc_code::INVALID_REQUEST, "missing method")),
            None => Dispatch::Reply(rpc_error(Value::Null, rpc_code::INVALID_REQUEST, "missing method")),
        };
    };
    let Some(id) = id else {
        // Notifications: initialized / cancelled / anything else need no answer.
        return Dispatch::None;
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str).unwrap_or_default();
            let version = PROTOCOL_VERSIONS.iter().find(|version| **version == requested).copied().unwrap_or(PROTOCOL_VERSIONS[0]);
            session.initialized = true;
            Dispatch::Reply(rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_INFO_NAME, "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "Fox Host tools. present_files hands finished files to the user as file cards; list_knowledge_bases / search_knowledge / read_knowledge_document read the conversation's knowledge bases; ask_user shows a question card, after which you end the turn and wait for the user's answer."
                }),
            ))
        }
        "ping" => Dispatch::Reply(rpc_result(id, json!({}))),
        _ if !session.initialized => Dispatch::Reply(rpc_error(id, rpc_code::NOT_INITIALIZED, format!("{method} before initialize"))),
        "tools/list" => Dispatch::Reply(rpc_result(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => {
            let Some(tool) = params.get("name").and_then(Value::as_str) else {
                return Dispatch::Reply(rpc_error(id, rpc_code::INVALID_PARAMS, "tools/call needs a tool name"));
            };
            if !TOOL_NAMES.contains(&tool) {
                return Dispatch::Reply(rpc_error(id, rpc_code::INVALID_PARAMS, format!("unknown tool: {tool}")));
            }
            let arguments = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(Value::Object(map)) => Value::Object(map.clone()),
                Some(_) => return Dispatch::Reply(rpc_error(id, rpc_code::INVALID_PARAMS, "tools/call arguments must be an object")),
            };
            Dispatch::Call { id, tool: tool.to_owned(), arguments }
        }
        other => Dispatch::Reply(rpc_error(id, rpc_code::METHOD_NOT_FOUND, format!("method not found: {other}"))),
    }
}

/// A token's grant: where it may connect and for whom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpGrant {
    pub endpoint: String,
    pub token: String,
    pub conversation_id: String,
    pub backend: AgentBackend,
}

struct Binding {
    conversation_id: String,
    backend: AgentBackend,
    revoked: CancellationToken,
}

struct BridgeInner {
    endpoint: String,
    host: Arc<dyn FoxToolHost>,
    tokens: Mutex<HashMap<String, Binding>>,
    next_call: std::sync::atomic::AtomicU64,
    call_prefix: String,
}

/// The Host's loopback MCP endpoint. One per Fox process; cheap to clone.
#[derive(Clone)]
pub struct McpBridge {
    inner: Arc<BridgeInner>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().zip(right).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

impl McpBridge {
    /// Bind `127.0.0.1:0` and serve on `handle`.
    pub fn start(host: Arc<dyn FoxToolHost>, handle: &tokio::runtime::Handle) -> std::io::Result<Self> {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = format!("127.0.0.1:{}", listener.local_addr()?.port());
        let prefix = crate::launch::random_hex(4).map_err(std::io::Error::other)?;
        let bridge = Self {
            inner: Arc::new(BridgeInner {
                endpoint,
                host,
                tokens: Mutex::new(HashMap::new()),
                next_call: std::sync::atomic::AtomicU64::new(1),
                call_prefix: prefix,
            }),
        };
        let serving = bridge.clone();
        let _guard = handle.enter();
        let listener = tokio::net::TcpListener::from_std(listener)?;
        handle.spawn(async move { serving.accept_loop(listener).await });
        Ok(bridge)
    }

    /// `127.0.0.1:<port>`.
    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    /// Issue a fresh token for one launch of `conversation_id`'s Agent.
    pub fn issue(&self, conversation_id: &str, backend: AgentBackend) -> Result<McpGrant, String> {
        let token = crate::launch::random_hex(32)?;
        lock(&self.inner.tokens).insert(
            token.clone(),
            Binding { conversation_id: conversation_id.to_owned(), backend, revoked: CancellationToken::new() },
        );
        Ok(McpGrant { endpoint: self.inner.endpoint.clone(), token, conversation_id: conversation_id.to_owned(), backend })
    }

    /// Revoke a token: refused from now on, open connections are closed.
    pub fn revoke(&self, token: &str) {
        if let Some(binding) = lock(&self.inner.tokens).remove(token) {
            binding.revoked.cancel();
        }
    }

    /// Live tokens (tests and diagnostics).
    pub fn live_tokens(&self) -> usize {
        lock(&self.inner.tokens).len()
    }

    fn claim(&self, token: &str) -> Option<(McpSession, CancellationToken)> {
        let tokens = lock(&self.inner.tokens);
        let (key, binding) = tokens.iter().find(|(key, _)| constant_time_eq(key.as_bytes(), token.as_bytes()))?;
        let _ = key;
        Some((
            McpSession { conversation_id: binding.conversation_id.clone(), backend: binding.backend, initialized: false },
            binding.revoked.clone(),
        ))
    }

    async fn accept_loop(self, listener: tokio::net::TcpListener) {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let bridge = self.clone();
                    tokio::spawn(async move { bridge.serve(stream).await });
                }
                Err(error) => {
                    eprintln!("fox.mcp accept_failed={error}");
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    }

    async fn serve(self, stream: tokio::net::TcpStream) {
        let _ = stream.set_nodelay(true);
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read);
        let mut first = String::new();
        let handshake = tokio::time::timeout(HANDSHAKE_TIMEOUT, read_line(&mut lines, &mut first)).await;
        let token = match handshake {
            Ok(Ok(true)) => serde_json::from_str::<Value>(first.trim()).ok().and_then(|value| value.get("foxMcpToken").and_then(Value::as_str).map(str::to_owned)),
            _ => None,
        };
        let Some((mut session, revoked)) = token.as_deref().and_then(|token| self.claim(token)) else {
            let refusal = rpc_error(Value::Null, rpc_code::TOKEN_REFUSED, "fox_mcp_token_refused: the Fox tool bridge token is unknown or was revoked (the Agent process it was issued to has stopped)");
            let _ = write.write_all(format!("{refusal}\n").as_bytes()).await;
            let _ = write.shutdown().await;
            return;
        };
        let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
        let writer_revoked = revoked.clone();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = writer_revoked.cancelled() => break,
                    message = out_rx.recv() => match message {
                        Some(message) => {
                            if write.write_all(format!("{message}\n").as_bytes()).await.is_err() { break; }
                        }
                        None => break,
                    }
                }
            }
            let _ = write.shutdown().await;
        });
        loop {
            let mut line = String::new();
            let read = tokio::select! {
                _ = revoked.cancelled() => break,
                read = read_line(&mut lines, &mut line) => read,
            };
            match read {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    let _ = out_tx.send(rpc_error(Value::Null, rpc_code::INVALID_REQUEST, error));
                    break;
                }
            }
            if line.trim().is_empty() {
                continue;
            }
            let message = match serde_json::from_str::<Value>(line.trim()) {
                Ok(message) => message,
                Err(error) => {
                    let _ = out_tx.send(rpc_error(Value::Null, rpc_code::PARSE_ERROR, format!("parse error: {error}")));
                    continue;
                }
            };
            match dispatch(&mut session, &message) {
                Dispatch::None => {}
                Dispatch::Reply(reply) => {
                    let _ = out_tx.send(reply);
                }
                Dispatch::Call { id, tool, arguments } => {
                    let number = self.inner.next_call.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let context = ToolCallContext {
                        conversation_id: session.conversation_id.clone(),
                        backend: session.backend,
                        tool_call_id: format!("fox-mcp-{}-{number}", self.inner.call_prefix),
                    };
                    let host = Arc::clone(&self.inner.host);
                    let out = out_tx.clone();
                    let call_revoked = revoked.clone();
                    tokio::spawn(async move {
                        let outcome = tokio::select! {
                            _ = call_revoked.cancelled() => return,
                            outcome = host.call_tool(context, tool, arguments) => outcome,
                        };
                        let _ = out.send(rpc_result(id, call_result(outcome)));
                    });
                }
            }
        }
        drop(out_tx);
        let _ = writer.await;
    }
}

/// Read one `\n`-terminated line, bounded by [`MAX_LINE_BYTES`]. `Ok(false)`
/// at end of stream.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R, line: &mut String) -> Result<bool, String> {
    let mut bytes = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|error| error.to_string())?;
        if available.is_empty() {
            if bytes.is_empty() {
                return Ok(false);
            }
            break;
        }
        if let Some(position) = available.iter().position(|byte| *byte == b'\n') {
            bytes.extend_from_slice(&available[..position]);
            reader.consume(position + 1);
            break;
        }
        let length = available.len();
        bytes.extend_from_slice(available);
        reader.consume(length);
        if bytes.len() > MAX_LINE_BYTES {
            return Err(format!("request line longer than {MAX_LINE_BYTES} bytes"));
        }
    }
    if bytes.len() > MAX_LINE_BYTES {
        return Err(format!("request line longer than {MAX_LINE_BYTES} bytes"));
    }
    *line = String::from_utf8(bytes).map_err(|_| "request line is not UTF-8".to_owned())?;
    Ok(true)
}

/// Where [`write_assets`] put the relay and the pi extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpAssets {
    pub relay: std::path::PathBuf,
    pub pi_extension: std::path::PathBuf,
}

/// Write the embedded relay and pi extension under `<state_root>/fox-mcp/`
/// (rewritten when the content differs, so an edited copy never runs).
pub fn write_assets(state_root: &std::path::Path) -> Result<McpAssets, String> {
    let dir = state_root.join("fox-mcp");
    std::fs::create_dir_all(&dir).map_err(|error| format!("无法创建 {}：{error}", dir.display()))?;
    let write = |name: &str, content: &str| -> Result<std::path::PathBuf, String> {
        let path = dir.join(name);
        if std::fs::read(&path).ok().as_deref() != Some(content.as_bytes()) {
            let staging = dir.join(format!(".{name}.tmp"));
            std::fs::write(&staging, content).map_err(|error| format!("无法写入 {}：{error}", staging.display()))?;
            std::fs::rename(&staging, &path).map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
        }
        Ok(path)
    };
    Ok(McpAssets { relay: write(RELAY_FILE, RELAY_SCRIPT)?, pi_extension: write(PI_EXTENSION_FILE, PI_EXTENSION_SCRIPT)? })
}

/// How a launch hands the bridge to its Agent (plan C3, per Agent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpInjection {
    /// ACP `session/new` / `session/load` `mcpServers`: one stdio server
    /// (Codex, dsh).
    AcpStdio(McpStdioServer),
    /// pi: environment for the gated pi, which loads the Fox tool extension
    /// ([`PI_EXTENSION_ENV`]) that connects to the bridge itself.
    PiExtension { env: Vec<(String, String)> },
}

/// An ACP `McpServerStdio` the Host hands to an Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpStdioServer {
    pub name: String,
    pub command: std::path::PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// The injection for `backend` with `grant` (node = the runtime's Node,
/// which runs the relay).
pub fn injection_for(backend: AgentBackend, grant: &McpGrant, node: &std::path::Path, assets: &McpAssets) -> Option<McpInjection> {
    let env = vec![(ENDPOINT_ENV.to_owned(), grant.endpoint.clone()), (TOKEN_ENV.to_owned(), grant.token.clone())];
    match backend {
        AgentBackend::Codex | AgentBackend::Deepseek => Some(McpInjection::AcpStdio(McpStdioServer {
            name: SERVER_NAME.to_owned(),
            command: node.to_path_buf(),
            args: vec![assets.relay.to_string_lossy().into_owned()],
            env,
        })),
        AgentBackend::Pi => {
            let mut env = env;
            env.push((PI_EXTENSION_ENV.to_owned(), assets.pi_extension.to_string_lossy().into_owned()));
            Some(McpInjection::PiExtension { env })
        }
        AgentBackend::Fox => None,
    }
}

/// Codex: the `session/new` answer lists client MCP servers it dropped
/// because the user's config already has one of that name
/// (`skippedMcpServers`, top level or under `_meta`). The Fox server must not
/// be among them.
pub fn skipped_by_agent(new_session_response: &Value) -> bool {
    let lists = [new_session_response.get("skippedMcpServers"), new_session_response.pointer("/_meta/skippedMcpServers")];
    lists.into_iter().flatten().filter_map(Value::as_array).flatten().any(|entry| {
        entry.as_str() == Some(SERVER_NAME) || entry.get("name").and_then(Value::as_str) == Some(SERVER_NAME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> McpSession {
        McpSession { conversation_id: "c1".into(), backend: AgentBackend::Codex, initialized: false }
    }

    fn reply(dispatch: Dispatch) -> Value {
        match dispatch {
            Dispatch::Reply(value) => value,
            Dispatch::None => panic!("expected a reply, got none"),
            Dispatch::Call { tool, .. } => panic!("expected a reply, got call {tool}"),
        }
    }

    #[test]
    fn initialize_negotiates_a_supported_version_and_declares_tools() {
        let mut session = session();
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": { "name": "codex" } } })));
        assert_eq!(answer["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(answer["result"]["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(answer["result"]["serverInfo"]["name"], SERVER_INFO_NAME);
        assert!(session.initialized);
        let mut other = self::session();
        let answer = reply(dispatch(&mut other, &json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } })));
        assert_eq!(answer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0], "unknown revision: answer the newest");
    }

    #[test]
    fn requests_before_initialize_are_refused_but_ping_is_answered() {
        let mut session = session();
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })));
        assert_eq!(answer["error"]["code"], rpc_code::NOT_INITIALIZED);
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" })));
        assert_eq!(answer["result"], json!({}));
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "ask_user" } })));
        assert_eq!(answer["error"]["code"], rpc_code::NOT_INITIALIZED, "no call runs before initialize");
    }

    #[test]
    fn tools_list_names_exactly_the_bridge_tools_with_object_schemas() {
        let mut session = session();
        session.initialized = true;
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": "x", "method": "tools/list" })));
        let tools = answer["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|tool| tool["name"].as_str().unwrap()).collect();
        assert_eq!(names, TOOL_NAMES);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{}", tool["name"]);
            assert!(!tool["description"].as_str().unwrap().is_empty());
        }
        assert_eq!(answer["id"], "x", "string ids are echoed");
    }

    #[test]
    fn tools_call_is_handed_to_the_host_and_bad_calls_are_errors() {
        let mut session = session();
        session.initialized = true;
        match dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": "search_knowledge", "arguments": { "query": "q" } } })) {
            Dispatch::Call { id, tool, arguments } => {
                assert_eq!(id, json!(7));
                assert_eq!(tool, "search_knowledge");
                assert_eq!(arguments, json!({ "query": "q" }));
            }
            _ => panic!("expected a call"),
        }
        match dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": { "name": "present_files" } })) {
            Dispatch::Call { arguments, .. } => assert_eq!(arguments, json!({}), "absent arguments are an empty object"),
            _ => panic!("expected a call"),
        }
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "run_command", "arguments": {} } })));
        assert_eq!(answer["error"]["code"], rpc_code::INVALID_PARAMS, "only the bridge tools exist");
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 10, "method": "tools/call", "params": { "name": "ask_user", "arguments": [1] } })));
        assert_eq!(answer["error"]["code"], rpc_code::INVALID_PARAMS);
        let answer = reply(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 11, "method": "resources/list" })));
        assert_eq!(answer["error"]["code"], rpc_code::METHOD_NOT_FOUND);
    }

    #[test]
    fn notifications_and_client_responses_get_no_answer() {
        let mut session = session();
        assert!(matches!(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })), Dispatch::None));
        assert!(matches!(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 1 } })), Dispatch::None));
        assert!(matches!(dispatch(&mut session, &json!({ "jsonrpc": "2.0", "id": 5, "result": {} })), Dispatch::None));
        assert_eq!(reply(dispatch(&mut session, &json!([1, 2])))["error"]["code"], rpc_code::INVALID_REQUEST);
    }

    #[test]
    fn results_share_the_64_kib_ceiling_and_errors_are_flagged() {
        let ok = call_result(Ok(json!({ "declared": 1 })));
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["content"][0]["text"], "{\"declared\":1}");
        let failed = call_result(Err(ToolError::new("permission.read_only", "blocked")));
        assert_eq!(failed["isError"], true);
        assert_eq!(failed["content"][0]["text"], "[permission.read_only] blocked");
        let big = "汉".repeat(40_000); // 120 000 bytes
        let capped = call_result(Ok(Value::String(big.clone())));
        let text = capped["content"][0]["text"].as_str().unwrap();
        assert!(text.len() <= MAX_TOOL_RESULT_BYTES + 512, "{}", text.len());
        assert!(text.contains("Fox 工具结果上限 65536 字节"));
        assert!(text.starts_with("汉") && text.ends_with("汉"), "head and tail are kept whole");
        assert_eq!(capped["_meta"]["foxResultCapped"], true);
        let exact = "a".repeat(MAX_TOOL_RESULT_BYTES);
        assert_eq!(cap_result_text(&exact), (exact.clone(), false), "a result at the limit is not cut");
    }

    #[test]
    fn injection_is_stdio_for_codex_and_dsh_and_an_extension_for_pi() {
        let grant = McpGrant { endpoint: "127.0.0.1:4000".into(), token: "t".into(), conversation_id: "c".into(), backend: AgentBackend::Codex };
        let assets = McpAssets { relay: "/state/fox-mcp/fox-mcp-relay.mjs".into(), pi_extension: "/state/fox-mcp/fox-mcp-tools.ts".into() };
        for backend in [AgentBackend::Codex, AgentBackend::Deepseek] {
            let Some(McpInjection::AcpStdio(server)) = injection_for(backend, &grant, std::path::Path::new("/rt/node"), &assets) else { panic!("stdio for {backend:?}") };
            assert_eq!(server.name, SERVER_NAME);
            assert_eq!(server.command, std::path::PathBuf::from("/rt/node"));
            assert_eq!(server.args, ["/state/fox-mcp/fox-mcp-relay.mjs"]);
            assert!(server.env.contains(&(TOKEN_ENV.to_owned(), "t".to_owned())));
            assert!(server.env.contains(&(ENDPOINT_ENV.to_owned(), "127.0.0.1:4000".to_owned())));
        }
        let Some(McpInjection::PiExtension { env }) = injection_for(AgentBackend::Pi, &grant, std::path::Path::new("/rt/node"), &assets) else { panic!("pi") };
        assert!(env.contains(&(PI_EXTENSION_ENV.to_owned(), "/state/fox-mcp/fox-mcp-tools.ts".to_owned())));
        assert!(injection_for(AgentBackend::Fox, &grant, std::path::Path::new("/rt/node"), &assets).is_none());
    }

    #[test]
    fn a_skipped_fox_server_is_detected_in_either_place() {
        assert!(!skipped_by_agent(&json!({ "sessionId": "s" })));
        assert!(skipped_by_agent(&json!({ "skippedMcpServers": [SERVER_NAME] })));
        assert!(skipped_by_agent(&json!({ "_meta": { "skippedMcpServers": [{ "name": SERVER_NAME }] } })));
        assert!(!skipped_by_agent(&json!({ "skippedMcpServers": ["other"] })));
    }

    #[test]
    fn assets_are_written_and_an_edited_copy_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let assets = write_assets(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&assets.relay).unwrap(), RELAY_SCRIPT);
        std::fs::write(&assets.pi_extension, "tampered").unwrap();
        let again = write_assets(dir.path()).unwrap();
        assert_eq!(std::fs::read_to_string(&again.pi_extension).unwrap(), PI_EXTENSION_SCRIPT);
    }

    struct EchoHost;
    impl FoxToolHost for EchoHost {
        fn call_tool(&self, context: ToolCallContext, tool: String, arguments: Value) -> BoxFuture<'static, Result<Value, ToolError>> {
            Box::pin(async move {
                if tool == "ask_user" {
                    return Err(ToolError::new("fox_mcp.no_active_run", "no turn"));
                }
                Ok(json!({ "conversation": context.conversation_id, "backend": context.backend.as_str(), "tool": tool, "arguments": arguments, "callId": context.tool_call_id }))
            })
        }
    }

    async fn connect(bridge: &McpBridge, token: &str) -> (tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>, tokio::net::tcp::OwnedWriteHalf) {
        let stream = tokio::net::TcpStream::connect(bridge.endpoint()).await.unwrap();
        let (read, mut write) = stream.into_split();
        write.write_all(format!("{}\n", json!({ "foxMcpToken": token })).as_bytes()).await.unwrap();
        (BufReader::new(read).lines(), write)
    }

    async fn send(write: &mut tokio::net::tcp::OwnedWriteHalf, message: Value) {
        write.write_all(format!("{message}\n").as_bytes()).await.unwrap();
    }

    async fn recv(lines: &mut tokio::io::Lines<BufReader<tokio::net::tcp::OwnedReadHalf>>) -> Option<Value> {
        tokio::time::timeout(std::time::Duration::from_secs(5), lines.next_line()).await.expect("answer in time").unwrap().map(|line| serde_json::from_str(&line).unwrap())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bridge_serves_a_tokened_connection_and_closes_it_on_revoke() {
        let bridge = McpBridge::start(Arc::new(EchoHost), &tokio::runtime::Handle::current()).unwrap();
        let grant = bridge.issue("conv-1", AgentBackend::Deepseek).unwrap();
        assert_eq!(grant.token.len(), 64);
        let (mut lines, mut write) = connect(&bridge, &grant.token).await;
        send(&mut write, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } })).await;
        assert_eq!(recv(&mut lines).await.unwrap()["result"]["protocolVersion"], "2025-06-18");
        send(&mut write, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
        send(&mut write, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "present_files", "arguments": { "files": [{ "path": "a.txt" }], "conversationId": "other" } } })).await;
        let answer = recv(&mut lines).await.unwrap();
        assert_eq!(answer["id"], 2);
        let text: Value = serde_json::from_str(answer["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["conversation"], "conv-1", "the token, not the arguments, names the conversation");
        assert_eq!(text["backend"], "deepseek");
        assert!(text["callId"].as_str().unwrap().starts_with("fox-mcp-"));
        send(&mut write, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "ask_user", "arguments": { "questions": [] } } })).await;
        let answer = recv(&mut lines).await.unwrap();
        assert_eq!(answer["result"]["isError"], true, "a Host refusal is a tool error");
        assert!(answer["result"]["content"][0]["text"].as_str().unwrap().starts_with("[fox_mcp.no_active_run]"));
        // Revoke: the open connection is closed, the token refused afterwards.
        bridge.revoke(&grant.token);
        assert!(recv(&mut lines).await.is_none(), "connection closed on revoke");
        let (mut lines, _write) = connect(&bridge, &grant.token).await;
        let refusal = recv(&mut lines).await.unwrap();
        assert_eq!(refusal["error"]["code"], rpc_code::TOKEN_REFUSED);
        assert!(recv(&mut lines).await.is_none());
        assert_eq!(bridge.live_tokens(), 0);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bridge_refuses_a_wrong_or_missing_token() {
        let bridge = McpBridge::start(Arc::new(EchoHost), &tokio::runtime::Handle::current()).unwrap();
        let _grant = bridge.issue("conv-1", AgentBackend::Codex).unwrap();
        let (mut lines, _write) = connect(&bridge, &"0".repeat(64)).await;
        assert_eq!(recv(&mut lines).await.unwrap()["error"]["code"], rpc_code::TOKEN_REFUSED);
        let stream = tokio::net::TcpStream::connect(bridge.endpoint()).await.unwrap();
        let (read, mut write) = stream.into_split();
        write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n").await.unwrap();
        let mut lines = BufReader::new(read).lines();
        assert_eq!(recv(&mut lines).await.unwrap()["error"]["code"], rpc_code::TOKEN_REFUSED, "MCP without the token line is refused");
    }
}
