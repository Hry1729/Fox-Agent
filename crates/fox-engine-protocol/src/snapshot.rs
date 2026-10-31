//! Read-only UI projection. Never carries executable outbox payloads or tool data.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelRunSnapshot {
    pub schema_version: u32,
    pub run_id: String,
    pub turn_id: String,
    pub engine_id: String,
    pub state: String,
    /// Decimal string: SQLite sequences must not lose precision in JavaScript.
    pub last_event_seq: String,
    pub running_elapsed_ms: i64,
    pub approval_deadline_wall_ms: Option<i64>,
    pub terminal_written: bool,
    pub tools: Vec<KernelToolSnapshot>,
    pub provider_attempts: u32,
    pub turn_attempts: u32,
    pub retry_due_wall_ms: Option<i64>,
    pub compactions: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KernelToolSnapshot {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    pub source_order: usize,
    pub state: String,
    pub approval_state: Option<String>,
}
