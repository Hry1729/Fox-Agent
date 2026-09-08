//! Testable Kernel Shadow reconciliation, free of any Tauri `AppHandle`.
//!
//! `ShadowReconciler` owns the in-memory per-run shadow contexts and drives
//! them against a real (temporary) `Database`, persisting comparison records
//! to `kernel_shadow_diffs`. Production `RuntimeHost` delegates to this same
//! implementation. It requires no Tauri app, so deterministic acceptance tests
//! and real Pi subprocess traces can exercise it through durable rows.
//!
//! Observability vs persistence: a comparison is computed in memory first;
//! if the durable write FAILS the run context is moved to a FAILED state (its
//! error is observable in-process) rather than silently dropped or falsely
//! reported as persisted.
//!
//! This never executes tools or leases the executable outbox: it is
//! observation-only. Legacy remains the production authority.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::database::Database;
use crate::kernel::{
    Clock, PolicyDecision, PolicyDecisionPort, RetryTimeoutFacts, RunFrozenConfig, RunOutcome,
    ShadowContext, ShadowDiff, ShadowToolObservation, ToolDisposition,
};
use crate::runtime_host::protocol::canonical_runtime_tool_contract;

/// Legacy execution-authority decision for a tool, derived ONLY from the
/// canonical tool contract + the observed `execution_location` +
/// `requires_approval` — never from terminal/running state.
///
/// - A tool whose canonical execution is "host" but whose projection did NOT
///   promote to execution_location "host" never acquired execution authority
///   (e.g. an early reject) -> `deny`.
/// - `requires_approval` -> `approval`.
/// - otherwise -> `allow`.
/// Unknown tools error rather than guessing.
pub(crate) fn legacy_tool_decision(
    tool: &str,
    execution_location: &str,
    requires_approval: bool,
) -> Result<&'static str, String> {
    let (_, canonical_execution, _) = canonical_runtime_tool_contract(tool)
        .ok_or_else(|| format!("tool '{tool}' is absent from the canonical catalog"))?;
    if canonical_execution == "host" && execution_location != "host" {
        Ok("deny")
    } else if requires_approval {
        Ok("approval")
    } else {
        Ok("allow")
    }
}

/// Kernel-side policy classification, derived from the SAME canonical tool
/// contract (no separate maintained list). Read/inspection tools (the runtime
/// read tools and read-class Host tools that never require approval) are
/// allowed; mutating/process/approval tools require human approval under the
/// frozen default ("ask") policy.
pub(crate) fn kernel_tool_policy(tool: &str) -> PolicyDecision {
    match canonical_runtime_tool_contract(tool) {
        // Runtime read tools are executed by the runtime on inspection only.
        Some((_category, "runtime", _)) => PolicyDecision::Allow,
        Some((_, "host", "none")) => PolicyDecision::Allow,
        // Read-class Host tools (category indicates inspection/reads, and the
        // approval kind is not policy/always) do not require human approval.
        Some((category, "host", approval))
            if approval != "policy"
                && approval != "always"
                && matches!(
                    category,
                    "project-read" | "attachment" | "knowledge" | "memory" | "mcp"
                ) =>
        {
            PolicyDecision::Allow
        }
        _ => PolicyDecision::RequireApproval,
    }
}

/// Production clock. Execution/elapsed time uses a real monotonic source
pub(super) fn frozen_kernel_tool_policy(snapshot: &Value, tool: &str, input_json: &str) -> PolicyDecision {
    let deny = |reason: &str| PolicyDecision::Deny {
        reason: reason.to_string(),
    };
    let Some((category, _, approval)) = canonical_runtime_tool_contract(tool) else {
        return deny("unknown tool");
    };
    let mode = snapshot
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or("ask");
    if !matches!(mode, "ask" | "read_only" | "allow") {
        return deny("unknown frozen permission mode");
    }
    if mode == "read_only" && matches!(category, "project-write" | "process") {
        return deny("frozen read-only policy");
    }
    let input: Value = match serde_json::from_str(input_json) {
        Ok(input) => input,
        Err(_) => return deny("invalid tool input"),
    };
    let scope = if matches!(tool, "write_file" | "edit_file" | "run_command") {
        snapshot
            .get("projectRoot")
            .and_then(Value::as_str)
            .and_then(|root| {
                crate::tool_host::prepare(tool, &input, root)
                    .ok()
                    .and_then(|prepared| super::host_permission_scope(tool, prepared.preview()))
            })
    } else if tool == "call_mcp_tool" {
        super::mcp_permission_scope(tool, &input)
    } else {
        super::capability_permission_scope(tool, &input)
    };
    let granted = scope.as_ref().is_some_and(|scope| {
        snapshot
            .get("grants")
            .and_then(Value::as_array)
            .is_some_and(|grants| {
                grants.iter().any(|grant| {
                    grant.get(0).and_then(Value::as_str) == Some(tool)
                        && grant.get(1).and_then(Value::as_str) == Some(scope)
                })
            })
    });
    if granted || (approval == "policy" && mode == "allow") {
        PolicyDecision::Allow
    } else {
        kernel_tool_policy(tool)
    }
}

/// Production clock. Execution/elapsed time uses a real monotonic source
/// (`Instant`); wall time (`SystemTime`) is used only for persisted deadlines
/// and cursors — never as a monotonic elapsed measurement.
pub(crate) struct ReconcilerClock;
impl Clock for ReconcilerClock {
    fn now_monotonic_ms(&self) -> i64 {
        // Monotonic milliseconds relative to an arbitrary anchor. Used only
        // for in-process elapsed timing within one process lifetime.
        static ANCHOR: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let anchor = ANCHOR.get_or_init(Instant::now);
        ANCHOR
            .get()
            .unwrap_or(anchor)
            .elapsed()
            .as_millis()
            .min(i64::MAX as u128) as i64
    }
    fn now_wall_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// Kernel-side shadow policy: independent of the legacy decision but derived
/// from the SAME canonical tool contract via [`kernel_tool_policy`] (no
/// separately maintained list).
struct ReconcilerPolicy {
    snapshot: Value,
}
impl PolicyDecisionPort for ReconcilerPolicy {
    fn decide(
        &self,
        _run_id: &str,
        _permission_snapshot_id: &str,
        tool: &str,
        canonical_input_json: &str,
    ) -> PolicyDecision {
        frozen_kernel_tool_policy(&self.snapshot, tool, canonical_input_json)
    }
}

/// Per-run reconciliation state.
enum ContextState {
    Active(ShadowContext),
    /// The context failed (e.g. durable diff write failed). The error is kept
    /// observable; the context is NOT falsely reported as healthy and is not
    /// silently dropped until an explicit close/terminal.
    Failed {
        error: String,
    },
}

impl ContextState {
    fn active_mut(&mut self) -> Option<&mut ShadowContext> {
        match self {
            ContextState::Active(context) => Some(context),
            ContextState::Failed { .. } => None,
        }
    }
    fn is_active(&self) -> bool {
        matches!(self, ContextState::Active(_))
    }
}

pub(crate) struct ShadowReconciler {
    database: Database,
    contexts: Mutex<HashMap<String, ContextState>>,
    operation: Mutex<()>,
}

/// Outcome of a reconciliation step that may produce a durable diff.
#[must_use = "callers must check whether the diff was actually persisted"]
pub(crate) struct ReconcileOutcome {
    pub diff: Option<ShadowDiff>,
    /// None on success, Some(error) if the durable write failed (the diff is
    /// not persisted and the context is in a Failed state).
    pub persist_error: Option<String>,
}

impl ReconcileOutcome {
    pub(crate) fn persisted(&self) -> bool {
        self.persist_error.is_none()
    }
}

impl ShadowReconciler {
    pub(crate) fn new(database: Database) -> Self {
        Self {
            database,
            contexts: Mutex::new(HashMap::new()),
            operation: Mutex::new(()),
        }
    }

    pub(crate) fn database(&self) -> &Database {
        &self.database
    }

    /// Persist a diff; on failure, mark the run context Failed and return the
    /// error — never silently ignore and never claim the diff was recorded.
    fn record_diff(&self, legacy_run_id: &str, diff: ShadowDiff) -> Result<(), String> {
        let checkpoint = self.with_active_context(legacy_run_id, |context| {
            context.checkpoint(ReconcilerClock.now_monotonic_ms())
        })?;
        let state = if diff.event_type.as_deref() == Some("terminal") {
            "closed"
        } else {
            "active"
        };
        match self
            .database
            .kernel_commit_shadow_checkpoint(&diff, &checkpoint, state)
        {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = format!("kernel shadow diff not persisted: {error}");
                let _ = self.database.kernel_save_shadow_checkpoint(
                    &format!("shadow-{legacy_run_id}"),
                    &error,
                    "failed",
                );
                if let Ok(mut contexts) = self.contexts.lock() {
                    contexts.insert(
                        legacy_run_id.to_string(),
                        ContextState::Failed {
                            error: error.clone(),
                        },
                    );
                }
                Err(error)
            }
        }
    }

    /// Bootstrap a shadow context after the run's request snapshot is frozen.
    /// Idempotent: a repeat bootstrap for the same run does NOT overwrite the
    /// existing context or its tool/approval state; it returns the existing
    /// context rather than rebuilding it (which would reset accumulated facts).
    pub(crate) fn bootstrap(
        &self,
        legacy_run_id: &str,
        conversation_id: &str,
        execution_profile_id: &str,
        frozen_config: RunFrozenConfig,
    ) -> Result<(), String> {
        self.bootstrap_with_policy(
            legacy_run_id,
            conversation_id,
            execution_profile_id,
            frozen_config,
            None,
        )
    }

    pub(crate) fn bootstrap_with_policy(
        &self,
        legacy_run_id: &str,
        conversation_id: &str,
        execution_profile_id: &str,
        frozen_config: RunFrozenConfig,
        policy_snapshot: Option<Value>,
    ) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        // Always validate conversation identity, even for a live repeated bootstrap.
        let created = self.database.kernel_create_shadow_run(
            &format!("shadow-{legacy_run_id}"),
            legacy_run_id,
            conversation_id,
            &format!("turn-{legacy_run_id}"),
            &frozen_config,
        )?;
        // The execution profile must agree with the frozen config; a caller
        // passing a mismatched profile is a programming error (fail-closed).
        if execution_profile_id != frozen_config.execution_profile_id {
            return Err(format!(
                "bootstrap profile argument {execution_profile_id} does not match frozen config {}",
                frozen_config.execution_profile_id
            ));
        }
        if let Ok(contexts) = self.contexts.lock() {
            match contexts.get(legacy_run_id) {
                // Active context: keep accumulated state, but verify the full
                // frozen identity/config matches (every field). Same config is
                // idempotent; ANY conflict fails closed (no reset).
                Some(ContextState::Active(context)) => {
                    if policy_snapshot
                        .as_ref()
                        .is_some_and(|snapshot| context.policy_snapshot() != snapshot)
                    {
                        return Err("conflicting frozen permission snapshot".into());
                    }
                    return context.verify_frozen_config(&frozen_config);
                }
                // A previously-failed context can NOT be revived by a repeat
                // bootstrap: return the failed diagnostic rather than Ok with
                // an unusable state.
                Some(ContextState::Failed { error }) => {
                    return Err(format!(
                        "shadow context for run {legacy_run_id} is in a failed state and cannot be re-bootstrapped: {error}"
                    ));
                }
                None => {}
            }
        }
        let shadow_run_id = format!("shadow-{legacy_run_id}");
        let turn_id = format!("turn-{legacy_run_id}");
        // kernel_create_shadow_run binds the run↔conversation identity, is
        // idempotent on identical frozen config, and fails closed on a config
        // mismatch.
        self.database.kernel_create_shadow_run(
            &shadow_run_id,
            legacy_run_id,
            conversation_id,
            &turn_id,
            &frozen_config,
        )?;
        let start_cursor = self.database.kernel_shadow_max_cursor(&shadow_run_id)?;
        let saved = self
            .database
            .kernel_load_shadow_checkpoint(&shadow_run_id)?;
        let mut context = if let Some((state, checkpoint)) = saved {
            if state != "active" {
                return Err(format!("shadow context is durably {state}"));
            }
            let restored = ShadowContext::restore(&checkpoint)?;
            restored.verify_frozen_config(&frozen_config)?;
            if restored.legacy_run_id() != legacy_run_id
                || restored.shadow_run_id() != shadow_run_id
            {
                return Err("shadow checkpoint identity mismatch".into());
            }
            restored
        } else {
            if !created || start_cursor > 0 {
                return Err("historical shadow has no semantic checkpoint; not comparable".into());
            }
            ShadowContext::start(
                &shadow_run_id,
                legacy_run_id,
                &turn_id,
                frozen_config,
                &ReconcilerClock,
                start_cursor,
            )
            .map_err(|error| format!("shadow controller failed to start: {error}"))?
        };
        if context.policy_snapshot().is_null() {
            context.freeze_policy(match policy_snapshot {
                Some(snapshot) => snapshot,
                None => self
                    .database
                    .kernel_shadow_permission_snapshot(conversation_id)?,
            });
        } else if policy_snapshot
            .as_ref()
            .is_some_and(|snapshot| context.policy_snapshot() != snapshot)
        {
            return Err("recovered permission snapshot conflicts with bootstrap".into());
        }
        self.database.kernel_save_shadow_checkpoint(
            &shadow_run_id,
            &context.checkpoint(ReconcilerClock.now_monotonic_ms())?,
            "active",
        )?;
        if let Ok(mut contexts) = self.contexts.lock() {
            contexts
                .entry(legacy_run_id.to_string())
                .or_insert_with(|| ContextState::Active(context));
        }
        let executable = self
            .database
            .kernel_executable_outbox_count(&shadow_run_id)?;
        if executable != 0 {
            return Err(format!(
                "shadow run {shadow_run_id} unexpectedly has {executable} executable outbox rows"
            ));
        }
        Ok(())
    }

    pub(crate) fn has_context(&self, legacy_run_id: &str) -> bool {
        self.contexts
            .lock()
            .map(|c| c.get(legacy_run_id).is_some_and(|s| s.is_active()))
            .unwrap_or(false)
    }

    fn with_active_context<T>(
        &self,
        legacy_run_id: &str,
        f: impl FnOnce(&mut ShadowContext) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self
            .contexts
            .lock()
            .map_err(|_| "shadow context lock poisoned".to_string())?;
        match guard.get_mut(legacy_run_id) {
            Some(ContextState::Active(context)) => f(context),
            Some(ContextState::Failed { error }) => Err(format!(
                "shadow context for run {legacy_run_id} is in a failed state: {error}"
            )),
            None => Err(format!("no shadow context for run {legacy_run_id}")),
        }
    }

    /// Move a run's context into the queryable FAILED state with a diagnostic,
    /// after any non-persistent reconciliation error (registration/settlement).
    /// This guarantees a failed sweep/settle can never continue to produce a
    /// healthy terminal: every subsequent operation reports the failed state.
    fn mark_context_failed(&self, legacy_run_id: &str, error: &str) {
        let _ = self.database.kernel_save_shadow_checkpoint(
            &format!("shadow-{legacy_run_id}"),
            error,
            "failed",
        );
        if let Ok(mut guard) = self.contexts.lock() {
            if guard.contains_key(legacy_run_id) {
                guard.insert(
                    legacy_run_id.to_string(),
                    ContextState::Failed {
                        error: error.to_string(),
                    },
                );
            }
        }
    }

    /// Run an operation against the active context; on error, mark the context
    /// FAILED (fail-closed) and return the error. Used by sweep/settle paths
    /// whose failures must leave a queryable failed state rather than an
    /// Active context.
    fn with_active_context_failing_closed<T>(
        &self,
        legacy_run_id: &str,
        f: impl FnOnce(&mut ShadowContext) -> Result<T, String>,
    ) -> Result<T, String> {
        let result = self.with_active_context(legacy_run_id, f);
        if let Err(error) = &result {
            self.mark_context_failed(legacy_run_id, error);
        }
        result
    }

    /// Feed a real tool.preflight decision. The toolCallId is the real Pi id
    /// (preflight/ToolCall/completed share it); `canonical_input` is the
    /// approved normalised payload (allowed) or the raw request (blocked);
    /// `legacy_decision` is the real Host decision ("allow"/"approval"/"deny").
    /// The kernel decides independently via the shadow policy; the durable
    /// diff write failure is propagated.
    pub(crate) fn feed_preflight(
        &self,
        legacy_run_id: &str,
        tool: &str,
        tool_call_id: &str,
        canonical_input: Value,
        legacy_decision: &str,
    ) -> Result<ReconcileOutcome, String> {
        let input_json = canonical_input.to_string();
        let batch_id = format!("preflight-batch-{tool_call_id}");
        let legacy_tool = ToolDisposition {
            tool_call_id: tool_call_id.to_string(),
            batch_id: batch_id.clone(),
            tool: tool.to_string(),
            canonical_input_json: input_json.clone(),
            source_order: 0,
            decision: legacy_decision.to_string(),
        };
        let calls = vec![ShadowToolObservation {
            tool_call_id: tool_call_id.to_string(),
            batch_id: batch_id.clone(),
            tool: tool.to_string(),
            canonical_input_json: input_json,
            source_order: 0,
        }];
        self.feed_batch(legacy_run_id, &batch_id, calls, legacy_tool)
    }

    /// Feed one tool observation with explicit independent legacy facts.
    pub(crate) fn feed_batch(
        &self,
        legacy_run_id: &str,
        batch_id: &str,
        calls: Vec<ShadowToolObservation>,
        legacy_tool: ToolDisposition,
    ) -> Result<ReconcileOutcome, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        self.feed_batch_inner(legacy_run_id, batch_id, calls, legacy_tool)
    }

    fn feed_batch_inner(
        &self,
        legacy_run_id: &str,
        batch_id: &str,
        calls: Vec<ShadowToolObservation>,
        legacy_tool: ToolDisposition,
    ) -> Result<ReconcileOutcome, String> {
        // Monotonic timing (Instant-based, immune to wall-clock jumps) for the
        // elapsed/execution domain; wall time only for durable deadlines.
        let mono = ReconcilerClock.now_monotonic_ms();
        let wall = crate::database::now_ms();
        let diff = self.with_active_context_failing_closed(legacy_run_id, |context| {
            let policy = ReconcilerPolicy {
                snapshot: context.policy_snapshot().clone(),
            };
            if context.same_legacy_observation(&legacy_tool)? {
                return Ok(None);
            }
            let (batch, calls, legacy_tools) = if context
                .pending_batch_for(&legacy_tool.tool_call_id)
                .is_some()
            {
                match context.stage_legacy_tool(legacy_tool)? {
                    Some(batch) => batch,
                    None => {
                        self.database.kernel_save_shadow_checkpoint(
                            &format!("shadow-{legacy_run_id}"),
                            &context.checkpoint(mono)?,
                            "active",
                        )?;
                        return Ok(None);
                    }
                }
            } else {
                (batch_id.to_string(), calls, vec![legacy_tool])
            };
            let diff = context
                .on_tool_batch(&batch, calls, &policy, legacy_tools, mono, wall)
                .map_err(|error| format!("shadow tool batch failed: {error}"))?;
            context
                .settle_deferred_results(mono)
                .map_err(|error| error.to_string())?;
            Ok(Some(diff))
        })?;
        match diff {
            Some(diff) => self.commit_diff(legacy_run_id, diff),
            None => Ok(ReconcileOutcome {
                diff: None,
                persist_error: None,
            }),
        }
    }

    /// Commit a diff to durable storage, returning an outcome that records any
    /// persistence failure (the context is already marked Failed inside).
    fn commit_diff(
        &self,
        legacy_run_id: &str,
        diff: ShadowDiff,
    ) -> Result<ReconcileOutcome, String> {
        match self.record_diff(legacy_run_id, diff.clone()) {
            Ok(()) => Ok(ReconcileOutcome {
                diff: Some(diff),
                persist_error: None,
            }),
            Err(error) => Ok(ReconcileOutcome {
                diff: Some(diff),
                persist_error: Some(error),
            }),
        }
    }

    /// Feed a nested Graph read (`observationScope=nested`). Returns true if
    /// the Host must SKIP an independent shadow registration (the parent is a
    /// genuine runtime `graph_readonly_run` ToolCall projected in the DB);
    /// returns false for a fabricated/missing parent, in which case the caller
    /// proceeds with a normal observation (the inner call IS observed).
    pub(crate) fn feed_nested_preflight(
        &self,
        legacy_run_id: &str,
        parent_tool_call_id: &str,
    ) -> Result<bool, String> {
        let valid_graph_parent = self
            .database
            .get_runtime_tool_call(legacy_run_id, parent_tool_call_id)
            .map_err(|error| format!("graph parent lookup failed: {error}"))?
            .is_some_and(|record| {
                record.tool_name == "graph_readonly_run" && record.execution_location == "runtime"
            });
        Ok(valid_graph_parent)
    }

    /// Whether any persisted diff for the run references this toolCallId.
    pub(crate) fn has_tool_diff(&self, legacy_run_id: &str, tool_call_id: &str) -> bool {
        self.database
            .kernel_shadow_diffs_for(&format!("shadow-{legacy_run_id}"))
            .map(|diffs| {
                diffs.into_iter().any(|diff| {
                    diff.legacy
                        .tools
                        .iter()
                        .chain(diff.kernel.tools.iter())
                        .any(|tool| tool.tool_call_id == tool_call_id)
                })
            })
            .unwrap_or(false)
    }

    /// Most recent persisted diff that references a given toolCallId
    /// (reads back durable tool-batch facts for assertions).
    pub(crate) fn last_tool_diff(
        &self,
        legacy_run_id: &str,
        tool_call_id: &str,
    ) -> Option<ShadowDiff> {
        self.database
            .kernel_shadow_diffs_for(&format!("shadow-{legacy_run_id}"))
            .ok()?
            .into_iter()
            .filter(|diff| {
                diff.legacy
                    .tools
                    .iter()
                    .chain(diff.kernel.tools.iter())
                    .any(|tool| tool.tool_call_id == tool_call_id)
            })
            .last()
    }

    /// Most recent persisted terminal diff for a run.
    pub(crate) fn last_terminal_diff(&self, legacy_run_id: &str) -> Option<ShadowDiff> {
        self.database
            .kernel_shadow_diffs_for(&format!("shadow-{legacy_run_id}"))
            .ok()
            .and_then(|diffs| {
                diffs
                    .into_iter()
                    .filter(|diff| diff.event_type.as_deref() == Some("terminal"))
                    .last()
            })
    }

    /// Feed a tool RESULT. `ok` is the mapped result (isError=true => false).
    pub(crate) fn settle_tool(
        &self,
        legacy_run_id: &str,
        tool_call_id: &str,
        ok: bool,
        result_json: &str,
    ) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        self.settle_tool_inner(legacy_run_id, tool_call_id, ok, result_json)
    }

    fn settle_tool_inner(
        &self,
        legacy_run_id: &str,
        tool_call_id: &str,
        ok: bool,
        result_json: &str,
    ) -> Result<(), String> {
        let mono = ReconcilerClock.now_monotonic_ms();
        // A settlement error marks the context Failed (fail-closed): the run
        // cannot reach a healthy terminal after a failed settlement.
        self.with_active_context_failing_closed(legacy_run_id, |context| {
            if !context.defer_result(tool_call_id, ok, result_json)? {
                context
                    .on_tool_settled(tool_call_id, ok, result_json, mono)
                    .map_err(|error| format!("shadow tool settlement failed: {error}"))?;
            }
            self.database.kernel_save_shadow_checkpoint(
                &format!("shadow-{legacy_run_id}"),
                &context.checkpoint(mono)?,
                "active",
            )
        })
    }

    /// Feed a cancellation before the terminal comparison.
    pub(crate) fn cancel(&self, legacy_run_id: &str) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        self.cancel_inner(legacy_run_id)
    }

    fn cancel_inner(&self, legacy_run_id: &str) -> Result<(), String> {
        self.with_active_context_failing_closed(legacy_run_id, |context| {
            context.on_cancel();
            self.database.kernel_save_shadow_checkpoint(
                &format!("shadow-{legacy_run_id}"),
                &context.checkpoint(ReconcilerClock.now_monotonic_ms())?,
                "active",
            )
        })
    }

    /// Feed a terminal event with INDEPENDENT legacy facts: `legacy_state` is
    /// the legacy run STATE ("completed"/"failed"/"cancelled") as the Host
    /// observed it — it is NOT derived from the kernel outcome. The kernel
    /// performs its own terminal transition via `kernel_outcome`; the two are
    /// then compared. The projection sweep accounts for runtime/host ToolCalls
    /// the shadow never observed. Persists the terminal diff, then drops the
    /// context (matching the Host's close).
    pub(crate) fn terminal(
        &self,
        legacy_run_id: &str,
        legacy_state: &str,
        kernel_outcome: RunOutcome,
        scan_db_projection: bool,
    ) -> Result<ReconcileOutcome, String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        if scan_db_projection {
            self.sweep_db_projection(legacy_run_id)?;
        }
        // Independently cancel the kernel side for a cancelled legacy run.
        if legacy_state == "cancelled" {
            self.cancel_inner(legacy_run_id)?;
        }
        let diff = self.with_active_context(legacy_run_id, |context| {
            Ok(context.on_terminal(legacy_state, kernel_outcome, RetryTimeoutFacts::default()))
        })?;
        let outcome = self.commit_diff(legacy_run_id, diff)?;
        // On success the terminal point closes the shadow context. On a
        // durable write failure the context is left in a queryable FAILED
        // state (record_diff marked it Failed and `persist_error` carries the
        // diagnostic); the caller is responsible for persisting/surfacing it
        // — we do NOT drop the failed context and falsely report closure.
        if outcome.persisted() {
            if let Ok(mut guard) = self.contexts.lock() {
                guard.remove(legacy_run_id);
            }
        }
        Ok(outcome)
    }

    /// Queryable state of a run's shadow context after operations:
    /// "active" | "failed" | "absent".
    pub(crate) fn context_state(&self, legacy_run_id: &str) -> &'static str {
        let Ok(guard) = self.contexts.lock() else {
            return "absent";
        };
        match guard.get(legacy_run_id) {
            Some(ContextState::Active(_)) => "active",
            Some(ContextState::Failed { .. }) => "failed",
            None => "absent",
        }
    }

    /// Account for runtime/host ToolCalls the shadow never observed via
    /// preflight. Execution-ownership facts follow the Host's ToolCall
    /// projection:
    /// - a RUNTIME-location ToolCall that was claimed by a runtime tool is an
    ///   allow/approval fact;
    /// - a HOST-location ToolCall that never acquired execution authority
    ///   (no runtime claim) retains a DENY fact (it was not executed by the
    ///   Host under Runtime authority);
    /// - terminal projected tools are settled so a zero-tool run never fakes a
    ///   Match and an unresolved mid-flight tool is not dropped.
    ///
    /// A projection that cannot be replayed into the kernel is recorded as a
    /// `shadow_error` diff and its persistence failure propagates.
    pub(crate) fn observe_projected_tools(&self, legacy_run_id: &str) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        self.sweep_db_projection(legacy_run_id)
    }

    fn sweep_db_projection(&self, legacy_run_id: &str) -> Result<(), String> {
        let result = (|| {
            let records = self
                .database
                .list_runtime_tool_calls_for_run(legacy_run_id)
                .map_err(|error| format!("sweep read failed: {error}"))?;
            for record in records {
                let id = if record.runtime_tool_call_id.is_empty() {
                    record.id.clone()
                } else {
                    record.runtime_tool_call_id.clone()
                };
                let has_tool =
                    self.with_active_context(legacy_run_id, |context| Ok(context.has_tool(&id)))?;
                if !has_tool {
                    let decision = legacy_tool_decision(
                        &record.tool_name,
                        &record.execution_location,
                        record.requires_approval,
                    )?;
                    let batch = format!("projection-batch-{id}");
                    let input = record.input.to_string();
                    let outcome = self.feed_batch_inner(
                        legacy_run_id,
                        &batch,
                        vec![ShadowToolObservation {
                            tool_call_id: id.clone(),
                            batch_id: batch.clone(),
                            tool: record.tool_name.clone(),
                            canonical_input_json: input.clone(),
                            source_order: 0,
                        }],
                        ToolDisposition {
                            tool_call_id: id.clone(),
                            batch_id: batch.clone(),
                            tool: record.tool_name,
                            canonical_input_json: input,
                            source_order: 0,
                            decision: decision.to_string(),
                        },
                    )?;
                    if let Some(error) = outcome.persist_error {
                        return Err(error);
                    }
                }
                if matches!(record.status.as_str(), "completed" | "failed" | "cancelled") {
                    self.settle_tool_inner(
                        legacy_run_id,
                        &id,
                        record.status == "completed",
                        &record.result.unwrap_or_else(|| json!({})).to_string(),
                    )?;
                }
            }
            Ok(())
        })();
        if let Err(error) = &result {
            self.mark_context_failed(legacy_run_id, error);
        }
        result
    }

    pub(crate) fn observe_model_request(
        &self,
        legacy_run_id: &str,
        begin: bool,
    ) -> Result<(), String> {
        let _operation = self
            .operation
            .lock()
            .map_err(|_| "shadow operation lock poisoned")?;
        self.with_active_context_failing_closed(legacy_run_id, |context| {
            let mono = ReconcilerClock.now_monotonic_ms();
            context.observe_model_request(begin, mono, ReconcilerClock.now_wall_ms());
            self.database.kernel_save_shadow_checkpoint(
                &format!("shadow-{legacy_run_id}"),
                &context.checkpoint(mono)?,
                "active",
            )
        })
    }

    /// Shared production/acceptance adapter, called only after the authoritative
    /// Runtime event was accepted by Database::apply_runtime_event.
    pub(crate) fn observe_applied_event(
        &self,
        run_id: &str,
        payload: &Value,
    ) -> Result<(), String> {
        let result = self.observe_applied_event_inner(run_id, payload);
        if let Err(error) = &result {
            self.mark_context_failed(run_id, error);
        }
        result
    }

    fn observe_applied_event_inner(&self, run_id: &str, payload: &Value) -> Result<(), String> {
        if self.context_state(run_id) == "absent" {
            return Ok(());
        }
        if let Some(batch) = payload.get("shadowToolBatch") {
            let _operation = self
                .operation
                .lock()
                .map_err(|_| "shadow operation lock poisoned")?;
            self.with_active_context_failing_closed(run_id, |context| {
                let batch_id = batch
                    .get("batchId")
                    .and_then(Value::as_str)
                    .ok_or("Pi batch missing id")?;
                let calls = batch
                    .get("calls")
                    .and_then(Value::as_array)
                    .ok_or("Pi batch missing calls")?
                    .iter()
                    .map(|call| {
                        Ok(ShadowToolObservation {
                            tool_call_id: call
                                .get("toolCallId")
                                .and_then(Value::as_str)
                                .ok_or("Pi tool missing id")?
                                .to_string(),
                            batch_id: batch_id.to_string(),
                            tool: call
                                .get("tool")
                                .and_then(Value::as_str)
                                .ok_or("Pi tool missing name")?
                                .to_string(),
                            canonical_input_json: call
                                .get("input")
                                .ok_or("Pi tool missing input")?
                                .to_string(),
                            source_order: call
                                .get("sourceOrder")
                                .and_then(Value::as_u64)
                                .ok_or("Pi tool missing source order")?
                                as usize,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                context.reserve_batch(batch_id, calls)?;
                self.database.kernel_save_shadow_checkpoint(
                    &format!("shadow-{run_id}"),
                    &context.checkpoint(ReconcilerClock.now_monotonic_ms())?,
                    "active",
                )
            })?;
        }
        if let Some(action) = payload.get("shadowModelRequest").and_then(Value::as_str) {
            if !matches!(action, "begin" | "settle") {
                return Err("unknown model observation".into());
            }
            self.observe_model_request(run_id, action == "begin")?;
        }
        match payload.get("type").and_then(Value::as_str) {
            Some("tool.completed" | "tool.failed") => {
                let id = payload
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .ok_or("tool result omitted toolCallId")?;
                self.settle_applied_tool(run_id, id, super::runtime_tool_event_succeeded(payload))?;
            }
            Some(
                event @ ("run.completed" | "run.cancelled" | "run.failed" | "run.interrupted"),
            ) => {
                let (state, outcome) = match event {
                    "run.completed" => ("completed", RunOutcome::Completed),
                    "run.cancelled" => ("cancelled", RunOutcome::Cancelled),
                    _ => (
                        "failed",
                        RunOutcome::Failed {
                            code: payload
                                .get("code")
                                .and_then(Value::as_str)
                                .unwrap_or(event)
                                .to_string(),
                            message: payload
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("run failed")
                                .to_string(),
                        },
                    ),
                };
                let outcome = self.terminal(run_id, state, outcome, true)?;
                if let Some(error) = outcome.persist_error {
                    return Err(error);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Startup recovery observes Legacy's repaired projection; it never starts
    pub(crate) fn settle_applied_tool(
        &self,
        run_id: &str,
        id: &str,
        ok: bool,
    ) -> Result<(), String> {
        let record = self
            .database
            .get_runtime_tool_call(run_id, id)?
            .ok_or("applied result has no ToolCall projection")?;
        if (record.status == "completed") != ok {
            return Err("applied tool outcome conflicts with its authoritative projection".into());
        }
        // The projection may contain a deliberately summarized large result.
        // Consume that canonical persisted result once, not once from the
        // projection and then again from the unbounded original payload.
        self.observe_projected_tools(run_id)
    }

    /// Startup recovery observes Legacy's repaired projection; it never starts
    /// a Runtime, resends an approval, or dispatches a tool.
    pub(crate) fn recover_all(&self) -> Result<usize, String> {
        let mut recovered = 0;
        let mut errors = Vec::new();
        for (run_id, conversation, config, legacy_status) in
            self.database.kernel_shadow_recovery_rows()?
        {
            let result = (|| {
                self.bootstrap(
                    &run_id,
                    &conversation,
                    &config.execution_profile_id.clone(),
                    config,
                )?;
                self.observe_projected_tools(&run_id)?;
                let event = match legacy_status.as_str() {
                    "completed" => Some("run.completed"),
                    "cancelled" => Some("run.cancelled"),
                    "failed" | "interrupted" => Some("run.interrupted"),
                    _ => None,
                };
                if let Some(event) = event {
                    self.observe_applied_event(&run_id, &json!({"type":event, "code":"runtime.recovered_terminal", "message":"Legacy terminal recovered after restart"}))?;
                }
                Ok::<_, String>(())
            })();
            match result {
                Ok(()) => recovered += 1,
                Err(error) => {
                    self.mark_context_failed(&run_id, &error);
                    errors.push(format!("{run_id}: {error}"));
                }
            }
        }
        if errors.is_empty() {
            Ok(recovered)
        } else {
            Err(errors.join("; "))
        }
    }

    /// Count persisted diffs by category for a shadow run.
    pub(crate) fn diff_counts(
        &self,
        legacy_run_id: &str,
    ) -> Result<std::collections::BTreeMap<String, i64>, String> {
        self.database
            .kernel_shadow_diff_counts(&format!("shadow-{legacy_run_id}"))
    }

    /// Build a frozen shadow config (test helper).
    pub(crate) fn shadow_config(profile: &str) -> RunFrozenConfig {
        RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "shadow".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "sha256:manifest".into(),
            permission_snapshot_id: "sha256:perm".into(),
            execution_profile_id: profile.into(),
            prompt_config_hash: "sha256:prompt".into(),
            model_request_timeout_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 1_800_000,
            approval_wait_timeout_ms: 3_600_000,
            provider_max_retries: 2,
            turn_max_retries: 0,
        }
    }
}
