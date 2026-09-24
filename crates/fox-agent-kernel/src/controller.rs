//! Fox Agent Kernel — `RunController`, the pure decision core.
//!
//! The controller owns the run state machine, tool-batch barrier, approval,
//! retry/compaction transitions, execution/approval/tool budgets and
//! cancellation. It is synchronous and free of IO: each transition returns an
//! ordered list of [`Effect`]s that the host adapter persists and performs. The
//! durable facts (events, run/tool state, approvals, outbox effects) are
//! committed in a single SQLite transaction BEFORE any external executor runs,
//! so a crash does not lose a decision; uncertain leased effects are reconciled
//! by idempotency key instead of being blindly executed again.
//!
//! Time is split into two domains (see [`crate::ports::Clock`]):
//! in-process execution time uses a monotonic clock that is not comparable
//! across restarts (the accumulated execution time is persisted instead), and
//! the human-approval deadline uses wall time so an approval wait survives
//! restart; a backwards wall jump is detected and fails closed.

use std::collections::{BTreeMap, BTreeSet};

use crate::ports::{
    Clock, CompactionState, EventStorePort, PolicyDecisionPort, RetryState, RunFrozenConfig,
    ToolCallRequest,
};
use crate::state::{ApprovalDecision, KernelError, RunOutcome, RunState, ToolCallState};

/// Ordered effects the host adapter must persist in the decision transaction.
/// External variants are performed only after commit; bookkeeping variants are
/// consumed inside that same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// First model call, committed before any engine starts. Payload is a hash
    /// of Host-frozen input, never a reconstructed conversation.
    RequestInitialModel { input_hash: String },
    /// A bounded Host continuation with immutable model input and its own lease.
    RequestContinuationModel { effect_key: String, payload_json: String },
    /// Append a durable event (`(run_id, seq)` idempotent).
    AppendEvent {
        seq: u64,
        event_type: String,
        payload_json: String,
    },
    /// Ask a human to approve a tool call; the run is parked in waiting_approval.
    /// Durable outbox key: `approval:{run}:{tool}`.
    RequestApproval {
        tool_call_id: String,
        tool: String,
        input_json: String,
    },
    /// Dispatch an allowed tool call to the Resource Gateway, which re-validates
    /// the final parameters before any side effect. Durable outbox key:
    /// `dispatch:{run}:{tool}` — stable across retry and restart.
    DispatchTool {
        tool_call_id: String,
        tool: String,
        input_json: String,
    },
    /// Cancel the engine turn (model stream / waiting hooks / host request).
    CancelEngineTurn { turn_id: String },
    /// Notify a running tool that it is cancelled.
    CancelToolCall { tool_call_id: String },
    /// Durable bookkeeping: resolve the approval fact in the same transaction as
    /// the resulting tool state and dispatch intent. This is not external IO.
    ApprovalResolved {
        tool_call_id: String,
        decision: ApprovalDecision,
    },
    /// Durable bookkeeping for a request that timed out before a human decision.
    ApprovalExpired { tool_call_id: String },
    /// Durable bookkeeping for a request abandoned by run cancellation.
    ApprovalCancelled { tool_call_id: String },
    /// Durable bookkeeping: settle the leased dispatch intent in the same
    /// transaction as the tool result. This closes the result/outbox crash window.
    ToolDispatchSettled { tool_call_id: String },
    /// All calls in a batch reached a terminal state. The result array handed to
    /// the model MUST be ordered by `ordered_tool_call_ids` (source order),
    /// regardless of completion order.
    BatchBarrier {
        batch_id: String,
        ordered_tool_call_ids: Vec<String>,
    },
    /// Publish a rebuilt run snapshot for React to read.
    PublishSnapshot,
    /// A late event arrived after the run was terminal; record for audit only.
    /// It must never resurrect or reclassify the run.
    Audit { message: String },
}

/// Stable outbox effect key (unique within a run).
pub fn dispatch_effect_key(tool_call_id: &str) -> String {
    format!("dispatch:{tool_call_id}")
}
/// Stable external idempotency key for a tool dispatch (unique within a run).
pub fn dispatch_idempotency_key(run_id: &str, tool_call_id: &str) -> String {
    // Both components are byte-bounded at start/proposal/rehydration. No
    // delimiter restriction: the byte length makes colons unambiguous.
    format!("tool-dispatch:{}:{run_id}:{tool_call_id}",run_id.len())
}
/// Stable outbox effect key for an approval prompt.
pub fn approval_effect_key(tool_call_id: &str) -> String {
    format!("approval:{tool_call_id}")
}

/// Stable outbox key for delivering one fully settled batch to the next model turn.
pub fn batch_delivery_effect_key(batch_id: &str) -> String {
    format!("deliver-batch:{batch_id}")
}

/// Stable idempotency key for one logical batch delivery.
pub fn batch_delivery_idempotency_key(batch_id: &str) -> String {
    format!("tool-batch-delivery:{batch_id}")
}

pub const INITIAL_MODEL_EFFECT_KEY: &str = "initial-model";
pub const INITIAL_MODEL_IDEMPOTENCY_KEY: &str = "initial-model-delivery";

/// Prefix of the error a terminal decision returns when it loses a race with a
/// mid-run request that was accepted moments earlier.
///
/// It describes a scheduling condition, not an execution failure: the
/// transport must re-plan a safe additional-input round (or detach and re-plan
/// from durable facts). The suffix carries how many rows were still open, for
/// logs. Emitting it as a plain error string would let the Host classify a
/// normal "the user added a request just now" as an engine failure.
pub const STEERING_COMPETITION: &str = "kernel.steering_competition:";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolCall {
    tool_call_id: String,
    tool: String,
    input_json: String,
    source_order: usize,
    batch_id: String,
    state: ToolCallState,
    result_json: Option<String>,
    /// Monotonic time the tool started running in THIS process. Not comparable
    /// across restarts; crash recovery of a running tool uses the durable outbox
    /// lease, not this value.
    started_at_mono_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct ToolBatch {
    batch_id: String,
    /// Source order of the calls in this batch.
    ordered: Vec<String>,
    barrier_emitted: bool,
}

/// Durable rehydration input for one tool call, loaded by the repository.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RehydratedToolCall {
    pub tool_call_id: String,
    pub tool: String,
    pub input_json: String,
    pub source_order: usize,
    pub batch_id: String,
    pub state: ToolCallState,
    pub result_json: Option<String>,
}

/// Durable rehydration input for one tool batch.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RehydratedBatch {
    pub batch_id: String,
    pub ordered: Vec<String>,
    pub barrier_emitted: bool,
}

/// Everything needed to reconstruct a `RunController` from durable facts. The
/// repository fills this from a consistent read; the controller never reads IO.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RehydratedRun {
    pub run_id: String,
    pub turn_id: String,
    pub config: RunFrozenConfig,
    pub state: RunState,
    pub seq: u64,
    pub running_elapsed_ms: i64,
    pub approval_deadline_wall_ms: Option<i64>,
    pub wait_deadline_wall_ms: Option<i64>,
    pub wait_accounted_until_wall_ms: Option<i64>,
    pub terminal_written: bool,
    pub batches: Vec<RehydratedBatch>,
    pub tools: Vec<RehydratedToolCall>,
    pub retry: RetryState,
    pub compaction: CompactionState,
    /// Persisted wall anchor of a model request that was in flight at the
    /// crash boundary; preserves the timeout window across restarts.
    pub model_request_since_wall_ms: Option<i64>,
}

/// Frozen identity of an unfinished child Job at the park boundary. The
/// repository verifies this exact set against its current scoped rows in the
/// same transaction that writes `run.waiting_jobs`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WaitingJobFact {
    pub job_id: String,
    pub attempt: i64,
    pub deadline_wall_ms: i64,
}

/// Per-run aggregate. Constructed via [`RunController::start`] for a new run or
/// [`RunController::rehydrate`] after a restart.
#[derive(Clone)]
pub struct RunController {
    run_id: String,
    turn_id: String,
    config: RunFrozenConfig,
    state: RunState,
    seq: u64,
    /// Accumulated running time (monotonic domain), excluding approval waits.
    /// Persisted across restarts.
    running_elapsed_ms: i64,
    /// Monotonic time the run entered Running in THIS process (None across
    /// restarts until the first tick re-anchors the fresh monotonic domain).
    running_since_mono_ms: Option<i64>,
    /// Persisted wall-clock approval deadline; survives restart.
    approval_deadline_wall_ms: Option<i64>,
    wait_deadline_wall_ms: Option<i64>,
    wait_accounted_until_wall_ms: Option<i64>,
    retry: RetryState,
    compaction: CompactionState,
    /// Whether a model request is EXPLICITLY in flight. Model-request timeout
    /// is only armed between an explicit `begin_model_request` (turn dispatch)
    /// and `settle_model_request` (first model output / tool batch / terminal);
    /// it is NOT inferred from "no tool in flight", which would misfire after
    /// tool settlement while a durable batch delivery is still pending.
    model_request_in_flight: bool,
    /// Monotonic anchor for the current model request (this process only).
    model_request_since_mono_ms: Option<i64>,
    /// Persisted wall-clock anchor for a model request that was in flight at a
    /// crash/restart boundary; survives restart so the timeout is NOT reset to a
    /// full fresh window on each crash. None when no request is in flight.
    model_request_since_wall_ms: Option<i64>,
    /// Monotonic instant of the first observed output (preview or settled
    /// response) for the current request. None until progress arrives; drives
    /// the first-response bound. Volatile: never persisted, re-anchored by the
    /// next observed progress after rehydrate.
    model_request_first_output_mono_ms: Option<i64>,
    /// Monotonic instant of the last observed text/thinking/tool-parameter
    /// progress for the current request. Volatile like the first-output
    /// anchor; after rehydrate, idleness is measured conservatively from the
    /// persisted wall anchor until fresh progress arrives.
    model_request_last_progress_mono_ms: Option<i64>,
    /// Wall instant of the last observed progress. Volatile; only used to keep
    /// idleness conservative across a restart when no monotonic anchor exists.
    model_request_last_progress_wall_ms: Option<i64>,
    tools: BTreeMap<String, ToolCall>,
    batches: Vec<ToolBatch>,
    terminal_written: bool,
}

impl RunController {
    /// Observation-only snapshot. No outbox or executable effects are included.
    pub fn shadow_checkpoint(&self, monotonic_ms: i64) -> RehydratedRun {
        RehydratedRun {
            run_id: self.run_id.clone(),
            turn_id: self.turn_id.clone(),
            config: self.config.clone(),
            state: self.state,
            seq: self.seq,
            running_elapsed_ms: self.running_elapsed_ms.saturating_add(
                self.running_since_mono_ms
                    .map(|start| monotonic_ms.saturating_sub(start).max(0))
                    .unwrap_or(0),
            ),
            approval_deadline_wall_ms: self.approval_deadline_wall_ms,
            wait_deadline_wall_ms: self.wait_deadline_wall_ms,
            wait_accounted_until_wall_ms: self.wait_accounted_until_wall_ms,
            terminal_written: self.terminal_written,
            batches: self
                .batches
                .iter()
                .map(|b| RehydratedBatch {
                    batch_id: b.batch_id.clone(),
                    ordered: b.ordered.clone(),
                    barrier_emitted: b.barrier_emitted,
                })
                .collect(),
            tools: self
                .tools
                .values()
                .map(|t| RehydratedToolCall {
                    tool_call_id: t.tool_call_id.clone(),
                    tool: t.tool.clone(),
                    input_json: t.input_json.clone(),
                    source_order: t.source_order,
                    batch_id: t.batch_id.clone(),
                    state: t.state,
                    result_json: t.result_json.clone(),
                })
                .collect(),
            retry: self.retry.clone(),
            compaction: self.compaction.clone(),
            model_request_since_wall_ms: self.model_request_since_wall_ms,
        }
    }

    /// Start a run. Unknown engine/mode or invalid budgets fail closed.
    pub fn start(
        run_id: &str,
        turn_id: &str,
        config: RunFrozenConfig,
        clock: &dyn Clock,
    ) -> Result<(RunController, Vec<Effect>), KernelError> {
        Self::validate_config(&config)?;
        if run_id.trim().is_empty() || run_id.len()>1024 || turn_id.trim().is_empty() {
            return Err(KernelError::FailClosed(
                "run id and turn id must be non-empty".into(),
            ));
        }
        let reading = clock.read();
        let mut controller = RunController {
            run_id: run_id.to_string(),
            turn_id: turn_id.to_string(),
            config,
            state: RunState::Created,
            seq: 0,
            running_elapsed_ms: 0,
            running_since_mono_ms: None,
            approval_deadline_wall_ms: None,
            wait_deadline_wall_ms: None,
            wait_accounted_until_wall_ms: None,
            retry: RetryState::default(),
            compaction: CompactionState::default(),
            model_request_in_flight: false,
            model_request_since_mono_ms: None,
            model_request_since_wall_ms: None,
            model_request_first_output_mono_ms: None,
            model_request_last_progress_mono_ms: None,
            model_request_last_progress_wall_ms: None,
            tools: BTreeMap::new(),
            batches: Vec::new(),
            terminal_written: false,
        };
        controller.retry.provider_max = controller.config.provider_max_retries;
        controller.retry.turn_max = controller.config.turn_max_retries;
        // The run dispatches its first model request immediately.
        controller.arm_model_request(reading.monotonic_ms, reading.wall_ms);
        let mut effects = vec![controller.append_event(
            "run.started",
            serde_json::json!({
                "engineId": controller.config.engine_id,
                "capabilityManifestHash": controller.config.capability_manifest_hash,
            }),
        )];
        controller.state = RunState::Running;
        controller.running_since_mono_ms = Some(reading.monotonic_ms);
        effects.push(Effect::PublishSnapshot);
        Ok((controller, effects))
    }

    pub fn start_with_initial_input(
        run_id: &str,
        turn_id: &str,
        config: RunFrozenConfig,
        input_hash: &str,
        clock: &dyn Clock,
    ) -> Result<(RunController, Vec<Effect>), KernelError> {
        if config.kernel_mode != "authoritative" || input_hash.trim().is_empty() {
            return Err(KernelError::FailClosed(
                "initial model input requires an authoritative hash".into(),
            ));
        }
        let (mut controller, mut effects) = Self::start(run_id, turn_id, config, clock)?;
        // Pending is not in-flight; the deadline is armed atomically with lease.
        controller.settle_model_request();
        effects.push(controller.append_event("engine.initial_requested", serde_json::json!({
            "turnId":turn_id, "inputHash":input_hash, "idempotencyKey":INITIAL_MODEL_IDEMPOTENCY_KEY,
        })));
        effects.push(Effect::RequestInitialModel {
            input_hash: input_hash.into(),
        });
        Ok((controller, effects))
    }

    pub fn begin_initial_model_request(
        &mut self,
        monotonic_ms: i64,
        wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Running
            || self.model_request_in_flight
            || !self.batches.is_empty()
            || !self.tools.is_empty()
        {
            return Err(KernelError::FailClosed(
                "initial model request has already advanced".into(),
            ));
        }
        self.arm_model_request(monotonic_ms, wall_ms);
        Ok(vec![self.append_event("engine.initial_dispatched", serde_json::json!({
            "turnId":self.turn_id, "idempotencyKey":INITIAL_MODEL_IDEMPOTENCY_KEY, "startedAt":wall_ms,
        }))])
    }

    pub fn record_initial_model_response(
        &mut self,
        response_json: &str,
    ) -> Result<Effect, KernelError> {
        if self.state != RunState::Running
            || !self.model_request_in_flight
            || !self.batches.is_empty()
            || response_json.len() > 1_048_576
        {
            return Err(KernelError::FailClosed(
                "initial response has no active model request".into(),
            ));
        }
        let response: serde_json::Value = serde_json::from_str(response_json)
            .map_err(|_| KernelError::FailClosed("invalid initial model response".into()))?;
        self.settle_model_request();
        Ok(self.append_event(
            "engine.initial_response",
            serde_json::json!({"response":response}),
        ))
    }

    /// Reconstruct a controller from durable facts after a restart. Monotonic
    /// execution time is restored from the persisted accumulator (monotonic
    /// readings are NOT compared across restarts); the approval deadline is a
    /// wall time that remains valid. Fails closed on inconsistent input.
    pub fn rehydrate(data: RehydratedRun) -> Result<RunController, KernelError> {
        Self::validate_config(&data.config)?;
        if let Some(pending) = &data.compaction.pending {
            if pending.id.trim().is_empty()
                || pending.id.len() > 512
                || pending
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.trim().is_empty() || owner.len() > 512)
                || !data.state.is_terminal()
                    && data.state != RunState::Cancelling
                    && (data.state != RunState::Compacting
                        || pending.owner.is_some() != data.model_request_since_wall_ms.is_some())
            {
                return Err(KernelError::FailClosed(
                    "invalid rehydrated compaction intent".into(),
                ));
            }
        }
        if data.run_id.trim().is_empty() || data.run_id.len()>1024 || data.turn_id.trim().is_empty() {
            return Err(KernelError::FailClosed(
                "rehydrated run id and turn id must be non-empty".into(),
            ));
        }
        if data.state.is_terminal() != data.terminal_written {
            return Err(KernelError::FailClosed(
                "rehydrated terminal flag disagrees with run state".into(),
            ));
        }
        if data.running_elapsed_ms < 0 {
            return Err(KernelError::FailClosed(
                "rehydrated running elapsed time must not be negative".into(),
            ));
        }
        if (data.state == RunState::WaitingApproval) != data.approval_deadline_wall_ms.is_some() {
            return Err(KernelError::FailClosed(
                "rehydrated approval deadline disagrees with run state".into(),
            ));
        }
        if data
            .approval_deadline_wall_ms
            .is_some_and(|deadline| deadline <= 0)
        {
            return Err(KernelError::FailClosed(
                "rehydrated approval deadline must be positive".into(),
            ));
        }
        if (data.state == RunState::WaitingJobs)
            != data.wait_deadline_wall_ms.is_some()
            || (data.state == RunState::WaitingJobs)
                != data.wait_accounted_until_wall_ms.is_some()
            || matches!((data.wait_deadline_wall_ms, data.wait_accounted_until_wall_ms),
                (Some(deadline), Some(accounted)) if accounted <= 0 || deadline < accounted)
            || data.state == RunState::WaitingJobs
                && (!data.config.experimental_compute_job_notice
                    || data.model_request_since_wall_ms.is_some())
        {
            return Err(KernelError::FailClosed(
                "rehydrated job wait is incomplete or inconsistent".into(),
            ));
        }
        // Terminal exhaustion commits turn_attempts == turn_max + 1 (the failed
        // attempt that spent the budget); reject only greater values. The same
        // boundary is mirrored in the persistence-layer commit guard.
        let terminal_retry_exhausted = data.state.is_terminal()
            && data.retry.turn_attempts == data.retry.turn_max.saturating_add(1);
        if data.retry.provider_max != data.config.provider_max_retries
            || data.retry.turn_max != data.config.turn_max_retries
            || data.retry.provider_attempts > data.retry.provider_max
            || (data.retry.turn_attempts > data.retry.turn_max && !terminal_retry_exhausted)
            || data.retry.completion_attempts > data.retry.turn_max
            || (data.retry.completion_attempts > 0 && data.retry.completion_effect_key.is_none())
        {
            return Err(KernelError::FailClosed(
                "rehydrated retry state disagrees with frozen retry policy".into(),
            ));
        }
        let retry_scheduled = data.state == RunState::RetryScheduled;
        if retry_scheduled
            != (data.retry.scheduled_at_wall_ms.is_some() && data.retry.due_wall_ms.is_some())
            || data.retry.scheduled_at_wall_ms.is_some() != data.retry.due_wall_ms.is_some()
            || matches!(
                (data.retry.scheduled_at_wall_ms, data.retry.due_wall_ms),
                (Some(start), Some(due)) if start <= 0 || due < start
            )
        {
            return Err(KernelError::FailClosed(
                "rehydrated retry schedule is incomplete or inconsistent".into(),
            ));
        }

        let mut batch_ids = BTreeSet::new();
        let mut ordered_members = BTreeSet::new();
        for batch in &data.batches {
            if batch.batch_id.trim().is_empty()
                || batch.ordered.is_empty()
                || !batch_ids.insert(batch.batch_id.clone())
            {
                return Err(KernelError::FailClosed(
                    "rehydrated tool batch identity is empty or duplicated".into(),
                ));
            }
            let mut local = BTreeSet::new();
            for tool_call_id in &batch.ordered {
                if tool_call_id.trim().is_empty()
                    || !local.insert(tool_call_id.clone())
                    || !ordered_members.insert(tool_call_id.clone())
                {
                    return Err(KernelError::FailClosed(format!(
                        "rehydrated batch {} has duplicate/empty tool membership",
                        batch.batch_id
                    )));
                }
            }
        }

        let mut seen_tools = BTreeSet::new();
        for tool in &data.tools {
            if tool.tool_call_id.trim().is_empty() || tool.tool_call_id.len()>1024
                || tool.tool.trim().is_empty()
                || tool.batch_id.trim().is_empty()
                || !seen_tools.insert(tool.tool_call_id.clone())
            {
                return Err(KernelError::FailClosed(
                    "rehydrated tool identity is empty or duplicated".into(),
                ));
            }
            if !matches!(
                serde_json::from_str::<serde_json::Value>(&tool.input_json),
                Ok(serde_json::Value::Object(_))
            ) {
                return Err(KernelError::FailClosed(format!(
                    "rehydrated tool input is not a JSON object: {}",
                    tool.tool_call_id
                )));
            }
            if !tool.state.is_terminal() && tool.result_json.is_some()
                || tool.state == ToolCallState::Completed && tool.result_json.is_none()
            {
                return Err(KernelError::FailClosed(format!(
                    "rehydrated tool result disagrees with state: {}",
                    tool.tool_call_id
                )));
            }
        }
        if seen_tools != ordered_members {
            return Err(KernelError::FailClosed(
                "rehydrated tools and batch membership disagree".into(),
            ));
        }
        for batch in &data.batches {
            let all_settled = batch.ordered.iter().enumerate().all(|(source_order, id)| {
                data.tools.iter().any(|tool| {
                    tool.tool_call_id == *id
                        && tool.batch_id == batch.batch_id
                        && tool.source_order == source_order
                        && tool.state.is_terminal()
                })
            });
            if batch.barrier_emitted && !all_settled
                || all_settled
                    && !batch.barrier_emitted
                    && !data.state.is_terminal()
                    && data.state != RunState::Cancelling
            {
                return Err(KernelError::FailClosed(format!(
                    "rehydrated batch barrier disagrees with settlement: {}",
                    batch.batch_id
                )));
            }
        }
        if data.state.is_terminal() && data.tools.iter().any(|tool| !tool.state.is_terminal()) {
            return Err(KernelError::FailClosed(
                "rehydrated terminal run has unresolved tool calls".into(),
            ));
        }
        if data.state == RunState::WaitingApproval
            && !data
                .tools
                .iter()
                .any(|tool| tool.state == ToolCallState::WaitingApproval)
        {
            return Err(KernelError::FailClosed(
                "rehydrated waiting run has no waiting approval tool".into(),
            ));
        }
        let mut tools = BTreeMap::new();
        for tool in data.tools {
            tools.insert(
                tool.tool_call_id.clone(),
                ToolCall {
                    tool_call_id: tool.tool_call_id,
                    tool: tool.tool,
                    input_json: tool.input_json,
                    source_order: tool.source_order,
                    batch_id: tool.batch_id,
                    state: tool.state,
                    result_json: tool.result_json,
                    // Restarted monotonic domain: in-process tool timeout cannot
                    // be carried over; running tools are reconciled via outbox.
                    started_at_mono_ms: None,
                },
            );
        }
        let batches = data
            .batches
            .into_iter()
            .map(|batch| ToolBatch {
                batch_id: batch.batch_id,
                ordered: batch.ordered,
                barrier_emitted: batch.barrier_emitted,
            })
            .collect();
        Ok(RunController {
            run_id: data.run_id,
            turn_id: data.turn_id,
            config: data.config,
            state: data.state,
            seq: data.seq,
            running_elapsed_ms: data.running_elapsed_ms,
            // Re-anchor on the first tick in the fresh monotonic domain.
            running_since_mono_ms: None,
            approval_deadline_wall_ms: data.approval_deadline_wall_ms,
            wait_deadline_wall_ms: data.wait_deadline_wall_ms,
            wait_accounted_until_wall_ms: data.wait_accounted_until_wall_ms,
            retry: data.retry,
            compaction: data.compaction,
            // After restart the model-request flag is re-anchored from the
            // PERSISTED wall anchor ONLY for a non-terminal run: a request in
            // flight at crash remains in flight (continuous crashes cannot grant
            // a fresh full window), but a terminal run must never hold an anchor.
            model_request_in_flight: data.model_request_since_wall_ms.is_some()
                && !data.state.is_terminal(),
            model_request_since_mono_ms: None,
            model_request_since_wall_ms: if data.state.is_terminal() {
                None
            } else {
                data.model_request_since_wall_ms
            },
            // Progress anchors are volatile: after restart, idleness is
            // measured conservatively from the persisted wall anchor until the
            // first fresh progress arrives (a crash never grants extra idle).
            model_request_first_output_mono_ms: None,
            model_request_last_progress_mono_ms: None,
            model_request_last_progress_wall_ms: None,
            tools,
            batches,
            terminal_written: data.terminal_written,
        })
    }

    /// Validate the frozen identity and budget contract at persistence boundaries.
    /// Adapters use the same validation as `start` and `rehydrate`.
    pub fn validate_config(config: &RunFrozenConfig) -> Result<(), KernelError> {
        if crate::state::EngineId::parse(&config.engine_id).is_none() {
            return Err(KernelError::FailClosed(format!(
                "unknown engine id: {}",
                config.engine_id
            )));
        }
        if crate::state::KernelMode::parse(&config.kernel_mode).is_none() {
            return Err(KernelError::FailClosed(format!(
                "unknown kernel mode: {}",
                config.kernel_mode
            )));
        }
        if config.permission_snapshot_id.trim().is_empty() {
            return Err(KernelError::FailClosed(
                "missing permission snapshot".into(),
            ));
        }
        if config.capability_manifest_version == 0
            || config.capability_manifest_hash.trim().is_empty()
            || config.execution_profile_id.trim().is_empty()
            || config.prompt_config_hash.trim().is_empty()
        {
            return Err(KernelError::FailClosed(
                "missing frozen capability manifest hash, execution profile, or prompt identity"
                    .into(),
            ));
        }
        if config.provider_max_retries > 5 || config.turn_max_retries > 5 {
            return Err(KernelError::FailClosed(
                "retry budgets exceed the supported bound".into(),
            ));
        }
        if config.model_request_timeout_ms <= 0
            || config.model_first_response_ms <= 0
            || config.model_idle_ms <= 0
            || config.tool_execution_timeout_ms <= 0
            || config.run_execution_budget_ms <= 0
            || config.approval_wait_timeout_ms <= 0
        {
            return Err(KernelError::FailClosed(
                "time budgets must be positive".into(),
            ));
        }
        if config.model_request_timeout_ms > 86_400_000
            || config.model_first_response_ms > 86_400_000
            || config.model_idle_ms > 86_400_000
        {
            return Err(KernelError::FailClosed(
                "model time budgets exceed the supported bound".into(),
            ));
        }
        if config.model_first_response_ms > config.model_request_timeout_ms
            || config.model_idle_ms > config.model_request_timeout_ms
        {
            return Err(KernelError::FailClosed(
                "model first-response/idle budgets must not exceed the whole-round budget".into(),
            ));
        }
        Ok(())
    }

    pub fn state(&self) -> RunState {
        self.state
    }

    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    pub fn turn_id(&self) -> &str {
        &self.turn_id
    }

    pub fn config(&self) -> &RunFrozenConfig {
        &self.config
    }

    /// (provider_attempts, provider_max, turn_attempts, turn_max) retry counters.
    pub fn retry_counters(&self) -> (u32, u32, u32, u32) {
        (
            self.retry.provider_attempts,
            self.retry.provider_max,
            self.retry.turn_attempts,
            self.retry.turn_max,
        )
    }

    pub fn terminal_written(&self) -> bool {
        self.terminal_written
    }

    pub fn last_event_seq(&self) -> u64 {
        self.seq
    }

    pub fn running_elapsed_ms(&self) -> i64 {
        self.running_elapsed_ms
    }

    pub fn approval_deadline_wall_ms(&self) -> Option<i64> {
        self.approval_deadline_wall_ms
    }

    pub fn wait_deadline_wall_ms(&self) -> Option<i64> {
        self.wait_deadline_wall_ms
    }

    pub fn wait_accounted_until_wall_ms(&self) -> Option<i64> {
        self.wait_accounted_until_wall_ms
    }

    /// Park only a settled response. The Host must commit this event with that
    /// response under its model lease; the repository rechecks the response,
    /// scoped Jobs, original deadlines and root before accepting the write.
    pub fn park_waiting_jobs(
        &mut self,
        monotonic_ms: i64,
        wall_ms: i64,
        data_root_id: &str,
        settled_response_seq: u64,
        response_json: &str,
        history_json: &str,
        jobs: &[WaitingJobFact],
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Running
            || !self.config.experimental_compute_job_notice
            || self.config.kernel_mode != "authoritative"
            || self.model_request_in_flight
            || self.retry.model_dispatch_pending
            || self.approval_deadline_wall_ms.is_some()
            || self.tools.values().any(|tool| !tool.state.is_terminal())
            || self.batches.iter().any(|batch| !batch.barrier_emitted)
            || self.seq != settled_response_seq
            || wall_ms <= 0
            || data_root_id.trim().is_empty()
            || jobs.is_empty()
            || response_json.len() > 1_048_576
            || history_json.len() > 1_048_576
            || self.running_since_mono_ms.is_some_and(|since| monotonic_ms < since)
        {
            return Err(KernelError::FailClosed("job wait has no settled response or valid scope".into()));
        }
        let response: serde_json::Value = serde_json::from_str(response_json)
            .map_err(|_| KernelError::FailClosed("invalid parked model response".into()))?;
        let history: serde_json::Value = serde_json::from_str(history_json)
            .map_err(|_| KernelError::FailClosed("invalid parked model history".into()))?;
        if !response.is_object() || !history.is_array() {
            return Err(KernelError::FailClosed("parked response/history shape is invalid".into()));
        }
        let mut ordered = jobs.to_vec();
        ordered.sort_by(|a, b| a.job_id.cmp(&b.job_id));
        if ordered.iter().any(|job| job.job_id.trim().is_empty()
            || job.attempt < 0 || job.deadline_wall_ms <= wall_ms)
            || ordered.windows(2).any(|pair| pair[0].job_id == pair[1].job_id)
        {
            return Err(KernelError::FailClosed("job wait has duplicate or expired Job identity".into()));
        }
        let active_delta = self.running_since_mono_ms
            .map(|since| monotonic_ms - since).unwrap_or(0);
        let elapsed = self.running_elapsed_ms.checked_add(active_delta)
            .ok_or_else(|| KernelError::FailClosed("job wait elapsed overflow".into()))?;
        let job_deadline = ordered.iter().map(|job| job.deadline_wall_ms).max().unwrap();
        let deadline = if self.config.run_execution_limited {
            let remaining = self.config.run_execution_budget_ms - elapsed;
            if remaining <= 0 {
                return Err(KernelError::FailClosed("job wait Run budget already exhausted".into()));
            }
            wall_ms.checked_add(remaining)
                .ok_or_else(|| KernelError::FailClosed("job wait deadline overflow".into()))?
                .min(job_deadline)
        } else {
            job_deadline
        };
        self.running_elapsed_ms = elapsed;
        self.running_since_mono_ms = None;
        self.state = RunState::WaitingJobs;
        self.wait_deadline_wall_ms = Some(deadline);
        self.wait_accounted_until_wall_ms = Some(wall_ms);
        Ok(vec![self.append_event("run.waiting_jobs", serde_json::json!({
            "dataRootId": data_root_id,
            "parkedAtWallMs": wall_ms,
            "waitDeadlineWallMs": deadline,
            "runningElapsedMs": elapsed,
            "settledResponseSeq": settled_response_seq,
            "response": response,
            "history": history,
            "jobs": ordered,
        })), Effect::PublishSnapshot])
    }

    /// Charge a waiting interval once in the wall-clock domain. The caller
    /// commits with a CAS on the durable park event and prior accounted cursor.
    /// Repeated observations at the same wall instant are no-ops.
    pub fn account_waiting_jobs(
        &mut self,
        park_seq: u64,
        wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::WaitingJobs || park_seq == 0 {
            return Err(KernelError::FailClosed("run is not waiting on Jobs".into()));
        }
        let (Some(deadline), Some(accounted)) =
            (self.wait_deadline_wall_ms, self.wait_accounted_until_wall_ms) else {
            return Err(KernelError::FailClosed("job wait cursor is missing".into()));
        };
        if wall_ms < accounted {
            return Err(KernelError::FailClosed("job wait wall clock moved backwards".into()));
        }
        let through = wall_ms.min(deadline);
        if through == accounted && wall_ms < deadline {
            return Ok(Vec::new());
        }
        let mut effects = Vec::new();
        if through > accounted {
            self.running_elapsed_ms = self.running_elapsed_ms.checked_add(through - accounted)
                .ok_or_else(|| KernelError::FailClosed("job wait elapsed overflow".into()))?;
            self.wait_accounted_until_wall_ms = Some(through);
            effects.push(self.append_event("run.jobs_wait_accounted", serde_json::json!({
                "parkSeq": park_seq,
                "fromWallMs": accounted,
                "throughWallMs": through,
                "runningElapsedMs": self.running_elapsed_ms,
            })));
        }
        if self.config.run_execution_limited
            && self.running_elapsed_ms >= self.config.run_execution_budget_ms {
            effects.extend(self.terminate(RunOutcome::BudgetExhausted {
                code: "runtime.duration_budget_exceeded".into(),
                message: format!("Run exceeded its {}ms execution budget while waiting for Jobs.",
                    self.config.run_execution_budget_ms),
            }));
        } else if wall_ms >= deadline {
            // B2b-3 will reconcile/expire the Jobs before selecting wake or
            // this honest stop. The persistence kernel itself never waits
            // forever if the original Job deadline arrives first.
            effects.extend(self.terminate(RunOutcome::Failed {
                code: "kernel.job_wait_deadline_reached".into(),
                message: "Original child Job deadline elapsed before a deliverable notice was ready.".into(),
            }));
        } else {
            effects.push(Effect::PublishSnapshot);
        }
        Ok(effects)
    }

    /// Prepare one wake and its one continuation intent. B2b-2 supplies the
    /// actual typed-notice input; B2b-1 only freezes the atomic identity. The
    /// repository verifies the still-pending Host notice and frozen Job facts.
    pub fn wake_waiting_jobs(
        &mut self,
        park_seq: u64,
        wall_ms: i64,
        continuation_payload_json: &str,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::WaitingJobs || park_seq == 0
            || self.model_request_in_flight || continuation_payload_json.len() > 1_048_576 {
            return Err(KernelError::FailClosed("job wake has no settled waiting Run".into()));
        }
        let (Some(deadline), Some(accounted)) =
            (self.wait_deadline_wall_ms, self.wait_accounted_until_wall_ms) else {
            return Err(KernelError::FailClosed("job wake cursor is missing".into()));
        };
        if wall_ms < accounted || wall_ms > deadline {
            return Err(KernelError::FailClosed("job wake is outside the frozen wait window".into()));
        }
        let elapsed = self.running_elapsed_ms.checked_add(wall_ms - accounted)
            .ok_or_else(|| KernelError::FailClosed("job wake elapsed overflow".into()))?;
        if self.config.run_execution_limited && elapsed >= self.config.run_execution_budget_ms {
            return Err(KernelError::FailClosed("job wake Run budget is exhausted".into()));
        }
        let next_wake_seq = self.seq + u64::from(wall_ms > accounted) + 1;
        let effect_key = format!("continuation:{next_wake_seq}");
        let payload: serde_json::Value = serde_json::from_str(continuation_payload_json)
            .map_err(|_| KernelError::FailClosed("invalid job wake continuation".into()))?;
        if payload["lane"] != "job_notice" || payload["effectKey"] != effect_key
            || payload["turnId"] != self.turn_id || payload["input"]["runId"] != self.run_id
            || payload["input"]["turnId"] != self.turn_id
            || payload["input"]["promptConfigHash"] != self.config.prompt_config_hash {
            return Err(KernelError::FailClosed("job wake continuation identity changed".into()));
        }
        let mut effects = Vec::new();
        if wall_ms > accounted {
            self.running_elapsed_ms = elapsed;
            self.wait_accounted_until_wall_ms = Some(wall_ms);
            effects.push(self.append_event("run.jobs_wait_accounted", serde_json::json!({
                "parkSeq": park_seq,
                "fromWallMs": accounted,
                "throughWallMs": wall_ms,
                "runningElapsedMs": elapsed,
            })));
        }
        self.state = RunState::Running;
        self.running_since_mono_ms = None; // next active tick anchors the new process domain
        self.wait_deadline_wall_ms = None;
        self.wait_accounted_until_wall_ms = None;
        effects.push(self.append_event("run.jobs_woken", serde_json::json!({
            "parkSeq": park_seq,
            "fromWallMs": accounted,
            "throughWallMs": wall_ms,
            "runningElapsedMs": elapsed,
        })));
        let payload_json = payload.to_string();
        effects.push(self.append_event("engine.continuation_requested", payload));
        effects.push(Effect::RequestContinuationModel { effect_key, payload_json });
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Read a tool call's state without exposing controller internals. Used by
    /// adapters and tests to tell an expired (never dispatched) call apart from
    /// a failed one, and to confirm a completed result is still banked.
    pub fn tool_call_state(&self, tool_call_id: &str) -> Option<ToolCallState> {
        self.tools.get(tool_call_id).map(|tool| tool.state)
    }

    /// The settled result of a tool call, if it has one.
    pub fn tool_call_result_json(&self, tool_call_id: &str) -> Option<&str> {
        self.tools
            .get(tool_call_id)
            .and_then(|tool| tool.result_json.as_deref())
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn append_event(&mut self, event_type: &str, payload: serde_json::Value) -> Effect {
        let seq = self.next_seq();
        Effect::AppendEvent {
            seq,
            event_type: event_type.to_string(),
            payload_json: payload.to_string(),
        }
    }

    /// The engine proposed a batch of tool calls.
    /// Persist an opaque engine checkpoint with the same decision transaction
    /// as its tool proposal. The adapter validates engine-specific contents.
    pub fn checkpoint_tool_batch(
        &mut self,
        batch_id: &str,
        checkpoint_json: &str,
    ) -> Result<Effect, KernelError> {
        self.ensure_live()?;
        if !self.batches.iter().any(|batch| batch.batch_id == batch_id)
            || checkpoint_json.len() > 1_048_576
        {
            return Err(KernelError::FailClosed(
                "invalid engine batch checkpoint scope/size".into(),
            ));
        }
        let checkpoint: serde_json::Value = serde_json::from_str(checkpoint_json)
            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
        if !checkpoint.is_object() {
            return Err(KernelError::FailClosed(
                "engine checkpoint must be an object".into(),
            ));
        }
        Ok(self.append_event("engine.batch_checkpoint", serde_json::json!({
            "batchId":batch_id, "engineId":self.config.engine_id, "turnId":self.turn_id, "checkpoint":checkpoint,
        })))
    }

    /// The engine proposed a batch of tool calls.
    pub fn propose_tool_batch(
        &mut self,
        batch_id: &str,
        calls: Vec<ToolCallRequest>,
        policy: &dyn PolicyDecisionPort,
        now_monotonic_ms: i64,
        now_wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        self.ensure_live()?;
        if self.state != RunState::Running {
            return Err(KernelError::IllegalTransition {
                from: self.state.as_str(),
                to: "tool_batch_proposed",
            });
        }
        if batch_id.trim().is_empty() {
            return Err(KernelError::FailClosed("empty tool batch id".into()));
        }
        if calls.is_empty() {
            return Err(KernelError::FailClosed("empty tool batch".into()));
        }
        let mut call_ids = BTreeSet::new();
        let mut source_orders = BTreeSet::new();
        for call in &calls {
            if call.tool_call_id.trim().is_empty() || call.tool_call_id.len()>1024 || call.tool.trim().is_empty() {
                return Err(KernelError::FailClosed(
                    "tool call id and tool name must be non-empty".into(),
                ));
            }
            if !call_ids.insert(call.tool_call_id.as_str()) {
                return Err(KernelError::FailClosed(format!(
                    "duplicate tool call id in batch: {}",
                    call.tool_call_id
                )));
            }
            if !source_orders.insert(call.source_order) {
                return Err(KernelError::FailClosed(format!(
                    "duplicate source order in batch: {}",
                    call.source_order
                )));
            }
            match serde_json::from_str::<serde_json::Value>(&call.canonical_input_json) {
                Ok(serde_json::Value::Object(_)) => {}
                _ => {
                    return Err(KernelError::FailClosed(format!(
                        "tool input is not a canonical JSON object: {}",
                        call.tool_call_id
                    )))
                }
            }
        }
        if source_orders.iter().copied().ne(0..calls.len()) {
            return Err(KernelError::FailClosed(
                "tool source orders must be contiguous from zero".into(),
            ));
        }

        let mut requested_order = calls.iter().collect::<Vec<_>>();
        requested_order.sort_by_key(|call| call.source_order);
        let requested_order = requested_order
            .into_iter()
            .map(|call| call.tool_call_id.clone())
            .collect::<Vec<_>>();
        if let Some(existing) = self.batches.iter().find(|batch| batch.batch_id == batch_id) {
            let same_identity = existing.ordered == requested_order
                && calls.iter().all(|call| {
                    self.tools.get(&call.tool_call_id).is_some_and(|tool| {
                        tool.batch_id == batch_id
                            && tool.tool == call.tool
                            && tool.input_json == call.canonical_input_json
                            && tool.source_order == call.source_order
                    })
                });
            if same_identity {
                return Ok(vec![Effect::Audit {
                    message: format!("duplicate tool batch {batch_id} replay ignored"),
                }]);
            }
            return Err(KernelError::FailClosed(format!(
                "tool batch identity conflict: {batch_id}"
            )));
        }
        if let Some(conflict) = calls
            .iter()
            .find(|call| self.tools.contains_key(&call.tool_call_id))
        {
            return Err(KernelError::FailClosed(format!(
                "tool call id already belongs to another batch: {}",
                conflict.tool_call_id
            )));
        }
        let mut effects = Vec::new();
        // State events are emitted first; external actions follow so the adapter
        // can persist the whole decision before showing an approval or dispatching.
        let mut action_effects = Vec::new();
        for call in &calls {
            self.tools.insert(
                call.tool_call_id.clone(),
                ToolCall {
                    tool_call_id: call.tool_call_id.clone(),
                    tool: call.tool.clone(),
                    input_json: call.canonical_input_json.clone(),
                    source_order: call.source_order,
                    batch_id: batch_id.to_string(),
                    state: ToolCallState::Pending,
                    result_json: None,
                    started_at_mono_ms: None,
                },
            );
        }
        self.batches.push(ToolBatch {
            batch_id: batch_id.to_string(),
            ordered: requested_order,
            barrier_emitted: false,
        });

        let mut any_waiting = false;
        for call in calls {
            if self.tools[&call.tool_call_id].state != ToolCallState::Pending {
                continue;
            }
            let decision = policy.decide(
                &self.run_id,
                &self.config.permission_snapshot_id,
                &call.tool,
                &call.canonical_input_json,
            );
            match decision {
                crate::ports::PolicyDecision::Allow => {
                    {
                        let tool = self.tools.get_mut(&call.tool_call_id).unwrap();
                        tool.state = ToolCallState::Running;
                        tool.started_at_mono_ms = Some(now_monotonic_ms);
                    }
                    action_effects.push(Effect::DispatchTool {
                        tool_call_id: call.tool_call_id.clone(),
                        tool: call.tool.clone(),
                        input_json: call.canonical_input_json.clone(),
                    });
                }
                crate::ports::PolicyDecision::RequireApproval => {
                    any_waiting = true;
                    self.tools.get_mut(&call.tool_call_id).unwrap().state =
                        ToolCallState::WaitingApproval;
                    action_effects.push(Effect::RequestApproval {
                        tool_call_id: call.tool_call_id.clone(),
                        tool: call.tool.clone(),
                        input_json: call.canonical_input_json.clone(),
                    });
                }
                crate::ports::PolicyDecision::Reject { code, message } => {
                    let result = serde_json::json!({
                        "isError": true,
                        "content": [{"type":"text", "text":message}],
                        "details": {"code":code, "errorCode":code, "executionStarted":false}
                    });
                    let tool = self.tools.get_mut(&call.tool_call_id).unwrap();
                    tool.state = ToolCallState::Failed;
                    tool.result_json = Some(result.to_string());
                    effects.push(self.append_event("tool.failed", serde_json::json!({
                        "toolCallId":call.tool_call_id, "tool":call.tool,
                        "code":code, "message":message, "executionStarted":false
                    })));
                }
                crate::ports::PolicyDecision::Deny { reason } => {
                    self.tools.get_mut(&call.tool_call_id).unwrap().state = ToolCallState::Failed;
                    effects.push(self.append_event(
                        "tool.failed",
                        serde_json::json!({
                            "toolCallId": call.tool_call_id,
                            "tool": call.tool,
                            "code": "kernel.policy_denied",
                            "message": reason,
                        }),
                    ));
                }
            }
        }

        if any_waiting && self.state == RunState::Running {
            // Suspend the execution budget while awaiting a human; approval uses
            // its own wall clock, never the ordinary host-request timeout. The
            // deadline is a persistent wall time so the wait survives restart.
            self.suspend_running_clock(now_monotonic_ms);
            self.approval_deadline_wall_ms =
                Some(now_wall_ms.saturating_add(self.config.approval_wait_timeout_ms));
            self.state = RunState::WaitingApproval;
            effects.push(self.append_event(
                "run.waiting_approval",
                serde_json::json!({
                    "approvalWaitTimeoutMs": self.config.approval_wait_timeout_ms,
                    "approvalDeadlineWallMs": self.approval_deadline_wall_ms,
                }),
            ));
        }
        effects.extend(action_effects);
        effects.extend(self.maybe_barrier_effects());
        // The model responded by proposing this batch; the model-request wait
        // is settled. It re-arms on the next turn dispatch (batch delivery to
        // the following model request).
        self.settle_model_request();
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Re-decide the SAME unconsumed intent after a policy change. Never append
    /// a second tool or resurrect a dispatched/terminal call. The adapter must
    /// persist `tool.reevaluated` and the replacement decision atomically with
    /// superseding its old approval/outbox fact.
    pub fn reevaluate_tool(
        &mut self, policy: &dyn PolicyDecisionPort, tool_call_id: &str,
        expected_policy_version: u64, now_monotonic_ms: i64, now_wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        self.ensure_live()?;
        if !matches!(self.state, RunState::Running | RunState::WaitingApproval) {
            return Err(KernelError::IllegalTransition { from: self.state.as_str(), to: "tool_reevaluated" });
        }
        let tool = self.tools.get(tool_call_id)
            .ok_or_else(|| KernelError::UnknownToolCall(tool_call_id.into()))?;
        if !matches!(tool.state, ToolCallState::Pending | ToolCallState::WaitingApproval)
            || tool.started_at_mono_ms.is_some() || tool.result_json.is_some()
            || self.batches.iter().any(|batch| batch.batch_id == tool.batch_id && batch.barrier_emitted) {
            return Err(KernelError::FailClosed("intent_consumed_or_terminal".into()));
        }
        let tool_name = tool.tool.clone();
        let input_json = tool.input_json.clone();
        let decision = policy.decide(&self.run_id, &self.config.permission_snapshot_id, &tool_name, &input_json);
        let mut effects = vec![self.append_event("tool.reevaluated", serde_json::json!({
            "toolCallId": tool_call_id, "tool": tool_name, "expectedPolicyVersion": expected_policy_version,
        }))];
        match decision {
            crate::ports::PolicyDecision::Allow => {
                let tool = self.tools.get_mut(tool_call_id).unwrap();
                tool.state = ToolCallState::Running;
                tool.started_at_mono_ms = Some(now_monotonic_ms);
                effects.push(Effect::DispatchTool { tool_call_id: tool_call_id.into(), tool: tool_name, input_json });
            }
            crate::ports::PolicyDecision::RequireApproval => {
                self.tools.get_mut(tool_call_id).unwrap().state = ToolCallState::WaitingApproval;
                effects.push(Effect::RequestApproval { tool_call_id: tool_call_id.into(), tool: tool_name, input_json });
            }
            crate::ports::PolicyDecision::Deny { reason } => {
                let result = serde_json::json!({"isError":true,"content":[{"type":"text","text":reason}],
                    "details":{"errorCode":"kernel.policy_denied","executionStarted":false}});
                let tool = self.tools.get_mut(tool_call_id).unwrap();
                tool.state = ToolCallState::Failed;
                tool.result_json = Some(result.to_string());
                effects.push(self.append_event("tool.failed", serde_json::json!({
                    "toolCallId":tool_call_id,"tool":tool_name,"code":"kernel.policy_denied","message":reason,
                    "executionStarted":false})));
            }
            crate::ports::PolicyDecision::Reject { code, message } => {
                let result = serde_json::json!({"isError":true,"content":[{"type":"text","text":message}],
                    "details":{"errorCode":code,"executionStarted":false}});
                let tool = self.tools.get_mut(tool_call_id).unwrap();
                tool.state = ToolCallState::Failed;
                tool.result_json = Some(result.to_string());
                effects.push(self.append_event("tool.failed", serde_json::json!({
                    "toolCallId":tool_call_id,"tool":tool_name,"code":code,"message":message,"executionStarted":false})));
            }
        }
        if self.tools.values().any(|tool| tool.state == ToolCallState::WaitingApproval) {
            if self.state == RunState::Running { self.suspend_running_clock(now_monotonic_ms); }
            self.state = RunState::WaitingApproval;
            self.approval_deadline_wall_ms = Some(now_wall_ms.saturating_add(self.config.approval_wait_timeout_ms));
            effects.push(self.append_event("run.waiting_approval", serde_json::json!({
                "approvalWaitTimeoutMs":self.config.approval_wait_timeout_ms,
                "approvalDeadlineWallMs":self.approval_deadline_wall_ms})));
        } else if self.state == RunState::WaitingApproval {
            self.state = RunState::Running;
            self.running_since_mono_ms = Some(now_monotonic_ms);
            self.approval_deadline_wall_ms = None;
        }
        effects.extend(self.maybe_barrier_effects());
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// A human resolved an approval. Only valid while the tool is waiting and the
    /// run is non-terminal; an approval after cancel/terminal fails closed.
    pub fn resolve_approval(
        &mut self,
        tool_call_id: &str,
        decision: ApprovalDecision,
        now_monotonic_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state.is_terminal() || self.state == RunState::Cancelling {
            return Err(KernelError::Terminal {
                current: self.state.as_str(),
            });
        }
        let (tool_name, input_json) = {
            let tool = self
                .tools
                .get(tool_call_id)
                .ok_or_else(|| KernelError::UnknownToolCall(tool_call_id.to_string()))?;
            if tool.state != ToolCallState::WaitingApproval {
                return Err(KernelError::NotWaitingApproval(tool_call_id.to_string()));
            }
            (tool.tool.clone(), tool.input_json.clone())
        };
        let mut effects = vec![Effect::ApprovalResolved {
            tool_call_id: tool_call_id.to_string(),
            decision,
        }];
        match decision {
            ApprovalDecision::Deny => {
                self.tools.get_mut(tool_call_id).unwrap().state = ToolCallState::Failed;
                effects.push(self.append_event(
                    "tool.failed",
                    serde_json::json!({
                        "toolCallId": tool_call_id,
                        "tool": tool_name,
                        "code": "approval.denied",
                        "message": "The user denied this action.",
                    }),
                ));
            }
            ApprovalDecision::AllowOnce | ApprovalDecision::AllowConversation => {
                {
                    let tool = self.tools.get_mut(tool_call_id).unwrap();
                    tool.state = ToolCallState::Running;
                    tool.started_at_mono_ms = Some(now_monotonic_ms);
                }
                effects.push(Effect::DispatchTool {
                    tool_call_id: tool_call_id.to_string(),
                    tool: tool_name,
                    input_json,
                });
            }
        }
        // Resume the run (and its execution budget clock) once nothing is still
        // waiting for approval. Wall deadline is cleared.
        if !self
            .tools
            .values()
            .any(|t| t.state == ToolCallState::WaitingApproval)
            && self.state == RunState::WaitingApproval
        {
            self.state = RunState::Running;
            self.running_since_mono_ms = Some(now_monotonic_ms);
            self.approval_deadline_wall_ms = None;
        }
        effects.extend(self.maybe_barrier_effects());
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// A tool reached a terminal result. Idempotent on tool call id. A late
    /// success after the run was cancelled is audited, never resurrects the run.
    pub fn tool_settled(
        &mut self,
        tool_call_id: &str,
        ok: bool,
        result_json: &str,
    ) -> Result<Vec<Effect>, KernelError> {
        let prior_state = match self.tools.get(tool_call_id) {
            Some(tool) => tool.state,
            None => {
                if self.state.is_terminal() || self.state == RunState::Cancelling {
                    return Ok(vec![Effect::Audit {
                        message: format!(
                            "late result for unknown tool {tool_call_id} after terminal"
                        ),
                    }]);
                }
                return Err(KernelError::UnknownToolCall(tool_call_id.to_string()));
            }
        };
        if prior_state.is_terminal() {
            return Ok(vec![Effect::Audit {
                message: format!("duplicate settlement for tool {tool_call_id} ignored"),
            }]);
        }
        let late_after_cancel = self.state == RunState::Cancelling || self.state.is_terminal();
        if !late_after_cancel && prior_state != ToolCallState::Running {
            return Err(KernelError::FailClosed(format!(
                "tool result received before dispatch/approval: {tool_call_id} ({})",
                prior_state.as_str()
            )));
        }
        let new_state = if late_after_cancel {
            ToolCallState::Cancelled
        } else if ok {
            ToolCallState::Completed
        } else {
            ToolCallState::Failed
        };
        if let Some(tool) = self.tools.get_mut(tool_call_id) {
            tool.state = new_state;
            tool.result_json = Some(result_json.to_string());
            tool.started_at_mono_ms = None;
        }
        let event = if late_after_cancel {
            self.append_event(
                "tool.audit_late",
                serde_json::json!({
                    "toolCallId": tool_call_id,
                    "lateAfterCancellation": true,
                    "ok": ok,
                }),
            )
        } else if ok {
            self.append_event(
                "tool.completed",
                serde_json::json!({ "toolCallId": tool_call_id, "result": serde_json::from_str::<serde_json::Value>(result_json).unwrap_or(serde_json::json!({})) }),
            )
        } else {
            self.append_event(
                "tool.failed",
                serde_json::json!({ "toolCallId": tool_call_id, "error": result_json }),
            )
        };
        let mut effects = vec![event];
        if prior_state == ToolCallState::Running {
            effects.push(Effect::ToolDispatchSettled {
                tool_call_id: tool_call_id.to_string(),
            });
        }
        if late_after_cancel {
            effects.push(Effect::Audit {
                message: format!(
                    "result for {tool_call_id} arrived during cancellation; audited, not applied"
                ),
            });
            return Ok(effects);
        }
        effects.extend(self.maybe_barrier_effects());
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// User/host requests cancellation. Run-scoped only.
    pub fn request_cancel(&mut self) -> Vec<Effect> {
        if self.state == RunState::WaitingJobs {
            return vec![Effect::Audit {
                message: "waiting Jobs require wall-clock accounting before cancellation".into(),
            }];
        }
        if self.terminal_written || self.state.is_terminal() {
            return vec![Effect::Audit {
                message: "cancel ignored; run already terminal".into(),
            }];
        }
        if self.state == RunState::Cancelling {
            return vec![Effect::Audit {
                message: "cancel ignored; already cancelling".into(),
            }];
        }
        self.retry.scheduled_at_wall_ms = None;
        self.retry.due_wall_ms = None;
        self.approval_deadline_wall_ms = None;
        self.wait_deadline_wall_ms = None;
        self.wait_accounted_until_wall_ms = None;
        self.state = RunState::Cancelling;
        let mut effects = vec![self.append_event("run.cancelling", serde_json::json!({}))];
        effects.push(Effect::CancelEngineTurn {
            turn_id: self.turn_id.clone(),
        });
        for tool in self.tools.values() {
            match tool.state {
                ToolCallState::Running => effects.push(Effect::CancelToolCall {
                    tool_call_id: tool.tool_call_id.clone(),
                }),
                ToolCallState::WaitingApproval => {
                    effects.push(Effect::ApprovalCancelled {
                        tool_call_id: tool.tool_call_id.clone(),
                    });
                    effects.push(Effect::Audit {
                        message: format!(
                            "approval for {} abandoned by cancellation",
                            tool.tool_call_id
                        ),
                    });
                }
                _ => {}
            }
        }
        effects.push(Effect::PublishSnapshot);
        effects
    }

    /// Cancellation has priority over a simultaneous Run budget expiry, but
    /// the elapsed wait interval is still charged exactly once before clearing
    /// the wait projection. Job process cancellation is a later Host action.
    pub fn cancel_waiting_jobs(&mut self, park_seq: u64, wall_ms: i64)
        -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::WaitingJobs || park_seq == 0 {
            return Err(KernelError::FailClosed("Run is not waiting on Jobs".into()));
        }
        let (Some(deadline), Some(accounted)) =
            (self.wait_deadline_wall_ms, self.wait_accounted_until_wall_ms) else {
            return Err(KernelError::FailClosed("job wait cursor is missing".into()));
        };
        if wall_ms < accounted {
            return Err(KernelError::FailClosed("job wait wall clock moved backwards".into()));
        }
        let through = wall_ms.min(deadline);
        let elapsed = self.running_elapsed_ms.checked_add(through - accounted)
            .ok_or_else(|| KernelError::FailClosed("job wait elapsed overflow".into()))?;
        let mut effects = Vec::new();
        self.running_elapsed_ms = elapsed;
        if through > accounted {
            effects.push(self.append_event("run.jobs_wait_accounted", serde_json::json!({
                "parkSeq": park_seq, "fromWallMs": accounted,
                "throughWallMs": through, "runningElapsedMs": elapsed,
            })));
        }
        self.wait_deadline_wall_ms = None;
        self.wait_accounted_until_wall_ms = None;
        self.state = RunState::Running;
        effects.extend(self.request_cancel());
        Ok(effects)
    }

    pub fn settle_cancellation(&mut self) -> Vec<Effect> {
        if self.terminal_written {
            return vec![Effect::Audit {
                message: "cancellation terminal already written".into(),
            }];
        }
        if self.state != RunState::Cancelling {
            return vec![Effect::Audit {
                message: format!(
                    "cancellation settlement ignored while run is {}",
                    self.state.as_str()
                ),
            }];
        }
        let mut cancelled_tool_ids = Vec::new();
        for tool in self.tools.values_mut() {
            if !tool.state.is_terminal() {
                tool.state = ToolCallState::Cancelled;
                cancelled_tool_ids.push(tool.tool_call_id.clone());
            }
        }
        // Invariant: a cancelled (terminal) run drops any in-flight model
        // request anchor.
        self.settle_model_request();
        self.terminal_written = true;
        self.state = RunState::Cancelled;
        self.wait_deadline_wall_ms = None;
        self.wait_accounted_until_wall_ms = None;
        let mut effects = cancelled_tool_ids
            .into_iter()
            .map(|tool_call_id| {
                self.append_event(
                    "tool.cancelled",
                    serde_json::json!({ "toolCallId": tool_call_id }),
                )
            })
            .collect::<Vec<_>>();
        effects.push(self.append_event("run.cancelled", serde_json::json!({})));
        effects.push(Effect::PublishSnapshot);
        effects
    }

    pub fn terminate(&mut self, outcome: RunOutcome) -> Vec<Effect> {
        if self.terminal_written || self.state.is_terminal() {
            return vec![Effect::Audit {
                message: format!(
                    "terminal {} ignored; run already {}",
                    outcome.event_type(),
                    self.state.as_str()
                ),
            }];
        }
        if self.state == RunState::Cancelling && !matches!(outcome, RunOutcome::Cancelled) {
            return vec![Effect::Audit {
                message: format!(
                    "terminal {} ignored while cancellation is authoritative",
                    outcome.event_type()
                ),
            }];
        }
        if matches!(outcome, RunOutcome::Cancelled) && self.state != RunState::Cancelling {
            return vec![Effect::Audit {
                message: format!(
                    "run.cancelled ignored without a cancelling transition (current {})",
                    self.state.as_str()
                ),
            }];
        }
        if matches!(outcome, RunOutcome::Cancelled) {
            return self.settle_cancellation();
        }
        if self.state == RunState::WaitingJobs && matches!(outcome, RunOutcome::Completed) {
            return vec![Effect::Audit {
                message: "run.completed ignored while child Job facts await a durable wake".into(),
            }];
        }
        if matches!(outcome, RunOutcome::Completed)
            && self.tools.values().any(|tool| !tool.state.is_terminal())
        {
            return vec![Effect::Audit {
                message: "run.completed ignored while tool calls are unresolved".into(),
            }];
        }
        let unresolved = self
            .tools
            .values()
            .filter(|tool| !tool.state.is_terminal())
            .map(|tool| (tool.tool_call_id.clone(), tool.state))
            .collect::<Vec<_>>();
        for (tool_call_id, _) in &unresolved {
            if let Some(tool) = self.tools.get_mut(tool_call_id) {
                tool.state = ToolCallState::Cancelled;
            }
        }
        self.retry.scheduled_at_wall_ms = None;
        self.retry.due_wall_ms = None;
        self.approval_deadline_wall_ms = None;
        self.wait_deadline_wall_ms = None;
        self.wait_accounted_until_wall_ms = None;
        // Invariant: a terminal run must not hold an in-flight model-request
        // anchor (it would be meaningless after termination and must not be
        // re-armed by a restart).
        self.settle_model_request();
        self.terminal_written = true;
        self.state = outcome.state();
        let payload = match &outcome {
            RunOutcome::Completed | RunOutcome::Cancelled => serde_json::json!({}),
            RunOutcome::Failed { code, message }
            | RunOutcome::BudgetExhausted { code, message } => {
                serde_json::json!({ "code": code, "message": message })
            }
            // Expiry keeps the completed work and stays explicitly continuable;
            // the terminal event must say so, otherwise a reader would treat the
            // run as a plain failure.
            RunOutcome::ApprovalExpired { code, message } => serde_json::json!({
                "code": code,
                "message": message,
                "continuable": true,
            }),
        };
        let mut effects = Vec::new();
        for (tool_call_id, prior_state) in unresolved {
            effects.push(self.append_event(
                "tool.cancelled",
                serde_json::json!({
                    "toolCallId": tool_call_id,
                    "reason": outcome.event_type(),
                }),
            ));
            if prior_state == ToolCallState::Running {
                effects.push(Effect::CancelToolCall { tool_call_id });
            }
        }
        effects.push(self.append_event(outcome.event_type(), payload));
        effects.push(Effect::PublishSnapshot);
        effects
    }

    /// Whole-turn retry: schedule another agent turn after a terminal turn
    /// failure. Distinct from provider HTTP retry. Enters `retry_scheduled`;
    /// exhausts to a failed terminal when the turn budget is spent.
    pub fn schedule_turn_retry(
        &mut self,
        now_monotonic_ms: i64,
        now_wall_ms: i64,
        reason_code: &str,
        delay_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        self.ensure_live()?;
        if self.state != RunState::Running {
            return Err(KernelError::IllegalTransition {
                from: self.state.as_str(),
                to: "retry_scheduled",
            });
        }
        self.retry.turn_attempts = self.retry.turn_attempts.saturating_add(1);
        if self.retry.turn_attempts > self.retry.turn_max {
            // Retry budget exhausted: terminal failure, not an infinite loop.
            return Ok(self.terminate(RunOutcome::Failed {
                code: "runtime.turn_retry_exhausted".into(),
                message: format!("Turn retry budget ({}) exhausted.", self.retry.turn_max),
            }));
        }
        self.suspend_running_clock(now_monotonic_ms);
        self.state = RunState::RetryScheduled;
        self.retry.scheduled_at_wall_ms = Some(now_wall_ms);
        self.retry.due_wall_ms = Some(now_wall_ms.saturating_add(delay_ms.max(0)));
        let mut effects = vec![self.append_event(
            "run.retrying",
            serde_json::json!({
                "attempt": self.retry.turn_attempts,
                "maxAttempts": self.retry.turn_max,
                "delayMs": delay_ms.max(0),
                "scheduledAtWallMs": self.retry.scheduled_at_wall_ms,
                "dueWallMs": self.retry.due_wall_ms,
                "reasonCode": reason_code,
            }),
        )];
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Retry only a model request known to have settled without a usable result.
    /// Resource effects are never replayed by this transition. The repository
    /// atomically verifies/reset its model lease alongside these events.
    pub fn schedule_model_retry(&mut self, now_monotonic_ms: i64, now_wall_ms: i64,
        effect_key: &str, failure_json: &str, provider: bool, delay_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        self.ensure_live()?;
        if self.state != RunState::Running
            || !self.model_request_in_flight
            || delay_ms < 0
            || self.tools.values().any(|tool| !tool.state.is_terminal())
        {
            return Err(KernelError::FailClosed(
                "model retry requires an exclusively active model request".into(),
            ));
        }
        let failure: serde_json::Value = serde_json::from_str(failure_json)
            .map_err(|_| KernelError::FailClosed("invalid model failure evidence".into()))?;
        let completion = failure["category"] == "incomplete_response";
        // model_timeout retries only the model request and is bounded by the
        // turn budget, not the provider budget. The caller (Host) has already
        // proven no tool was dispatched in the failed round.
        let timeout = failure["category"] == "model_timeout";
        let transport = failure["category"] == "model_transport_failure";
        let turn_failure = timeout || transport;
        if provider != (failure["category"] == "provider_unavailable")
            || (!provider && !completion && !turn_failure)
        {
            return Err(KernelError::FailClosed("model retry category mismatch".into()));
        }
        let (used, maximum) = if provider {
            (self.retry.provider_attempts, self.retry.provider_max)
        } else if turn_failure {
            (self.retry.turn_attempts, self.retry.turn_max)
        } else {
            (
                if self.retry.completion_effect_key.as_deref() == Some(effect_key) {
                    self.retry.completion_attempts
                } else {
                    0
                },
                self.retry.turn_max,
            )
        };
        self.suspend_running_clock(now_monotonic_ms);
        self.settle_model_request();
        let mut effects = vec![self.append_event(
            "engine.model_rejected",
            serde_json::json!({ "effectKey": effect_key, "failure": failure }),
        )];
        if used >= maximum || (self.config.run_execution_limited && delay_ms >= self.config.run_execution_budget_ms.saturating_sub(self.running_elapsed_ms)) {
            self.retry.model_dispatch_pending = false;
            let (code, message) = if failure["category"] == "incomplete_response" {
                ("kernel.model_incomplete", "模型未完成本轮答复，自动续答次数已用完。已完成的工具结果已保留；请发送“继续完成剩余工作”。")
            } else if timeout {
                ("kernel.model_retry_exhausted", "模型请求多次超时（无首响应/流停滞/整轮超限），自动重试已停止。已完成的工具结果已保留；已执行的外部操作不会被重放，请检查后继续。")
            } else {
                ("kernel.model_retry_exhausted", "模型服务请求失败，自动重试已停止。已完成的工具结果已保留，请稍后继续。")
            };
            effects.extend(self.terminate(RunOutcome::Failed { code: code.into(), message: message.into() }));
            return Ok(effects);
        }
        self.state = RunState::RetryScheduled;
        self.retry.scheduled_at_wall_ms = Some(now_wall_ms);
        self.retry.due_wall_ms = Some(now_wall_ms.saturating_add(delay_ms));
        self.retry.model_dispatch_pending = true;
        if provider {
            self.retry.provider_attempts += 1;
        } else if turn_failure {
            self.retry.turn_attempts += 1;
        } else {
            self.retry.completion_effect_key = Some(effect_key.to_owned());
            self.retry.completion_attempts = used + 1;
        }
        effects.push(self.append_event(
            "run.retrying",
            serde_json::json!({
                "kind": if provider { "provider" } else if timeout { "model_timeout" } else if transport { "model_transport_failure" } else { "completion" }, "attempt": used + 1,
                "effectKey": effect_key,
                "maxAttempts": maximum, "delayMs": delay_ms, "scheduledAtWallMs": now_wall_ms,
                "dueWallMs": self.retry.due_wall_ms,
            }),
        ));
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Resume after a scheduled turn retry.
    pub fn retry_resume(
        &mut self,
        now_monotonic_ms: i64,
        now_wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::RetryScheduled {
            return Err(KernelError::IllegalTransition {
                from: self.state.as_str(),
                to: "running",
            });
        }
        let scheduled_at = self.retry.scheduled_at_wall_ms.ok_or_else(|| {
            KernelError::FailClosed("retry_scheduled is missing its durable wall anchor".into())
        })?;
        let due = self.retry.due_wall_ms.ok_or_else(|| {
            KernelError::FailClosed("retry_scheduled is missing its durable due time".into())
        })?;
        if now_wall_ms < scheduled_at {
            return Err(KernelError::FailClosed(
                "wall clock moved backwards during a scheduled retry".into(),
            ));
        }
        if now_wall_ms < due {
            return Err(KernelError::FailClosed(format!(
                "scheduled retry is not due until {due}"
            )));
        }
        self.state = RunState::Running;
        self.retry.scheduled_at_wall_ms = None;
        self.retry.due_wall_ms = None;
        self.running_since_mono_ms = Some(now_monotonic_ms);
        // The retried turn dispatches a fresh model request.
        if !self.retry.model_dispatch_pending {
            self.arm_model_request(now_monotonic_ms, now_wall_ms);
        }
        let mut effects = vec![self.append_event(
            "run.retry.completed",
            serde_json::json!({ "success": true, "attempt": self.retry.turn_attempts }),
        )];
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Record a provider HTTP retry (observational; bounded independently of turn
    /// retry). Fails closed once the provider budget is spent.
    pub fn record_provider_retry(&mut self) -> Result<Vec<Effect>, KernelError> {
        self.retry.provider_attempts = self.retry.provider_attempts.saturating_add(1);
        if self.retry.provider_attempts > self.retry.provider_max {
            return Err(KernelError::FailClosed(format!(
                "provider retry budget ({}) exhausted",
                self.retry.provider_max
            )));
        }
        Ok(vec![self.append_event(
            "run.provider_retry",
            serde_json::json!({ "attempt": self.retry.provider_attempts }),
        )])
    }

    /// Begin context compaction. Distinct state from retry.
    pub fn begin_compaction(
        &mut self,
        now_monotonic_ms: i64,
        reason: &str,
    ) -> Result<Vec<Effect>, KernelError> {
        self.ensure_live()?;
        if self.state != RunState::Running {
            return Err(KernelError::IllegalTransition {
                from: self.state.as_str(),
                to: "compacting",
            });
        }
        self.suspend_running_clock(now_monotonic_ms);
        self.state = RunState::Compacting;
        let mut effects = vec![self.append_event(
            "context.compaction.started",
            serde_json::json!({ "reason": reason }),
        )];
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// End context compaction. `aborted` returns the run to Running for a retry
    /// path; a completed compaction resumes Running directly.
    pub fn end_compaction(
        &mut self,
        now_monotonic_ms: i64,
        aborted: bool,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Compacting {
            return Err(KernelError::IllegalTransition {
                from: self.state.as_str(),
                to: "running",
            });
        }
        if !aborted {
            self.compaction.compactions = self.compaction.compactions.saturating_add(1);
        }
        self.compaction.last_reason = None;
        self.compaction.pending = None;
        self.state = RunState::Running;
        self.running_since_mono_ms = Some(now_monotonic_ms);
        // A prepared normal model delivery is still pending, not in flight.
        // Its own atomic dispatch will arm a fresh, paired clock reading.
        self.settle_model_request();
        let mut effects = vec![self.append_event(
            "context.compaction.completed",
            serde_json::json!({ "aborted": aborted, "willRetry": aborted }),
        )];
        effects.push(Effect::PublishSnapshot);
        Ok(effects)
    }

    /// Persist an input before launching a summarizer. No tool may be in flight.
    pub fn prepare_context_compaction(
        &mut self,
        id: &str,
        plan_json: &str,
        monotonic_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.config.kernel_mode != "authoritative"
            || self.model_request_in_flight
            || self.compaction.pending.is_some()
            || id.trim().is_empty()
            || id.len() > 512
            || self.tools.values().any(|tool| !tool.state.is_terminal())
            || plan_json.len() > 2_097_152
        {
            return Err(KernelError::FailClosed(
                "context compaction requires an idle authoritative boundary".into(),
            ));
        }
        let plan: serde_json::Value = serde_json::from_str(plan_json)
            .map_err(|_| KernelError::FailClosed("invalid compaction plan".into()))?;
        let mut effects = self.begin_compaction(monotonic_ms, "host_context_threshold")?;
        self.compaction.last_reason = Some("host_context_threshold".into());
        self.compaction.pending = Some(crate::ports::PendingCompaction {
            id: id.into(),
            owner: None,
        });
        // Summarization is execution work, unlike waiting for human approval.
        self.running_since_mono_ms = Some(monotonic_ms);
        effects.push(self.append_event(
            "context.compaction.prepared",
            serde_json::json!({"id":id,"plan":plan}),
        ));
        Ok(effects)
    }

    pub fn dispatch_context_compaction(
        &mut self,
        id: &str,
        owner: &str,
        monotonic_ms: i64,
        wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Compacting
            || self.model_request_in_flight
            || owner.trim().is_empty()
            || owner.len() > 512
            || wall_ms <= 0
            || self
                .compaction
                .pending
                .as_ref()
                .is_none_or(|pending| pending.id != id || pending.owner.is_some())
        {
            return Err(KernelError::FailClosed(
                "compaction dispatch is already claimed or invalid".into(),
            ));
        }
        self.compaction.pending.as_mut().unwrap().owner = Some(owner.into());
        self.arm_model_request(monotonic_ms, wall_ms);
        Ok(vec![self.append_event(
            "context.compaction.dispatched",
            serde_json::json!({"id":id,"owner":owner,"startedAt":wall_ms}),
        )])
    }

    pub fn complete_context_compaction(
        &mut self,
        id: &str,
        owner: &str,
        result_json: &str,
        applied: bool,
        monotonic_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Compacting
            || !self.model_request_in_flight
            || self
                .compaction
                .pending
                .as_ref()
                .is_none_or(|pending| pending.id != id || pending.owner.as_deref() != Some(owner))
            || result_json.len() > 32_768
        {
            return Err(KernelError::FailClosed(
                "compaction result lost its dispatch owner".into(),
            ));
        }
        let result: serde_json::Value = serde_json::from_str(result_json)
            .map_err(|_| KernelError::FailClosed("invalid compaction result".into()))?;
        let mut effects = vec![self.append_event(
            "context.compaction.result",
            serde_json::json!({"id":id,"owner":owner,"result":result}),
        )];
        effects.extend(self.end_compaction(monotonic_ms, !applied)?);
        Ok(effects)
    }

    /// Periodic budget/timeout check. `monotonic_ms` drives in-process execution
    /// and tool timeouts; `wall_ms` drives the persistent approval deadline.
    pub fn tick(&mut self, monotonic_ms: i64, wall_ms: i64) -> Vec<Effect> {
        self.tick_budgets(monotonic_ms, wall_ms, true)
    }

    /// The caller has received a settled frame or reaped the exclusively owned
    /// worker. Continue enforcing Run/tool/approval clocks while its model outcome
    /// is committed through the dispatch lease, rather than racing a second terminal.
    pub fn tick_settled_model(&mut self, monotonic_ms: i64, wall_ms: i64) -> Vec<Effect> {
        self.tick_budgets(monotonic_ms, wall_ms, false)
    }

    fn tick_budgets(&mut self, monotonic_ms: i64, wall_ms: i64, check_model: bool) -> Vec<Effect> {
        if matches!(self.state, RunState::Running | RunState::WaitingApproval) {
            // A recovered process has a fresh monotonic-clock domain. Anchor
            // every in-flight tool on its first tick so its timeout resumes
            // instead of remaining disabled forever after rehydration.
            for tool in self.tools.values_mut() {
                if tool.state == ToolCallState::Running && tool.started_at_mono_ms.is_none() {
                    tool.started_at_mono_ms = Some(monotonic_ms);
                }
            }
            let mut tool_timeout_effects = self.expire_running_tools(monotonic_ms);
            if !tool_timeout_effects.is_empty() {
                tool_timeout_effects.extend(self.maybe_barrier_effects());
                tool_timeout_effects.push(Effect::PublishSnapshot);
                return tool_timeout_effects;
            }
        }

        if self.state == RunState::RetryScheduled {
            let Some(scheduled_at) = self.retry.scheduled_at_wall_ms else {
                return self.terminate(RunOutcome::Failed {
                    code: "retry.schedule_missing".into(),
                    message: "Scheduled retry is missing its durable wall-clock anchor.".into(),
                });
            };
            let Some(due) = self.retry.due_wall_ms else {
                return self.terminate(RunOutcome::Failed {
                    code: "retry.schedule_missing".into(),
                    message: "Scheduled retry is missing its durable due time.".into(),
                });
            };
            if wall_ms < scheduled_at {
                return self.terminate(RunOutcome::Failed {
                    code: "retry.clock_rollback".into(),
                    message: "Wall clock moved backwards during a scheduled retry.".into(),
                });
            }
            if wall_ms >= due {
                return self
                    .retry_resume(monotonic_ms, wall_ms)
                    .unwrap_or_else(|error| {
                        self.terminate(RunOutcome::Failed {
                            code: "retry.resume_failed".into(),
                            message: error.to_string(),
                        })
                    });
            }
            return Vec::new();
        }

        if self.state == RunState::WaitingApproval {
            let Some(deadline) = self.approval_deadline_wall_ms else {
                return vec![Effect::Audit {
                    message: "waiting_approval has no persisted approval deadline".into(),
                }];
            };
            // Fail closed on a backwards wall clock rather than waiting forever.
            if wall_ms < deadline.saturating_sub(self.config.approval_wait_timeout_ms) {
                return self.terminate(RunOutcome::Failed {
                    code: "approval.clock_rollback".into(),
                    message: "Wall clock moved backwards during an approval wait.".into(),
                });
            }
            if wall_ms < deadline {
                return Vec::new();
            }
            let waiting_ids = self
                .tools
                .values()
                .filter(|tool| tool.state == ToolCallState::WaitingApproval)
                .map(|tool| tool.tool_call_id.clone())
                .collect::<Vec<_>>();
            let running_ids = self
                .tools
                .values()
                .filter(|tool| tool.state == ToolCallState::Running)
                .map(|tool| tool.tool_call_id.clone())
                .collect::<Vec<_>>();
            for id in &waiting_ids {
                if let Some(tool) = self.tools.get_mut(id) {
                    // A call that was waiting for a human was NEVER dispatched.
                    // It is expired, not failed: the work is still open and a
                    // continuation may re-propose it under a fresh approval.
                    // Marking it `Failed` made the whole Run look unrecoverable.
                    tool.state = ToolCallState::Expired;
                }
            }
            for id in &running_ids {
                if let Some(tool) = self.tools.get_mut(id) {
                    tool.state = ToolCallState::Cancelled;
                }
            }
            self.approval_deadline_wall_ms = None;
            let completed_ids = self
                .tools
                .values()
                .filter(|tool| tool.state == ToolCallState::Completed)
                .map(|tool| tool.tool_call_id.clone())
                .collect::<Vec<_>>();
            let mut effects = Vec::new();
            for tool_call_id in &waiting_ids {
                effects.push(Effect::ApprovalExpired {
                    tool_call_id: tool_call_id.clone(),
                });
                effects.push(self.append_event(
                    "tool.expired",
                    serde_json::json!({
                        "toolCallId": tool_call_id,
                        "code": "approval.wait_timeout",
                        "message": "The approval request expired before the user decided; the call was not executed.",
                        "continuable": true
                    }),
                ));
            }
            for tool_call_id in running_ids {
                effects.push(Effect::CancelToolCall {
                    tool_call_id: tool_call_id.clone(),
                });
                effects.push(self.append_event(
                    "tool.cancelled",
                    serde_json::json!({
                        "toolCallId": tool_call_id,
                        "reason": "approval_wait_timeout"
                    }),
                ));
            }
            // Record the durable resume facts before the terminal write: which
            // approvals are dead, that they are not executable, and which
            // results were already banked. A continuation re-verifies the scope
            // and opens a NEW approval; it never reuses these.
            effects.push(self.append_event(
                "run.awaiting_approval_expired",
                serde_json::json!({
                    "approvalWaitTimeoutMs": self.config.approval_wait_timeout_ms,
                    "expiredToolCallIds": waiting_ids,
                    "expiredApprovalsExecutable": false,
                    "completedToolCallIds": completed_ids,
                    "resumable": true
                }),
            ));
            effects.extend(self.terminate(RunOutcome::ApprovalExpired {
                code: "approval.wait_timeout".into(),
                message: format!(
                    "Approval was not resolved within {}ms. The expired approval cannot be executed; the Host can re-verify the scope and open a new one.",
                    self.config.approval_wait_timeout_ms
                ),
            }));
            return effects;
        }

        if self.state == RunState::Running
            || self.state == RunState::Compacting && self.compaction.pending.is_some()
        {
            if self.state == RunState::Compacting
                && self
                    .model_request_since_wall_ms
                    .is_some_and(|since| wall_ms < since)
            {
                return self.terminate(RunOutcome::Failed {
                    code: "kernel.compaction_clock_regressed".into(),
                    message: "Compaction clock moved backwards; request was not repeated.".into(),
                });
            }
            if let Some(since) = self.running_since_mono_ms.take() {
                self.running_elapsed_ms += (monotonic_ms - since).max(0);
                self.running_since_mono_ms = Some(monotonic_ms);
            } else {
                // Rehydrate cannot compare the previous process's monotonic
                // value. The first tick establishes the new local anchor for the
                // RUNNING budget; the model-request timeout deliberately keeps
                // its monotonic anchor None after rehydrate so it measures
                // against the PERSISTED WALL anchor (a crash must not grant a
                // fresh full model-request window).
                self.running_since_mono_ms = Some(monotonic_ms);
            }
            if self.config.run_execution_limited && self.running_elapsed_ms >= self.config.run_execution_budget_ms {
                return self.terminate(RunOutcome::BudgetExhausted {
                    code: "runtime.duration_budget_exceeded".into(),
                    message: format!(
                        "Run exceeded its {}ms execution budget.",
                        self.config.run_execution_budget_ms
                    ),
                });
            }
            // Model-request timeout is armed ONLY by an explicit
            // begin_model_request (turn dispatch) and disarmed on
            // settle_model_request (first output / tool batch / terminal). It is
            // NOT inferred from "no tool in flight", which would misfire after a
            // tool settles while a durable batch delivery is still pending. It
            // never arms during an approval wait (separate wall deadline). The
            // timeout uses the persisted WALL anchor so a crash cannot reset it
            // to a fresh full window; within one process the monotonic anchor
            // measures the elapsed time.
            if check_model && self.model_request_in_flight {
                // Same-process request: monotonic anchors measure elapsed time.
                // Rehydrated after a crash: monotonic anchors are gone; fall
                // back to the persisted wall anchor elapsed time so the
                // pre-crash wait still counts against every bound.
                let total_elapsed = match self.model_request_since_mono_ms {
                    Some(since) => monotonic_ms.saturating_sub(since),
                    None => self
                        .model_request_since_wall_ms
                        .map(|since| wall_ms.saturating_sub(since))
                        .unwrap_or(0),
                };
                let has_output = self.model_request_first_output_mono_ms.is_some()
                    || self.model_request_last_progress_wall_ms.is_some();
                // Idleness is measured from the last observed progress; before
                // any progress it equals the whole wait (conservative across
                // restarts, where volatile progress anchors are None).
                let idle_elapsed = match (
                    self.model_request_last_progress_mono_ms,
                    self.model_request_last_progress_wall_ms,
                ) {
                    (Some(since), _) => monotonic_ms.saturating_sub(since),
                    (None, Some(since)) => wall_ms.saturating_sub(since),
                    (None, None) => total_elapsed,
                };
                let first_elapsed = match self.model_request_first_output_mono_ms {
                    Some(_) => 0,
                    None => total_elapsed,
                };
                let breach = if !has_output
                    && first_elapsed >= self.config.model_first_response_ms
                {
                    Some(("model.first_response_timeout",
                        format!("Model produced no output within its {}ms first-response budget (waited {}ms).",
                            self.config.model_first_response_ms, first_elapsed)))
                } else if has_output && idle_elapsed >= self.config.model_idle_ms {
                    Some(("model.idle_timeout",
                        format!("Model produced no text, thinking, or tool-parameter progress for {}ms (idle budget {}ms, round elapsed {}ms).",
                            idle_elapsed, self.config.model_idle_ms, total_elapsed)))
                } else if total_elapsed >= self.config.model_request_timeout_ms {
                    Some(("model.request_timeout",
                        format!("Model request exceeded its {}ms whole-round timeout (elapsed {}ms, idle {}ms).",
                            self.config.model_request_timeout_ms, total_elapsed, idle_elapsed)))
                } else {
                    None
                };
                if let Some((code, message)) = breach {
                    // The Host-side bound is a backstop for a worker that can
                    // no longer report (crash/lost stream). Without the
                    // dispatch lease owner the delivery cannot be safely
                    // reset, so this fails closed; the live retry path is
                    // driven by the worker's own settled model_timeout
                    // evidence through the coordinator.
                    self.settle_model_request();
                    return self.terminate(RunOutcome::Failed {
                        code: code.into(),
                        message,
                    });
                }
            }
        }
        Vec::new()
    }

    fn expire_running_tools(&mut self, monotonic_ms: i64) -> Vec<Effect> {
        let timed_out = self
            .tools
            .values()
            .filter(|tool| {
                tool.state == ToolCallState::Running
                    && tool.started_at_mono_ms.is_some_and(|started| {
                        monotonic_ms.saturating_sub(started)
                            >= self.config.tool_execution_timeout_ms
                    })
            })
            .map(|tool| tool.tool_call_id.clone())
            .collect::<Vec<_>>();
        let mut effects = Vec::new();
        for id in timed_out {
            if let Some(tool) = self.tools.get_mut(&id) {
                tool.state = ToolCallState::Failed;
                tool.started_at_mono_ms = None;
            }
            effects.push(Effect::CancelToolCall {
                tool_call_id: id.clone(),
            });
            effects.push(Effect::ToolDispatchSettled {
                tool_call_id: id.clone(),
            });
            effects.push(self.append_event(
                "tool.failed",
                serde_json::json!({
                    "toolCallId": id,
                    "code": "tool.execution_timeout",
                    "message": format!("Tool exceeded its {}ms execution timeout.", self.config.tool_execution_timeout_ms),
                }),
            ));
        }
        effects
    }

    /// Set the persisted wall approval deadline when entering waiting_approval.
    /// Called by the adapter with the wall time at decision commit.
    pub fn set_approval_deadline(&mut self, deadline_wall_ms: i64) {
        self.approval_deadline_wall_ms = Some(deadline_wall_ms);
    }

    /// Begin waiting on a model request (turn dispatch). Arms the explicit
    /// model-request timeout; the wall anchor is persisted so a crash does not
    /// reset the timeout window.
    pub fn begin_model_request(&mut self, monotonic_ms: i64, wall_ms: i64) {
        self.arm_model_request(monotonic_ms, wall_ms);
    }

    /// The host must persist this decision together with its batch-delivery
    /// lease before invoking the engine. Never renew an in-flight request.
    pub fn begin_batch_model_request(
        &mut self,
        batch_id: &str,
        monotonic_ms: i64,
        wall_ms: i64,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Running
            || self.model_request_in_flight
            || !self
                .batches
                .iter()
                .any(|batch| batch.batch_id == batch_id && batch.barrier_emitted)
        {
            return Err(KernelError::FailClosed(
                "model dispatch requires a settled batch and no in-flight request".into(),
            ));
        }
        self.arm_model_request(monotonic_ms, wall_ms);
        Ok(vec![self.append_event(
            "engine.batch_dispatched",
            serde_json::json!({
                "batchId": batch_id, "turnId": self.turn_id, "engineId": self.config.engine_id,
                "idempotencyKey": batch_delivery_idempotency_key(batch_id), "startedAt": wall_ms,
            }),
        )])
    }

    pub fn record_batch_model_response(
        &mut self,
        batch_id: &str,
        response_json: &str,
    ) -> Result<Effect, KernelError> {
        if self.state != RunState::Running
            || !self.model_request_in_flight
            || !self
                .batches
                .iter()
                .any(|batch| batch.batch_id == batch_id && batch.barrier_emitted)
            || response_json.len() > 1_048_576
        {
            return Err(KernelError::FailClosed(
                "model response has no active batch request".into(),
            ));
        }
        let response: serde_json::Value = serde_json::from_str(response_json)
            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
        if !response.is_object() {
            return Err(KernelError::FailClosed(
                "model response must be an object".into(),
            ));
        }
        self.settle_model_request();
        Ok(self.append_event("engine.batch_response", serde_json::json!({
            "batchId": batch_id, "turnId": self.turn_id, "engineId": self.config.engine_id, "response": response,
        })))
    }

    /// Commit a Host-authored continuation and its exact input with the preceding
    /// response. Only the model view is stored; this never authorizes tool execution.
    pub fn request_continuation(&mut self, prompt: &str, input_json: &str) -> Result<Vec<Effect>, KernelError> {
        self.request_host_followup(prompt, input_json, None)
    }

    /// Same durable transport as [`RunController::request_continuation`], but the
    /// event/outbox payload carries `lane:"delivery_repair"`. Host counters treat
    /// the two lanes independently: business repair rounds never consume the
    /// bounded stop-review budget, and neither lane authorizes tool execution.
    pub fn request_delivery_repair(
        &mut self,
        prompt: &str,
        input_json: &str,
    ) -> Result<Vec<Effect>, KernelError> {
        self.request_host_followup(prompt, input_json, Some("delivery_repair"))
    }

    /// Same durable transport as [`RunController::request_continuation`], but the
    /// event/outbox payload carries `lane:"steering"`. This lane answers
    /// additional user input that the Host accepted during the Run: it is new
    /// user work, not a review of the model's own stop, so it must never consume
    /// the bounded stop-review budget. It authorizes no tool execution either —
    /// the frozen Run scope still decides what may run.
    pub fn request_steering_followup(
        &mut self,
        prompt: &str,
        input_json: &str,
    ) -> Result<Vec<Effect>, KernelError> {
        self.request_host_followup(prompt, input_json, Some("steering"))
    }

    fn request_host_followup(
        &mut self,
        prompt: &str,
        input_json: &str,
        lane: Option<&str>,
    ) -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Running || self.model_request_in_flight
            || self.tools.values().any(|tool| !tool.state.is_terminal())
            || prompt.is_empty() || prompt.len() > 16_384 || input_json.len() > 1_048_576 {
            return Err(KernelError::FailClosed("invalid continuation input or unsettled execution".into()));
        }
        let input: serde_json::Value = serde_json::from_str(input_json)
            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
        if input["runId"] != self.run_id || input["turnId"] != self.turn_id
            || input["promptConfigHash"] != self.config.prompt_config_hash {
            return Err(KernelError::FailClosed("continuation input identity changed".into()));
        }
        let effect_key = format!("continuation:{}", self.last_event_seq() + 1);
        let mut payload = serde_json::json!({"turnId":self.turn_id,"effectKey":effect_key,
            "prompt":prompt,"input":input});
        if let Some(lane) = lane {
            payload["lane"] = serde_json::Value::String(lane.to_owned());
        }
        let payload_json = payload.to_string();
        let event = self.append_event("engine.continuation_requested",
            serde_json::from_str(&payload_json).unwrap());
        Ok(vec![event, Effect::RequestContinuationModel { effect_key, payload_json }])
    }

    pub fn begin_continuation_model_request(&mut self, effect_key: &str, monotonic_ms: i64, wall_ms: i64)
        -> Result<Vec<Effect>, KernelError> {
        if self.state != RunState::Running || self.model_request_in_flight
            || self.tools.values().any(|tool| !tool.state.is_terminal())
            || !effect_key.starts_with("continuation:") {
            return Err(KernelError::FailClosed("continuation cannot dispatch with unresolved work".into()));
        }
        self.arm_model_request(monotonic_ms, wall_ms);
        Ok(vec![self.append_event("engine.continuation_dispatched", serde_json::json!({
            "turnId":self.turn_id,"effectKey":effect_key,"startedAt":wall_ms,
        }))])
    }

    /// Record the model output that answers a continuation round. The caller
    /// decides the next step (batch proposal, another continuation, terminal)
    /// in the same transaction, settling only its exclusively leased model request.
    pub fn record_continuation_model_response(
        &mut self,
        response_json: &str,
    ) -> Result<Effect, KernelError> {
        if self.state != RunState::Running || !self.model_request_in_flight
            || response_json.len() > 1_048_576
        {
            return Err(KernelError::FailClosed(
                "continuation response requires an active model request".into(),
            ));
        }
        let response: serde_json::Value = serde_json::from_str(response_json)
            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
        if !response.is_object() {
            return Err(KernelError::FailClosed(
                "continuation response must be an object".into(),
            ));
        }
        self.settle_model_request();
        Ok(self.append_event(
            "engine.continuation_response",
            serde_json::json!({"turnId": self.turn_id, "response": response}),
        ))
    }

    /// Settle the in-flight model request (first output / tool batch / terminal).
    /// Disarms the model-request timeout; the tool-execution timeout governs
    /// any dispatched tools instead.
    pub fn settle_model_request(&mut self) {
        self.model_request_in_flight = false;
        self.model_request_since_mono_ms = None;
        self.model_request_since_wall_ms = None;
        self.model_request_first_output_mono_ms = None;
        self.model_request_last_progress_mono_ms = None;
        self.model_request_last_progress_wall_ms = None;
    }

    pub fn model_request_in_flight(&self) -> bool {
        self.model_request_in_flight
    }

    /// Record worker-observed output progress (streaming preview, settled
    /// tool-parameter bytes, or any Host-verified model output) for the
    /// in-flight request. Drives the first-response/idle bounds; a no-op when
    /// no request is in flight. Volatile: never persisted, so a restart
    /// conservatively measures idleness from the persisted wall anchor until
    /// fresh progress arrives. Host must only call this for output that
    /// belongs to the current dispatch (previews are revision/cursor-gated).
    pub fn note_model_progress(&mut self, monotonic_ms: i64, wall_ms: i64) {
        if !self.model_request_in_flight {
            return;
        }
        self.model_request_first_output_mono_ms
            .get_or_insert(monotonic_ms);
        self.model_request_last_progress_mono_ms = Some(monotonic_ms);
        self.model_request_last_progress_wall_ms = Some(wall_ms);
    }

    fn arm_model_request(&mut self, monotonic_ms: i64, wall_ms: i64) {
        self.retry.model_dispatch_pending = false;
        self.model_request_in_flight = true;
        self.model_request_since_mono_ms = Some(monotonic_ms);
        self.model_request_since_wall_ms = Some(wall_ms);
        self.model_request_first_output_mono_ms = None;
        self.model_request_last_progress_mono_ms = None;
        self.model_request_last_progress_wall_ms = None;
    }

    fn suspend_running_clock(&mut self, now_monotonic_ms: i64) {
        if let Some(since) = self.running_since_mono_ms.take() {
            self.running_elapsed_ms += (now_monotonic_ms - since).max(0);
        }
    }

    fn ensure_live(&self) -> Result<(), KernelError> {
        if self.state.is_terminal() {
            return Err(KernelError::Terminal {
                current: self.state.as_str(),
            });
        }
        if self.state == RunState::Cancelling {
            return Err(KernelError::Terminal {
                current: self.state.as_str(),
            });
        }
        Ok(())
    }

    fn maybe_barrier_effects(&mut self) -> Vec<Effect> {
        let mut effects = Vec::new();
        for batch in &mut self.batches {
            if batch.barrier_emitted {
                continue;
            }
            let all_settled = batch.ordered.iter().all(|id| {
                self.tools
                    .get(id)
                    .map(|t| t.state.is_terminal())
                    .unwrap_or(false)
            });
            if all_settled {
                let mut ordered = batch.ordered.clone();
                ordered.sort_by_key(|id| self.tools[id].source_order);
                effects.push(Effect::BatchBarrier {
                    batch_id: batch.batch_id.clone(),
                    ordered_tool_call_ids: ordered,
                });
                batch.barrier_emitted = true;
            }
        }
        effects
    }
}

/// Persist a sequence of events through an adapter, stopping on the first error.
pub fn persist_events(
    store: &dyn EventStorePort,
    run_id: &str,
    effects: &[Effect],
) -> Result<(), String> {
    for effect in effects {
        if let Effect::AppendEvent {
            seq,
            event_type,
            payload_json,
        } = effect
        {
            store.append_event(run_id, *seq, event_type, payload_json)?;
        }
    }
    Ok(())
}

/// One durable event to append in the decision transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistEvent {
    pub seq: u64,
    pub event_type: String,
    pub payload_json: String,
}

/// One tool call's durable state to upsert in the decision transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistTool {
    pub tool_call_id: String,
    pub batch_id: String,
    pub tool: String,
    pub canonical_input_json: String,
    pub source_order: usize,
    pub state: ToolCallState,
    pub result_json: Option<String>,
    pub dispatch_idempotency_key: Option<String>,
}

/// One batch identity and monotonic barrier state to persist atomically.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistBatch {
    pub batch_id: String,
    pub ordered_tool_call_ids: Vec<String>,
    pub barrier_emitted: bool,
}

/// One approval CAS performed inside the decision transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistApprovalResolution {
    pub tool_call_id: String,
    pub state: String,
}

/// One external side effect to enqueue durably before execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistOutboxEffect {
    pub effect_key: String,
    pub kind: crate::ports::OutboxEffectKind,
    pub idempotency_key: String,
    pub tool_call_id: Option<String>,
    pub batch_id: Option<String>,
    pub payload_json: String,
}

/// The complete durable write-set for one decision. The repository commits this
/// in a single transaction BEFORE any external executor runs, so events, run/tool
/// state, approvals and pending side effects are all-or-nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelPersistCommand {
    pub run_state: RunState,
    pub terminal_written: bool,
    pub running_elapsed_ms: i64,
    pub approval_deadline_wall_ms: Option<i64>,
    pub wait_deadline_wall_ms: Option<i64>,
    pub wait_accounted_until_wall_ms: Option<i64>,
    /// Persisted wall anchor of an in-flight model request; None when settled.
    pub model_request_since_wall_ms: Option<i64>,
    pub turn_id: String,
    pub retry: crate::ports::RetryState,
    pub compaction: crate::ports::CompactionState,
    pub events: Vec<PersistEvent>,
    pub batches: Vec<PersistBatch>,
    pub tools: Vec<PersistTool>,
    pub outbox: Vec<PersistOutboxEffect>,
    pub approval_resolutions: Vec<PersistApprovalResolution>,
    pub settled_dispatch_tool_call_ids: Vec<String>,
}

impl RunController {
    /// Build the durable write-set for the effects just produced. Derived purely
    /// from controller state, so the adapter commits one transaction and only
    /// then performs the returned external effects via the outbox.
    pub fn persist_command(&self, effects: &[Effect]) -> KernelPersistCommand {
        let mut events = Vec::new();
        let mut outbox = Vec::new();
        let mut approval_resolutions = Vec::new();
        let mut settled_dispatch_tool_call_ids = Vec::new();
        for effect in effects {
            match effect {
                Effect::RequestContinuationModel { effect_key, payload_json } => outbox.push(PersistOutboxEffect {
                    effect_key: effect_key.clone(), kind: crate::ports::OutboxEffectKind::ContinuationModel,
                    idempotency_key: format!("continuation-delivery:{effect_key}"), tool_call_id: None,
                    batch_id: None, payload_json: payload_json.clone(),
                }),
                Effect::RequestInitialModel { input_hash } => outbox.push(PersistOutboxEffect {
                    effect_key: INITIAL_MODEL_EFFECT_KEY.into(), kind: crate::ports::OutboxEffectKind::InitialModel,
                    idempotency_key: INITIAL_MODEL_IDEMPOTENCY_KEY.into(), tool_call_id: None, batch_id: None,
                    payload_json: serde_json::json!({"turnId":self.turn_id,"inputHash":input_hash}).to_string(),
                }),
                Effect::AppendEvent {
                    seq,
                    event_type,
                    payload_json,
                } => events.push(PersistEvent {
                    seq: *seq,
                    event_type: event_type.clone(),
                    payload_json: payload_json.clone(),
                }),
                Effect::DispatchTool {
                    tool_call_id,
                    tool,
                    input_json,
                } => outbox.push(PersistOutboxEffect {
                    effect_key: dispatch_effect_key(tool_call_id),
                    kind: crate::ports::OutboxEffectKind::DispatchTool,
                    idempotency_key: dispatch_idempotency_key(&self.run_id, tool_call_id),
                    tool_call_id: Some(tool_call_id.clone()),
                    batch_id: self.tools.get(tool_call_id).map(|t| t.batch_id.clone()),
                    payload_json: serde_json::json!({
                        "tool": tool,
                        "input": serde_json::from_str::<serde_json::Value>(input_json)
                            .expect("controller tool input invariant")
                    })
                    .to_string(),
                }),
                Effect::RequestApproval {
                    tool_call_id,
                    tool,
                    input_json,
                } => outbox.push(PersistOutboxEffect {
                    effect_key: approval_effect_key(tool_call_id),
                    kind: crate::ports::OutboxEffectKind::RequestApproval,
                    idempotency_key: approval_effect_key(tool_call_id),
                    tool_call_id: Some(tool_call_id.clone()),
                    batch_id: self.tools.get(tool_call_id).map(|t| t.batch_id.clone()),
                    payload_json: serde_json::json!({
                        "tool": tool,
                        "input": serde_json::from_str::<serde_json::Value>(input_json)
                            .expect("controller tool input invariant")
                    })
                    .to_string(),
                }),
                Effect::CancelEngineTurn { turn_id } => outbox.push(PersistOutboxEffect {
                    effect_key: format!("cancel-engine:{turn_id}"),
                    kind: crate::ports::OutboxEffectKind::CancelEngineTurn,
                    idempotency_key: format!("cancel-engine:{turn_id}"),
                    tool_call_id: None,
                    batch_id: None,
                    payload_json: serde_json::json!({ "turnId": turn_id }).to_string(),
                }),
                Effect::CancelToolCall { tool_call_id } => outbox.push(PersistOutboxEffect {
                    effect_key: format!("cancel-tool:{tool_call_id}"),
                    kind: crate::ports::OutboxEffectKind::CancelToolCall,
                    idempotency_key: format!("cancel-tool:{tool_call_id}"),
                    tool_call_id: Some(tool_call_id.clone()),
                    batch_id: None,
                    payload_json: "{}".to_string(),
                }),
                Effect::ApprovalResolved {
                    tool_call_id,
                    decision,
                } => approval_resolutions.push(PersistApprovalResolution {
                    tool_call_id: tool_call_id.clone(),
                    state: decision.as_str().to_string(),
                }),
                Effect::ApprovalExpired { tool_call_id } => {
                    approval_resolutions.push(PersistApprovalResolution {
                        tool_call_id: tool_call_id.clone(),
                        state: "expired".to_string(),
                    });
                }
                Effect::ApprovalCancelled { tool_call_id } => {
                    approval_resolutions.push(PersistApprovalResolution {
                        tool_call_id: tool_call_id.clone(),
                        state: "cancelled".to_string(),
                    });
                }
                Effect::ToolDispatchSettled { tool_call_id } => {
                    settled_dispatch_tool_call_ids.push(tool_call_id.clone());
                }
                Effect::BatchBarrier {
                    batch_id,
                    ordered_tool_call_ids,
                } => outbox.push(PersistOutboxEffect {
                    effect_key: batch_delivery_effect_key(batch_id),
                    kind: crate::ports::OutboxEffectKind::DeliverToolBatch,
                    idempotency_key: batch_delivery_idempotency_key(batch_id),
                    tool_call_id: None,
                    batch_id: Some(batch_id.clone()),
                    payload_json: serde_json::json!({
                        "orderedToolCallIds": ordered_tool_call_ids
                    })
                    .to_string(),
                }),
                Effect::PublishSnapshot | Effect::Audit { .. } => {}
            }
        }
        let batches = self
            .batches
            .iter()
            .map(|batch| PersistBatch {
                batch_id: batch.batch_id.clone(),
                ordered_tool_call_ids: batch.ordered.clone(),
                barrier_emitted: batch.barrier_emitted,
            })
            .collect();
        let tools = self
            .tools
            .values()
            .map(|tool| PersistTool {
                tool_call_id: tool.tool_call_id.clone(),
                batch_id: tool.batch_id.clone(),
                tool: tool.tool.clone(),
                canonical_input_json: tool.input_json.clone(),
                source_order: tool.source_order,
                state: tool.state,
                result_json: tool.result_json.clone(),
                dispatch_idempotency_key: (tool.state == ToolCallState::Running)
                    .then(|| dispatch_idempotency_key(&self.run_id, &tool.tool_call_id)),
            })
            .collect();
        KernelPersistCommand {
            run_state: self.state,
            terminal_written: self.terminal_written,
            running_elapsed_ms: self.running_elapsed_ms,
            approval_deadline_wall_ms: self.approval_deadline_wall_ms,
            wait_deadline_wall_ms: self.wait_deadline_wall_ms,
            wait_accounted_until_wall_ms: self.wait_accounted_until_wall_ms,
            model_request_since_wall_ms: self.model_request_since_wall_ms,
            turn_id: self.turn_id.clone(),
            retry: self.retry.clone(),
            compaction: self.compaction.clone(),
            events,
            batches,
            tools,
            outbox,
            approval_resolutions,
            settled_dispatch_tool_call_ids,
        }
    }
}

#[cfg(test)]
mod reevaluate_tests {
    use super::*;
    fn waiting() -> RunController {
        let clock = crate::TestClock::new(0);
        let (mut controller, _) = RunController::start("reeval-run", "turn", crate::test_config(), &clock).unwrap();
        controller.propose_tool_batch("batch", vec![ToolCallRequest {
            tool_call_id: "same-intent".into(), tool: "write_file".into(),
            canonical_input_json: "{}".into(), source_order: 0,
        }], &crate::TestPolicy { allow: vec![], deny: vec![] }, 0, 0).unwrap();
        controller
    }
    #[test]
    fn reevaluation_preserves_intent_and_dispatches_only_once() {
        let mut controller = waiting();
        let policy = crate::TestPolicy { allow: vec!["write_file"], deny: vec![] };
        let effects = controller.reevaluate_tool(&policy, "same-intent", 2, 10, 10).unwrap();
        assert_eq!(controller.tools.len(), 1);
        assert_eq!(controller.batches.len(), 1);
        assert_eq!(controller.tool_call_state("same-intent"), Some(ToolCallState::Running));
        assert!(effects.iter().any(|effect| matches!(effect, Effect::DispatchTool { tool_call_id, .. } if tool_call_id == "same-intent")));
        let command = controller.persist_command(&effects);
        let event = command.events.iter().find(|event| event.event_type == "tool.reevaluated").unwrap();
        assert_eq!(serde_json::from_str::<serde_json::Value>(&event.payload_json).unwrap()["expectedPolicyVersion"], 2);
        assert!(controller.reevaluate_tool(&policy, "same-intent", 3, 20, 20).is_err());
    }
    #[test]
    fn reevaluation_replaces_approval_or_refuses_without_new_tool() {
        let mut controller = waiting();
        let ask = crate::TestPolicy { allow: vec![], deny: vec![] };
        let effects = controller.reevaluate_tool(&ask, "same-intent", 2, 10, 10).unwrap();
        assert!(effects.iter().any(|effect| matches!(effect, Effect::RequestApproval { tool_call_id, .. } if tool_call_id == "same-intent")));
        assert_eq!(controller.state(), RunState::WaitingApproval);
        let deny = crate::TestPolicy { allow: vec![], deny: vec!["write_file"] };
        let effects = controller.reevaluate_tool(&deny, "same-intent", 3, 20, 20).unwrap();
        assert!(!effects.iter().any(|effect| matches!(effect, Effect::DispatchTool { .. })));
        assert_eq!(controller.tool_call_state("same-intent"), Some(ToolCallState::Failed));
        assert_eq!(controller.tools.len(), 1);
        assert!(controller.reevaluate_tool(&ask, "same-intent", 4, 30, 30).is_err());
    }
}


#[cfg(test)]
mod dispatch_identity_tests {
    #[test]
    fn dispatch_identity_uses_utf8_byte_lengths_and_preserves_colons() {
        assert_eq!(super::dispatch_idempotency_key("运行:a","call:b"),"tool-dispatch:8:运行:a:call:b");
        assert_ne!(super::dispatch_idempotency_key("a:b","c"),super::dispatch_idempotency_key("a","b:c"));
    }
}
