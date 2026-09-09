//! Explicit Host-owned summarization. This request grants no tool authority.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelCompactionRequest {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub compaction_id: String,
    pub input_hash: String,
    /// Only old, plain conversational prose; never tool calls/results or images.
    pub messages: Vec<Value>,
    pub max_summary_bytes: u32,
}

impl KernelCompactionRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1
            || [&self.run_id, &self.turn_id, &self.compaction_id]
                .iter()
                .any(|id| id.trim().is_empty() || id.len() > 512)
            || !valid_hash(&self.input_hash)
            || !(512..=8192).contains(&self.max_summary_bytes)
            || self.messages.is_empty()
            || serde_json::to_vec(self)
                .map_err(|_| "invalid compaction input")?
                .len()
                > 262_144
            || self.messages.iter().any(|message| !plain_message(message))
        {
            return Err("invalid Kernel compaction input".into());
        }
        Ok(())
    }
}

pub fn plain_message(message: &Value) -> bool {
    matches!(message["role"].as_str(), Some("user" | "assistant"))
        && (message["content"].is_string()
            || message["content"].as_array().is_some_and(|blocks| {
                !blocks.is_empty()
                    && blocks
                        .iter()
                        .all(|block| block["type"] == "text" && block["text"].is_string())
            }))
        && message
            .get("stopReason")
            .is_none_or(|reason| reason == "stop")
}

pub fn valid_hash(hash: &str) -> bool {
    hash.strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelCompactionResponse {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub compaction_id: String,
    pub input_hash: String,
    pub summary: String,
    /// Bounded provider usage, accounted separately from visible chat messages.
    pub usage: Value,
}

impl KernelCompactionResponse {
    pub fn validate_for(&self, request: &KernelCompactionRequest) -> Result<(), String> {
        request.validate()?;
        if self.schema_version != 1
            || self.run_id != request.run_id
            || self.turn_id != request.turn_id
            || self.compaction_id != request.compaction_id
            || self.input_hash != request.input_hash
            || self.summary.trim().is_empty()
            || self.summary.len() > request.max_summary_bytes as usize
            || !self.usage.is_object()
            || self.usage.as_object().unwrap().iter().any(|(key, value)| {
                !["input", "output", "cacheRead", "cacheWrite", "totalTokens"]
                    .contains(&key.as_str())
                    || value.as_u64().is_none_or(|count| count > 1_000_000_000)
            })
        {
            return Err("invalid Kernel compaction response".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compaction_wire_contract_rejects_tool_content_foreign_identity_and_unbounded_output() {
        let request = KernelCompactionRequest {
            schema_version: 1,
            run_id: "r".into(),
            turn_id: "t".into(),
            compaction_id: "job".into(),
            input_hash: format!("sha256:{}", "a".repeat(64)),
            messages: vec![json!({"role":"user","content":"old notes"})],
            max_summary_bytes: 512,
        };
        let response = KernelCompactionResponse {
            schema_version: 1,
            run_id: "r".into(),
            turn_id: "t".into(),
            compaction_id: "job".into(),
            input_hash: request.input_hash.clone(),
            summary: "中文 😀".into(),
            usage: json!({"input":10,"output":4}),
        };
        response.validate_for(&request).unwrap();
        let mut bad = request.clone();
        bad.messages = vec![
            json!({"role":"assistant","content":[{"type":"toolCall","id":"x","name":"read","arguments":{}}]}),
        ];
        assert!(bad.validate().is_err());
        let mut bad = response.clone();
        bad.run_id = "foreign".into();
        assert!(bad.validate_for(&request).is_err());
        let mut bad = response.clone();
        bad.summary = "中".repeat(200);
        assert!(bad.validate_for(&request).is_err());
        let mut bad = response.clone();
        bad.usage = json!({"input":-1});
        assert!(bad.validate_for(&request).is_err());
        let mut bad = response;
        bad.usage = json!({"apiKey":"secret"});
        assert!(bad.validate_for(&request).is_err());
    }
}
