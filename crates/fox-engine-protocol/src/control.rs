//! Frozen, versioned control identity shared by Host, engines and read models.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const CONTROL_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionAuthority { Legacy, Authoritative }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResourceExecutor { Runtime, Rust }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode { Ask, ReadOnly, Allow }

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self { Self::Ask => "ask", Self::ReadOnly => "read_only", Self::Allow => "allow" }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PermissionGrant { pub tool: String, pub scope: String }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrozenPermission {
    pub mode: PermissionMode,
    pub project_root: Option<String>,
    pub grants: Vec<PermissionGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeBudgets {
    pub model_request_ms: i64,
    pub tool_execution_ms: i64,
    pub run_execution_ms: i64,
    pub approval_wait_ms: i64,
}

impl Default for TimeBudgets {
    fn default() -> Self {
        Self { model_request_ms: 120_000, tool_execution_ms: 600_000, run_execution_ms: 1_800_000, approval_wait_ms: 300_000 }
    }
}

impl TimeBudgets {
    pub fn validate(&self) -> Result<(), String> {
        if [self.model_request_ms, self.tool_execution_ms, self.run_execution_ms, self.approval_wait_ms]
            .iter().any(|value| *value <= 0 || *value > 86_400_000) {
            return Err("control time budgets must be positive and at most 24 hours".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunControlBinding {
    pub schema_version: u32,
    pub run_id: String,
    pub conversation_id: String,
    pub engine_id: String,
    pub execution_profile_id: String,
    pub authority: ExecutionAuthority,
    pub read_only_executor: ResourceExecutor,
    pub permission_snapshot_id: String,
    pub permission: FrozenPermission,
    pub budgets: TimeBudgets,
}

impl RunControlBinding {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONTROL_SCHEMA_VERSION { return Err("unsupported frozen control schema".into()); }
        if [&self.run_id, &self.conversation_id, &self.execution_profile_id, &self.permission_snapshot_id]
            .iter().any(|value| value.trim().is_empty()) { return Err("frozen control identity is incomplete".into()); }
        if !matches!(self.engine_id.as_str(), "pi" | "codex" | "deepseek_harness") { return Err("unsupported frozen control engine".into()); }
        if self.permission.project_root.as_ref().is_some_and(|root| root.trim().is_empty()) { return Err("frozen project root is empty".into()); }
        if self.permission.grants.iter().any(|grant| grant.scope.trim().is_empty() || super::canonical_runtime_tool_contract(&grant.tool).is_none()) {
            return Err("invalid frozen permission grant".into());
        }
        self.budgets.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_modes_and_unbounded_time_budgets_fail_closed() {
        assert!(serde_json::from_str::<ExecutionAuthority>("\"shadow_executor\"").is_err());
        assert!(serde_json::from_str::<PermissionMode>("\"unknown\"").is_err());
        let mut budgets = TimeBudgets::default();
        budgets.approval_wait_ms = 0;
        assert!(budgets.validate().is_err());
        budgets.approval_wait_ms = 86_400_001;
        assert!(budgets.validate().is_err());
    }
}
