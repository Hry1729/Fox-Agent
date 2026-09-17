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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeBudgets {
    /// Whole-round backstop for one model request, including slow-but-progressing
    /// generation. Stall detection is owned by the first-response/idle bounds
    /// below; this bound only caps total spend per round.
    pub model_request_ms: i64,
    /// No output at all within this long after dispatch fails the round.
    /// Old bindings predate this field and deserialize to the default.
    #[serde(default = "default_model_first_response_ms")]
    pub model_first_response_ms: i64,
    /// No text/thinking/tool-parameter progress within this long fails the
    /// round, even if the whole-round bound has not been reached. Old bindings
    /// predate this field and deserialize to the default.
    #[serde(default = "default_model_idle_ms")]
    pub model_idle_ms: i64,
    pub tool_execution_ms: i64,
    pub run_execution_ms: i64,
    /// Old frozen runs retain their explicit limit. Ordinary new conversations
    /// opt out of a whole-run deadline; model/tool stall limits still apply.
    #[serde(default = "default_run_execution_limited", skip_serializing_if = "is_true")]
    pub run_execution_limited: bool,
    pub approval_wait_ms: i64,
}

// Missing stall budgets in older persisted records inherit a bound no larger
// than their original total window. Explicit invalid values still fail validation.
impl<'de> Deserialize<'de> for TimeBudgets {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        fn present_budget<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
            i64::deserialize(d).map(Some)
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Stored {
            model_request_ms: i64,
            #[serde(default, deserialize_with = "present_budget")]
            model_first_response_ms: Option<i64>,
            #[serde(default, deserialize_with = "present_budget")]
            model_idle_ms: Option<i64>,
            tool_execution_ms: i64,
            run_execution_ms: i64,
            #[serde(default = "default_run_execution_limited")]
            run_execution_limited: bool,
            approval_wait_ms: i64,
        }
        let stored = Stored::deserialize(deserializer)?;
        Ok(Self {
            model_request_ms: stored.model_request_ms,
            model_first_response_ms: stored.model_first_response_ms.unwrap_or_else(|| default_model_first_response_ms().min(stored.model_request_ms)),
            model_idle_ms: stored.model_idle_ms.unwrap_or_else(|| default_model_idle_ms().min(stored.model_request_ms)),
            tool_execution_ms: stored.tool_execution_ms,
            run_execution_ms: stored.run_execution_ms,
            run_execution_limited: stored.run_execution_limited,
            approval_wait_ms: stored.approval_wait_ms,
        })
    }
}

fn default_run_execution_limited() -> bool { true }
fn is_true(value: &bool) -> bool { *value }

pub fn default_model_first_response_ms() -> i64 {
    60_000
}

pub fn default_model_idle_ms() -> i64 {
    120_000
}

impl Default for TimeBudgets {
    fn default() -> Self {
        Self { model_request_ms: 300_000, model_first_response_ms: default_model_first_response_ms(),
            model_idle_ms: default_model_idle_ms(), tool_execution_ms: 600_000,
            run_execution_ms: 1_800_000, run_execution_limited: true, approval_wait_ms: 300_000 }
    }
}

impl TimeBudgets {
    /// New ordinary tasks run until completion or cancellation, without a total
    /// duration cap. Default remains the legacy frozen-record representation.
    pub fn continuous() -> Self {
        Self { run_execution_limited: false, ..Self::default() }
    }

    pub fn remaining_run_ms(&self, elapsed_ms: i64) -> Option<i64> {
        self.run_execution_limited.then(|| self.run_execution_ms.saturating_sub(elapsed_ms))
    }

    /// Always returns a finite operation window, even for a continuous run.
    pub fn limit_operation_ms(&self, operation_ms: i64, elapsed_ms: i64) -> i64 {
        self.remaining_run_ms(elapsed_ms).map_or(operation_ms, |remaining| operation_ms.min(remaining))
    }

    /// Derive a tighter execution budget while preserving all sub-window bounds.
    /// Approval waiting has its own clock and is not narrowed by execution time.
    pub fn restrict_execution_ms(&mut self, maximum: i64) {
        self.run_execution_ms = if self.run_execution_limited {
            self.run_execution_ms.min(maximum)
        } else { maximum };
        self.run_execution_limited = true;
        self.tool_execution_ms = self.tool_execution_ms.min(self.run_execution_ms);
        self.model_request_ms = self.model_request_ms.min(self.run_execution_ms);
        self.model_first_response_ms = self.model_first_response_ms.min(self.model_request_ms);
        self.model_idle_ms = self.model_idle_ms.min(self.model_request_ms);
    }

    pub fn validate(&self) -> Result<(), String> {
        if [self.model_request_ms, self.model_first_response_ms, self.model_idle_ms,
            self.tool_execution_ms, self.run_execution_ms, self.approval_wait_ms]
            .iter().any(|value| *value <= 0 || *value > 86_400_000) {
            return Err("control time budgets must be positive and at most 24 hours".into());
        }
        // Sub-round bounds are meaningless above the whole-round bound; fail
        // closed instead of silently clamping a caller's intent.
        if self.model_first_response_ms > self.model_request_ms
            || self.model_idle_ms > self.model_request_ms {
            return Err("model first-response/idle budgets must not exceed the whole-round budget".into());
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
    fn continuous_task_policy_preserves_legacy_hashes_and_explicit_child_limits() {
        let legacy_json = serde_json::to_string(&TimeBudgets::default()).unwrap();
        assert!(!legacy_json.contains("runExecutionLimited"));
        let legacy: TimeBudgets = serde_json::from_str(&legacy_json).unwrap();
        assert!(legacy.run_execution_limited);
        assert_eq!(serde_json::to_string(&legacy).unwrap(), legacy_json);
        let mut continuous = TimeBudgets::continuous();
        continuous.validate().unwrap();
        assert_eq!(continuous.remaining_run_ms(86_400_000), None);
        assert_eq!(continuous.limit_operation_ms(120_000, 86_400_000), 120_000);
        let decoded: TimeBudgets = serde_json::from_str(&serde_json::to_string(&continuous).unwrap()).unwrap();
        assert_eq!(decoded, continuous);
        continuous.restrict_execution_ms(7_200_000);
        assert_eq!(continuous.remaining_run_ms(7_200_001), Some(-1));
        assert_eq!(continuous.tool_execution_ms, 600_000);
        assert_eq!(continuous.model_request_ms, 300_000);
    }

    #[test]
    fn legacy_short_windows_and_child_derivation_preserve_total_budget() {
        for total in [1_000, 30_000, 90_000] {
            let value = serde_json::json!({"modelRequestMs":total,"toolExecutionMs":600000,
                "runExecutionMs":1800000,"approvalWaitMs":300000});
            let budgets: TimeBudgets = serde_json::from_value(value.clone()).unwrap();
            budgets.validate().unwrap();
            assert_eq!(budgets.model_request_ms, total);
            assert_eq!(budgets.model_first_response_ms, total.min(60_000));
            assert_eq!(budgets.model_idle_ms, total.min(120_000));
            let mut explicit = value.clone();
            explicit["modelIdleMs"] = serde_json::json!(total + 1);
            assert!(serde_json::from_value::<TimeBudgets>(explicit).unwrap().validate().is_err());
            let mut null = value;
            null["modelIdleMs"] = serde_json::Value::Null;
            assert!(serde_json::from_value::<TimeBudgets>(null).is_err());
        }
        let mut child = TimeBudgets::default();
        child.restrict_execution_ms(30_000);
        child.validate().unwrap();
        assert_eq!(child.model_request_ms, 30_000);
        assert_eq!(child.model_first_response_ms, 30_000);
        assert_eq!(child.model_idle_ms, 30_000);
        assert_eq!(child.tool_execution_ms, 30_000);
        assert_eq!(child.approval_wait_ms, 300_000);
        child.restrict_execution_ms(0);
        assert!(child.validate().is_err());
    }

    #[test]
    fn unknown_modes_and_unbounded_time_budgets_fail_closed() {
        assert!(serde_json::from_str::<ExecutionAuthority>("\"shadow_executor\"").is_err());
        assert!(serde_json::from_str::<PermissionMode>("\"unknown\"").is_err());
        let mut budgets = TimeBudgets::default();
        budgets.approval_wait_ms = 0;
        assert!(budgets.validate().is_err());
        budgets.approval_wait_ms = 86_400_001;
        assert!(budgets.validate().is_err());
        // Sub-round bounds above the whole-round bound fail closed.
        budgets = TimeBudgets::default();
        budgets.model_idle_ms = budgets.model_request_ms + 1;
        assert!(budgets.validate().is_err());
        budgets = TimeBudgets::default();
        budgets.model_first_response_ms = budgets.model_request_ms + 1;
        assert!(budgets.validate().is_err());
        // Old bindings without the new fields deserialize to the defaults.
        let legacy: TimeBudgets = serde_json::from_str(
            r#"{"modelRequestMs":120000,"toolExecutionMs":600000,"runExecutionMs":1800000,"approvalWaitMs":300000}"#,
        ).expect("legacy budgets decode");
        assert_eq!(legacy.model_first_response_ms, default_model_first_response_ms());
        assert_eq!(legacy.model_idle_ms, default_model_idle_ms());
        assert!(legacy.validate().is_ok());
    }
}
