//! Host-owned, already-settled batch handoff. Never an approval or dispatch request.
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A Host-owned terminal fact. A model or a user-shaped message cannot mint
/// one: the Host checks every identity against the durable notice ledger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostJobNotice {
    pub source: String,
    pub data_root_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub job_id: String,
    pub attempt: u32,
    pub terminal_state: String,
    pub finished_at: i64,
    pub result_ref: Option<String>,
    pub result_sha256: Option<String>,
    pub result_bytes: Option<u64>,
    pub error_code: Option<String>,
}

impl HostJobNotice {
    pub fn validate(&self) -> Result<(), String> {
        if self.source != "fox_kernel_host"
            || [&self.data_root_id, &self.conversation_id, &self.run_id, &self.job_id]
                .iter().any(|value| value.trim().is_empty())
            || !matches!(self.terminal_state.as_str(), "completed" | "failed" | "cancelled")
            || (self.terminal_state == "completed") != self.result_ref.is_some()
            || self.result_ref.is_some() != self.result_sha256.is_some()
            || self.result_ref.is_some() != self.result_bytes.is_some()
            || self.result_ref.as_ref().is_some_and(|value| value.trim().is_empty())
            || self.result_sha256.as_ref().is_some_and(|value| {
                value.len() != 71 || !value.starts_with("sha256:")
                    || !value[7..].bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
            || serde_json::to_vec(self).map_err(|_| "invalid Host job notice")?.len() > 2_048
        {
            return Err("invalid Host job notice identity, result or size".into());
        }
        Ok(())
    }

    pub fn history_marker(&self) -> Value {
        serde_json::json!({"role":"hostJobNotice","notice":self})
    }
}

pub fn validate_host_job_notices(notices: &[HostJobNotice]) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    let mut bytes = 0usize;
    for notice in notices {
        notice.validate()?;
        if !seen.insert(&notice.job_id) { return Err("duplicate Host job notice".into()); }
        bytes += serde_json::to_vec(notice).map_err(|_| "invalid Host job notice")?.len();
        if bytes > 16 * 1024 { return Err("Host job notices exceed model input limit".into()); }
    }
    Ok(())
}

pub fn historical_host_job_notice_bytes(history: &[Value]) -> Result<usize, String> {
    let mut bytes = 0usize;
    for message in history.iter().filter(|value| value["role"] == "hostJobNotice") {
        let notice: HostJobNotice = serde_json::from_value(message["notice"].clone())
            .map_err(|_| "invalid historical Host job notice")?;
        notice.validate()?;
        bytes = bytes.saturating_add(serde_json::to_vec(&notice)
            .map_err(|_| "invalid historical Host job notice")?.len());
    }
    Ok(bytes)
}

/// Sanitized evidence of a settled model failure, never a permission to retry.
/// Host still owns retry admission, counters, delay, lease and cancellation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelModelFailure {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub checkpoint_seq: u64,
    pub category: String,
    pub http_status: Option<u16>,
    pub retry_after_ms: Option<u64>,
    /// Low-sensitivity progress telemetry captured by the worker when the
    /// round failed. Never contains credentials, model input, or complete
    /// tool arguments: only elapsed times and output byte counts, so Host can
    /// distinguish slow generation from a stalled stream. Absent for old
    /// workers and for non-timeout categories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub telemetry: Option<KernelModelTelemetry>,
}

/// Byte/time counters for one failed model round. All values are worker-side
/// observations for diagnosis; Host timeout anchors remain authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelModelTelemetry {
    pub elapsed_ms: i64,
    pub idle_elapsed_ms: i64,
    pub first_response_ms: Option<i64>,
    pub text_bytes: u64,
    pub reasoning_bytes: u64,
    pub tool_param_bytes: u64,
}

impl KernelModelFailure {
    pub fn validate(&self) -> Result<(), String> {
        let category_valid = match self.category.as_str() {
            "provider_unavailable" => self.http_status.is_some_and(|status| matches!(status, 429 | 500 | 502 | 503 | 504 | 529)),
            "incomplete_response" => self.http_status.is_none() && self.retry_after_ms.is_none(),
            // Worker-side first-response/idle/whole-round timeout. Carries no
            // HTTP semantics; retry admission additionally requires that no
            // new tool was dispatched in the failed round (Host checks durable
            // tool state, never this flag alone).
            "model_timeout" | "model_transport_failure" => self.http_status.is_none() && self.retry_after_ms.is_none(),
            _ => false,
        };
        if let Some(telemetry) = &self.telemetry {
            if telemetry.elapsed_ms < 0 || telemetry.idle_elapsed_ms < 0
                || telemetry.elapsed_ms > 86_400_000 || telemetry.idle_elapsed_ms > 86_400_000
                || telemetry.first_response_ms.is_some_and(|value| value < 0 || value > 86_400_000)
                || telemetry.text_bytes > 67_108_864 || telemetry.reasoning_bytes > 67_108_864
                || telemetry.tool_param_bytes > 67_108_864 {
                return Err("invalid Kernel model failure telemetry".into());
            }
        }
        if self.schema_version != 1 || !category_valid
            || [&self.run_id, &self.turn_id].iter().any(|id| id.trim().is_empty() || id.len() > 512)
            || self.checkpoint_seq == 0 || self.checkpoint_seq > 9_007_199_254_740_991
            || self.retry_after_ms.is_some_and(|delay| delay > 86_400_000) {
            return Err("invalid settled Kernel model failure".into());
        }
        Ok(())
    }
}
use schemars::JsonSchema;

/// Display content only. Host may persist partial text; never a decision or replay input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelModelPreview {
    pub schema_version: u32,
    pub run_id: String,
    pub conversation_id: String,
    pub turn_id: String,
    pub checkpoint_seq: u64,
    pub revision: u64,
    pub text: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub reasoning: String,
    /// Cumulative output bytes observed by the worker this round, including
    /// tool-parameter bytes that never appear in `text`. Lets Host tell slow
    /// generation apart from a stalled stream without receiving the content.
    /// Absent for old workers; display code must treat absence as unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_bytes: Option<u64>,
}

impl KernelModelPreview {
    pub fn validate(&self) -> Result<(),String> {
        if self.schema_version!=1 || [&self.run_id,&self.conversation_id,&self.turn_id].iter()
            .any(|id|id.trim().is_empty() || id.len()>512) || self.checkpoint_seq==0 || self.checkpoint_seq>9_007_199_254_740_991
            || self.revision==0 || self.revision>9_007_199_254_740_991 || self.text.len()>262_144 || self.reasoning.len()>262_144
            || self.progress_bytes.is_some_and(|bytes| bytes > 67_108_864) {
            return Err("invalid transient Kernel model preview".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KernelSettledToolState { Completed, Failed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelEngineBatchCheckpoint {
    pub schema_version: u32,
    pub batch_id: String,
    pub history: Vec<Value>,
    pub assistant_message: Value,
}

/// One model response to an identified, durable batch delivery. History and
/// the next batch identity remain Host-owned and are never supplied by Node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelModelResponse {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub batch_id: String,
    pub checkpoint_seq: u64,
    pub assistant_message: Value,
}

impl KernelModelResponse {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.run_id.trim().is_empty() || self.turn_id.trim().is_empty() || self.batch_id.trim().is_empty()
            || self.checkpoint_seq == 0 || self.checkpoint_seq > 9_007_199_254_740_991
            || serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > 1_048_576 {
            return Err("invalid Kernel model response identity or size".into());
        }
        validate_model_message(&self.assistant_message)
    }
}

fn validate_model_message(message: &Value) -> Result<(), String> {
        if message["stopReason"] == "toolUse" { return validate_checkpoint_parts(&[], message); }
        if message["role"] != "assistant" || message["stopReason"] != "stop" {
            return Err("model response is neither a completed answer nor a tool proposal".into());
        }
        let content = message["content"].as_array().ok_or("missing model response content")?;
        if content.iter().any(|block| !(block["type"] == "text" && block["text"].is_string()
            || block["type"] == "thinking" && block["thinking"].is_string())) {
            return Err("invalid completed model response content".into());
        }
        if !content.iter().any(|block| block["type"] == "text"
            && block["text"].as_str().is_some_and(|text| !text.trim().is_empty())) {
            return Err("completed model response has no public answer".into());
        }
        Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelInitialModelFrame {
    pub schema_version: u32,
    pub input: KernelInitialModelInput,
    pub idempotency_key: String,
    pub checkpoint_seq: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_job_notices: Vec<HostJobNotice>,
    /// Present when a fresh worker restores a durably leased continuation input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation_key: Option<String>,
}

impl KernelInitialModelFrame {
    pub fn validate(&self) -> Result<(), String> {
        validate_host_job_notices(&self.host_job_notices)?;
        if historical_host_job_notice_bytes(&self.input.messages)?
            + serde_json::to_vec(&self.host_job_notices).map_err(|_| "invalid Host job notices")?.len()
            > 16 * 1024 { return Err("Host job notice input exceeds total limit".into()); }
        self.input.validate()?;
        let expected_key = match &self.continuation_key {
            Some(key) if key.strip_prefix("continuation:").is_some_and(|seq|
                seq.parse::<u64>().is_ok_and(|n| n > 0 && n <= 9_007_199_254_740_991 && n.to_string() == seq)) => format!("continuation-delivery:{key}"),
            Some(_) => return Err("invalid continuation delivery identity".into()),
            None => "initial-model-delivery".into(),
        };
        if self.schema_version != 1 || self.idempotency_key != expected_key
            || self.checkpoint_seq == 0 || self.checkpoint_seq > 9_007_199_254_740_991
            || serde_json::to_vec(self).map_err(|_| "invalid initial model frame")?.len() > 1_048_576 {
            return Err("invalid initial model delivery frame".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelInitialModelResponse {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub checkpoint_seq: u64,
    pub assistant_message: Value,
}

impl KernelInitialModelResponse {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.run_id.trim().is_empty() || self.turn_id.trim().is_empty()
            || self.checkpoint_seq == 0 || self.checkpoint_seq > 9_007_199_254_740_991
            || serde_json::to_vec(self).map_err(|_| "invalid initial model response")?.len() > 1_048_576 {
            return Err("invalid initial model response identity or size".into());
        }
        validate_model_message(&self.assistant_message)
    }
}

#[cfg(test)]
#[test]
fn model_response_cannot_claim_completion_with_tools_or_unfinished_output() {
    let mut response = KernelModelResponse {
        schema_version: 1, run_id: "run".into(), turn_id: "turn".into(), batch_id: "batch".into(), checkpoint_seq: 8,
        assistant_message: serde_json::json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"done"}]}),
    };
    assert!(response.validate().is_ok());
    for content in [serde_json::json!([]), serde_json::json!([{"type":"thinking","thinking":"I will analyze"}]),
        serde_json::json!([{"type":"text","text":" \n\t"}])] {
        let mut empty = response.clone();
        empty.assistant_message["content"] = content;
        assert!(empty.validate().is_err());
    }
    response.assistant_message["stopReason"] = serde_json::json!("length");
    assert!(response.validate().is_err());
    response.assistant_message = serde_json::json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"next","name":"read","arguments":{"path":"next.txt"}}]});
    assert!(response.validate().is_ok());
    response.assistant_message["stopReason"] = serde_json::json!("stop");
    assert!(response.validate().is_err());
    response.checkpoint_seq = 0;
    assert!(response.validate().is_err());
}

pub fn validate_kernel_history(history: &[Value]) -> Result<(), String> {
    let mut pending = std::collections::HashMap::new();
    let mut host_jobs = std::collections::HashSet::new();
    for message in history {
        match message["role"].as_str() {
            Some("hostJobNotice") => {
                if !pending.is_empty() { return Err("Host notice interrupts tool results".into()); }
                let notice: HostJobNotice = serde_json::from_value(message["notice"].clone())
                    .map_err(|_| "invalid historical Host job notice")?;
                notice.validate()?;
                if !host_jobs.insert(notice.job_id) { return Err("duplicate historical Host job notice".into()); }
                if message.as_object().is_none_or(|value| value.len() != 2) {
                    return Err("invalid historical Host job notice marker".into());
                }
            }
            Some("toolResult") => {
                let id = message["toolCallId"].as_str().ok_or("missing historical result id")?;
                let name = message["toolName"].as_str().ok_or("missing historical result name")?;
                if id.trim().is_empty() || name.trim().is_empty() || pending.remove(id) != Some(name) { return Err("orphan or mismatched historical result".into()); }
                if !message["isError"].is_boolean() { return Err("historical result has no error classification".into()); }
                validate_result_content(message)?;
            }
            Some("user" | "assistant") => {
                if !pending.is_empty() { return Err("incomplete historical tool batch".into()); }
                if !message["content"].is_string() && !message["content"].is_array() { return Err("invalid history content".into()); }
                if message["role"] == "assistant" {
                    if let Some(blocks) = message["content"].as_array() {
                        for block in blocks.iter().filter(|block| block["type"] == "toolCall") {
                            let id = block["id"].as_str().filter(|value| !value.trim().is_empty()).ok_or("invalid historical tool id")?;
                            let name = block["name"].as_str().filter(|value| !value.trim().is_empty()).ok_or("invalid historical tool name")?;
                            if pending.insert(id, name).is_some() { return Err("duplicate historical tool id".into()); }
                        }
                    }
                }
            }
            _ => return Err("unsupported history message".into()),
        }
    }
    if !pending.is_empty() { return Err("incomplete historical tool batch".into()); }
    if historical_host_job_notice_bytes(history)? > 16 * 1024 {
        return Err("historical Host job notices exceed input limit".into());
    }
    Ok(())
}

fn validate_checkpoint_parts(history: &[Value], assistant: &Value) -> Result<(), String> {
    validate_kernel_history(history)?;
    if assistant["role"] != "assistant" || assistant["stopReason"] != "toolUse" { return Err("missing original assistant tool proposal".into()); }
    let content = assistant["content"].as_array().ok_or("missing assistant content")?;
    let calls = content.iter().filter(|block| block["type"] == "toolCall").collect::<Vec<_>>();
    if calls.is_empty() || calls.len() > 64 { return Err("invalid engine tool batch size".into()); }
    let mut ids = std::collections::HashSet::new();
    for call in calls {
        let id = call["id"].as_str().filter(|value| !value.trim().is_empty()).ok_or("invalid proposed tool id")?;
        let tool = call["name"].as_str().ok_or("missing proposed tool name")?;
        if !ids.insert(id) || !call["arguments"].is_object() || super::canonical_runtime_tool_contract(tool).is_none() {
            return Err("invalid proposed tool identity or input".into());
        }
    }
    Ok(())
}

/// Host-owned initial prompt snapshot, not a dispatch request or permission.
/// A separate durable dispatch intent is required before invoking an engine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelInitialModelInput {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub prompt_config_hash: String,
    pub messages: Vec<Value>,
}

impl KernelInitialModelInput {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.run_id.trim().is_empty() || self.turn_id.trim().is_empty()
            || self.prompt_config_hash.trim().is_empty() || self.messages.is_empty()
            || serde_json::to_vec(self).map_err(|_| "invalid initial input")?.len() > 1_048_576 {
            return Err("invalid Kernel initial input identity or size".into());
        }
        validate_kernel_history(&self.messages)?;
        if self.messages.last().is_none_or(|message| message["role"] != "user") {
            return Err("Kernel initial input must end with the current user message".into());
        }
        // Accept only adapter-supported blocks. In particular a user message
        // cannot smuggle tool calls/results into the model's input history.
        for message in &self.messages {
            if message["role"] == "assistant" {
                let calls = message["content"].as_array().is_some_and(|blocks| blocks.iter().any(|block| block["type"] == "toolCall"));
                if let Some(reason) = message.get("stopReason") {
                    if reason != if calls { "toolUse" } else { "stop" } {
                        return Err("unfinished or inconsistent historical assistant message".into());
                    }
                }
            }
            if let Some(blocks) = message["content"].as_array() {
                for block in blocks {
                    let valid = block["type"] == "text" && block["text"].is_string()
                        || message["role"] != "assistant" && block["type"] == "image"
                            && block["data"].is_string() && block["mimeType"].is_string()
                        || message["role"] == "assistant" && block["type"] == "thinking" && block["thinking"].is_string()
                        || message["role"] == "assistant" && block["type"] == "toolCall" && block["arguments"].is_object();
                    if !valid { return Err("unsupported Kernel initial input content".into()); }
                }
            }
        }
        Ok(())
    }
}

fn validate_result_content(result: &Value) -> Result<(), String> {
    let blocks = result.get("content").and_then(Value::as_array).ok_or("missing durable result content")?;
    for block in blocks {
        let valid = block["type"] == "text" && block["text"].is_string()
            || block["type"] == "image" && block["data"].is_string() && block["mimeType"].is_string();
        if !valid { return Err("unsupported durable result content".into()); }
    }
    Ok(())
}

impl KernelEngineBatchCheckpoint {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.batch_id.trim().is_empty() || serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > 1_048_576 {
            return Err("invalid engine checkpoint identity or size".into());
        }
        validate_checkpoint_parts(&self.history, &self.assistant_message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelSettledToolResult {
    pub tool_call_id: String,
    pub tool: String,
    pub canonical_input: Value,
    pub source_order: u32,
    pub state: KernelSettledToolState,
    pub result: Value,
    /// Storage facts supplied by the Host, never by the executed tool.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<Value>,
}

/// One engine round output from a live loop session: either a tool proposal or
/// a completed answer. History, batch identity and the checkpoint cursor remain
/// Host-owned and are never supplied or echoed by Node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelRoundOutputFrame {
    pub schema_version: u32,
    pub turn_id: String,
    pub assistant_message: Value,
}

impl KernelRoundOutputFrame {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 || self.turn_id.trim().is_empty() || self.turn_id.len() > 512
            || serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > 1_048_576 {
            return Err("invalid Kernel round output frame".into());
        }
        validate_model_message(&self.assistant_message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum KernelRoundDirectiveKind { Batch, Continuation, Final }

/// Host decision after committing one engine round output. `Batch` hands the
/// settled durable results of the proposed batch back to the live session;
/// `Continuation` injects a bounded Host-authored review prompt; `Final` ends
/// the loop. The engine never derives any of these itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelRoundDirective {
    pub schema_version: u32,
    pub kind: KernelRoundDirectiveKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_seq: Option<u64>,
    /// Preview-attribution cursor for a continuation round: the message id the
    /// engine must tag streamed previews with while answering the review.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<KernelSettledToolResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// Mid-run user requests durably received before this round boundary.
    /// The engine appends them as ordinary user messages after the settled
    /// tool results (batch) or review prompt (continuation); they never carry
    /// authority, tools, or grants. Final directives never carry steering.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering: Vec<KernelSteeringNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_job_notices: Vec<HostJobNotice>,
}

/// One additional user request attached to a round directive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelSteeringNotice {
    /// Stable idempotency id of the durable steering row.
    pub message_id: String,
    pub content: String,
    /// Original Host receipt time. Optional for older frames; new Host
    /// dispatches use it so live and replacement transcripts are identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at: Option<i64>,
}

impl KernelSteeringNotice {
    pub fn validate(&self) -> Result<(), String> {
        if self.message_id.trim().is_empty() || self.message_id.len() > 128
            || self.content.trim().is_empty()
            || self.content.chars().count() > 8_000
            || self.received_at.is_some_and(|value| value < 0)
        {
            return Err("invalid steering notice".into());
        }
        Ok(())
    }
}

impl KernelRoundDirective {
    pub fn validate(&self) -> Result<(), String> {
        validate_host_job_notices(&self.host_job_notices)?;
        if self.schema_version != 1 {
            return Err("invalid Kernel round directive".into());
        }
        let size = serde_json::to_vec(self).map_err(|error| error.to_string())?.len();
        if size > 1_048_576 {
            return Err("Kernel round directive exceeds the protocol size limit".into());
        }
        match self.kind {
            KernelRoundDirectiveKind::Batch => {
                if self.batch_id.as_deref().is_none_or(|id| id.trim().is_empty())
                    || self.checkpoint_seq.is_none_or(|seq| seq == 0 || seq > 9_007_199_254_740_991)
                    || self.tools.is_empty() || self.prompt.is_some() || self.preview_seq.is_some() {
                    return Err("invalid Kernel batch directive".into());
                }
            }
            KernelRoundDirectiveKind::Continuation => {
                if self.prompt.as_deref().is_none_or(|text| text.trim().is_empty() || text.len() > 16_384)
                    || self.batch_id.is_some() || self.checkpoint_seq.is_some() || !self.tools.is_empty()
                    || self.preview_seq.is_some_and(|seq| seq == 0 || seq > 9_007_199_254_740_991)
                {
                    return Err("invalid Kernel continuation directive".into());
                }
            }
            KernelRoundDirectiveKind::Final => {
                if self.batch_id.is_some() || self.checkpoint_seq.is_some() || !self.tools.is_empty()
                    || self.prompt.is_some() || self.preview_seq.is_some()
                    || !self.steering.is_empty()
                {
                    return Err("invalid Kernel final directive".into());
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        let mut steering_bytes = 0usize;
        for notice in &self.steering {
            notice.validate()?;
            if !seen.insert(notice.message_id.as_str()) {
                return Err("duplicate steering notice in directive".into());
            }
            steering_bytes = steering_bytes.saturating_add(notice.content.len());
            if steering_bytes > 64 * 1024 {
                return Err("directive steering exceeds the bounded queue size".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelBatchResumeFrame {
    pub schema_version: u32,
    pub turn_id: String,
    pub batch_id: String,
    pub idempotency_key: String,
    pub checkpoint_seq: u64,
    /// Complete prior history. Engines must reject rather than synthesize missing results.
    pub history: Vec<Value>,
    /// Original proposal, including provider metadata needed by the engine adapter.
    pub assistant_message: Value,
    pub tools: Vec<KernelSettledToolResult>,
    /// Mid-run user requests bound to this (possibly retried) dispatch. The
    /// engine appends them as ordinary user messages *after* the settled tool
    /// results, in seq order. Like the rest of this frame they carry no
    /// authority, tools, or grants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering: Vec<KernelSteeringNotice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub host_job_notices: Vec<HostJobNotice>,
}

impl KernelBatchResumeFrame {
    pub fn validate(&self) -> Result<(), String> {
        validate_host_job_notices(&self.host_job_notices)?;
        if historical_host_job_notice_bytes(&self.history)?
            + serde_json::to_vec(&self.host_job_notices).map_err(|_| "invalid Host job notices")?.len()
            > 16 * 1024 { return Err("Host job notice input exceeds total limit".into()); }
        validate_checkpoint_parts(&self.history, &self.assistant_message)?;
        if self.schema_version != 1 || self.turn_id.trim().is_empty() || self.batch_id.trim().is_empty()
            || self.idempotency_key != format!("tool-batch-delivery:{}", self.batch_id)
            || self.checkpoint_seq == 0 || self.checkpoint_seq > 9_007_199_254_740_991 {
            return Err("invalid durable batch resume identity".into());
        }
        if serde_json::to_vec(self).map_err(|error| error.to_string())?.len() > 1_048_576 {
            return Err("batch resume exceeds the protocol size limit".into());
        }
        if self.assistant_message["role"] != "assistant" || self.assistant_message["stopReason"] != "toolUse" {
            return Err("batch resume requires the original assistant tool proposal".into());
        }
        let content = self.assistant_message["content"].as_array().ok_or("missing assistant content")?;
        let calls = content.iter().filter(|block| block["type"] == "toolCall").collect::<Vec<_>>();
        if calls.is_empty() || calls.len() > 64 || calls.len() != self.tools.len() {
            return Err("incomplete durable result barrier".into());
        }
        let mut ids = std::collections::HashSet::new();
        for item in &self.tools {
            if item.tool_call_id.trim().is_empty() || !ids.insert(item.tool_call_id.as_str()) {
                return Err("duplicate or missing durable result identity".into());
            }
            let call = calls.get(item.source_order as usize).ok_or("invalid result source order")?;
            if call["id"].as_str() != Some(item.tool_call_id.as_str()) || call["name"].as_str() != Some(item.tool.as_str())
                || !item.canonical_input.is_object() || call["arguments"] != item.canonical_input
                || super::canonical_runtime_tool_contract(&item.tool).is_none() {
                return Err("durable result differs from approved tool identity".into());
            }
            let result = item.result.as_object().ok_or("missing durable tool result")?;
            if result.get("isError").is_some_and(|value| value.as_bool() != Some(item.state == KernelSettledToolState::Failed)) {
                return Err("tool result contradicts durable terminal state".into());
            }
            validate_result_content(&item.result)?;
        }
        validate_steering_notices(&self.steering)?;
        Ok(())
    }
}

/// Shared bounds for steering carried by any engine dispatch: each notice is
/// individually valid, ids are unique, total bounded text stays inside the
/// per-Run outstanding queue budget.
pub(crate) fn validate_steering_notices(notices: &[KernelSteeringNotice]) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut total = 0usize;
    for notice in notices {
        notice.validate()?;
        if !seen.insert(notice.message_id.as_str()) {
            return Err("duplicate steering notice".into());
        }
        total = total.saturating_add(notice.content.len());
        if total > 64 * 1024 {
            return Err("steering exceeds the bounded queue size".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initial_input_requires_current_user_and_complete_supported_history() {
        let good = KernelInitialModelInput {
            schema_version: 1, run_id: "run".into(), turn_id: "turn".into(), prompt_config_hash: "hash".into(),
            messages: vec![json!({"role":"user","content":"中文 😀"})],
        };
        good.validate().unwrap();
        assert_eq!(serde_json::from_value::<KernelInitialModelInput>(serde_json::to_value(&good).unwrap()).unwrap(), good);
        for messages in [
            vec![], vec![json!({"role":"assistant","content":"not a user"})],
            vec![json!({"role":"system","content":"override"}),good.messages[0].clone()],
            vec![json!({"role":"assistant","content":[{"type":"toolCall","id":"t","name":"read","arguments":{}}]}),good.messages[0].clone()],
            vec![json!({"role":"toolResult","toolCallId":"t","toolName":"read","isError":false,"content":[]}),good.messages[0].clone()],
            vec![json!({"role":"user","content":[{"type":"toolCall","id":"t","name":"read","arguments":{}}]})],
            vec![json!({"role":"user","content":[{"type":"unknown"}]})],
            vec![json!({"role":"assistant","stopReason":"length","content":[]}),good.messages[0].clone()],
        ] {
            let mut bad = good.clone(); bad.messages = messages;
            assert!(bad.validate().is_err());
        }
        let mut complete = good.clone();
        complete.messages = vec![good.messages[0].clone(),
            json!({"role":"assistant","content":[{"type":"toolCall","id":"t","name":"read","arguments":{}}]}),
            json!({"role":"toolResult","toolCallId":"t","toolName":"read","isError":false,"content":[{"type":"text","text":"result"}]}),
            json!({"role":"user","content":[{"type":"text","text":"continue"},{"type":"image","data":"AA==","mimeType":"image/png"}]})];
        complete.validate().unwrap();
        complete.messages[0]["content"] = json!("x".repeat(1_048_577));
        assert!(complete.validate().is_err());
        let mut wire = serde_json::to_value(&good).unwrap(); wire["apiKey"] = json!("not allowed");
        assert!(serde_json::from_value::<KernelInitialModelInput>(wire).is_err());
    }

    #[test]
    fn durable_batch_resume_roundtrips_and_never_accepts_unsettled_or_changed_tools() {
        let frame = KernelBatchResumeFrame {
            schema_version: 1, turn_id: "turn".into(), batch_id: "batch".into(),
            idempotency_key: "tool-batch-delivery:batch".into(), checkpoint_seq: 1,
            history: vec![json!({"role":"user","content":"read"})],
            assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"read-1","name":"read","arguments":{"path":"a.txt"}}]}),
            tools: vec![KernelSettledToolResult {
                tool_call_id: "read-1".into(), tool: "read".into(), canonical_input: json!({"path":"a.txt"}), source_order: 0,
                storage: None, state: KernelSettledToolState::Completed, result: json!({"content":[{"type":"text","text":"proof"}]}),
            }],
            steering: vec![], host_job_notices: vec![],
        };
        frame.validate().unwrap();
        let wire = serde_json::to_value(&frame).unwrap();
        assert_eq!(serde_json::from_value::<KernelBatchResumeFrame>(wire.clone()).unwrap(), frame);
        let mut bad = frame.clone();
        bad.tools[0].canonical_input = json!({"path":"changed"});
        assert!(bad.validate().is_err());
        bad = frame.clone();
        bad.tools.clear();
        assert!(bad.validate().is_err());
        bad = frame.clone();
        bad.tools[0].result["isError"] = json!(true);
        assert!(bad.validate().is_err());
        let mut unsettled = wire;
        unsettled["tools"][0]["state"] = json!("running");
        assert!(serde_json::from_value::<KernelBatchResumeFrame>(unsettled).is_err());
    }
}

#[cfg(test)]
#[test]
fn restored_continuation_has_a_distinct_validated_delivery_identity() {
    let mut frame = KernelInitialModelFrame {
        schema_version: 1, idempotency_key: "initial-model-delivery".into(), checkpoint_seq: 2,
        continuation_key: None, host_job_notices: vec![],
        input: KernelInitialModelInput { schema_version: 1, run_id: "r".into(), turn_id: "t".into(),
            prompt_config_hash: "frozen".into(), messages: vec![serde_json::json!({"role":"user","content":"continue"})] },
    };
    frame.validate().unwrap();
    frame.continuation_key = Some("continuation:1".into());
    assert!(frame.validate().is_err(), "a continuation cannot reuse the initial delivery identity");
    frame.idempotency_key = "continuation-delivery:continuation:1".into();
    frame.validate().unwrap();
    for key in ["continuation:0","continuation:01","continuation:9007199254740992","other:1"] {
        frame.continuation_key = Some(key.into());
        frame.idempotency_key = format!("continuation-delivery:{key}");
        assert!(frame.validate().is_err());
    }
    let mut failure = KernelModelFailure { schema_version:1, run_id:"r".into(), turn_id:"t".into(), checkpoint_seq:2,
        category:"model_transport_failure".into(), http_status:None, retry_after_ms:None, telemetry:None };
    failure.validate().unwrap();
    failure.http_status = Some(429);
    assert!(failure.validate().is_err(), "transport loss must not forge provider rejection evidence");
}
