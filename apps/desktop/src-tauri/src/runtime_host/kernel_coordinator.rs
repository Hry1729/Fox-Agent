//! Durable authoritative coordinator. Not attached to default RuntimeHost startup:
//! Pi protocol, capability and desktop rollout gates remain separate prerequisites.
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

fn checkpoint_hash(value: &Value) -> String {
    format!(
        "sha256:{}",
        hex::encode(Sha256::digest(value.to_string().as_bytes()))
    )
}

enum DecisionLease<'a> {
    ModelRetry(&'a str, &'a str),
    Initial(&'a str),
    Tool(&'a str, &'a str),
    Batch(&'a str, &'a str),
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
    pub(crate) fn start_prepared(database: &'a Database, clock: &'a dyn Clock, run_id: &str,
        cancellation: &'a CancellationRegistry) -> Result<Self, String> {
        let (input, config, hash) = database.kernel_initial_start_state(run_id)?;
        let (controller, effects) = RunController::start_with_initial_input(run_id, &input.turn_id, config, &hash, clock)
            .map_err(|error| error.to_string())?;
        database.kernel_commit_decision(run_id, clock.now_wall_ms(), &controller.persist_command(&effects))?;
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
            || binding.budgets.tool_execution_ms != config.tool_execution_timeout_ms
            || binding.budgets.run_execution_ms != config.run_execution_budget_ms
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

    pub(super) fn with_preview(mut self, preview: &'a super::kernel_model_worker::PreviewSink) -> Self {
        self.preview = Some(preview);
        self
    }

    /// A failed decision transaction never leaves speculative state in memory.
    fn apply(
        &self,
        lease: Option<DecisionLease<'_>>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
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
            Some(DecisionLease::ModelRetry(effect_key,owner)) => self.database.kernel_commit_model_retry(
                &self.binding.run_id,now.wall_ms,&command,effect_key,owner,
            )?,
            Some(DecisionLease::Initial(owner)) => self.database.kernel_commit_initial_model(
                &self.binding.run_id, now.wall_ms, &command, owner, true,
            )?,
            Some(DecisionLease::Tool(tool_call_id, owner)) => self.database.kernel_commit_tool_result(
                &self.binding.run_id,
                now.wall_ms,
                &command,
                tool_call_id,
                owner,
            )?,
            Some(DecisionLease::Batch(batch_id, owner)) => self.database.kernel_commit_batch_response(
                &self.binding.run_id, now.wall_ms, &command, batch_id, owner,
            )?,
            None => self.database.kernel_commit_decision(&self.binding.run_id, now.wall_ms, &command)?,
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
        self.apply(None, |controller, now| {
            Ok(controller.tick(now.monotonic_ms, now.wall_ms))
        })
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
        let frame =
            self.prepare_batch_resume(batch_id, checkpoint.history, checkpoint.assistant_message)?;
        if event["turnId"].as_str() != Some(frame.turn_id.as_str()) {
            return Err("persisted engine checkpoint turn mismatch".into());
        }
        Ok(frame)
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

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.request_cancel()))
    }

    /// Host calls this only after its owned model/tool executors have stopped.
    pub(crate) fn settle_cancellation(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.settle_cancellation()))
    }

    pub(crate) fn fail(&self, code: &str, message: &str) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.terminate(kernel::RunOutcome::Failed {
            code: code.into(), message: message.into(),
        })))
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
    ) -> Result<fox_engine_protocol::KernelBatchResumeFrame, String> {
        use fox_engine_protocol::{
            KernelBatchResumeFrame, KernelSettledToolResult, KernelSettledToolState,
        };
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
        let mut tools = Vec::new();
        for id in &batch.ordered {
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
            let result = if raw.get("content").is_some() {
                raw
            } else {
                serde_json::json!({"content":[{"type":"text","text":raw.to_string()}]})
            };
            tools.push(KernelSettledToolResult {
                tool_call_id: id.clone(),
                tool: tool.tool.clone(),
                canonical_input: serde_json::from_str(&tool.input_json)
                    .map_err(|error| error.to_string())?,
                source_order: u32::try_from(tool.source_order)
                    .map_err(|error| error.to_string())?,
                state,
                result,
            });
        }
        let frame = KernelBatchResumeFrame {
            schema_version: 1,
            turn_id: data.turn_id,
            batch_id: batch_id.into(),
            idempotency_key: effect.idempotency_key.clone(),
            checkpoint_seq: snapshot.last_event_seq,
            history,
            assistant_message,
            tools,
        };
        frame.validate()?;
        Ok(frame)
    }

    pub(super) fn dispatch_initial_with_worker(&self, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, api_key: &str) -> Result<(), String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        self.dispatch_initial(owner, policy, |binding, frame, token| {
            let now = self.clock.read();
            let facts = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?.shadow_checkpoint(now.monotonic_ms);
            let since = facts.model_request_since_wall_ms.ok_or("initial model deadline is missing")?;
            if now.wall_ms < since { return Err("initial model clock moved backwards".into()); }
            let remaining = binding.budgets.run_execution_ms.saturating_sub(facts.running_elapsed_ms)
                .min(binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)));
            super::kernel_model_worker::deliver_initial_with_preview(runtime, &config, api_key, binding, frame, token, remaining,self.preview)
        })
    }

    fn retry_settled_model(&self, effect_key: &str, owner: &str, cursor: u64, error: String) -> Result<(), String> {
        let failure = super::kernel_model_worker::settled_failure(&error).ok_or_else(||error.clone())?;
        if failure.run_id != self.binding.run_id || failure.checkpoint_seq != cursor {
            return Err("model failure belongs to another dispatch".into());
        }
        self.tick()?;
        self.cancellation.run_token(&self.binding.run_id)?.check()?;
        let failure_json = serde_json::to_string(&failure).map_err(|_|"invalid model failure")?;
        self.apply(Some(DecisionLease::ModelRetry(effect_key,owner)),|controller,now| {
            let (providers,_,turns,_) = controller.retry_counters();
            let provider = failure.category == "provider_unavailable";
            let attempt = if provider { providers } else { turns };
            let delay = (1_000_i64.saturating_mul(1_i64 << attempt.min(5)))
                .max(failure.retry_after_ms.unwrap_or(0) as i64);
            let elapsed = controller.shadow_checkpoint(now.monotonic_ms).running_elapsed_ms;
            if delay >= self.binding.budgets.run_execution_ms.saturating_sub(elapsed) {
                return Err(KernelError::FailClosed("model retry delay exceeds remaining Run budget".into()));
            }
            controller.schedule_model_retry(now.monotonic_ms,now.wall_ms,effect_key,&failure_json,provider,delay)
        })
    }

    pub(crate) fn dispatch_initial(&self, owner: &str, policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(&RunControlBinding, &fox_engine_protocol::KernelInitialModelFrame, &kernel::CancellationToken)
            -> Result<fox_engine_protocol::KernelInitialModelResponse, String>) -> Result<(), String> {
        self.tick()?;
        self.database.kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let input = self.database.kernel_initial_input(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        let frame = {
            let mut guard = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?;
            if self.database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding) { return Err("initial control binding changed".into()); }
            let frame = fox_engine_protocol::KernelInitialModelFrame { schema_version: 1, input,
                idempotency_key: kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(), checkpoint_seq: guard.last_event_seq() };
            frame.validate()?;
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = candidate.begin_initial_model_request(now.monotonic_ms, now.wall_ms).map_err(|error| error.to_string())?;
            self.database.kernel_commit_initial_model(&self.binding.run_id, now.wall_ms, &candidate.persist_command(&effects), owner, false)?;
            *guard = candidate;
            frame
        };
        token.check()?;
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => return self.retry_settled_model(kernel::INITIAL_MODEL_EFFECT_KEY,owner,frame.checkpoint_seq,error),
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.input.turn_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("initial response belongs to another request".into());
        }
        self.tick()?;
        token.check()?;
        let encoded = serde_json::to_string(&response).map_err(|_| "invalid initial response")?;
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint { schema_version: 1,
                batch_id: format!("initial-batch:{}:{}", self.binding.run_id, frame.checkpoint_seq), history: frame.input.messages,
                assistant_message: response.assistant_message };
            checkpoint.validate()?;
            Some(checkpoint)
        } else { None };
        self.apply(Some(DecisionLease::Initial(owner)), |controller, now| {
            let mut effects = vec![controller.record_initial_model_response(&encoded)?];
            if let Some(checkpoint) = next {
                let value = serde_json::to_value(&checkpoint).map_err(|_| KernelError::FailClosed("invalid initial checkpoint".into()))?;
                let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                let calls = checkpoint.assistant_message["content"].as_array().unwrap().iter()
                    .filter(|block| block["type"] == "toolCall").enumerate().map(|(source_order, call)| ToolCallRequest {
                        tool_call_id:call["id"].as_str().unwrap().into(), tool:call["name"].as_str().unwrap().into(),
                        canonical_input_json:call["arguments"].to_string(), source_order,
                    }).collect();
                effects.extend(controller.propose_tool_batch(&checkpoint.batch_id, calls, policy, now.monotonic_ms, now.wall_ms)?);
                effects.push(controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?);
            } else { effects.extend(controller.terminate(kernel::RunOutcome::Completed)); }
            Ok(effects)
        })
    }

    /// Only the durable snapshot can configure a formal worker dispatch.
    pub(super) fn dispatch_stored_batch_with_worker(
        &self, batch_id: &str, owner: &str, policy: &dyn PolicyDecisionPort,
        runtime: &super::RuntimeCommand, api_key: &str,
    ) -> Result<(), String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
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
            let since = facts.model_request_since_wall_ms.ok_or("Kernel model request has no durable deadline")?;
            if now.wall_ms < since { return Err("Kernel model clock moved backwards".into()); }
            let remaining = binding.budgets.run_execution_ms.saturating_sub(facts.running_elapsed_ms)
                .min(binding.budgets.model_request_ms.saturating_sub(now.wall_ms.saturating_sub(since)));
            super::kernel_model_worker::deliver_with_preview(runtime, config, api_key, binding, frame, token, remaining,self.preview)
        })
    }

    /// Deliver a committed barrier, then atomically persist the original model
    /// response with its terminal or next-batch decision and lease completion.
    pub(crate) fn dispatch_batch(
        &self, batch_id: &str, owner: &str,
        policy: &dyn PolicyDecisionPort,
        deliver: impl FnOnce(&RunControlBinding, &fox_engine_protocol::KernelBatchResumeFrame,
            &kernel::CancellationToken) -> Result<fox_engine_protocol::KernelModelResponse, String>,
    ) -> Result<(), String> {
        let frame = self.prepare_stored_batch_resume(batch_id)?;
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
            self.database.kernel_commit_batch_dispatch(&self.binding.run_id, now.wall_ms,
                &candidate.persist_command(&effects), batch_id, owner)?;
            *guard = candidate;
        }
        token.check()?;
        // No lock is held across engine I/O. Uncertain failures retain the lease.
        let response = match deliver(&self.binding, &frame, &token) {
            Ok(response) => response,
            Err(error) => return self.retry_settled_model(&kernel::batch_delivery_effect_key(batch_id),owner,frame.checkpoint_seq,error),
        };
        response.validate()?;
        if response.run_id != self.binding.run_id || response.turn_id != frame.turn_id
            || response.batch_id != frame.batch_id || response.checkpoint_seq != frame.checkpoint_seq {
            return Err("model response belongs to another batch delivery".into());
        }
        self.tick()?;
        token.check()?;
        let response_json = serde_json::to_string(&response).map_err(|error| error.to_string())?;
        // Node supplies only the new assistant message. Preserve the original
        // history and reconstruct prior results exclusively from durable facts.
        let next = if response.assistant_message["stopReason"] == "toolUse" {
            let mut history = frame.history.clone();
            history.push(frame.assistant_message.clone());
            history.extend(frame.tools.iter().map(|tool| serde_json::json!({
                "role":"toolResult", "toolCallId":tool.tool_call_id, "toolName":tool.tool,
                "content":tool.result["content"], "details":{},
                "isError":tool.state == fox_engine_protocol::KernelSettledToolState::Failed,
                "timestamp":frame.assistant_message["timestamp"].as_i64().unwrap_or(0),
            })));
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1, batch_id: format!("model-batch:{}:{}", self.binding.run_id, frame.checkpoint_seq),
                history, assistant_message: response.assistant_message,
            };
            checkpoint.validate()?;
            Some(checkpoint)
        } else { None };
        self.apply(Some(DecisionLease::Batch(batch_id, owner)), |controller, now| {
            let mut effects = vec![controller.record_batch_model_response(batch_id, &response_json)?];
            if let Some(checkpoint) = next {
                let value = serde_json::to_value(&checkpoint).map_err(|error| KernelError::FailClosed(error.to_string()))?;
                let stored = serde_json::json!({"hash":checkpoint_hash(&value),"value":value});
                let calls = checkpoint.assistant_message["content"].as_array().unwrap().iter()
                    .filter(|block| block["type"] == "toolCall").enumerate().map(|(source_order, call)| ToolCallRequest {
                        tool_call_id:call["id"].as_str().unwrap().into(), tool:call["name"].as_str().unwrap().into(),
                        canonical_input_json:call["arguments"].to_string(), source_order,
                    }).collect();
                effects.extend(controller.propose_tool_batch(&checkpoint.batch_id, calls, policy, now.monotonic_ms, now.wall_ms)?);
                effects.push(controller.checkpoint_tool_batch(&checkpoint.batch_id, &stored.to_string())?);
            } else {
                effects.extend(controller.terminate(kernel::RunOutcome::Completed));
            }
            Ok(effects)
        })
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
        let effect_key = kernel::dispatch_effect_key(tool_call_id);
        let leased = {
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
                return Ok(false);
            }
            let Some(effect) = self.database.kernel_outbox_lease_effect(
                &self.binding.run_id,
                &effect_key,
                owner,
            )?
            else {
                return Ok(false);
            };
            let payload: Value =
                serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
            let expected_input: Value =
                serde_json::from_str(&tool.input_json).map_err(|error| error.to_string())?;
            if effect.kind != kernel::OutboxEffectKind::DispatchTool
                || effect.tool_call_id.as_deref() != Some(tool_call_id)
                || effect.idempotency_key != kernel::dispatch_idempotency_key(tool_call_id)
                || payload.get("tool").and_then(Value::as_str) != Some(tool.tool.as_str())
                || payload.get("input") != Some(&expected_input)
            {
                return Err("durable dispatch differs from the approved tool identity".into());
            }
            effect
        };
        let token = self
            .cancellation
            .tool_token(&self.binding.run_id, tool_call_id)?;
        token.check()?;
        let (succeeded, result) = execute(&self.binding, &leased, &token)?;
        self.tick()?;
        self.apply(Some(DecisionLease::Tool(tool_call_id, owner)), |controller, _| {
            controller.tool_settled(tool_call_id, succeeded, &result.to_string())
        })?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
