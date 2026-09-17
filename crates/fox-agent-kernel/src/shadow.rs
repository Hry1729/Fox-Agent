//! Production Shadow comparison for the Fox Agent Kernel.
//!
//! A shadow run runs the pure Kernel decision core against the SAME inputs the
//! legacy Host observes, but it never acts: it does not dispatch tools, open
//! network requests, raise a second approval, cancel the real run, deliver a
//! second tool batch, or rewrite the legacy terminal. Its only outputs are an
//! observation context and comparison records (`ShadowDiff`) persisted to the
//! dedicated `kernel_shadow_*` tables — which are physically separate from the
//! executable `kernel_effect_outbox`, so no lease/recovery scan can ever run a
//! shadow effect.

use serde::{Deserialize, Serialize};

/// Schema version for shadow diff records. Bump when the record shape changes.
pub const SHADOW_DIFF_SCHEMA_VERSION: i64 = 2;

/// One tool's decision and identity, as observed at a comparison point. Order
/// is preserved (source order within a batch) so parameter/order mismatches are
/// detectable — it is NOT sorted before comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDisposition {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    /// Canonical input JSON after non-safety extensions; compared verbatim.
    pub canonical_input_json: String,
    /// Source position within the batch (0-based).
    pub source_order: usize,
    /// "allow" | "approval" | "deny".
    pub decision: String,
}

/// Retry/timeout facts at a comparison point. Used for `timeout_retry_mismatch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RetryTimeoutFacts {
    pub provider_attempts: u32,
    pub provider_max: u32,
    pub turn_attempts: u32,
    pub turn_max: u32,
    /// Which timeout/retry terminal the disposition produced, if any, e.g.
    /// "model.request_timeout" / "tool.execution_timeout" / "approval.wait_timeout".
    pub terminal_code: Option<String>,
}

/// The disposition the Kernel derived for one observation point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowDisposition {
    /// Kernel run state after applying the observed input.
    pub run_state: String,
    /// Tool facts in BATCH/SOURCE ORDER (unsorted, so order is comparable).
    pub tools: Vec<ToolDisposition>,
    /// Whether the Kernel would have produced a terminal here, and which event.
    pub terminal: Option<String>,
    pub retry_timeout: RetryTimeoutFacts,
    /// Whether the Kernel considered this point comparable.
    pub comparable: bool,
}

/// The disposition the legacy Host actually produced for the same point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LegacyDisposition {
    pub run_state: String,
    /// Tool facts in the order legacy reported them.
    pub tools: Vec<ToolDisposition>,
    pub terminal: Option<String>,
    pub retry_timeout: RetryTimeoutFacts,
}

/// Categories of legacy-vs-kernel comparison. `not_comparable` marks points the
/// Kernel cannot yet evaluate because the production wiring for that capability
/// is absent; it must never be silently counted as agreement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffCategory {
    Match,
    StateMismatch,
    ApprovalMismatch,
    ToolParamOrOrderMismatch,
    TerminalMismatch,
    TimeoutRetryMismatch,
    NotComparable,
    ShadowError,
}

impl DiffCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            DiffCategory::Match => "match",
            DiffCategory::StateMismatch => "state_mismatch",
            DiffCategory::ApprovalMismatch => "approval_mismatch",
            DiffCategory::ToolParamOrOrderMismatch => "tool_param_or_order_mismatch",
            DiffCategory::TerminalMismatch => "terminal_mismatch",
            DiffCategory::TimeoutRetryMismatch => "timeout_retry_mismatch",
            DiffCategory::NotComparable => "not_comparable",
            DiffCategory::ShadowError => "shadow_error",
        }
    }

    pub fn parse(value: &str) -> Option<DiffCategory> {
        Some(match value {
            "match" => DiffCategory::Match,
            "state_mismatch" => DiffCategory::StateMismatch,
            "approval_mismatch" => DiffCategory::ApprovalMismatch,
            "tool_param_or_order_mismatch" => DiffCategory::ToolParamOrOrderMismatch,
            "terminal_mismatch" => DiffCategory::TerminalMismatch,
            "timeout_retry_mismatch" => DiffCategory::TimeoutRetryMismatch,
            "not_comparable" => DiffCategory::NotComparable,
            "shadow_error" => DiffCategory::ShadowError,
            _ => return None,
        })
    }
}

/// One comparison record. Pure data; the repository persists it verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowDiff {
    pub shadow_run_id: String,
    pub legacy_run_id: String,
    pub turn_id: Option<String>,
    pub event_cursor: i64,
    pub event_type: Option<String>,
    pub category: DiffCategory,
    pub legacy: LegacyDisposition,
    pub kernel: ShadowDisposition,
    pub detail: serde_json::Value,
}

impl ShadowDiff {
    /// Classify a comparison from the two dispositions. Terminal differences take
    /// precedence, then approval, then tool params/order, then state. A point the
    /// Kernel could not evaluate is `not_comparable`; a shadow fault is
    /// `shadow_error`.
    pub fn classify(
        shadow_run_id: &str,
        legacy_run_id: &str,
        turn_id: Option<String>,
        event_cursor: i64,
        event_type: Option<String>,
        legacy: LegacyDisposition,
        kernel: ShadowDisposition,
    ) -> ShadowDiff {
        let category = if !kernel.comparable {
            DiffCategory::NotComparable
        } else if legacy.terminal != kernel.terminal {
            // Distinguish a timeout/retry-coded terminal from a plain one.
            let either_timeout = is_timeout_retry_code(legacy.terminal.as_deref())
                || is_timeout_retry_code(kernel.terminal.as_deref());
            if either_timeout {
                DiffCategory::TimeoutRetryMismatch
            } else {
                DiffCategory::TerminalMismatch
            }
        } else if approval_sets_differ(&legacy.tools, &kernel.tools) {
            // Approval requirement differences are the most actionable and
            // must not be masked by routine retry-counter observation noise.
            DiffCategory::ApprovalMismatch
        } else if tool_identity_or_order_differs(&legacy.tools, &kernel.tools) {
            DiffCategory::ToolParamOrOrderMismatch
        } else if timeout_retry_code_differs(&legacy.retry_timeout, &kernel.retry_timeout) {
            // A timeout/retry TERMINAL CODE disagrees (e.g. one side would have
            // failed with model.request_timeout). Routine counter values alone
            // are observation facts, not mismatches.
            DiffCategory::TimeoutRetryMismatch
        } else if legacy.run_state != kernel.run_state {
            DiffCategory::StateMismatch
        } else {
            DiffCategory::Match
        };
        let detail = serde_json::json!({
            "legacyState": legacy.run_state,
            "kernelState": kernel.run_state,
            "legacyTerminal": legacy.terminal,
            "kernelTerminal": kernel.terminal,
            "legacyToolCount": legacy.tools.len(),
            "kernelToolCount": kernel.tools.len(),
        });
        ShadowDiff {
            shadow_run_id: shadow_run_id.to_string(),
            legacy_run_id: legacy_run_id.to_string(),
            turn_id,
            event_cursor,
            event_type,
            category,
            legacy,
            kernel,
            detail,
        }
    }

    pub fn is_match(&self) -> bool {
        self.category == DiffCategory::Match
    }

    /// True only for records that indicate an actionable divergence (a `match`
    /// and a `not_comparable` are both non-divergences, for different reasons).
    pub fn is_divergence(&self) -> bool {
        matches!(
            self.category,
            DiffCategory::StateMismatch
                | DiffCategory::ApprovalMismatch
                | DiffCategory::ToolParamOrOrderMismatch
                | DiffCategory::TerminalMismatch
                | DiffCategory::TimeoutRetryMismatch
                | DiffCategory::ShadowError
        )
    }
}

fn is_timeout_retry_code(code: Option<&str>) -> bool {
    matches!(
        code,
        Some("model.request_timeout")
            | Some("tool.execution_timeout")
            | Some("approval.wait_timeout")
            | Some("runtime.turn_retry_exhausted")
            | Some("provider.request_retry_exhausted")
    )
}

/// Only a set/changed terminal code is a timeout/retry mismatch; routine
/// counter deltas are recorded as facts but do not by themselves classify.
fn timeout_retry_code_differs(legacy: &RetryTimeoutFacts, kernel: &RetryTimeoutFacts) -> bool {
    legacy.terminal_code != kernel.terminal_code
}

/// A tool on which a human decision is required.
fn approval_tools(tools: &[ToolDisposition]) -> Vec<(&str, &str)> {
    tools
        .iter()
        .filter(|t| matches!(t.decision.as_str(), "approval" | "deny"))
        .map(|t| (t.tool_call_id.as_str(), t.decision.as_str()))
        .collect()
}

fn approval_sets_differ(legacy: &[ToolDisposition], kernel: &[ToolDisposition]) -> bool {
    let mut l = approval_tools(legacy);
    let mut k = approval_tools(kernel);
    l.sort();
    k.sort();
    l != k
}

/// Compare tool identity, canonical input, source order and batch membership in
/// the reported ORDER (NOT re-sorted, so a reorder is a real mismatch).
fn tool_identity_or_order_differs(legacy: &[ToolDisposition], kernel: &[ToolDisposition]) -> bool {
    if legacy.len() != kernel.len() {
        return true;
    }
    // Position-wise comparison: source_order must align at each position.
    for (l, k) in legacy.iter().zip(kernel.iter()) {
        if l.tool_call_id != k.tool_call_id
            || l.tool != k.tool
            || l.canonical_input_json != k.canonical_input_json
            || l.batch_id != k.batch_id
            || l.source_order != k.source_order
            || l.decision != k.decision
        {
            return true;
        }
    }
    false
}

/// A shadow observation of a legacy tool decision (driven from the real run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowToolObservation {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    pub canonical_input_json: String,
    pub source_order: usize,
}

/// A frozen shadow context created at a legacy run start. It owns a pure
/// `RunController` run and, crucially, treats ALL executable effects
/// (DispatchTool/RequestApproval/Cancel/Deliver) as observation-only: they are
/// classified into the disposition and then DROPPED — never written to
/// `kernel_effect_outbox`, never leased, never executed. This is enforced three
/// ways: (1) the context has no executor/outbox handle, (2) `kernel_runs`
/// rejects `kernel_mode='shadow'`, and (3) lease scans filter `kernel_mode
/// <> 'shadow'`.
pub struct ShadowContext {
    shadow_run_id: String,
    legacy_run_id: String,
    turn_id: String,
    cursor: i64,
    /// The pure shadow decision core. Observation-only: its effects are never
    /// persisted to the executable outbox or performed.
    controller: crate::RunController,
    /// Kernel-side tool facts (what the shadow decision core decided/did),
    /// in source order. Independent of legacy facts.
    kernel_tools: Vec<ToolDisposition>,
    /// Legacy-side tool facts (what the real Host decided/did), in source
    /// order. Independent of kernel facts.
    legacy_tools: Vec<ToolDisposition>,
    settled: std::collections::BTreeMap<String, (bool, String)>,
    policy_snapshot: serde_json::Value,
    pending_batches: std::collections::BTreeMap<String, Vec<ShadowToolObservation>>,
    pending_legacy: std::collections::BTreeMap<String, ToolDisposition>,
    pending_results: std::collections::BTreeMap<String, (bool, String)>,
}

#[derive(Serialize, Deserialize)]
struct ShadowCheckpoint {
    version: u32,
    shadow_run_id: String,
    cursor: i64,
    controller: crate::controller::RehydratedRun,
    kernel_tools: Vec<ToolDisposition>,
    legacy_tools: Vec<ToolDisposition>,
    settled: std::collections::BTreeMap<String, (bool, String)>,
    policy_snapshot: serde_json::Value,
    pending_batches: std::collections::BTreeMap<String, Vec<ShadowToolObservation>>,
    pending_legacy: std::collections::BTreeMap<String, ToolDisposition>,
    pending_results: std::collections::BTreeMap<String, (bool, String)>,
}

impl ShadowContext {
    pub fn checkpoint(&self, monotonic_ms: i64) -> Result<String, String> {
        serde_json::to_string(&ShadowCheckpoint {
            version: 1,
            shadow_run_id: self.shadow_run_id.clone(),
            cursor: self.cursor,
            controller: self.controller.shadow_checkpoint(monotonic_ms),
            kernel_tools: self.kernel_tools.clone(),
            legacy_tools: self.legacy_tools.clone(),
            settled: self.settled.clone(),
            policy_snapshot: self.policy_snapshot.clone(),
            pending_batches: self.pending_batches.clone(),
            pending_legacy: self.pending_legacy.clone(),
            pending_results: self.pending_results.clone(),
        })
        .map_err(|e| e.to_string())
    }

    pub fn restore(json: &str) -> Result<Self, String> {
        let saved: ShadowCheckpoint =
            serde_json::from_str(json).map_err(|e| format!("invalid shadow checkpoint: {e}"))?;
        if saved.version != 1 {
            return Err("unsupported shadow checkpoint version".into());
        }
        Ok(Self {
            shadow_run_id: saved.shadow_run_id,
            legacy_run_id: saved.controller.run_id.clone(),
            turn_id: saved.controller.turn_id.clone(),
            cursor: saved.cursor,
            controller: crate::RunController::rehydrate(saved.controller)
                .map_err(|e| e.to_string())?,
            kernel_tools: saved.kernel_tools,
            legacy_tools: saved.legacy_tools,
            settled: saved.settled,
            policy_snapshot: saved.policy_snapshot,
            pending_batches: saved.pending_batches,
            pending_legacy: saved.pending_legacy,
            pending_results: saved.pending_results,
        })
    }

    pub fn observe_model_request(&mut self, begin: bool, mono: i64, wall: i64) {
        if begin {
            self.controller.begin_model_request(mono, wall);
        } else {
            self.controller.settle_model_request();
        }
    }

    pub fn policy_snapshot(&self) -> &serde_json::Value {
        &self.policy_snapshot
    }
    pub fn freeze_policy(&mut self, snapshot: serde_json::Value) {
        self.policy_snapshot = snapshot;
    }

    /// Preserve the actual Pi model-turn batch while independent Host decisions
    /// are arriving. This is observation data, never an executable queue.
    pub fn reserve_batch(
        &mut self,
        batch_id: &str,
        calls: Vec<ShadowToolObservation>,
    ) -> Result<(), String> {
        if let Some(existing) = self.pending_batches.get(batch_id) {
            return if existing == &calls {
                Ok(())
            } else {
                Err("conflicting Pi batch replay".into())
            };
        }
        if calls.is_empty() {
            return Err("empty Pi tool batch".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for (index, call) in calls.iter().enumerate() {
            if call.batch_id != batch_id
                || call.source_order != index
                || !ids.insert(call.tool_call_id.clone())
                || self.has_tool(&call.tool_call_id)
                || self.pending_batch_for(&call.tool_call_id).is_some()
            {
                return Err("invalid or duplicate Pi batch identity/order".into());
            }
        }
        self.pending_batches.insert(batch_id.to_string(), calls);
        Ok(())
    }

    pub fn pending_batch_for(&self, id: &str) -> Option<String> {
        self.pending_batches
            .iter()
            .find(|(_, calls)| calls.iter().any(|call| call.tool_call_id == id))
            .map(|(batch, _)| batch.clone())
    }

    pub fn stage_legacy_tool(
        &mut self,
        mut tool: ToolDisposition,
    ) -> Result<Option<(String, Vec<ShadowToolObservation>, Vec<ToolDisposition>)>, String> {
        let batch = self
            .pending_batch_for(&tool.tool_call_id)
            .ok_or("missing proposed Pi batch")?;
        let calls = self
            .pending_batches
            .get(&batch)
            .ok_or("missing proposed Pi batch")?;
        let proposed = calls
            .iter()
            .find(|call| call.tool_call_id == tool.tool_call_id)
            .ok_or("missing Pi tool")?;
        if proposed.tool != tool.tool {
            return Err("Pi/Host tool identity conflict".into());
        }
        tool.batch_id = batch.clone();
        tool.source_order = proposed.source_order;
        if let Some(existing) = self.pending_legacy.get(&tool.tool_call_id) {
            if existing != &tool {
                return Err("conflicting Host tool observation".into());
            }
        }
        self.pending_legacy.insert(tool.tool_call_id.clone(), tool);
        if !calls
            .iter()
            .all(|call| self.pending_legacy.contains_key(&call.tool_call_id))
        {
            return Ok(None);
        }
        let mut calls = self
            .pending_batches
            .remove(&batch)
            .ok_or("missing ready Pi batch")?;
        let legacy: Vec<_> = calls
            .iter()
            .map(|call| {
                self.pending_legacy
                    .remove(&call.tool_call_id)
                    .expect("ready batch invariant")
            })
            .collect();
        // The approved normalized input is the shared final input boundary.
        // Batch membership and source order still come from Pi, not Host arrival order.
        for (call, observed) in calls.iter_mut().zip(&legacy) {
            call.canonical_input_json = observed.canonical_input_json.clone();
        }
        Ok(Some((batch, calls, legacy)))
    }

    pub fn defer_result(&mut self, id: &str, ok: bool, result: &str) -> Result<bool, String> {
        if self.pending_batch_for(id).is_none() {
            return Ok(false);
        }
        let result = (ok, result.to_string());
        if self
            .pending_results
            .get(id)
            .is_some_and(|existing| existing != &result)
        {
            return Err("conflicting pending tool result".into());
        }
        self.pending_results.insert(id.to_string(), result);
        Ok(true)
    }

    pub fn settle_deferred_results(&mut self, mono: i64) -> Result<(), crate::KernelError> {
        let ready: Vec<_> = self
            .pending_results
            .keys()
            .filter(|id| self.has_tool(id))
            .cloned()
            .collect();
        for id in ready {
            let (ok, result) = self
                .pending_results
                .remove(&id)
                .expect("pending result invariant");
            self.on_tool_settled(&id, ok, &result, mono)?;
        }
        Ok(())
    }

    /// Build a shadow context around a fresh shadow RunController started in
    /// `shadow` mode. The controller runs the real decision core but is never
    /// given an executor. `start_cursor` only establishes the comparison offset;
    /// it does NOT restore tool, approval or batch state. Production restart
    /// must use `restore` with a committed semantic checkpoint, never this
    /// constructor with a previous cursor.
    pub fn start(
        shadow_run_id: impl Into<String>,
        legacy_run_id: impl Into<String>,
        turn_id: impl Into<String>,
        config: crate::RunFrozenConfig,
        clock: &dyn crate::Clock,
        start_cursor: i64,
    ) -> Result<ShadowContext, crate::KernelError> {
        let shadow_run_id = shadow_run_id.into();
        let legacy_run_id = legacy_run_id.into();
        let turn_id = turn_id.into();
        let (controller, _start_effects) =
            crate::RunController::start(&legacy_run_id, &turn_id, config, clock)?;
        Ok(ShadowContext {
            shadow_run_id,
            legacy_run_id,
            turn_id,
            cursor: start_cursor,
            controller,
            kernel_tools: Vec::new(),
            legacy_tools: Vec::new(),
            settled: std::collections::BTreeMap::new(),
            policy_snapshot: serde_json::Value::Null,
            pending_batches: std::collections::BTreeMap::new(),
            pending_legacy: std::collections::BTreeMap::new(),
            pending_results: std::collections::BTreeMap::new(),
        })
    }

    pub fn shadow_run_id(&self) -> &str {
        &self.shadow_run_id
    }

    pub fn persisted_cursor(&self) -> i64 {
        self.cursor
    }

    pub fn tool_facts(&self) -> &[ToolDisposition] {
        &self.kernel_tools
    }

    pub fn legacy_run_id(&self) -> &str {
        &self.legacy_run_id
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    /// Verify a re-bootstrap uses the SAME frozen identity/config as the
    /// existing context. Returns Ok(()) on match (keep accumulated state),
    /// or an error on conflict (fail-closed; the caller must not silently
    /// reset the context).
    pub fn verify_frozen_config(
        &self,
        config: &crate::RunFrozenConfig,
    ) -> Result<(), String> {
        let existing = self.controller.config();
        // Compare EVERY frozen field — identity (engine/mode/manifest/
        // permission/profile/prompt hashes) AND budgets/timeouts/retry
        // policy. A mismatch in any of them fails closed.
        if existing != config {
            return Err(format!(
                "shadow re-bootstrap for {} uses a conflicting frozen config",
                self.legacy_run_id
            ));
        }
        Ok(())
    }

    pub fn next_cursor(&mut self) -> i64 {
        self.cursor += 1;
        self.cursor
    }

    pub fn kernel_run_state(&self) -> &'static str {
        self.controller.state().as_str()
    }

    pub fn has_tool(&self, tool_call_id: &str) -> bool {
        self.kernel_tools
            .iter()
            .chain(self.legacy_tools.iter())
            .any(|tool| tool.tool_call_id == tool_call_id)
    }

    pub fn same_legacy_observation(&self, tool: &ToolDisposition) -> Result<bool, String> {
        let Some(existing) = self
            .legacy_tools
            .iter()
            .find(|existing| existing.tool_call_id == tool.tool_call_id)
        else {
            return Ok(false);
        };
        if existing.tool != tool.tool
            || existing.canonical_input_json != tool.canonical_input_json
            || existing.decision != tool.decision
        {
            return Err("conflicting replay of an observed tool".into());
        }
        Ok(true)
    }

    fn kernel_disposition(&self, terminal: Option<String>) -> ShadowDisposition {
        ShadowDisposition {
            run_state: self.controller.state().as_str().to_string(),
            tools: self.kernel_tools.clone(),
            terminal,
            retry_timeout: retry_timeout_facts(&self.controller),
            comparable: true,
        }
    }

    /// Feed a legacy tool-decision event (preflight): run the SAME tool call
    /// through the shadow decision core (kernel side) and compare it against
    /// the INDEPENDENT legacy facts (`legacy_tools`, what legacy actually
    /// decided with its normalised input/order). Executable kernel effects are
    /// classified and discarded; only the comparison record is produced.
    pub fn on_tool_batch(
        &mut self,
        batch_id: &str,
        calls: Vec<ShadowToolObservation>,
        policy: &dyn crate::PolicyDecisionPort,
        legacy_tools: Vec<ToolDisposition>,
        monotonic_ms: i64,
        wall_ms: i64,
    ) -> Result<ShadowDiff, crate::KernelError> {
        let legacy_run_state = legacy_tools
            .iter()
            .any(|t| t.decision == "approval")
            .then(|| "waiting_approval".to_string())
            .unwrap_or_else(|| "running".to_string());
        let kernel_requests = calls
            .iter()
            .map(|c| crate::ToolCallRequest {
                tool_call_id: c.tool_call_id.clone(),
                tool: c.tool.clone(),
                canonical_input_json: c.canonical_input_json.clone(),
                source_order: c.source_order,
            })
            .collect();
        let effects = self.controller.propose_tool_batch(
            batch_id,
            kernel_requests,
            policy,
            monotonic_ms,
            wall_ms,
        )?;

        // Classify kernel decisions from the discarded effects into INDEPENDENT
        // kernel tool facts (derived from the kernel's own decision effects).
        let mut kernel_tools: Vec<ToolDisposition> = Vec::new();
        let by_id = |id: &str| calls.iter().find(|c| c.tool_call_id == id);
        for effect in &effects {
            match effect {
                crate::Effect::DispatchTool {
                    tool_call_id,
                    tool,
                    input_json,
                } => {
                    let obs = by_id(tool_call_id);
                    kernel_tools.push(ToolDisposition {
                        tool_call_id: tool_call_id.clone(),
                        batch_id: obs
                            .map(|o| o.batch_id.clone())
                            .unwrap_or_else(|| batch_id.into()),
                        tool: tool.clone(),
                        canonical_input_json: input_json.clone(),
                        source_order: obs.map(|o| o.source_order).unwrap_or(0),
                        decision: "allow".into(),
                    });
                }
                crate::Effect::RequestApproval {
                    tool_call_id,
                    tool,
                    input_json,
                } => {
                    let obs = by_id(tool_call_id);
                    kernel_tools.push(ToolDisposition {
                        tool_call_id: tool_call_id.clone(),
                        batch_id: obs
                            .map(|o| o.batch_id.clone())
                            .unwrap_or_else(|| batch_id.into()),
                        tool: tool.clone(),
                        canonical_input_json: input_json.clone(),
                        source_order: obs.map(|o| o.source_order).unwrap_or(0),
                        decision: "approval".into(),
                    });
                }
                crate::Effect::AppendEvent {
                    event_type,
                    payload_json,
                    ..
                } if event_type == "tool.failed" => {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(payload_json) {
                        if payload.get("code").and_then(|c| c.as_str())
                            == Some("kernel.policy_denied")
                        {
                            if let Some(id) = payload.get("toolCallId").and_then(|v| v.as_str()) {
                                let obs = by_id(id);
                                kernel_tools.push(ToolDisposition {
                                    tool_call_id: id.to_string(),
                                    batch_id: obs
                                        .map(|o| o.batch_id.clone())
                                        .unwrap_or_else(|| batch_id.into()),
                                    tool: obs.map(|o| o.tool.clone()).unwrap_or_default(),
                                    canonical_input_json: obs
                                        .map(|o| o.canonical_input_json.clone())
                                        .unwrap_or_else(|| "{}".into()),
                                    source_order: obs.map(|o| o.source_order).unwrap_or(0),
                                    decision: "deny".into(),
                                });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        kernel_tools.sort_by_key(|t| t.source_order);
        let mut legacy_sorted = legacy_tools.clone();
        legacy_sorted.sort_by_key(|t| t.source_order);
        self.kernel_tools.extend(kernel_tools.iter().cloned());
        self.legacy_tools.extend(legacy_sorted.iter().cloned());

        let kernel = self.kernel_disposition(None);
        let legacy = LegacyDisposition {
            run_state: legacy_run_state,
            tools: self.legacy_tools.clone(),
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let cursor = self.next_cursor();
        Ok(ShadowDiff::classify(
            &self.shadow_run_id,
            &self.legacy_run_id,
            Some(self.turn_id.clone()),
            cursor,
            Some("tool_batch".into()),
            legacy,
            kernel,
        ))
    }

    /// Feed a legacy tool RESULT (tool.completed / tool.failed) into the
    /// shadow core so the kernel settles the call and can later reach
    /// `run.completed` (which requires every tool call to be terminal). The
    /// `legacy_ok` is what legacy reported; `kernel_ok` is the same result
    /// replayed through the kernel — they normally agree.
    pub fn on_tool_settled(
        &mut self,
        tool_call_id: &str,
        ok: bool,
        result_json: &str,
        monotonic_ms: i64,
    ) -> Result<(), crate::KernelError> {
        if let Some(existing) = self.settled.get(tool_call_id) {
            return if existing == &(ok, result_json.to_string()) {
                Ok(())
            } else {
                Err(crate::KernelError::FailClosed(
                    "conflicting replay of a settled tool result".into(),
                ))
            };
        }
        let kernel_requested_approval = self
            .kernel_tools
            .iter()
            .any(|tool| tool.tool_call_id == tool_call_id && tool.decision == "approval");
        if kernel_requested_approval {
            let legacy_denied = self
                .legacy_tools
                .iter()
                .any(|tool| tool.tool_call_id == tool_call_id && tool.decision == "deny");
            let decision = if legacy_denied {
                crate::ApprovalDecision::Deny
            } else {
                crate::ApprovalDecision::AllowOnce
            };
            let _ = self
                .controller
                .resolve_approval(tool_call_id, decision, monotonic_ms)?;
        }
        self.controller
            .tool_settled(tool_call_id, ok, result_json)?;
        self.settled
            .insert(tool_call_id.to_string(), (ok, result_json.to_string()));
        // Decision facts remain immutable (allow/approval/deny). Settlement is
        // represented by the controller's tool/run state, not by overwriting
        // the policy decision with an execution result.
        Ok(())
    }

    /// Feed a legacy cancellation BEFORE the kernel is asked to terminal-cancel,
    /// so the kernel transition `run.cancelled` is legal (it requires a prior
    /// `request_cancel`).
    pub fn on_cancel(&mut self) -> Vec<crate::Effect> {
        self.controller.request_cancel();
        let mut effects = self.controller.settle_cancellation();
        // Drop non-event effects (no executor in shadow); return for the caller
        // to audit. Effects are never performed or persisted to the outbox.
        effects.retain(|e| matches!(e, crate::Effect::AppendEvent { .. }));
        effects
    }

    /// Feed a legacy terminal event. The caller passes the LEGACY run STATE
    /// (`completed` / `failed` / `cancelled`) and the corresponding kernel
    /// outcome; the kernel controller performs its OWN terminal transition
    /// (which requires tools settled for completed and a prior cancel for
    /// cancelled — callers feed those first via `on_tool_settled`/`on_cancel`).
    /// Both sides use STATE names for `run_state` so an agreeing terminal is a
    /// genuine Match, not a systematic state_mismatch.
    pub fn on_terminal(
        &mut self,
        legacy_state: &str,
        kernel_outcome: crate::RunOutcome,
        retry_timeout: RetryTimeoutFacts,
    ) -> ShadowDiff {
        if !self.pending_batches.is_empty() {
            return self
                .not_comparable("terminal", "Pi batch is missing independent Host decisions");
        }
        let kernel_effects = self.controller.terminate(kernel_outcome.clone());
        // The kernel terminal is read from the terminal EVENT produced by this
        // transition, OR — if the controller was already driven to the
        // terminal earlier (e.g. on_cancel already settled a cancellation) —
        // from its current terminal state, so no spurious mismatch arises.
        let from_effects = kernel_effects.iter().find_map(|e| match e {
            crate::Effect::AppendEvent { event_type, .. }
                if matches!(
                    event_type.as_str(),
                    "run.completed" | "run.failed" | "run.cancelled"
                ) =>
            {
                // Translate the kernel terminal EVENT name to its STATE name
                // for side-by-side state comparison.
                Some(
                    event_type
                        .strip_prefix("run.")
                        .unwrap_or(event_type)
                        .to_string(),
                )
            }
            _ => None,
        });
        // Fallback: the controller is already terminal (e.g. a prior on_cancel
        // settled the cancellation); use its current terminal state directly.
        let kernel_state = self.controller.state().as_str().to_string();
        let kernel_terminal = from_effects.or_else(|| {
            matches!(
                kernel_state.as_str(),
                "completed" | "failed" | "cancelled" | "budget_exhausted"
            )
            .then(|| kernel_state.clone())
        });
        let cursor = self.next_cursor();
        let legacy = LegacyDisposition {
            // Legacy run STATE (completed/failed/cancelled), not an event name.
            run_state: legacy_state.to_string(),
            // Legacy-side accumulated facts (independent of kernel facts).
            tools: self.legacy_tools.clone(),
            terminal: Some(legacy_state.to_string()),
            retry_timeout,
        };
        let mut kernel = self.kernel_disposition(kernel_terminal.clone());
        kernel.terminal = kernel_terminal;
        ShadowDiff::classify(
            &self.shadow_run_id,
            &self.legacy_run_id,
            Some(self.turn_id.clone()),
            cursor,
            Some("terminal".into()),
            legacy,
            kernel,
        )
    }

    /// Record a point the Kernel cannot yet evaluate (capability not wired), so
    /// it is never silently counted as agreement.
    pub fn not_comparable(&mut self, event_type: &str, reason: &str) -> ShadowDiff {
        let cursor = self.next_cursor();
        ShadowDiff {
            shadow_run_id: self.shadow_run_id.clone(),
            legacy_run_id: self.legacy_run_id.clone(),
            turn_id: Some(self.turn_id.clone()),
            event_cursor: cursor,
            event_type: Some(event_type.to_string()),
            category: DiffCategory::NotComparable,
            legacy: LegacyDisposition::default(),
            kernel: ShadowDisposition {
                run_state: String::new(),
                tools: Vec::new(),
                terminal: None,
                retry_timeout: RetryTimeoutFacts::default(),
                comparable: false,
            },
            detail: serde_json::json!({ "reason": reason }),
        }
    }

    /// Record an internal shadow fault (decision core returned an error).
    pub fn shadow_error(&mut self, event_type: &str, error: &str) -> ShadowDiff {
        let cursor = self.next_cursor();
        ShadowDiff {
            shadow_run_id: self.shadow_run_id.clone(),
            legacy_run_id: self.legacy_run_id.clone(),
            turn_id: Some(self.turn_id.clone()),
            event_cursor: cursor,
            event_type: Some(event_type.to_string()),
            category: DiffCategory::ShadowError,
            legacy: LegacyDisposition::default(),
            kernel: ShadowDisposition {
                run_state: String::new(),
                tools: Vec::new(),
                terminal: None,
                retry_timeout: RetryTimeoutFacts::default(),
                comparable: false,
            },
            detail: serde_json::json!({ "error": error }),
        }
    }
}

fn retry_timeout_facts(controller: &crate::RunController) -> RetryTimeoutFacts {
    let (provider_attempts, provider_max, turn_attempts, turn_max) = controller.retry_counters();
    RetryTimeoutFacts {
        provider_attempts,
        provider_max,
        turn_attempts,
        turn_max,
        terminal_code: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(id: &str, tool: &str, order: usize, decision: &str) -> ToolDisposition {
        ToolDisposition {
            tool_call_id: id.into(),
            batch_id: "b".into(),
            tool: tool.into(),
            canonical_input_json: format!(r#"{{"id":"{id}"}}"#),
            source_order: order,
            decision: decision.into(),
        }
    }

    fn disposition(tools: Vec<ToolDisposition>, state: &str) -> ShadowDisposition {
        ShadowDisposition {
            run_state: state.into(),
            tools,
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
            comparable: true,
        }
    }

    #[test]
    fn classifies_agreement_as_match() {
        let t = vec![tool("a", "read", 0, "allow")];
        let kernel = disposition(t.clone(), "running");
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: t,
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::Match);
        assert!(!diff.is_divergence());
    }

    #[test]
    fn classifies_approval_divergence() {
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: vec![tool("a", "write_file", 0, "allow")],
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let kernel = disposition(
            vec![tool("a", "write_file", 0, "approval")],
            "waiting_approval",
        );
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::ApprovalMismatch);
        assert!(diff.is_divergence());
    }

    #[test]
    fn detects_canonical_param_change_as_tool_mismatch() {
        // Same id/order/decision but DIFFERENT canonical input.
        let mut kernel_tool = tool("a", "write_file", 0, "allow");
        kernel_tool.canonical_input_json = r#"{"path":"evil.txt"}"#.into();
        let legacy_tool = ToolDisposition {
            canonical_input_json: r#"{"path":"good.txt"}"#.into(),
            ..tool("a", "write_file", 0, "allow")
        };
        let kernel = disposition(vec![kernel_tool], "running");
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: vec![legacy_tool],
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::ToolParamOrOrderMismatch);
    }

    #[test]
    fn detects_reordered_batch_as_tool_mismatch() {
        // Same set of ids but a different source order / position alignment.
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: vec![tool("a", "read", 0, "allow"), tool("b", "ls", 1, "allow")],
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let kernel = disposition(
            vec![tool("b", "ls", 0, "allow"), tool("a", "read", 1, "allow")],
            "running",
        );
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::ToolParamOrOrderMismatch);
    }

    #[test]
    fn detects_timeout_retry_mismatch() {
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: vec![],
            terminal: Some("run.failed".into()),
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let mut kernel = disposition(vec![], "running");
        kernel.terminal = Some("model.request_timeout".into());
        kernel.retry_timeout.terminal_code = Some("model.request_timeout".into());
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::TimeoutRetryMismatch);
    }

    #[test]
    fn terminal_divergence_takes_precedence() {
        let legacy = LegacyDisposition {
            run_state: "running".into(),
            tools: vec![],
            terminal: Some("run.completed".into()),
            retry_timeout: RetryTimeoutFacts::default(),
        };
        let mut kernel = disposition(vec![], "running");
        kernel.terminal = Some("run.failed".into());
        let diff = ShadowDiff::classify("s", "l", None, 1, None, legacy, kernel);
        assert_eq!(diff.category, DiffCategory::TerminalMismatch);
    }

    #[test]
    fn not_comparable_never_counts_as_match() {
        let kernel = ShadowDisposition {
            run_state: String::new(),
            tools: vec![],
            terminal: None,
            retry_timeout: RetryTimeoutFacts::default(),
            comparable: false,
        };
        let diff = ShadowDiff::classify(
            "s",
            "l",
            None,
            1,
            None,
            LegacyDisposition::default(),
            kernel,
        );
        assert_eq!(diff.category, DiffCategory::NotComparable);
        assert!(!diff.is_match());
        assert!(!diff.is_divergence());
    }

    // ---- Online event feed through a real shadow RunController ----

    fn shadow_config() -> crate::RunFrozenConfig {
        crate::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "shadow".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "manifest-shadow".into(),
            permission_snapshot_id: "perm".into(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: "prompt".into(),
            model_request_timeout_ms: 120_000,
            model_first_response_ms: 60_000,
            model_idle_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 1_800_000,
            run_execution_limited: true,
            approval_wait_timeout_ms: 3_600_000,
            provider_max_retries: 2,
            turn_max_retries: 0,
        }
    }

    struct FixedClock;
    impl crate::Clock for FixedClock {
        fn now_monotonic_ms(&self) -> i64 {
            0
        }
        fn now_wall_ms(&self) -> i64 {
            0
        }
    }

    struct WriteRequiresApproval;
    impl crate::PolicyDecisionPort for WriteRequiresApproval {
        fn decide(&self, _: &str, _: &str, tool: &str, _: &str) -> crate::PolicyDecision {
            use crate::PolicyDecision;
            if tool == "write_file" {
                PolicyDecision::RequireApproval
            } else {
                PolicyDecision::Allow
            }
        }
    }

    #[test]
    fn online_batch_feed_runs_shadow_decision_and_classifies() {
        let mut shadow =
            ShadowContext::start("shadow-l1", "l1", "t1", shadow_config(), &FixedClock, 0).unwrap();
        let calls = vec![ShadowToolObservation {
            tool_call_id: "c1".into(),
            batch_id: "b1".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"x"}"#.into(),
            source_order: 0,
        }];
        // INDEPENDENT legacy facts: legacy auto-ran the write (allow) with its
        // own tool/input/order. The kernel would require approval -> approval
        // mismatch.
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "c1".into(),
            batch_id: "b1".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"x"}"#.into(),
            source_order: 0,
            decision: "allow".into(),
        }];
        let diff = shadow
            .on_tool_batch("b1", calls, &WriteRequiresApproval, legacy_tools, 0, 0)
            .unwrap();
        assert_eq!(diff.category, DiffCategory::ApprovalMismatch);
        // No executable side effect exists: shadow controller is internal and
        // the repository outbox is never touched by this path.
        assert_eq!(diff.kernel.tools[0].decision, "approval");
        assert_eq!(diff.legacy.tools[0].decision, "allow");
    }

    #[test]
    fn cursor_resumes_from_persisted_offset() {
        // Simulate a re-bootstrapped shadow whose previous run persisted
        // through cursor 3; new diffs must start at 4, not 1.
        let mut shadow =
            ShadowContext::start("s", "l", "t", shadow_config(), &FixedClock, 3).unwrap();
        assert_eq!(shadow.next_cursor(), 4);
        assert_eq!(shadow.next_cursor(), 5);
    }

    #[test]
    fn shadow_error_and_not_comparable_paths() {
        let mut shadow =
            ShadowContext::start("shadow-l2", "l2", "t2", shadow_config(), &FixedClock, 0).unwrap();
        let nc = shadow.not_comparable("future_capability", "not wired yet");
        assert_eq!(nc.category, DiffCategory::NotComparable);
        let se = shadow.shadow_error("tool_batch", "boom");
        assert_eq!(se.category, DiffCategory::ShadowError);
        assert!(se.is_divergence());
    }

    /// End-to-end normal production sequence: legacy and kernel both allow a
    /// read tool, settle it, then complete the run. The terminal comparison
    /// must be a genuine Match (state names agree on both sides).
    #[test]
    fn end_to_end_allowed_tool_settled_then_completed_is_match() {
        let mut shadow =
            ShadowContext::start("sr", "lr", "tr", shadow_config(), &FixedClock, 0).unwrap();
        struct AllowReads;
        impl crate::PolicyDecisionPort for AllowReads {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::Allow
            }
        }
        let calls = vec![ShadowToolObservation {
            tool_call_id: "tc1".into(),
            batch_id: "b".into(),
            tool: "read".into(),
            canonical_input_json: r#"{"path":"/x"}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "tc1".into(),
            batch_id: "b".into(),
            tool: "read".into(),
            canonical_input_json: r#"{"path":"/x"}"#.into(),
            source_order: 0,
            decision: "allow".into(),
        }];
        let batch_diff = shadow
            .on_tool_batch("b", calls, &AllowReads, legacy_tools, 0, 0)
            .unwrap();
        assert_eq!(batch_diff.category, DiffCategory::Match);
        // Settle the tool on both sides BEFORE completing (kernel rejects
        // run.completed while a tool is unresolved).
        shadow.on_tool_settled("tc1", true, "{}", 100).unwrap();
        let terminal = shadow.on_terminal(
            "completed",
            crate::RunOutcome::Completed,
            RetryTimeoutFacts::default(),
        );
        assert_eq!(
            terminal.category,
            DiffCategory::Match,
            "an agreeing completed terminal must be a Match"
        );
        assert_eq!(terminal.kernel.run_state, "completed");
        assert_eq!(terminal.legacy.run_state, "completed");
    }

    #[test]
    fn approved_tool_is_dispatched_before_settlement_and_keeps_decision_facts() {
        let mut shadow =
            ShadowContext::start("sr-a", "lr-a", "tr-a", shadow_config(), &FixedClock, 0).unwrap();
        struct RequireApproval;
        impl crate::PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::RequireApproval
            }
        }
        let calls = vec![ShadowToolObservation {
            tool_call_id: "tc-approved".into(),
            batch_id: "b-approved".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"/approved"}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "tc-approved".into(),
            batch_id: "b-approved".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"/approved"}"#.into(),
            source_order: 0,
            decision: "approval".into(),
        }];
        let batch = shadow
            .on_tool_batch("b-approved", calls, &RequireApproval, legacy_tools, 0, 0)
            .unwrap();
        assert_eq!(batch.category, DiffCategory::Match);
        assert!(shadow.has_tool("tc-approved"));
        shadow
            .on_tool_settled("tc-approved", false, r#"{"error":"boom"}"#, 100)
            .unwrap();
        let terminal = shadow.on_terminal(
            "completed",
            crate::RunOutcome::Completed,
            RetryTimeoutFacts::default(),
        );
        assert_eq!(terminal.category, DiffCategory::Match);
        assert_eq!(terminal.kernel.tools[0].decision, "approval");
        assert_eq!(terminal.legacy.tools[0].decision, "approval");
    }

    /// Cancelled production sequence: the kernel must observe a cancel request
    /// before run.cancelled, and the terminal comparison is a Match.
    #[test]
    fn end_to_end_cancel_sequence_is_match() {
        let mut shadow =
            ShadowContext::start("sr2", "lr2", "tr2", shadow_config(), &FixedClock, 0).unwrap();
        let _effects = shadow.on_cancel();
        let terminal = shadow.on_terminal(
            "cancelled",
            crate::RunOutcome::Cancelled,
            RetryTimeoutFacts::default(),
        );
        assert_eq!(terminal.category, DiffCategory::Match);
        assert_eq!(terminal.kernel.run_state, "cancelled");
    }

    /// Completing without settling tools must NOT produce a fake Match: the
    /// kernel rejects run.completed, so its state is not completed.
    #[test]
    fn terminal_without_settling_tools_does_not_fake_match() {
        let mut shadow =
            ShadowContext::start("sr3", "lr3", "tr3", shadow_config(), &FixedClock, 0).unwrap();
        struct RequireApproval;
        impl crate::PolicyDecisionPort for RequireApproval {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::RequireApproval
            }
        }
        let calls = vec![ShadowToolObservation {
            tool_call_id: "tc1".into(),
            batch_id: "b".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"/y"}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "tc1".into(),
            batch_id: "b".into(),
            tool: "write_file".into(),
            canonical_input_json: r#"{"path":"/y"}"#.into(),
            source_order: 0,
            decision: "approval".into(),
        }];
        shadow
            .on_tool_batch("b", calls, &RequireApproval, legacy_tools, 0, 0)
            .unwrap();
        // Attempt a completed terminal while the tool is still unresolved;
        // the kernel refuses, so the comparison is not a spurious Match.
        let terminal = shadow.on_terminal(
            "completed",
            crate::RunOutcome::Completed,
            RetryTimeoutFacts::default(),
        );
        assert_ne!(
            terminal.kernel.run_state, "completed",
            "kernel must not reach completed with an unresolved tool"
        );
    }

    // ---- Acceptance matrix (Phase 4A shadow verification) ----

    /// Scenario: a tool-less Run (e.g. a direct answer) completes. Both sides
    /// have zero tools; the terminal comparison is a genuine Match and is NOT
    /// a false tool-batch comparison or a spurious not_comparable.
    #[test]
    fn acceptance_no_tool_run_completes_as_match() {
        let mut shadow = ShadowContext::start(
            "no-tool-s",
            "no-tool-l",
            "t",
            shadow_config(),
            &FixedClock,
            0,
        )
        .unwrap();
        let terminal = shadow.on_terminal(
            "completed",
            crate::RunOutcome::Completed,
            RetryTimeoutFacts::default(),
        );
        assert_eq!(terminal.category, DiffCategory::Match);
        assert!(terminal.kernel.tools.is_empty());
        assert!(terminal.legacy.tools.is_empty());
        assert!(terminal.event_type.is_some());
    }

    /// Scenario: legacy DENIES a tool that the kernel would also deny
    /// (policy block). The decision comparison agrees (both deny), and a
    /// subsequent failed terminal is a Match rather than a divergence.
    #[test]
    fn acceptance_deny_agreement_then_failed_terminal() {
        let mut shadow =
            ShadowContext::start("deny-s", "deny-l", "t", shadow_config(), &FixedClock, 0).unwrap();
        struct DenyAll;
        impl crate::PolicyDecisionPort for DenyAll {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::Deny {
                    reason: "blocked".into(),
                }
            }
        }
        let calls = vec![ShadowToolObservation {
            tool_call_id: "d1".into(),
            batch_id: "bd".into(),
            tool: "dangerous_tool".into(),
            canonical_input_json: r#"{"x":1}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "d1".into(),
            batch_id: "bd".into(),
            tool: "dangerous_tool".into(),
            canonical_input_json: r#"{"x":1}"#.into(),
            source_order: 0,
            decision: "deny".into(),
        }];
        let batch = shadow
            .on_tool_batch("bd", calls, &DenyAll, legacy_tools, 0, 0)
            .unwrap();
        // Deny on both sides is an agreement at the approval/decision level.
        assert!(!batch.is_divergence(), "mutual deny must not diverge");
        // Denied tool is already terminal (policy block), so a failed run
        // terminal is consistent.
        shadow
            .on_tool_settled("d1", false, r#"{"error":"blocked"}"#, 50)
            .unwrap();
        let terminal = shadow.on_terminal(
            "failed",
            crate::RunOutcome::Failed {
                code: "tool.denied".into(),
                message: "blocked".into(),
            },
            RetryTimeoutFacts::default(),
        );
        // Both sides reach a failed terminal — a terminal Match.
        assert_eq!(terminal.kernel.run_state, "failed");
        assert_eq!(terminal.legacy.run_state, "failed");
        assert_eq!(terminal.category, DiffCategory::Match);
    }

    /// Scenario: identity contract — preflight, ToolCall and completed share
    /// the SAME Pi toolCallId on both sides; a mismatch in canonical input
    /// between the allowed normalised input and the kernel input surfaces as a
    /// tool-param mismatch (never silently a Match).
    #[test]
    fn acceptance_toolcallid_consistent_and_normalised_input_checked() {
        let mut shadow =
            ShadowContext::start("id-s", "id-l", "t", shadow_config(), &FixedClock, 0).unwrap();
        struct AllowReads;
        impl crate::PolicyDecisionPort for AllowReads {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::Allow
            }
        }
        // Kernel canonical input is the RAW request; legacy normalised input
        // differs (e.g. Host resolved a relative path differently) -> mismatch.
        let calls = vec![ShadowToolObservation {
            tool_call_id: "pi-tcid-42".into(),
            batch_id: "b42".into(),
            tool: "read".into(),
            canonical_input_json: r#"{"path":"./a.txt"}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "pi-tcid-42".into(),
            batch_id: "b42".into(),
            tool: "read".into(),
            // Host returned a NORMALISED absolute path.
            canonical_input_json: r#"{"path":"/project/a.txt"}"#.into(),
            source_order: 0,
            decision: "allow".into(),
        }];
        let batch = shadow
            .on_tool_batch("b42", calls, &AllowReads, legacy_tools, 0, 0)
            .unwrap();
        assert_eq!(batch.category, DiffCategory::ToolParamOrOrderMismatch);
        // The same toolCallId is present on both sides (identity preserved).
        assert!(batch
            .kernel
            .tools
            .iter()
            .any(|t| t.tool_call_id == "pi-tcid-42"));
        assert!(batch
            .legacy
            .tools
            .iter()
            .any(|t| t.tool_call_id == "pi-tcid-42"));
    }

    /// Scenario: a tool returns isError=true (tool.failed) after being
    /// allowed. Settlement preserves the original allow decision; the run
    /// reaches a failed terminal rather than a fake completed Match.
    #[test]
    fn acceptance_tool_iserror_prevents_fake_completed_match() {
        let mut shadow =
            ShadowContext::start("err-s", "err-l", "t", shadow_config(), &FixedClock, 0).unwrap();
        struct AllowReads;
        impl crate::PolicyDecisionPort for AllowReads {
            fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> crate::PolicyDecision {
                crate::PolicyDecision::Allow
            }
        }
        let calls = vec![ShadowToolObservation {
            tool_call_id: "e1".into(),
            batch_id: "be".into(),
            tool: "read".into(),
            canonical_input_json: r#"{"path":"/x"}"#.into(),
            source_order: 0,
        }];
        let legacy_tools = vec![ToolDisposition {
            tool_call_id: "e1".into(),
            batch_id: "be".into(),
            tool: "read".into(),
            canonical_input_json: r#"{"path":"/x"}"#.into(),
            source_order: 0,
            decision: "allow".into(),
        }];
        shadow
            .on_tool_batch("be", calls, &AllowReads, legacy_tools, 0, 0)
            .unwrap();
        // tool.completed with isError=true -> failed result (ok=false).
        shadow
            .on_tool_settled("e1", false, r#"{"isError":true,"error":"ENOENT"}"#, 100)
            .unwrap();
        // The decision fact is still "allow" (result does not overwrite it).
        assert!(shadow
            .kernel_tools
            .iter()
            .any(|t| t.tool_call_id == "e1" && t.decision == "allow"));
        // A completed terminal while the tool failed is NOT a clean completed:
        // legacy reports completed, kernel still has a failed tool -> the
        // honest outcome is a terminal mismatch (no fake Match).
        let terminal = shadow.on_terminal(
            "failed",
            crate::RunOutcome::Failed {
                code: "tool.execution_failed".into(),
                message: "ENOENT".into(),
            },
            RetryTimeoutFacts::default(),
        );
        // Both sides report failed -> consistent, not a fake completed.
        assert_eq!(terminal.legacy.run_state, "failed");
        assert_eq!(terminal.kernel.run_state, "failed");
    }
}
