//! Host-owned, already-settled batch handoff. Never an approval or dispatch request.
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

impl KernelModelFailure {
    pub fn validate(&self) -> Result<(), String> {
        let category_valid = match self.category.as_str() {
            "provider_unavailable" => self.http_status.is_some_and(|status| matches!(status, 429 | 500 | 502 | 503 | 504 | 529)),
            "incomplete_response" => self.http_status.is_none() && self.retry_after_ms.is_none(),
            _ => false,
        };
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
}

impl KernelModelPreview {
    pub fn validate(&self) -> Result<(),String> {
        if self.schema_version!=1 || [&self.run_id,&self.conversation_id,&self.turn_id].iter()
            .any(|id|id.trim().is_empty() || id.len()>512) || self.checkpoint_seq==0 || self.checkpoint_seq>9_007_199_254_740_991
            || self.revision==0 || self.revision>9_007_199_254_740_991 || self.text.len()>262_144 || self.reasoning.len()>262_144 {
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
}

impl KernelInitialModelFrame {
    pub fn validate(&self) -> Result<(), String> {
        self.input.validate()?;
        if self.schema_version != 1 || self.idempotency_key != "initial-model-delivery"
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
    for message in history {
        match message["role"].as_str() {
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
}

impl KernelRoundDirective {
    pub fn validate(&self) -> Result<(), String> {
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
                {
                    return Err("invalid Kernel final directive".into());
                }
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
}

impl KernelBatchResumeFrame {
    pub fn validate(&self) -> Result<(), String> {
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
        Ok(())
    }
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
                state: KernelSettledToolState::Completed, result: json!({"content":[{"type":"text","text":"proof"}]}),
            }],
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
