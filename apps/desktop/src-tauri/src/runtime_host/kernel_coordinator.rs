//! Durable coordinator for opt-in authoritative RuntimeHost execution.
//! Default Legacy mode and production rollout gates remain separate.
//! The executor receives a frozen binding and must revalidate resources itself.
use crate::{
    database::Database,
    kernel::{
        self, CancellationPort, CancellationRegistry, Clock, ClockReading, Effect, KernelError,
        PolicyDecisionPort, RunController, ToolCallRequest,
    },
};
use fox_engine_protocol::{ExecutionAuthority, RunControlBinding};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

mod compaction;
pub(super) mod live;
mod receipt;
mod steering;

/// Returned by a model transport when a terminal decision lost a race with a
/// mid-run request that was accepted moments earlier.
///
/// It is a scheduling outcome, not an execution failure: the drive loop
/// re-plans the Run from durable facts, where the freshly accepted row is
/// visible and becomes an ordinary additional-input round. Reported as a plain
/// error it would turn a normal user action into a task failure.
pub(crate) const STEERING_REPLAN: &str = "kernel.steering_replan";

/// Translate a lost terminal race into the re-plan signal.
///
/// The decision write-set rolled back, so nothing is committed and no engine
/// work was consumed; re-entering the drive loop re-reads durable facts (where
/// the freshly accepted request is still `received`) and plans an additional
/// input round for it.
/// Normalize a decision error: a lost terminal race becomes the re-plan signal,
/// everything else is passed through unchanged.
pub(crate) fn replan_or_error(error: String) -> String {
    if error.starts_with(kernel::STEERING_COMPETITION) {
        STEERING_REPLAN.to_string()
    } else {
        error
    }
}

fn checkpoint_hash(value: &Value) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(value.to_string().as_bytes()))
    )
}

/// Rebuild the pre-response transcript from the very frame leased to Node.
/// The caller may not invent a middle segment or a historyStart for a park.
pub(crate) fn bound_model_delivery_history(
    run_id: &str,
    payload: &Value,
    live_history: Option<&[Value]>,
) -> Result<(Vec<Value>, usize), String> {
    if let Some(history) = live_history {
        let directive: fox_engine_protocol::KernelRoundDirective =
            serde_json::from_value(payload.clone()).map_err(|_| "invalid bound live directive")?;
        directive.validate()?;
        let mut delivered = history.to_vec();
        let start = delivered.len();
        delivered.extend(directive.host_job_notices.iter()
            .map(fox_engine_protocol::HostJobNotice::history_marker));
        return Ok((delivered,start));
    }
    if payload.get("input").is_some() {
        let frame: fox_engine_protocol::KernelInitialModelFrame =
            serde_json::from_value(payload.clone()).map_err(|_| "invalid bound initial frame")?;
        frame.validate()?;
        let mut delivered = frame.input.messages;
        let start = delivered.len();
        delivered.extend(frame.host_job_notices.iter()
            .map(fox_engine_protocol::HostJobNotice::history_marker));
        return Ok((delivered,start));
    }
    let frame: fox_engine_protocol::KernelBatchResumeFrame =
        serde_json::from_value(payload.clone()).map_err(|_| "invalid bound batch frame")?;
    frame.validate()?;
    let mut delivered = frame.history;
    delivered.push(frame.assistant_message.clone());
    delivered.extend(steering::settled_tool_result_messages(
        run_id, &frame.tools, &frame.assistant_message));
    delivered.extend(frame.steering.iter().map(|notice| serde_json::json!({
        "role":"user","content":[{"type":"text",
            "text":steering::steering_notice_text(&notice.content)}],
        "timestamp":notice.received_at.unwrap_or(0),
    })));
    let start = delivered.len();
    delivered.extend(frame.host_job_notices.iter()
        .map(fox_engine_protocol::HostJobNotice::history_marker));
    Ok((delivered,start))
}

#[cfg(test)]
mod notice_lease_test_barrier {
    use std::collections::HashMap;
    use std::sync::{Mutex,OnceLock};
    type Hook=Box<dyn Fn() + Send + 'static>;
    fn hooks()->&'static Mutex<HashMap<String,Hook>> {
        static HOOKS:OnceLock<Mutex<HashMap<String,Hook>>>=OnceLock::new();
        HOOKS.get_or_init(||Mutex::new(HashMap::new()))
    }
    pub(super) fn install(run:&str,hook:Hook) {
        hooks().lock().expect("notice barrier lock").insert(run.to_owned(),hook);
    }
    pub(super) fn fire(run:&str) {
        let hook=hooks().lock().expect("notice barrier lock").remove(run);
        if let Some(hook)=hook { hook(); }
    }
}

enum DecisionLease<'a> {
    Approval(u64),
    ModelRetry(&'a str, &'a str),
    Initial(&'a str),
    Continuation(&'a str, &'a str),
    ContinuationDispatch(&'a str, &'a str, Option<&'a crate::database::ModelNoticeInput<'a>>),
    Tool(&'a str, &'a str),
    Batch(&'a str, &'a str),
    /// Claim a settled batch's pending delivery and arm its model request.
    BatchDispatch(&'a str, &'a str, Option<&'a crate::database::ModelNoticeInput<'a>>),
}

pub(crate) struct KernelCoordinator<'a> {
    database: &'a Database,
    clock: &'a dyn Clock,
    cancellation: &'a CancellationRegistry,
    binding: RunControlBinding,
    controller: Mutex<RunController>,
    preview: Option<&'a super::kernel_model_worker::PreviewSink>,
}

impl<'a> KernelCoordinator<'a> {
    fn unfinished_compute_wait_facts(&self) -> Result<Vec<kernel::WaitingJobFact>, String> {
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(Vec::new());
        }
        self.database.kernel_waiting_job_facts(
            &self.binding.conversation_id, &self.binding.run_id)
    }

    fn pending_host_job_notices(
        &self, history: &[Value],
    ) -> Result<Vec<fox_engine_protocol::HostJobNotice>, String> {
        self.database.validate_host_job_notice_history(&self.binding.conversation_id,
            &self.binding.run_id, history)?;
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(Vec::new());
        }
        let historical = fox_engine_protocol::historical_host_job_notice_bytes(history)?;
        self.database.pending_host_job_notices(&self.binding.conversation_id,
            &self.binding.run_id,(16 * 1024usize).saturating_sub(historical))
    }

    /// Both direct completion and a durable wake resume from the assistant
    /// response the model actually produced. Notices remain a typed Host lane,
    /// never a fabricated user turn or a second tool result.
    fn job_notice_followup_input(
        &self, pre_history: &[Value], assistant: &Value,
    ) -> Result<Option<(fox_engine_protocol::KernelInitialModelInput,
        Vec<fox_engine_protocol::HostJobNotice>)>, String> {
        if !self.database.compute_job_notice_enabled(&self.binding.run_id)? {
            return Ok(None);
        }
        let mut messages = pre_history.to_vec();
        messages.push(assistant.clone());
        let notices = self.pending_host_job_notices(&messages)?;
        if notices.is_empty() { return Ok(None); }
        let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages = messages;
        input.validate_job_notice()?;
        Ok(Some((input, notices)))
    }

    pub(crate) fn start_prepared(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        let (input, config, hash) = database.kernel_initial_start_state(run_id)?;
        let (controller, effects) =
            RunController::start_with_initial_input(run_id, &input.turn_id, config, &hash, clock)
                .map_err(|error| error.to_string())?;
        database.kernel_commit_decision(
            run_id,
            clock.now_wall_ms(),
            &controller.persist_command(&effects),
        )?;
        Self::reopen(database, clock, run_id, cancellation)
    }

    pub(crate) fn reopen(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        let binding = database
            .run_control_binding(run_id)?
            .ok_or("authoritative Run has no frozen control binding")?;
        let data = database
            .kernel_rehydrate(run_id)?
            .ok_or("authoritative Run has no durable Kernel state")?;
        let config = &data.config;
        if binding.authority != ExecutionAuthority::Authoritative
            || config.kernel_mode != "authoritative"
            || binding.engine_id != config.engine_id
            || binding.permission_snapshot_id != config.permission_snapshot_id
            || binding.execution_profile_id != config.execution_profile_id
            || binding.budgets.approval_wait_ms != config.approval_wait_timeout_ms
            || binding.budgets.model_request_ms != config.model_request_timeout_ms
            || binding.budgets.model_first_response_ms != config.model_first_response_ms
            || binding.budgets.model_idle_ms != config.model_idle_ms
            || binding.budgets.tool_execution_ms != config.tool_execution_timeout_ms
            || binding.budgets.run_execution_ms != config.run_execution_budget_ms
            || binding.budgets.run_execution_limited != config.run_execution_limited
        {
            return Err(
                "Kernel and resource control bindings disagree; no authority fallback is allowed"
                    .into(),
            );
        }
        let controller = RunController::rehydrate(data).map_err(|error| error.to_string())?;
        cancellation.register_run(run_id)?;
        if controller.is_terminal() || controller.state() == kernel::RunState::Cancelling {
            cancellation.request_run_cancel(run_id);
        }
        Ok(Self {
            database,
            clock,
            cancellation,
            binding,
            controller: Mutex::new(controller),
            preview: None,
        })
    }

    pub(super) fn with_preview(
        mut self,
        preview: &'a super::kernel_model_worker::PreviewSink,
    ) -> Self {
        self.preview = Some(preview);
        self
    }

    /// A failed decision transaction never leaves speculative state in memory.
    fn apply(
        &self,
        lease: Option<DecisionLease<'_>>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
    ) -> Result<(), String> {
        self.apply_decision(lease, action, None)
    }

    /// Same durable decision as [`Self::apply`], additionally transitioning
    /// mid-run steering rows inside the same write-set (the round response
    /// answers delivered rows; the directive built with it arms new ones).
    fn apply_decision(
        &self,
        lease: Option<DecisionLease<'_>>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
        steering: Option<&crate::database::SteeringDecision>,
    ) -> Result<(), String> {
        // Re-read the immutable, hash-verified resource binding on each entry.
        if self
            .database
            .run_control_binding(&self.binding.run_id)?
            .as_ref()
            != Some(&self.binding)
        {
            return Err("authoritative Run control binding changed".into());
        }
        let mut guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let mut candidate = guard.clone();
        let now = self.clock.read();
        let effects = action(&mut candidate, now).map_err(|error| error.to_string())?;
        let command = candidate.persist_command(&effects);
        match lease {
            Some(DecisionLease::Approval(version)) => self.database.kernel_commit_decision_with_approval_version(
                &self.binding.run_id, now.wall_ms, &command, version)?,
            Some(DecisionLease::ModelRetry(effect_key, owner)) => {
                self.database.kernel_commit_model_retry(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    effect_key,
                    owner,
                )?
            }
            Some(DecisionLease::Continuation(effect_key, owner)) => self.database.kernel_commit_continuation_model(
                &self.binding.run_id, now.wall_ms, &command, effect_key, owner, true, steering, None)?,
            Some(DecisionLease::ContinuationDispatch(effect_key, owner, input)) => self.database.kernel_commit_continuation_model(
                &self.binding.run_id, now.wall_ms, &command, effect_key, owner, false, None, input)?,
            Some(DecisionLease::Initial(owner)) => self.database.kernel_commit_initial_model(
                &self.binding.run_id,
                now.wall_ms,
                &command,
                owner,
                true,
                steering,
                None,
            )?,
            Some(DecisionLease::Tool(tool_call_id, owner)) => {
                self.database.kernel_commit_tool_result(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    tool_call_id,
                    owner,
                )?
            }
            Some(DecisionLease::Batch(batch_id, owner)) => {
                self.database.kernel_commit_batch_response(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    batch_id,
                    owner,
                    steering,
                )?
            }
            Some(DecisionLease::BatchDispatch(batch_id, owner, input)) => {
                self.database.kernel_commit_batch_dispatch(
                    &self.binding.run_id,
                    now.wall_ms,
                    &command,
                    batch_id,
                    owner,
                    input,
                )?
            }
            None => {
                self.database
                    .kernel_commit_decision(&self.binding.run_id, now.wall_ms, &command)?
            }
        }
        // Persist cancellation first, then signal every issued execution token.
        for effect in &effects {
            if let Effect::CancelToolCall { tool_call_id } = effect {
                self.cancellation
                    .request_tool_cancel(&self.binding.run_id, tool_call_id);
            }
        }
        if candidate.is_terminal() || candidate.state() == kernel::RunState::Cancelling {
            self.cancellation.request_run_cancel(&self.binding.run_id);
        }
        *guard = candidate;
        Ok(())
    }

    pub(crate) fn tick(&self) -> Result<(), String> {
        self.tick_budgets(false)
    }

    pub(crate) fn account_waiting_jobs_now(&self) -> Result<(), String> {
        let park_seq = self.database.kernel_waiting_park_seq(&self.binding.run_id)?;
        self.apply(None, |controller, now| {
            controller.account_waiting_jobs(park_seq, now.wall_ms)
        })
    }

    pub(crate) fn cancel_waiting_jobs_now(&self) -> Result<(), String> {
        let park_seq = self.database.kernel_waiting_park_seq(&self.binding.run_id)?;
        self.apply(None, |controller, now| {
            controller.cancel_waiting_jobs(park_seq, now.wall_ms)
        })
    }

    fn tick_settled_model(&self) -> Result<(), String> {
        self.tick_budgets(true)
    }

    fn tick_budgets(&self, model_settled: bool) -> Result<(), String> {
        // Fold pending worker-observed output progress into the controller so
        // the first-response/idle bounds measure real output. The registry is
        // consume-once: every preview counts exactly once, and ticks without
        // fresh output grow idleness. The wall-anchored signal is bridged into
        // the controller's monotonic domain by subtracting the observed idle
        // span from the current monotonic reading (production clocks advance
        // both domains together; unit tests drive note_model_progress
        // directly and never depend on this conversion).
        let progress = self
            .database
            .consume_kernel_model_progress(&self.binding.run_id);
        self.apply(None, |controller, now| {
            if let Some(progress_wall) = progress {
                let idle = now.wall_ms.saturating_sub(progress_wall).max(0);
                let mono = now.monotonic_ms.saturating_sub(idle).max(0);
                controller.note_model_progress(mono, progress_wall);
            }
            Ok(if model_settled { controller.tick_settled_model(now.monotonic_ms, now.wall_ms) }
               else { controller.tick(now.monotonic_ms, now.wall_ms) })
        })?;
        if self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .is_terminal()
        {
            self.database.clear_kernel_model_progress(&self.binding.run_id);
        }
        Ok(())
    }

    pub(crate) fn propose_tools(
        &self,
        batch_id: &str,
        calls: Vec<ToolCallRequest>,
        policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.propose_tool_batch(batch_id, calls, policy, now.monotonic_ms, now.wall_ms)
        })
    }

    pub(crate) fn propose_engine_batch(
        &self,
        checkpoint: fox_engine_protocol::KernelEngineBatchCheckpoint,
        policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        checkpoint.validate()?;
        if self
            .database
            .run_control_binding(&self.binding.run_id)?
            .as_ref()
            != Some(&self.binding)
        {
            return Err("authoritative Run control binding changed".into());
        }
        let value = serde_json::to_value(&checkpoint).map_err(|error| error.to_string())?;
        let stored = serde_json::json!({"value":value,"hash":checkpoint_hash(&value)});
        if let Some(existing) = self
            .database
            .kernel_engine_batch_checkpoint(&self.binding.run_id, &checkpoint.batch_id)?
        {
            if existing["checkpoint"] == stored
                && existing["engineId"].as_str() == Some(self.binding.engine_id.as_str())
            {
                return Ok(());
            }
            return Err("immutable engine checkpoint replay conflict".into());
        }
        let calls = checkpoint.assistant_message["content"]
            .as_array()
            .ok_or("missing engine proposal")?
            .iter()
            .filter(|block| block["type"] == "toolCall")
            .enumerate()
            .map(|(source_order, call)| ToolCallRequest {
                tool_call_id: call["id"].as_str().unwrap().into(),
                tool: call["name"].as_str().unwrap().into(),
                canonical_input_json: call["arguments"].to_string(),
                source_order,
            })
            .collect();
        self.tick()?;
        self.apply(None, |controller, now| {
            let mut effects = controller.propose_tool_batch(
                &checkpoint.batch_id,
                calls,
                policy,
                now.monotonic_ms,
                now.wall_ms,
            )?;
            effects
                .push(controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?);
            Ok(effects)
        })
    }

    pub(crate) fn prepare_stored_batch_resume(
        &self,
        batch_id: &str,
    ) -> Result<fox_engine_protocol::KernelBatchResumeFrame, String> {
        let event = self
            .database
            .kernel_engine_batch_checkpoint(&self.binding.run_id, batch_id)?
            .ok_or("original engine checkpoint is missing; it must not be regenerated")?;
        let value = &event["checkpoint"]["value"];
        if event["checkpoint"]["hash"].as_str() != Some(checkpoint_hash(value).as_str())
            || event["engineId"].as_str() != Some(self.binding.engine_id.as_str())
        {
            return Err("persisted engine checkpoint hash or engine identity mismatch".into());
        }
        let checkpoint: fox_engine_protocol::KernelEngineBatchCheckpoint =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        checkpoint.validate()?;
        if checkpoint.batch_id != batch_id {
            return Err("persisted engine checkpoint batch mismatch".into());
        }
        let history = self.model_retry_context(batch_id, self.context_view(batch_id, &checkpoint.history)?)?;
        // Replacement transport for a dead/compacting live session: re-collect
        // every active steering row (new rows plus rows already delivered to
        // the failed live attempt) under the batch delivery identity, so the
        // rebuilt dispatch carries each row exactly once.
        let steering_rows = self.database.deliver_steering_for_dispatch(
            &self.binding.run_id,
            &kernel::batch_delivery_idempotency_key(batch_id),
        )?;
        let notices = steering::steering_notices(&steering_rows);
        let frame = self.prepare_batch_resume(batch_id, history, checkpoint.assistant_message, notices)?;
        if event["turnId"].as_str() != Some(frame.turn_id.as_str()) {
            return Err("persisted engine checkpoint turn mismatch".into());
        }
        Ok(frame)
    }

    /// Reevaluate without a preliminary mutation transaction: superseding the
    /// old approval and installing the new decision are one durable write-set.
    /// Rehydration affects only apply()'s candidate; failure leaves memory intact.
    pub(crate) fn reevaluate_tool(
        &self, tool_call_id: &str, policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        let version = self.database.execution_policy(&self.binding.conversation_id)?.version;
        self.apply(Some(DecisionLease::Approval(version)), |controller, now| {
            let data = self.database.kernel_rehydrate(&self.binding.run_id)
                .map_err(KernelError::FailClosed)?
                .ok_or_else(|| KernelError::FailClosed("missing durable run for reevaluation".into()))?;
            *controller = RunController::rehydrate(data)?;
            controller.reevaluate_tool(policy, tool_call_id, version, now.monotonic_ms, now.wall_ms)
        })
    }

    pub(crate) fn resolve_approval(
        &self,
        tool_call_id: &str,
        decision: kernel::ApprovalDecision,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.resolve_approval(tool_call_id, decision, now.monotonic_ms)
        })
    }

    pub(crate) fn resolve_approval_at_version(
        &self, tool_call_id: &str, decision: kernel::ApprovalDecision, version: u64,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(Some(DecisionLease::Approval(version)), |controller, now| {
            controller.resolve_approval(tool_call_id, decision, now.monotonic_ms)
        })
    }

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.request_cancel()))
    }

    /// Host calls this only after its owned model/tool executors have stopped.
    pub(crate) fn settle_cancellation(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.settle_cancellation()))
    }

    pub(crate) fn fail(&self, code: &str, message: &str) -> Result<(), String> {
        self.apply(None, |controller, _| {
            Ok(controller.terminate(kernel::RunOutcome::Failed {
                code: code.into(),
                message: message.into(),
            }))
        })
    }

    pub(crate) fn snapshot(&self) -> Result<kernel::KernelSnapshot, String> {
        self.database
            .kernel_build_full_snapshot(&self.binding.run_id)
    }

    /// Project a committed result barrier for a replacement engine session.
    /// The caller supplies the original engine checkpoint, never fresh model
    /// output. This read does not claim or complete the batch-delivery outbox.
    fn prepare_batch_resume(
        &self,
        batch_id: &str,
        history: Vec<Value>,
        assistant_message: Value,
        steering: Vec<fox_engine_protocol::KernelSteeringNotice>,
    ) -> Result<fox_engine_protocol::KernelBatchResumeFrame, String> {
        use fox_engine_protocol::KernelBatchResumeFrame;
        self.tick()?;
        let guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let snapshot = self.snapshot()?;
        if guard.state() != kernel::RunState::Running
            || snapshot.last_event_seq != guard.last_event_seq()
        {
            return Err("batch resume requires a current running Kernel snapshot".into());
        }
        let data = guard.shadow_checkpoint(self.clock.now_monotonic_ms());
        let batch = data
            .batches
            .iter()
            .find(|batch| batch.batch_id == batch_id && batch.barrier_emitted)
            .ok_or("batch result barrier has not committed")?;
        let effect = snapshot.pending_effects.iter().find(|effect| effect.effect_key == kernel::batch_delivery_effect_key(batch_id)
            && effect.kind == kernel::OutboxEffectKind::DeliverToolBatch && effect.status == kernel::OutboxStatus::Pending)
            .ok_or("batch delivery is missing, claimed or already completed; reconcile instead of replaying")?;
        let payload: Value =
            serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
        if effect.batch_id.as_deref() != Some(batch_id)
            || effect.idempotency_key != kernel::batch_delivery_idempotency_key(batch_id)
            || payload["orderedToolCallIds"] != serde_json::json!(batch.ordered)
        {
            return Err("durable batch delivery identity mismatch".into());
        }
        let tools = self.project_settled_tools(&data, &snapshot, &batch.ordered)?;
        let frame = KernelBatchResumeFrame {
            schema_version: 1,
            turn_id: data.turn_id,
            batch_id: batch_id.into(),
            idempotency_key: effect.idempotency_key.clone(),
            checkpoint_seq: snapshot.last_event_seq,
            history,
            assistant_message,
            tools,
            steering,
            host_job_notices: Vec::new(),
        };
        frame.validate()?;
        Ok(frame)
    }

    /// Durable result projection shared by the replacement-session resume frame
    /// and live-session directives. Includes the execution receipt so the model
    /// sees Host-confirmed approval and execution facts either way.
    fn project_settled_tools(
        &self,
        data: &kernel::RehydratedRun,
        snapshot: &kernel::KernelSnapshot,
        ordered: &[String],
    ) -> Result<Vec<fox_engine_protocol::KernelSettledToolResult>, String> {
        use fox_engine_protocol::{KernelSettledToolResult, KernelSettledToolState};
        let mut tools = Vec::new();
        for id in ordered {
            let tool = data
                .tools
                .iter()
                .find(|tool| &tool.tool_call_id == id)
                .ok_or("batch tool fact is missing")?;
            let state = match tool.state {
                kernel::ToolCallState::Completed => KernelSettledToolState::Completed,
                kernel::ToolCallState::Failed => KernelSettledToolState::Failed,
                _ => return Err("unsettled or cancelled tool cannot resume the model".into()),
            };
            let raw: Value = match tool.result_json.as_deref() {
                Some(value) => serde_json::from_str(value).map_err(|error| error.to_string())?,
                // Policy denial is a durable failed fact, not an unknown result.
                None if state == KernelSettledToolState::Failed => {
                    serde_json::json!({"state":"failed","toolCallId":id,"outputRecorded":false})
                }
                None => return Err("completed tool has no durable result".into()),
            };
            let mut result = if raw.get("content").is_some() {
                raw
            } else {
                serde_json::json!({"content":[{"type":"text","text":raw.to_string()}]})
            };
            let canonical_input: Value =
                serde_json::from_str(&tool.input_json).map_err(|error| error.to_string())?;
            // The directive that carries this to the worker is a protocol frame
            // capped at 1 MiB, so the content sent must be the same *bounded*
            // projection the Host keeps for its own history — never the complete
            // durable payload. The durable row is untouched, and the reference
            // below is what makes an omitted byte reachable through
            // `read_tool_result`, so nothing is lost by not shipping it twice.
            let reference =
                crate::kernel_compaction::tool_result_ref(&self.binding.run_id, id);
            let storage = self
                .database
                .tool_result_storage(&self.binding.run_id, id)
                .unwrap_or_default();
            if state == KernelSettledToolState::Completed {
                if let Some(bounded) = crate::kernel_compaction::bound_tool_result_content_with_storage(
                    live::effective_boundable_tool(&tool.tool, &canonical_input),
                    false,
                    &result["content"],
                    reference.as_deref(),
                    &storage,
                ) {
                    result["content"] = bounded;
                }
            }
            let projected = snapshot.tool_calls.iter().find(|item| item.tool_call_id == *id)
                .ok_or("missing durable tool approval projection")?;
            receipt::append_execution_receipt(
                &mut result,
                &self.binding.run_id,
                id,
                &tool.tool,
                state == KernelSettledToolState::Completed,
                projected.approval_state.as_deref(),
                &canonical_input,
            )?;
            tools.push(KernelSettledToolResult {
                tool_call_id: id.clone(),
                tool: tool.tool.clone(),
                canonical_input,
                source_order: u32::try_from(tool.source_order)
                    .map_err(|error| error.to_string())?,
                state,
                storage: serde_json::to_value(self.database.tool_result_storage(&self.binding.run_id, id).unwrap_or_default()).ok(),
                result,
            });
        }
        Ok(tools)
    }

    pub(super) fn dispatch_initial_with_worker(
        &self,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand,
        api_key: &str,
    ) -> Result<(), String> {
        self.ensure_context_with_worker("initial", 4096, owner, runtime, api_key)?;
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        self.dispatch_initial(owner, policy, |binding, frame, token| {
            let now = self.clock.read();
            let facts = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?
                .shadow_checkpoint(now.monotonic_ms);
            let since = facts
                .model_request_since_wall_ms
                .ok_or("initial model deadline is missing")?;
            if now.wall_ms < since {
                return Err("initial model clock moved backwards".into());
            }
            let remaining = binding.budgets.limit_operation_ms(
                binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)),
                facts.running_elapsed_ms,
            );
            super::kernel_model_worker::deliver_initial_with_preview(
                runtime,
                &config,
                api_key,
                binding,
                frame,
                token,
                remaining,
                self.preview,
                Some(self.database),
            )
        })
    }

    fn model_request_window_expired(&self) -> Result<bool, String> {
        let now = self.clock.read();
        let facts = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(now.monotonic_ms);
        Ok(facts.model_request_since_wall_ms.is_some_and(|since|
            now.wall_ms.saturating_sub(since) >= self.binding.budgets.model_request_ms))
    }

    fn retry_settled_model(
        &self,
        effect_key: &str,
        owner: &str,
        cursor: u64,
        error: String,
    ) -> Result<(), String> {
        // This is called only after the owning transport has dropped/reaped its
        // worker. A missing terminal frame is recoverable while the same dispatch
        // lease still owns the model request and every tool is already settled.
        let failure = match super::kernel_model_worker::settled_failure(&error) {
            Some(failure) => failure,
            None if super::kernel_model_worker::is_reaped_transport_failure(&error) => {
                let turn_id = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
                    .shadow_checkpoint(self.clock.now_monotonic_ms()).turn_id;
                fox_engine_protocol::KernelModelFailure {
                    schema_version: 1, run_id: self.binding.run_id.clone(), turn_id,
                    checkpoint_seq: cursor, category: if error == "Kernel worker disconnected; reconcile delivery" || error == "Kernel worker write failed" {
                        "model_transport_failure".into()
                    } else { "model_timeout".into() }, http_status: None,
                    retry_after_ms: None, telemetry: None,
                }
            }
            None => return Err(error),
        };
        if failure.run_id != self.binding.run_id || failure.checkpoint_seq != cursor {
            return Err("model failure belongs to another dispatch".into());
        }
        self.tick_settled_model()?;
        self.cancellation.run_token(&self.binding.run_id)?.check()?;
        let failure_json = serde_json::to_string(&failure).map_err(|_| "invalid model failure")?;
        self.apply(
            Some(DecisionLease::ModelRetry(effect_key, owner)),
            |controller, now| {
                let (providers, _, turns, _) = controller.retry_counters();
                let provider = failure.category == "provider_unavailable";
                let attempt = if provider { providers } else { turns };
                let delay = (1_000_i64.saturating_mul(1_i64 << attempt.min(5)))
                    .max(failure.retry_after_ms.unwrap_or(0) as i64);
                controller.schedule_model_retry(
                    now.monotonic_ms,
                    now.wall_ms,
                    effect_key,
                    &failure_json,
                    provider,
                    delay,
                )
            },
        )
    }

    pub(crate) fn dispatch_initial(
        &self,
        owner: &str,
        policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelInitialModelFrame,
            &kernel::CancellationToken,
        ) -> Result<fox_engine_protocol::KernelInitialModelResponse, String>,
    ) -> Result<(), String> {
        self.tick()?;
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages = self.model_retry_context("initial", self.context_view("initial", &input.messages)?)?;
        // Same steering boundary as the live transport: collect all active
        // rows into this frozen initial dispatch before it is armed.
        let steering_rows = self.database.deliver_steering_for_dispatch(
            &self.binding.run_id,
            kernel::INITIAL_MODEL_IDEMPOTENCY_KEY,
        )?;
        input
            .messages
            .extend(steering_rows.iter().map(steering::steering_user_message));
        let notice_mode = self.database.compute_job_notice_enabled(&self.binding.run_id)?;
        let historical_notice_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&input.messages)?;
        let new_notices = self.pending_host_job_notices(&input.messages)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        let frame = {
            let mut guard = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?;
            if self
                .database
                .run_control_binding(&self.binding.run_id)?
                .as_ref()
                != Some(&self.binding)
            {
                return Err("initial control binding changed".into());
            }
            let frame = fox_engine_protocol::KernelInitialModelFrame {
                schema_version: 1,
                input,
                idempotency_key: kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(),
                continuation_key: None,
                continuation_lane: None,
                checkpoint_seq: guard.last_event_seq(),
                host_job_notices: new_notices,
            };
            frame.validate()?;
            let payload = serde_json::to_value(&frame).map_err(|_|"invalid initial model input")?;
            let mut delivered_history = frame.input.messages.clone();
            delivered_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
            let notice_binding = crate::database::ModelNoticeInput {
                payload: &payload, delivered_history: &delivered_history,
                history_start: frame.input.messages.len(),
                historical_bytes: historical_notice_bytes, live_history: None,
                checkpoint_seq: frame.checkpoint_seq,
            };
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = candidate
                .begin_initial_model_request(now.monotonic_ms, now.wall_ms)
                .map_err(|error| error.to_string())?;
            #[cfg(test)]
            notice_lease_test_barrier::fire(&self.binding.run_id);
            self.database.kernel_commit_initial_model(
                &self.binding.run_id,
                now.wall_ms,
                &candidate.persist_command(&effects),
                owner,
                false,
                None,
                notice_mode.then_some(&notice_binding),
            )?;
            *guard = candidate;
            frame
        };
        token.check()?;
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => {
                return self.retry_settled_model(
                    kernel::INITIAL_MODEL_EFFECT_KEY,
                    owner,
                    frame.checkpoint_seq,
                    error,
                )
            }
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.input.turn_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("initial response belongs to another request".into());
        }
        self.tick_settled_model()?;
        if self.model_request_window_expired()? {
            return self.retry_settled_model(kernel::INITIAL_MODEL_EFFECT_KEY, owner, frame.checkpoint_seq,
                super::kernel_model_worker::MODEL_WINDOW_EXPIRED.into());
        }
        token.check()?;
        let encoded = serde_json::to_string(&response).map_err(|_| "invalid initial response")?;
        let mut initial_history = frame.input.messages.clone();
        initial_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1,
                batch_id: format!(
                    "initial-batch:{}:{}",
                    self.binding.run_id, frame.checkpoint_seq
                ),
                history: initial_history.clone(),
                assistant_message: response.assistant_message.clone(),
            };
            checkpoint.validate()?;
            Some(checkpoint)
        } else { None };
        // A non-tool stop may not abandon input the Host already accepted: when
        // the user's additional request arrived while this round was frozen, the
        // round is answered by its own `steering` lane instead of completing.
        // The next request carries everything this round saw — the frozen
        // initial history plus this assistant turn — never a rebuild from the
        // Run's initial input, which would drop the work already done. The reply
        // is appended by the helper, exactly once.
        let steering_input = if next.is_none() {
            steering::steering_followup_input(
                &self.database,
                &self.binding,
                initial_history.clone(),
                response.assistant_message.clone(),
            )?
        } else {
            None
        };
        let wait_jobs = if next.is_none() && steering_input.is_none() {
            self.unfinished_compute_wait_facts()?
        } else { Vec::new() };
        let notice_followup = if next.is_none() && steering_input.is_none() && wait_jobs.is_empty() {
            self.job_notice_followup_input(&initial_history,&response.assistant_message)?
        } else { None };
        let parked_history = serde_json::to_string(&initial_history)
            .map_err(|_| "invalid parked initial history")?;
        // A request accepted between the queue read above and this write-set can
        // make a **Final** decision refuse. The model result is still in hand and
        // the write-set rolled back, so the safe recovery is to answer the new
        // request in this round and commit again — never to fail the Run (or
        // leave the model request leased) over a race the user cannot see.
        //
        // One retry is enough by construction: the re-planned decision arms a
        // `steering` continuation, which is not terminal, so the guard that
        // rejected the completion cannot apply to it.
        let mut steering_input = steering_input;
        let mut notice_followup = notice_followup;
        let mut attempts = 0u8;
        loop {
            #[cfg(test)]
            live::test_barrier::fire(&self.binding.run_id);
            let outcome = self.apply(Some(DecisionLease::Initial(owner)), |controller, now| {
                let mut effects = vec![controller.record_initial_model_response(&encoded)?];
                if let Some(checkpoint) = next.clone() {
                    let value = serde_json::to_value(&checkpoint).map_err(|_| KernelError::FailClosed("invalid initial checkpoint".into()))?;
                    let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                    let calls = checkpoint.assistant_message["content"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|block| block["type"] == "toolCall")
                        .enumerate()
                        .map(|(source_order, call)| ToolCallRequest {
                            tool_call_id: call["id"].as_str().unwrap().into(),
                            tool: call["name"].as_str().unwrap().into(),
                            canonical_input_json: call["arguments"].to_string(),
                            source_order,
                        })
                        .collect();
                    effects.extend(controller.propose_tool_batch(
                        &checkpoint.batch_id,
                        calls,
                        policy,
                        now.monotonic_ms,
                        now.wall_ms,
                    )?);
                    effects.push(
                        controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?,
                    );
                } else if let Some(input) = &steering_input {
                    let stored = serde_json::to_string(input)
                        .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                    effects.extend(
                        controller.request_steering_followup(steering::STEERING_PROMPT, &stored)?,
                    );
                } else if !wait_jobs.is_empty() {
                    let response_seq = controller.last_event_seq();
                    effects.extend(controller.park_waiting_jobs(
                        now.monotonic_ms, now.wall_ms, self.database.kernel_data_root_id(),
                        response_seq, &encoded, &parked_history, &wait_jobs,
                    )?);
                } else if let Some((input,notices)) = &notice_followup {
                    effects.extend(controller.request_job_notice_followup(
                        &serde_json::to_string(input)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                        &serde_json::to_string(notices)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                    )?);
                } else {
                    effects.extend(controller.terminate(kernel::RunOutcome::Completed));
                }
                Ok(effects)
            });
            match outcome {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.starts_with(kernel::STEERING_COMPETITION)
                        && attempts < live::STEERING_COMPETITION_ATTEMPTS =>
                {
                    attempts += 1;
                    if steering_input.is_none() {
                        steering_input = steering::steering_followup_input(
                            &self.database,
                            &self.binding,
                            initial_history.clone(),
                            response.assistant_message.clone(),
                        )?;
                    }
                    notice_followup = None;
                    if steering_input.is_none() {
                        // The queue emptied again (an explicit cancel raced us):
                        // there is nothing to plan for, so this is a real refusal.
                        return Err(replan_or_error(error));
                    }
                }
                Err(error) => return Err(replan_or_error(error)),
            }
        }
    }

    /// Only the durable snapshot can configure a formal worker dispatch.
    pub(super) fn dispatch_stored_batch_with_worker(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, api_key: &str,
    ) -> Result<(), String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        let frame = self.prepare_stored_batch_resume(batch_id)?;
        let extra = crate::kernel_compaction::bytes(&frame.assistant_message)?
            .saturating_add(crate::kernel_compaction::bytes(&frame.tools)?)
            .saturating_add(4096);
        self.ensure_context_with_worker(batch_id, extra, owner, runtime, api_key)?;
        self.dispatch_batch_with_worker(batch_id, owner, policy, runtime, &config, api_key)
    }

    /// Internal transport, with configuration checked before claiming.
    fn dispatch_batch_with_worker(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, config: &super::kernel_model_worker::KernelModelConfig,
        api_key: &str,
    ) -> Result<(), String> {
        let stored = self.database.kernel_rehydrate(&self.binding.run_id)?
            .ok_or("Kernel Run is missing")?;
        if config.hash()? != stored.config.prompt_config_hash
            || config.execution_profile_id != self.binding.execution_profile_id {
            return Err("Kernel model configuration differs from the frozen Run".into());
        }
        self.dispatch_batch(batch_id, owner, policy, |binding, frame, token| {
            let now = self.clock.read();
            let facts = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?
                .shadow_checkpoint(now.monotonic_ms);
            let since = facts
                .model_request_since_wall_ms
                .ok_or("Kernel model request has no durable deadline")?;
            if now.wall_ms < since {
                return Err("Kernel model clock moved backwards".into());
            }
            let remaining = binding.budgets.limit_operation_ms(
                binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)),
                facts.running_elapsed_ms,
            );
            super::kernel_model_worker::deliver_with_preview(
                runtime,
                config,
                api_key,
                binding,
                frame,
                token,
                remaining,
                self.preview,
                Some(self.database),
            )
        })
    }

    /// Deliver a committed barrier, then atomically persist the original model
    /// response with its terminal or next-batch decision and lease completion.
    pub(crate) fn dispatch_batch(
        &self, batch_id: &str, owner: &str,
        policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelBatchResumeFrame,
            &kernel::CancellationToken,
        ) -> Result<fox_engine_protocol::KernelModelResponse, String>,
    ) -> Result<(), String> {
        let mut frame = self.prepare_stored_batch_resume(batch_id)?;
        let notice_mode = self.database.compute_job_notice_enabled(&self.binding.run_id)?;
        let historical_notice_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&frame.history)?;
        frame.host_job_notices = self.pending_host_job_notices(&frame.history)?;
        frame.validate()?;
        let notice_payload = serde_json::to_value(&frame).map_err(|_|"invalid batch model input")?;
        let mut delivered_history = frame.history.clone();
        delivered_history.push(frame.assistant_message.clone());
        delivered_history.extend(steering::settled_tool_result_messages(
            &self.binding.run_id, &frame.tools, &frame.assistant_message));
        delivered_history.extend(frame.steering.iter().map(|notice| serde_json::json!({
            "role":"user","content":[{"type":"text",
                "text":steering::steering_notice_text(&notice.content)}],
            "timestamp":notice.received_at.unwrap_or(0),
        })));
        let history_start = delivered_history.len();
        delivered_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let notice_binding = crate::database::ModelNoticeInput {
            payload: &notice_payload, delivered_history: &delivered_history,
            history_start,
            historical_bytes: historical_notice_bytes, live_history: None,
            checkpoint_seq: frame.checkpoint_seq,
        };
        self.database.kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        {
            let mut guard = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?;
            if guard.last_event_seq() != frame.checkpoint_seq
                || self.database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding) {
                return Err("Kernel changed before batch dispatch; rebuild the frame".into());
            }
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = candidate.begin_batch_model_request(batch_id, now.monotonic_ms, now.wall_ms)
                .map_err(|error| error.to_string())?;
            self.database.kernel_commit_batch_dispatch(
                &self.binding.run_id,
                now.wall_ms,
                &candidate.persist_command(&effects),
                batch_id,
                owner,
                notice_mode.then_some(&notice_binding),
            )?;
            *guard = candidate;
        }
        token.check()?;
        // No lock is held across engine I/O. Uncertain failures retain the lease.
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => {
                return self.retry_settled_model(
                    &kernel::batch_delivery_effect_key(batch_id),
                    owner,
                    frame.checkpoint_seq,
                    error,
                )
            }
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.turn_id
            || response.batch_id != frame.batch_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("model response belongs to another batch delivery".into());
        }
        self.tick_settled_model()?;
        if self.model_request_window_expired()? {
            return self.retry_settled_model(&kernel::batch_delivery_effect_key(batch_id), owner, frame.checkpoint_seq,
                super::kernel_model_worker::MODEL_WINDOW_EXPIRED.into());
        }
        token.check()?;
        let response_json = serde_json::to_string(&response).map_err(|error| error.to_string())?;
        // Node supplies only the new assistant message. Preserve the original
        // history and reconstruct prior results exclusively from durable facts.
        // The batch's own history is needed for both branches: the next tool
        // proposal and the steering follow-up must see the same rounds.
        let mut batch_history = frame.history.clone();
        batch_history.push(frame.assistant_message.clone());
        batch_history.extend(steering::settled_tool_result_messages(
            &self.binding.run_id,
            &frame.tools,
            &frame.assistant_message,
        ));
        batch_history.extend(frame.steering.iter().map(|notice| serde_json::json!({
            "role":"user","content":[{"type":"text",
                "text":steering::steering_notice_text(&notice.content)}],"timestamp":notice.received_at.unwrap_or(0),
        })));
        batch_history.extend(frame.host_job_notices.iter().map(fox_engine_protocol::HostJobNotice::history_marker));
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1, batch_id: format!("model-batch:{}:{}", self.binding.run_id, frame.checkpoint_seq),
                history: batch_history.clone(), assistant_message: response.assistant_message.clone(),
            };
            checkpoint.validate()?;
            Some(checkpoint)
        } else {
            None
        };
        // Same guarantee as the live round loop: accepted-but-unanswered user
        // input is answered by its own lane before the Run may complete, and the
        // follow-up round keeps the settled tool calls and results this round
        // produced instead of restarting from the Run's initial input. The reply
        // is appended by the helper, exactly once, after the batch history.
        let steering_input = if next.is_none() {
            steering::steering_followup_input(
                &self.database,
                &self.binding,
                batch_history.clone(),
                response.assistant_message.clone(),
            )?
        } else {
            None
        };
        let wait_jobs = if next.is_none() && steering_input.is_none() {
            self.unfinished_compute_wait_facts()?
        } else { Vec::new() };
        let notice_followup = if next.is_none() && steering_input.is_none() && wait_jobs.is_empty() {
            self.job_notice_followup_input(&batch_history,&response.assistant_message)?
        } else { None };
        let parked_history = serde_json::to_string(&batch_history)
            .map_err(|_| "invalid parked batch history")?;
        // Same recovery as the initial path: a Final decision that loses a race
        // with a freshly accepted request is re-planned into its own steering
        // round in this call, with the reply the Host already holds.
        let mut steering_input = steering_input;
        let mut notice_followup = notice_followup;
        let mut attempts = 0u8;
        loop {
            #[cfg(test)]
            live::test_barrier::fire(&self.binding.run_id);
            let outcome = self.apply(
                Some(DecisionLease::Batch(batch_id, owner)),
                |controller, now| {
                    let mut effects =
                        vec![controller.record_batch_model_response(batch_id, &response_json)?];
                    if let Some(checkpoint) = next.clone() {
                        let value = serde_json::to_value(&checkpoint)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                        let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                        let calls = checkpoint.assistant_message["content"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter(|block| block["type"] == "toolCall")
                            .enumerate()
                            .map(|(source_order, call)| ToolCallRequest {
                                tool_call_id: call["id"].as_str().unwrap().into(),
                                tool: call["name"].as_str().unwrap().into(),
                                canonical_input_json: call["arguments"].to_string(),
                                source_order,
                            })
                            .collect();
                        effects.extend(controller.propose_tool_batch(
                            &checkpoint.batch_id,
                            calls,
                            policy,
                            now.monotonic_ms,
                            now.wall_ms,
                        )?);
                        effects.push(
                            controller
                                .checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?,
                        );
                    } else if let Some(input) = &steering_input {
                        let stored = serde_json::to_string(input)
                            .map_err(|error| KernelError::FailClosed(error.to_string()))?;
                        effects.extend(controller.request_steering_followup(
                            steering::STEERING_PROMPT,
                            &stored,
                        )?);
                    } else if !wait_jobs.is_empty() {
                        let response_seq = controller.last_event_seq();
                        effects.extend(controller.park_waiting_jobs(
                            now.monotonic_ms, now.wall_ms, self.database.kernel_data_root_id(),
                            response_seq, &response_json, &parked_history, &wait_jobs,
                        )?);
                    } else if let Some((input,notices)) = &notice_followup {
                        effects.extend(controller.request_job_notice_followup(
                            &serde_json::to_string(input)
                                .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                            &serde_json::to_string(notices)
                                .map_err(|error| KernelError::FailClosed(error.to_string()))?,
                        )?);
                    } else {
                        effects.extend(controller.terminate(kernel::RunOutcome::Completed));
                    }
                    Ok(effects)
                },
            );
            match outcome {
                Ok(()) => return Ok(()),
                Err(error)
                    if error.starts_with(kernel::STEERING_COMPETITION)
                        && attempts < live::STEERING_COMPETITION_ATTEMPTS =>
                {
                    attempts += 1;
                    if steering_input.is_none() {
                        steering_input = steering::steering_followup_input(
                            &self.database,
                            &self.binding,
                            batch_history.clone(),
                            response.assistant_message.clone(),
                        )?;
                    }
                    notice_followup = None;
                    if steering_input.is_none() {
                        return Err(replan_or_error(error));
                    }
                }
                Err(error) => return Err(replan_or_error(error)),
            }
        }
    }

    /// Lease one durable dispatch and verify it still matches the approved
    /// tool identity. `None` means the call is not dispatchable right now
    /// (unknown tool, non-running/terminal state, or a lost pending CAS); the
    /// caller retries through the durable loop instead of replaying it.
    fn lease_dispatch_effect(
        &self,
        tool_call_id: &str,
        owner: &str,
    ) -> Result<Option<kernel::OutboxEffect>, String> {
        let effect_key = kernel::dispatch_effect_key(tool_call_id);
        let guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let data = guard.shadow_checkpoint(self.clock.now_monotonic_ms());
        let tool = data
            .tools
            .iter()
            .find(|tool| tool.tool_call_id == tool_call_id)
            .ok_or("unknown Kernel tool")?;
        if data.state.is_terminal()
            || data.state == kernel::RunState::Cancelling
            || tool.state != kernel::ToolCallState::Running
        {
            return Ok(None);
        }
        let Some(effect) = self.database.kernel_outbox_lease_effect(
            &self.binding.run_id,
            &effect_key,
            owner,
        )?
        else {
            return Ok(None);
        };
        let payload: Value =
            serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
        let expected_input: Value =
            serde_json::from_str(&tool.input_json).map_err(|error| error.to_string())?;
        if effect.kind != kernel::OutboxEffectKind::DispatchTool
            || effect.tool_call_id.as_deref() != Some(tool_call_id)
            || effect.idempotency_key != kernel::dispatch_idempotency_key(&self.binding.run_id, tool_call_id)
            || payload.get("tool").and_then(Value::as_str) != Some(tool.tool.as_str())
            || payload.get("input") != Some(&expected_input)
        {
            return Err("durable dispatch differs from the approved tool identity".into());
        }
        Ok(Some(effect))
    }

    /// Commit one certain tool result inside its own CAS-guarded decision.
    fn settle_dispatch_result(
        &self,
        tool_call_id: &str,
        owner: &str,
        succeeded: bool,
        result: &Value,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(
            Some(DecisionLease::Tool(tool_call_id, owner)),
            |controller, _| {
                controller.tool_settled(tool_call_id, succeeded, &result.to_string())
            },
        )
    }

    /// Dispatch exactly one durable intent. Err after lease is uncertain and is
    /// deliberately left leased; neither this method nor reopen retries it.
    /// The callback runs outside the controller lock, permitting cancellation and
    /// parallel independent calls. Results are committed in source-order batches.
    pub(crate) fn dispatch_tool(
        &self,
        tool_call_id: &str,
        owner: &str,
        execute: impl FnOnce(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>,
    ) -> Result<bool, String> {
        self.tick()?;
        let Some(leased) = self.lease_dispatch_effect(tool_call_id, owner)? else {
            return Ok(false);
        };
        let token = self
            .cancellation
            .tool_token(&self.binding.run_id, tool_call_id)?;
        token.check()?;
        let (succeeded, result) = execute(&self.binding, &leased, &token)?;
        self.settle_dispatch_result(tool_call_id, owner, succeeded, &result)?;
        Ok(true)
    }

    /// Run a Host-classified group of independent, read-only tool calls with
    /// bounded concurrency, then settle every certain result in source order.
    ///
    /// Safety/durability shape:
    ///  * each call acquires its own `pending → leased` CAS under its own
    ///    idempotency key, so no call is executed twice and a crashed owner's
    ///    lease is never stolen here;
    ///  * worker threads touch only owned copies of the frozen binding, the
    ///    leased effect and a per-tool cancellation token — never coordinator
    ///    or database state. All lease/settle decisions stay on this thread;
    ///  * each thread's result is persisted as its own decision in source
    ///    order, regardless of which thread finished first;
    ///  * an `Err`/panic means uncertain execution: that effect stays leased
    ///    (never replayed), the other completed reads still settle, and the
    ///    error propagates exactly as on the serial path so the live loop's
    ///    leased-effect guard detaches the session;
    ///  * cancellation is per tool (`tool_token`); a user cancel trips every
    ///    in-flight token, and the pre-execution check prevents starting any
    ///    remaining call.
    pub(crate) fn dispatch_read_only_group(
        &self,
        effects: &[kernel::OutboxEffect],
        owner: &str,
        execute: &(dyn Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
             + Sync),
    ) -> Result<(), String> {
        if effects.is_empty() {
            return Ok(());
        }
        if effects.len() == 1 {
            let tool_call_id = effects[0]
                .tool_call_id
                .clone()
                .ok_or("missing dispatch tool identity")?;
            return self
                .dispatch_tool(&tool_call_id, owner, |binding, leased, token| {
                    execute(binding, leased, token)
                })
                .map(|_| ());
        }
        self.tick()?;
        // Lease phase: every chosen call gets its own durable CAS before any of
        // them executes. A call that is no longer dispatchable ends the group;
        // what was already leased still has to run (its lease cannot dangle).
        let mut leased: Vec<(String, kernel::OutboxEffect, kernel::CancellationToken)> =
            Vec::with_capacity(effects.len());
        for effect in effects {
            let tool_call_id = effect
                .tool_call_id
                .clone()
                .ok_or("missing dispatch tool identity")?;
            let Some(leased_effect) = self.lease_dispatch_effect(&tool_call_id, owner)? else {
                break;
            };
            let token = self
                .cancellation
                .tool_token(&self.binding.run_id, &tool_call_id)?;
            token.check()?;
            leased.push((tool_call_id, leased_effect, token));
        }
        // Execution phase: owned per-thread copies keep the Send requirements
        // independent of coordinator internals (the clock trait object etc.).
        let jobs: Vec<(RunControlBinding, kernel::OutboxEffect, kernel::CancellationToken)> = leased
            .iter()
            .map(|(_tool_call_id, effect, token)| {
                (self.binding.clone(), effect.clone(), token.clone())
            })
            .collect();
        let outcomes: Vec<Result<(bool, Value), String>> = std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(jobs.len());
            for (binding, effect, token) in jobs {
                handles.push(scope.spawn(move || execute(&binding, &effect, &token)));
            }
            handles
                .into_iter()
                .map(|handle| {
                    handle.join().unwrap_or_else(|_| {
                        // A panicking reader has no durable result: treat it as
                        // uncertain execution; the leased effect is not settled.
                        Err("parallel read-only tool execution panicked without a durable result"
                            .to_owned())
                    })
                })
                .collect()
        });
        // Settlement phase: source order, one CAS-guarded decision per tool.
        let mut uncertain: Option<String> = None;
        for ((tool_call_id, _effect, _token), outcome) in leased.into_iter().zip(outcomes) {
            match outcome {
                Ok((succeeded, result)) => {
                    self.settle_dispatch_result(&tool_call_id, owner, succeeded, &result)?;
                }
                Err(error) => {
                    // This effect stays leased; keep the first failure to
                    // propagate after the other certain results settle.
                    uncertain.get_or_insert(error);
                }
            }
        }
        if let Some(error) = uncertain {
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
