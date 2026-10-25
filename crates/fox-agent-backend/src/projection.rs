//! ACP `session/update` -> Fox runtime events (plan 6.7 "`session/update` 到
//! Fox 事件模型的映射"; external backend wiring batch).
//!
//! The desktop Host already persists and renders a small, stable set of
//! runtime events (the same ones the yuxi runtime emits): `run.started`,
//! `message.started` / `message.delta` / `message.completed` (one assistant
//! message per run, id `assistant-<run>`), `reasoning.delta`,
//! `tool.started` / `tool.updated` / `tool.completed` (one tool card per
//! `toolCallId`, the start input is immutable), and the four terminals
//! `run.completed` / `run.cancelled` / `run.interrupted` / `run.failed`.
//! This module turns ACP updates into exactly those shapes so the existing
//! projection (`apply_runtime_event`) and UI components are reused; nothing
//! here touches a database or a window.
//!
//! Rules (each one has a unit test):
//! * the assistant message is started once, on the first visible agent text,
//!   and completed only when the turn ends: text after a tool call keeps
//!   appending to the same message (the Host projection allows exactly one
//!   streaming message per run);
//! * a tool card is started once per `toolCallId`; a later `tool_call` with
//!   the same id, or any `tool_call_update`, is an update; `completed` /
//!   `failed` end the card (`isError` for failed); an update for an id never
//!   started first starts it (with what the update carries);
//! * ACP tool kinds are mapped onto Fox tool names the UI already groups
//!   (`execute` -> `bash`, `read` -> `read_file`, ...); the ACP title and the
//!   first location travel in the input;
//! * `agent_thought_chunk` -> `reasoning.delta`; `plan` -> `plan.updated`
//!   (kept as a run event); session bookkeeping updates (`available_commands_update`,
//!   `current_mode_update`, `usage_update`, ...) are kept as
//!   `agent_backend.update` audit events, never as visible text;
//! * `user_message_chunk` (history replay during `session/load`) is dropped:
//!   the user's messages are already Fox messages;
//! * the stop reason decides the terminal ([`terminal_for_stop_reason`]).

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Fox tool name for an ACP `ToolKind`. Names are the ones the desktop
/// activity view already recognises (`run-activity-presentation.ts`).
pub fn fox_tool_name(kind: Option<&str>) -> &'static str {
    match kind.unwrap_or("other") {
        "read" => "read_file",
        "edit" => "edit_file",
        "delete" => "delete_file",
        "move" => "move_file",
        "search" => "grep",
        "execute" => "bash",
        "think" => "think",
        "fetch" => "web_fetch",
        "switch_mode" => "switch_mode",
        _ => "agent_tool",
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ToolState {
    Running,
    Ended,
}

/// Turns one turn's ACP updates into Fox runtime events. One projector per
/// run (a new turn starts a new projector).
#[derive(Debug, Default)]
pub struct AcpProjector {
    message_started: bool,
    message_completed: bool,
    tools: BTreeMap<String, ToolState>,
    /// Backend status lines taken out of the answer ([`backend_notice_kind`]).
    notices: Vec<Value>,
    delivered: bool,
}

/// Kind of a backend status line that is not part of the answer, or `None`.
///
/// * `model_metadata_missing`: Codex's "Model metadata for `<model>` not
///   found. Defaulting to fallback metadata; ..." (a model outside its
///   built-in catalog, i.e. any non-OpenAI provider);
/// * `network_reconnecting`: Codex's "Reconnecting... <n>/<max>" /
///   "waiting for network" retry notice (the stream cannot reach the model
///   host; inside srt that is usually the network allowlist).
pub fn backend_notice_kind(line: &str) -> Option<&'static str> {
    let line = line.trim();
    if line.contains("Model metadata for") && line.contains("not found") {
        Some("model_metadata_missing")
    } else if line.contains("Reconnecting...") || line.contains("waiting for network") || line.contains("stream connection failed; waiting to retry") {
        Some("network_reconnecting")
    } else {
        None
    }
}

/// Split `text` into (answer text, backend notice lines). Only whole lines
/// that are notices are taken out; everything else stays byte-for-byte.
pub fn split_backend_notices(text: &str) -> (String, Vec<(&'static str, String)>) {
    if backend_notice_kind(text).is_none() {
        return (text.to_owned(), Vec::new());
    }
    let mut kept = String::new();
    let mut notices = Vec::new();
    for line in text.split_inclusive('\n') {
        match backend_notice_kind(line) {
            Some(kind) => notices.push((kind, line.trim().to_owned())),
            None => kept.push_str(line),
        }
    }
    (kept, notices)
}

/// What a finished turn means for the Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnTerminal {
    Completed,
    Cancelled,
    /// Ended early by a limit (`max_tokens`, `max_turn_requests`); the
    /// output may be incomplete. Projected as `run.interrupted`.
    Interrupted { code: String, message: String },
    Failed { code: String, message: String },
}

/// Error code prefix of every external-backend terminal the Host writes.
pub const CODE_PREFIX: &str = "agent_backend_";

/// ACP `StopReason` -> terminal. Unknown values fail (never shown as done).
pub fn terminal_for_stop_reason(stop_reason: &Value) -> TurnTerminal {
    match stop_reason.as_str() {
        Some("end_turn") => TurnTerminal::Completed,
        Some("cancelled") => TurnTerminal::Cancelled,
        Some(reason @ ("max_tokens" | "max_turn_requests")) => TurnTerminal::Interrupted {
            code: format!("{CODE_PREFIX}stop_{reason}"),
            message: format!("外部 Agent 因达到上限（{reason}）提前结束本轮，输出可能不完整"),
        },
        Some("refusal") => TurnTerminal::Failed {
            code: format!("{CODE_PREFIX}refusal"),
            message: "外部 Agent 拒绝继续本轮".to_owned(),
        },
        other => TurnTerminal::Failed {
            code: format!("{CODE_PREFIX}unknown_stop_reason"),
            message: format!("外部 Agent 以未知原因结束本轮：{}", other.map(str::to_owned).unwrap_or_else(|| stop_reason.to_string())),
        },
    }
}

impl TurnTerminal {
    /// The terminal runtime event, with the agent backend and anything extra
    /// (`extra` keys are added to the payload; `type`, `code`, `message` win).
    pub fn event(&self, extra: Value) -> Value {
        let mut payload = match extra {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        let (kind, code, message) = match self {
            Self::Completed => ("run.completed", None, None),
            Self::Cancelled => ("run.cancelled", None, None),
            Self::Interrupted { code, message } => ("run.interrupted", Some(code), Some(message)),
            Self::Failed { code, message } => ("run.failed", Some(code), Some(message)),
        };
        payload.insert("type".into(), json!(kind));
        if let Some(code) = code {
            payload.insert("code".into(), json!(code));
        }
        if let Some(message) = message {
            payload.insert("message".into(), json!(message));
        }
        Value::Object(payload)
    }
}

fn text_of_content(content: &Value) -> Option<String> {
    match content.get("type").and_then(Value::as_str) {
        Some("text") => content.get("text").and_then(Value::as_str).map(str::to_owned),
        Some("resource_link") => content.get("uri").and_then(Value::as_str).map(|uri| format!("[{uri}]")),
        Some(other) => Some(format!("[{other}]")),
        None => None,
    }
}

/// Plain text of a tool call's `content` array (text blocks, diffs as a
/// one-line summary, terminals as their id), for the tool card result.
fn tool_content_summary(content: Option<&Value>) -> Option<String> {
    let items = content?.as_array()?;
    let mut parts = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("content") => {
                if let Some(text) = item.get("content").and_then(text_of_content) {
                    parts.push(text);
                }
            }
            Some("diff") => {
                let path = item.get("path").and_then(Value::as_str).unwrap_or("?");
                parts.push(format!("[diff] {path}"));
            }
            Some("terminal") => {
                let id = item.get("terminalId").and_then(Value::as_str).unwrap_or("?");
                parts.push(format!("[terminal] {id}"));
            }
            _ => {}
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// The immutable `input` of a tool card: the agent's raw input (when it is an
/// object) plus `title`, `acpKind` and the first location as `path`.
fn tool_input(update: &Value) -> Value {
    let mut input = match update.get("rawInput") {
        Some(Value::Object(map)) => map.clone(),
        Some(Value::Null) | None => Map::new(),
        Some(other) => {
            let mut map = Map::new();
            map.insert("raw".into(), other.clone());
            map
        }
    };
    if let Some(title) = update.get("title").and_then(Value::as_str) {
        input.entry("title").or_insert_with(|| json!(title));
    }
    if let Some(kind) = update.get("kind").and_then(Value::as_str) {
        input.entry("acpKind").or_insert_with(|| json!(kind));
    }
    if let Some(path) = update.pointer("/locations/0/path").and_then(Value::as_str) {
        input.entry("path").or_insert_with(|| json!(path));
    }
    Value::Object(input)
}

fn tool_result(update: &Value) -> Value {
    let mut result = Map::new();
    if let Some(status) = update.get("status").and_then(Value::as_str) {
        result.insert("status".into(), json!(status));
    }
    if let Some(text) = tool_content_summary(update.get("content")) {
        result.insert("text".into(), json!(text));
    }
    if let Some(raw) = update.get("rawOutput").filter(|value| !value.is_null()) {
        result.insert("rawOutput".into(), raw.clone());
    }
    if let Some(title) = update.get("title").and_then(Value::as_str) {
        result.insert("title".into(), json!(title));
    }
    Value::Object(result)
}

impl AcpProjector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the assistant message was started (for tests and the Host).
    pub fn message_started(&self) -> bool {
        self.message_started
    }

    pub fn has_delivery(&self) -> bool { self.delivered }

    fn message_delta(&mut self, text: &str, out: &mut Vec<Value>) {
        if text.is_empty() || self.message_completed {
            return;
        }
        if !self.message_started {
            self.message_started = true;
            out.push(json!({ "type": "message.started" }));
        }
        if !text.trim().is_empty() { self.delivered = true; }
        out.push(json!({ "type": "message.delta", "delta": text }));
    }

    fn tool(&mut self, update: &Value, first_seen_as_call: bool, out: &mut Vec<Value>) {
        let Some(id) = update.get("toolCallId").and_then(Value::as_str) else {
            return;
        };
        let status = update.get("status").and_then(Value::as_str);
        match self.tools.get(id) {
            Some(ToolState::Ended) => {
                // Already terminal: keep the late update as an audit record only.
                out.push(json!({ "type": "agent_backend.update", "kind": "tool_call_after_end", "toolCallId": id, "update": update }));
                return;
            }
            Some(ToolState::Running) => {}
            None => {
                let _ = first_seen_as_call;
                let kind = update.get("kind").and_then(Value::as_str);
                out.push(json!({
                    "type": "tool.started",
                    "toolCallId": id,
                    "tool": fox_tool_name(kind),
                    "input": tool_input(update),
                }));
                self.tools.insert(id.to_owned(), ToolState::Running);
                if !matches!(status, Some("completed" | "failed")) {
                    return;
                }
            }
        }
        match status {
            Some("completed" | "failed") => {
                if status == Some("completed") { self.delivered = true; }
                out.push(json!({
                    "type": "tool.completed",
                    "toolCallId": id,
                    "isError": status == Some("failed"),
                    "result": tool_result(update),
                }));
                self.tools.insert(id.to_owned(), ToolState::Ended);
            }
            _ => out.push(json!({ "type": "tool.updated", "toolCallId": id, "update": tool_result(update) })),
        }
    }

    /// Project one ACP `SessionUpdate` (the JSON of `session/update`'s
    /// `update` field) into zero or more Fox runtime events.
    pub fn project(&mut self, update: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or("");
        if update.pointer("/_meta/fox/kind").and_then(Value::as_str) == Some("backend_status") {
            out.push(json!({"type":"agent_backend.diagnostic","kind":"backend_status","update":update}));
            return out;
        }
        match kind {
            "agent_message_chunk" => {
                if let Some(text) = update.get("content").and_then(text_of_content) {
                    let text = self.take_notices(&text, &mut out);
                    self.message_delta(&text, &mut out);
                }
            }
            "agent_thought_chunk" => {
                if let Some(text) = update.get("content").and_then(text_of_content).filter(|text| !text.is_empty()) {
                    let text = self.take_notices(&text, &mut out);
                    if !text.is_empty() {
                        out.push(json!({ "type": "reasoning.delta", "delta": text, "source": "agent_backend" }));
                    }
                }
            }
            "tool_call" => self.tool(update, true, &mut out),
            "tool_call_update" => self.tool(update, false, &mut out),
            "plan" => out.push(json!({ "type": "plan.updated", "entries": update.get("entries").cloned().unwrap_or(json!([])) })),
            // History replay of the user's own text: already a Fox message.
            "user_message_chunk" => {}
            "" => {}
            other => out.push(json!({ "type": "agent_backend.update", "kind": other, "update": update })),
        }
        out
    }

    /// Move backend status lines out of `text` into diagnostics: an
    /// `agent_backend.diagnostic` event each (never `message.delta`), kept
    /// for the terminal's `diagnostics`. Returns the rest of the text.
    fn take_notices(&mut self, text: &str, out: &mut Vec<Value>) -> String {
        let (kept, notices) = split_backend_notices(text);
        for (kind, line) in notices {
            let notice = json!({ "kind": kind, "text": line });
            out.push(json!({ "type": "agent_backend.diagnostic", "kind": kind, "text": line, "source": "agent_backend" }));
            if !self.notices.contains(&notice) {
                self.notices.push(notice);
            }
        }
        kept
    }

    /// Backend status lines taken out of this turn's answer, in order.
    pub fn notices(&self) -> &[Value] {
        &self.notices
    }

    /// Events that close the turn before its terminal: the assistant message
    /// (when one was started) is completed. Idempotent.
    pub fn finish(&mut self) -> Vec<Value> {
        if self.message_started && !self.message_completed {
            self.message_completed = true;
            return vec![json!({ "type": "message.completed" })];
        }
        Vec::new()
    }

    /// Tool cards still running (the terminal projection interrupts them).
    pub fn running_tools(&self) -> Vec<String> {
        self.tools.iter().filter(|(_, state)| **state == ToolState::Running).map(|(id, _)| id.clone()).collect()
    }
}

/// The approval card the Host shows for one ACP `session/request_permission`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionCard {
    /// Title shown on the card (the ACP tool call title, or a fallback).
    pub title: String,
    /// ACP `toolCallId` the request is about, when given.
    pub native_tool_call_id: Option<String>,
    pub tool: &'static str,
    /// What the agent wants to run (raw input + title / kind / path).
    pub input: Value,
    /// Native options, unchanged (id, name, kind).
    pub options: Vec<PermissionOption>,
    /// The only Fox decisions the native options support. `allow_conversation`
    /// is never offered in this batch: an ACP `allow_always` grant lives in the
    /// backend, Fox cannot show or revoke it (plan 6.3 rule 2).
    pub available_decisions: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

/// Build the approval card from the raw `RequestPermissionRequest` JSON.
pub fn permission_card(request: &Value) -> PermissionCard {
    let tool_call = request.get("toolCall").cloned().unwrap_or(Value::Null);
    let options: Vec<PermissionOption> = request
        .get("options")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(PermissionOption {
                        option_id: item.get("optionId")?.as_str()?.to_owned(),
                        name: item.get("name").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        kind: item.get("kind").and_then(Value::as_str).unwrap_or_default().to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let mut available = Vec::new();
    if allow_option(&options).is_some() {
        available.push("allow_once");
    }
    available.push("deny");
    PermissionCard {
        title: tool_call
            .get("title")
            .and_then(Value::as_str)
            .filter(|title| !title.trim().is_empty())
            .unwrap_or("外部 Agent 请求执行操作")
            .to_owned(),
        native_tool_call_id: tool_call.get("toolCallId").and_then(Value::as_str).map(str::to_owned),
        tool: fox_tool_name(tool_call.get("kind").and_then(Value::as_str)),
        input: tool_input(&tool_call),
        options,
        available_decisions: available,
    }
}

/// The native option that means "allow this once": `allow_once` kind first,
/// then (only if nothing else allows) nothing. `allow_always` is never picked
/// for a one-shot Fox approval.
pub fn allow_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options.iter().find(|option| option.kind == "allow_once")
}

/// The native option that means "reject this once" (`reject_once`, else
/// `reject_always` as the only rejecting choice). `None` = answer the request
/// as `cancelled`.
pub fn reject_option(options: &[PermissionOption]) -> Option<&PermissionOption> {
    options
        .iter()
        .find(|option| option.kind == "reject_once")
        .or_else(|| options.iter().find(|option| option.kind == "reject_always"))
}

/// Fox's execution mode (the project / conversation `permissionMode`: 询问 /
/// 自动 / 只读) applied to an external backend's `session/request_permission`
/// (settings batch, item 7). It only decides how a native permission request
/// is answered; the sandbox and the permission mode of the session
/// (sandboxed / full access) are untouched: "自动" is not full access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExecutionMode {
    /// `ask`: every request becomes an approval card (unchanged behaviour).
    #[default]
    Ask,
    /// `allow`: Fox answers the native one-shot allow (`allow_once`) itself
    /// and records it; no card. Never widened to `allow_always`.
    Allow,
    /// `read_only`: read-only kinds are allowed once; anything that may
    /// modify (and anything Fox cannot classify) is rejected.
    ReadOnly,
}

impl ExecutionMode {
    /// Parse Fox's stored value (`ask` / `allow` / `read_only`). Anything
    /// else is `Ask`: an unreadable mode never auto-allows.
    pub fn parse(value: &str) -> Self {
        match value.trim() {
            "allow" => Self::Allow,
            "read_only" => Self::ReadOnly,
            _ => Self::Ask,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow => "allow",
            Self::ReadOnly => "read_only",
        }
    }
}

/// ACP tool kinds `read_only` lets through. Deliberately short: `fetch`
/// (network egress), `switch_mode`, `other`, a missing kind and every kind
/// added to ACP later are treated as possibly modifying and rejected.
pub const READ_ONLY_TOOL_KINDS: [&str; 3] = ["read", "search", "think"];

/// How Fox answers one native permission request under an [`ExecutionMode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoPermission {
    /// Show the approval card.
    Ask,
    /// Answer this native option (an `allow_once`) without a card.
    Allow { option_id: String },
    /// Reject without a card: this native option (`reject_once`, else
    /// `reject_always`), or `None` = answer `cancelled`.
    Reject { option_id: Option<String> },
}

impl AutoPermission {
    pub fn decision(&self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Allow { .. } => "allow_once",
            Self::Reject { .. } => "reject",
        }
    }
}

/// The ACP tool kind of a permission request (`toolCall.kind`), if given.
pub fn permission_tool_kind(request: &Value) -> Option<&str> {
    request.get("toolCall").and_then(|call| call.get("kind")).and_then(Value::as_str).map(str::trim).filter(|kind| !kind.is_empty())
}

/// Decide a native permission request under `mode` (conservative: a request
/// Fox cannot classify is treated as modifying).
///
/// * `Ask`: always the card.
/// * `Allow`: the native `allow_once`; with no `allow_once` option the card
///   is shown (Fox never picks `allow_always`).
/// * `ReadOnly`: kinds in [`READ_ONLY_TOOL_KINDS`] are allowed once (the card
///   when no `allow_once` exists); `edit` / `delete` / `move` / `execute`,
///   any other kind and a missing kind are rejected.
pub fn auto_permission(mode: ExecutionMode, request: &Value, options: &[PermissionOption]) -> AutoPermission {
    let allow = || allow_option(options).map(|option| AutoPermission::Allow { option_id: option.option_id.clone() }).unwrap_or(AutoPermission::Ask);
    match mode {
        ExecutionMode::Ask => AutoPermission::Ask,
        ExecutionMode::Allow => allow(),
        ExecutionMode::ReadOnly => match permission_tool_kind(request) {
            Some(kind) if READ_ONLY_TOOL_KINDS.contains(&kind) => allow(),
            _ => AutoPermission::Reject { option_id: reject_option(options).map(|option| option.option_id.clone()) },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &str, text: &str) -> Value {
        json!({ "sessionUpdate": kind, "content": { "type": "text", "text": text } })
    }

    fn types(events: &[Value]) -> Vec<&str> {
        events.iter().map(|event| event["type"].as_str().unwrap()).collect()
    }

    #[test]
    fn backend_warnings_go_to_diagnostics_not_the_answer() {
        let mut projector = AcpProjector::new();
        let warning = "Model metadata for `deepseek-chat` not found. Defaulting to fallback metadata; this can degrade performance and cause issues.\n";
        let events = projector.project(&chunk("agent_message_chunk", warning));
        assert_eq!(types(&events), ["agent_backend.diagnostic"]);
        assert_eq!(events[0]["kind"], "model_metadata_missing");
        assert!(!projector.message_started(), "the warning never starts the answer");
        let events = projector.project(&chunk("agent_message_chunk", "⚠ Reconnecting... 2/5\nHello"));
        assert_eq!(types(&events), ["agent_backend.diagnostic", "message.started", "message.delta"]);
        assert_eq!(events[2]["delta"], "Hello");
        assert_eq!(projector.notices().len(), 2);
        // Ordinary text that merely mentions a model is untouched.
        let events = projector.project(&chunk("agent_message_chunk", "metadata for the model was found"));
        assert_eq!(events[0]["delta"], "metadata for the model was found");
        assert_eq!(backend_notice_kind("Reconnecting... waiting for network"), Some("network_reconnecting"));
    }

    #[test]
    fn one_assistant_message_spans_the_turn_and_completes_at_the_end() {
        let mut projector = AcpProjector::new();
        assert_eq!(types(&projector.project(&chunk("agent_message_chunk", "Hel"))), ["message.started", "message.delta"]);
        assert_eq!(types(&projector.project(&chunk("agent_message_chunk", "lo"))), ["message.delta"]);
        let tool = json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "ls", "kind": "execute", "status": "pending", "rawInput": { "command": "ls" } });
        assert_eq!(types(&projector.project(&tool)), ["tool.started"]);
        // Text after a tool call keeps appending to the same message.
        assert_eq!(types(&projector.project(&chunk("agent_message_chunk", "!"))), ["message.delta"]);
        assert_eq!(types(&projector.finish()), ["message.completed"]);
        assert!(projector.finish().is_empty(), "finish is idempotent");
        assert!(projector.project(&chunk("agent_message_chunk", "late")).is_empty(), "no text after completion");
        assert!(projector.project(&chunk("agent_message_chunk", "")).is_empty());
    }

    #[test]
    fn thoughts_plans_and_bookkeeping_never_become_visible_text() {
        let mut projector = AcpProjector::new();
        let thought = projector.project(&chunk("agent_thought_chunk", "thinking"));
        assert_eq!(thought, [json!({ "type": "reasoning.delta", "delta": "thinking", "source": "agent_backend" })]);
        let plan = projector.project(&json!({ "sessionUpdate": "plan", "entries": [{ "content": "a", "priority": "high", "status": "pending" }] }));
        assert_eq!(types(&plan), ["plan.updated"]);
        assert_eq!(plan[0]["entries"][0]["content"], "a");
        let mode = projector.project(&json!({ "sessionUpdate": "current_mode_update", "currentModeId": "read-only" }));
        assert_eq!(types(&mode), ["agent_backend.update"]);
        assert!(projector.project(&chunk("user_message_chunk", "replayed")).is_empty(), "replayed user text dropped");
        assert!(!projector.message_started());
        assert!(projector.finish().is_empty(), "no message when the agent said nothing");
    }

    #[test]
    fn tool_cards_start_once_update_and_end_with_error_flag() {
        let mut projector = AcpProjector::new();
        let call = json!({ "sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read a.txt", "kind": "read",
            "status": "pending", "locations": [{ "path": "/p/a.txt" }], "rawInput": { "limit": 5 } });
        let started = projector.project(&call);
        assert_eq!(started[0]["tool"], "read_file");
        assert_eq!(started[0]["input"], json!({ "limit": 5, "title": "Read a.txt", "acpKind": "read", "path": "/p/a.txt" }));
        // Same id again: an update, never a second start (input is immutable).
        assert_eq!(types(&projector.project(&call)), ["tool.updated"]);
        let progress = projector.project(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "in_progress" }));
        assert_eq!(types(&progress), ["tool.updated"]);
        assert_eq!(projector.running_tools(), ["t1"]);
        let done = projector.project(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "failed",
            "content": [{ "type": "content", "content": { "type": "text", "text": "ENOENT" } }] }));
        assert_eq!(types(&done), ["tool.completed"]);
        assert_eq!(done[0]["isError"], true);
        assert_eq!(done[0]["result"]["text"], "ENOENT");
        assert!(projector.running_tools().is_empty());
        let late = projector.project(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" }));
        assert_eq!(types(&late), ["agent_backend.update"], "nothing reopens an ended card");
    }

    #[test]
    fn an_update_for_an_unknown_tool_starts_it_and_an_instant_completion_ends_it() {
        let mut projector = AcpProjector::new();
        let events = projector.project(&json!({ "sessionUpdate": "tool_call_update", "toolCallId": "x", "kind": "execute", "status": "completed",
            "rawOutput": { "exitCode": 0 } }));
        assert_eq!(types(&events), ["tool.started", "tool.completed"]);
        assert_eq!(events[0]["tool"], "bash");
        assert_eq!(events[1]["isError"], false);
        assert_eq!(events[1]["result"]["rawOutput"]["exitCode"], 0);
        assert!(projector.project(&json!({ "sessionUpdate": "tool_call_update" })).is_empty(), "no id, no card");
    }

    #[test]
    fn stop_reasons_map_to_one_terminal_each() {
        assert_eq!(terminal_for_stop_reason(&json!("end_turn")), TurnTerminal::Completed);
        assert_eq!(terminal_for_stop_reason(&json!("cancelled")), TurnTerminal::Cancelled);
        assert!(matches!(terminal_for_stop_reason(&json!("max_tokens")), TurnTerminal::Interrupted { code, .. } if code == "agent_backend_stop_max_tokens"));
        assert!(matches!(terminal_for_stop_reason(&json!("max_turn_requests")), TurnTerminal::Interrupted { .. }));
        assert!(matches!(terminal_for_stop_reason(&json!("refusal")), TurnTerminal::Failed { code, .. } if code == "agent_backend_refusal"));
        assert!(matches!(terminal_for_stop_reason(&json!("weird")), TurnTerminal::Failed { code, .. } if code == "agent_backend_unknown_stop_reason"));
        let event = TurnTerminal::Failed { code: "c".into(), message: "m".into() }.event(json!({ "agentBackend": "pi", "type": "x" }));
        assert_eq!(event, json!({ "type": "run.failed", "code": "c", "message": "m", "agentBackend": "pi" }));
        assert_eq!(TurnTerminal::Completed.event(Value::Null), json!({ "type": "run.completed" }));
    }

    #[test]
    fn permission_cards_offer_only_native_one_shot_choices() {
        let request = json!({ "sessionId": "s", "toolCall": { "toolCallId": "t1", "title": "run npm test", "kind": "execute", "rawInput": { "command": "npm test" } },
            "options": [
                { "optionId": "always", "name": "Always", "kind": "allow_always" },
                { "optionId": "allow", "name": "Allow", "kind": "allow_once" },
                { "optionId": "reject", "name": "Reject", "kind": "reject_once" }
            ] });
        let card = permission_card(&request);
        assert_eq!(card.title, "run npm test");
        assert_eq!(card.tool, "bash");
        assert_eq!(card.native_tool_call_id.as_deref(), Some("t1"));
        assert_eq!(card.available_decisions, ["allow_once", "deny"], "allow_always is never offered");
        assert_eq!(allow_option(&card.options).unwrap().option_id, "allow");
        assert_eq!(reject_option(&card.options).unwrap().option_id, "reject");
        assert_eq!(card.input["command"], "npm test");

        // pi gate (yes / no) and dsh style options.
        let pi = permission_card(&json!({ "toolCall": {}, "options": [
            { "optionId": "yes", "name": "Yes", "kind": "allow_once" }, { "optionId": "no", "name": "No", "kind": "reject_once" } ] }));
        assert_eq!(pi.title, "外部 Agent 请求执行操作");
        assert_eq!(allow_option(&pi.options).unwrap().option_id, "yes");
        // Only rejecting options: no allow decision is offered.
        let reject_only = permission_card(&json!({ "toolCall": { "title": "x" }, "options": [{ "optionId": "r", "name": "R", "kind": "reject_always" }] }));
        assert_eq!(reject_only.available_decisions, ["deny"]);
        assert_eq!(reject_option(&reject_only.options).unwrap().option_id, "r");
        assert!(reject_option(&[]).is_none());
    }

    fn perm_request(kind: Option<&str>, options: Value) -> (Value, Vec<PermissionOption>) {
        let mut call = json!({ "toolCallId": "t1", "title": "x" });
        if let Some(kind) = kind {
            call["kind"] = json!(kind);
        }
        let request = json!({ "sessionId": "s", "toolCall": call, "options": options });
        let options = permission_card(&request).options;
        (request, options)
    }

    fn full_options() -> Value {
        json!([
            { "optionId": "always", "name": "Always", "kind": "allow_always" },
            { "optionId": "once", "name": "Once", "kind": "allow_once" },
            { "optionId": "no", "name": "No", "kind": "reject_once" }
        ])
    }

    #[test]
    fn execution_mode_parses_fox_values_and_defaults_to_ask() {
        assert_eq!(ExecutionMode::parse("ask"), ExecutionMode::Ask);
        assert_eq!(ExecutionMode::parse("allow"), ExecutionMode::Allow);
        assert_eq!(ExecutionMode::parse("read_only"), ExecutionMode::ReadOnly);
        for junk in ["", "full_access", "ALLOW", "readonly"] {
            assert_eq!(ExecutionMode::parse(junk), ExecutionMode::Ask, "{junk:?} never auto-allows");
        }
        assert_eq!(ExecutionMode::ReadOnly.as_str(), "read_only");
    }

    #[test]
    fn ask_mode_always_shows_the_card() {
        for kind in [Some("read"), Some("execute"), None] {
            let (request, options) = perm_request(kind, full_options());
            assert_eq!(auto_permission(ExecutionMode::Ask, &request, &options), AutoPermission::Ask);
        }
    }

    #[test]
    fn allow_mode_answers_allow_once_and_never_allow_always() {
        for kind in [Some("execute"), Some("edit"), Some("read"), None] {
            let (request, options) = perm_request(kind, full_options());
            assert_eq!(auto_permission(ExecutionMode::Allow, &request, &options), AutoPermission::Allow { option_id: "once".into() });
        }
        let (request, options) = perm_request(Some("execute"), json!([{ "optionId": "always", "name": "A", "kind": "allow_always" }, { "optionId": "no", "name": "N", "kind": "reject_once" }]));
        assert_eq!(auto_permission(ExecutionMode::Allow, &request, &options), AutoPermission::Ask, "no allow_once: the card, not allow_always");
    }

    #[test]
    fn read_only_mode_rejects_modifying_and_unknown_kinds_and_allows_reads() {
        for kind in ["read", "search", "think"] {
            let (request, options) = perm_request(Some(kind), full_options());
            assert_eq!(auto_permission(ExecutionMode::ReadOnly, &request, &options), AutoPermission::Allow { option_id: "once".into() }, "{kind}");
        }
        for kind in [Some("edit"), Some("delete"), Some("move"), Some("execute"), Some("fetch"), Some("switch_mode"), Some("other"), Some("brand_new_kind"), Some("  "), None] {
            let (request, options) = perm_request(kind, full_options());
            assert_eq!(auto_permission(ExecutionMode::ReadOnly, &request, &options), AutoPermission::Reject { option_id: Some("no".into()) }, "{kind:?}");
        }
        let (request, options) = perm_request(Some("edit"), json!([{ "optionId": "once", "name": "O", "kind": "allow_once" }]));
        assert_eq!(auto_permission(ExecutionMode::ReadOnly, &request, &options), AutoPermission::Reject { option_id: None }, "no reject option: cancelled");
        let (request, options) = perm_request(Some("read"), json!([{ "optionId": "no", "name": "N", "kind": "reject_once" }]));
        assert_eq!(auto_permission(ExecutionMode::ReadOnly, &request, &options), AutoPermission::Ask, "a read with no allow_once falls back to the card");
    }
}
