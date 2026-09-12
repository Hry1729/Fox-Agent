//! Durable round loop for one live loop-capable engine session (Pi). The
//! engine drives its own tool loop inside the worker; every round output is
//! committed with the same transaction shape as the per-round transport, and
//! settled batches are handed back to the same session. Any failure detaches
//! the session and lets the drive loop recover through the existing durable
//! delivery, retry and restart paths.

use super::*;
use serde_json::json;
use std::time::{Duration, Instant};

pub(super) const CONTINUATION_LIMIT: i64 = 2;

/// Generic bounded stop-review prompt. It references only durable facts
/// (original request, actual tool results) and never grants permission.
pub(super) const CONTINUATION_PROMPT: &str = "Fox 续答检查：在结束前，请对照上方原始用户请求与本次任务中实际发生的工具结果，检查尚未完成的部分。若仍有可在授权工具范围内继续的工作，请立即发出下一个工具调用继续执行，不要只说明你将开始分析。若工作确实已完成、遇到具体阻碍、或必须由用户补充信息，请直接给出有用的最终答复。";

pub(crate) const LIVE_CANCEL: &str = "kernel.live.cancel";
pub(crate) const LIVE_DETACHED: &str = "kernel.live.detached";

/// Which durable delivery the next engine output answers.
enum Stage {
    Initial { cursor: u64 },
    Batch { batch_id: String, cursor: u64 },
    Continuation { pre_history: Vec<Value> },
}

type ExecuteFn<'a> = &'a dyn Fn(
    &RunControlBinding,
    &kernel::OutboxEffect,
    &kernel::CancellationToken,
) -> Result<(bool, Value), String>;
type AfterCommitFn<'a> = &'a dyn Fn(&str) -> Result<(), String>;
type SettleChildrenFn<'a> = &'a dyn Fn(bool) -> Result<(), String>;

impl KernelCoordinator<'_> {
    fn live_deadline(&self) -> Result<Instant, String> {
        let now = self.clock.read();
        let facts = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(now.monotonic_ms);
        let remaining = self
            .binding
            .budgets
            .run_execution_ms
            .saturating_sub(facts.running_elapsed_ms);
        if remaining <= 0 {
            return Err("Kernel Run execution budget exhausted".into());
        }
        Ok(Instant::now() + Duration::from_millis(remaining as u64))
    }

    fn processed_initial_input(&self) -> Result<Vec<Value>, String> {
        let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages =
            self.model_retry_context("initial", self.context_view("initial", &input.messages)?)?;
        Ok(input.messages)
    }

    /// Durable, engine-reconstructable parts of one settled batch, independent
    /// of the delivery outbox status (a live session holds its lease while its
    /// results are projected).
    fn stored_batch_parts(
        &self,
        batch_id: &str,
    ) -> Result<(Vec<Value>, Value, Vec<fox_engine_protocol::KernelSettledToolResult>), String>
    {
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
        let history =
            self.model_retry_context(batch_id, self.context_view(batch_id, &checkpoint.history)?)?;
        let (data, snapshot) = {
            let guard = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?;
            (
                guard.shadow_checkpoint(self.clock.now_monotonic_ms()),
                self.snapshot()?,
            )
        };
        let batch = data
            .batches
            .iter()
            .find(|batch| batch.batch_id == batch_id && batch.barrier_emitted)
            .ok_or("batch result barrier has not committed")?;
        let tools = self.project_settled_tools(&data, &snapshot, &batch.ordered)?;
        Ok((history, checkpoint.assistant_message, tools))
    }

    fn tool_result_messages(
        assistant: &Value,
        tools: &[fox_engine_protocol::KernelSettledToolResult],
    ) -> Vec<Value> {
        tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "role":"toolResult", "toolCallId":tool.tool_call_id, "toolName":tool.tool,
                    "content":tool.result["content"], "details":{},
                    "isError":tool.state == fox_engine_protocol::KernelSettledToolState::Failed,
                    "timestamp":assistant["timestamp"].as_i64().unwrap_or(0),
                })
            })
            .collect()
    }

    /// Drive approvals and dispatch for one proposed batch until every tool of
    /// the batch has settled. Cancel requests and cancellation tokens bail so
    /// the drive loop keeps ownership of terminal transitions.
    #[allow(clippy::too_many_arguments)]
    fn drive_batch_to_barrier(
        &self,
        owner: &str,
        batch_id: &str,
        execute: ExecuteFn,
        after_commit: AfterCommitFn,
        settle_children: SettleChildrenFn,
    ) -> Result<(), String> {
        loop {
            for tool_id in self.database.pending_kernel_host_action_ids(&self.binding.run_id)? {
                after_commit(&tool_id)?;
            }
            settle_children(false)?;
            for command in self.database.pending_kernel_host_commands(&self.binding.run_id)? {
                if command.kind == "cancel" {
                    // Leave the durable command for the drive loop; it owns the
                    // cancel transition and the worker cleanup.
                    return Err(LIVE_CANCEL.to_string());
                }
                let current = self.snapshot()?;
                let tool_id = command
                    .tool_call_id
                    .as_deref()
                    .ok_or("missing queued approval tool")?;
                if current.tool_calls.iter().any(|tool| {
                    tool.tool_call_id == tool_id
                        && tool.approval_state.as_deref() == Some("pending")
                }) {
                    let decision = match command.decision.as_deref() {
                        Some("allow_once") => kernel::ApprovalDecision::AllowOnce,
                        Some("allow_conversation") => {
                            kernel::ApprovalDecision::AllowConversation
                        }
                        Some("denied") => kernel::ApprovalDecision::Deny,
                        _ => return Err("invalid durable approval decision".into()),
                    };
                    self.resolve_approval(tool_id, decision)?;
                }
                self.database
                    .complete_kernel_host_command(&self.binding.run_id, command.seq)?;
            }
            self.tick()?;
            let snapshot = self.snapshot()?;
            if snapshot.state == "cancelling" || kernel_state_is_terminal(&snapshot.state) {
                return Err(LIVE_CANCEL.to_string());
            }
            if super::super::kernel_delegation::correction_limit_reached(&snapshot) {
                settle_children(true)?;
                self.fail(
                    "kernel.child_arguments_exhausted",
                    "子任务参数连续校验失败，已提供 3 次纠错机会并停止继续派发。请检查工具参数后重试；此错误不是 API 额度不足。",
                )?;
                return Err(LIVE_DETACHED.to_string());
            }
            if snapshot
                .pending_effects
                .iter()
                .any(|effect| effect.status == kernel::OutboxStatus::Leased)
            {
                // A leased dispatch must never be replayed here; detach and let
                // the durable recovery decide.
                return Err(LIVE_DETACHED.to_string());
            }
            let batch = snapshot
                .batches
                .iter()
                .find(|batch| batch.batch_id == batch_id)
                .ok_or("proposed batch is missing from the durable snapshot")?;
            let all_settled = batch.barrier_emitted
                && batch.ordered_tool_call_ids.iter().all(|id| {
                    snapshot
                        .tool_calls
                        .iter()
                        .any(|tool| &tool.tool_call_id == id && (tool.state == "completed" || tool.state == "failed"))
                });
            if all_settled {
                return Ok(());
            }
            let pending = snapshot
                .pending_effects
                .iter()
                .find(|effect| {
                    effect.status == kernel::OutboxStatus::Pending
                        && effect.kind == kernel::OutboxEffectKind::DispatchTool
                })
                .cloned();
            if let Some(effect) = pending {
                let tool_call_id = effect
                    .tool_call_id
                    .clone()
                    .ok_or("missing dispatch tool identity")?;
                self.dispatch_tool(&tool_call_id, owner, execute)?;
                continue;
            }
            if snapshot.state == "waiting_approval" {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            return Err("Kernel batch made no progress; detaching live session".into());
        }
    }

    /// Commit the durable record for one engine round output and produce the
    /// directive that keeps the live session going.
    #[allow(clippy::too_many_arguments)]
    fn service_round_output(
        &self,
        stage: &mut Stage,
        owner: &str,
        policy: &dyn kernel::PolicyDecisionPort,
        frame: fox_engine_protocol::KernelRoundOutputFrame,
        execute: ExecuteFn,
        after_commit: AfterCommitFn,
        settle_children: SettleChildrenFn,
    ) -> Result<fox_engine_protocol::KernelRoundDirective, String> {
        self.tick()?;
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check().map_err(|_| LIVE_CANCEL.to_string())?;
        let output = frame.assistant_message;
        let tool_use = output["stopReason"] == "toolUse";

        // Pre-read durable context needed by the commit tail.
        let (pre_history, response_json, answered_batch) = match stage {
            Stage::Initial { cursor } => {
                let response = fox_engine_protocol::KernelInitialModelResponse {
                    schema_version: 1,
                    run_id: self.binding.run_id.clone(),
                    turn_id: frame.turn_id.clone(),
                    checkpoint_seq: *cursor,
                    assistant_message: output.clone(),
                };
                response.validate()?;
                (
                    self.processed_initial_input()?,
                    serde_json::to_string(&response).map_err(|_| "invalid response")?,
                    None,
                )
            }
            Stage::Batch { batch_id, cursor } => {
                let (history, assistant, tools) = self.stored_batch_parts(batch_id)?;
                let mut pre = history;
                pre.push(assistant.clone());
                pre.extend(Self::tool_result_messages(&assistant, &tools));
                let response = fox_engine_protocol::KernelModelResponse {
                    schema_version: 1,
                    run_id: self.binding.run_id.clone(),
                    turn_id: frame.turn_id.clone(),
                    batch_id: batch_id.clone(),
                    checkpoint_seq: *cursor,
                    assistant_message: output.clone(),
                };
                response.validate()?;
                (
                    pre,
                    serde_json::to_string(&response).map_err(|_| "invalid response")?,
                    Some(batch_id.clone()),
                )
            }
            Stage::Continuation { pre_history } => {
                let response = serde_json::json!({
                    "schemaVersion": 1, "runId": self.binding.run_id, "turnId": frame.turn_id,
                    "checkpointSeq": self
                        .controller
                        .lock()
                        .map_err(|_| "Kernel coordinator lock poisoned")?
                        .last_event_seq(),
                    "assistantMessage": output,
                });
                (
                    pre_history.clone(),
                    serde_json::to_string(&response).map_err(|_| "invalid response")?,
                    None,
                )
            }
        };

        // Continuation admission is decided from durable facts before the
        // commit that records the decision: some tool ran in this Run, the
        // bounded count has not been exhausted, and the Run is not cancelled.
        let continuation_allowed = !tool_use
            && !self.snapshot()?.tool_calls.is_empty()
            && self.database.kernel_count_continuations(&self.binding.run_id)?
                < CONTINUATION_LIMIT;

        // The next checkpoint for a tool proposal, committed with the response.
        let next_checkpoint = if tool_use {
            let seq = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?
                .last_event_seq();
            let batch_id = match stage {
                Stage::Initial { cursor } => {
                    format!("initial-batch:{}:{}", self.binding.run_id, cursor)
                }
                _ => format!("model-batch:{}:{}", self.binding.run_id, seq),
            };
            let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
                schema_version: 1,
                batch_id: batch_id.clone(),
                history: pre_history.clone(),
                assistant_message: output.clone(),
            };
            checkpoint.validate()?;
            Some((batch_id, checkpoint))
        } else {
            None
        };

        // Commit the recorded response together with its next-step decision.
        let next_batch = {
            let next_checkpoint = next_checkpoint.clone();
            match (&answered_batch, &*stage) {
                (Some(batch_id), _) => {
                    let batch_id = batch_id.clone();
                    self.apply(Some(DecisionLease::Batch(&batch_id, owner)), |controller, now| {
                        let mut effects =
                            vec![controller.record_batch_model_response(&batch_id, &response_json)?];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            continuation_allowed,
                        )?);
                        Ok(effects)
                    })?;
                }
                (None, Stage::Initial { .. }) => {
                    self.apply(Some(DecisionLease::Initial(owner)), |controller, now| {
                        let mut effects =
                            vec![controller.record_initial_model_response(&response_json)?];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            continuation_allowed,
                        )?);
                        Ok(effects)
                    })?;
                }
                (None, Stage::Continuation { .. }) => {
                    self.apply(None, |controller, now| {
                        let mut effects = vec![
                            controller.record_continuation_model_response(&response_json)?
                        ];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            continuation_allowed,
                        )?);
                        Ok(effects)
                    })?;
                }
                (None, Stage::Batch { .. }) => return Err("continuation stage mismatch".into()),
            }
            next_checkpoint
        };

        if let Some((batch_id, _checkpoint)) = next_batch {
            // Drive the just-proposed batch to the durable barrier, hand the
            // settled results to the live session and arm the next round.
            self.drive_batch_to_barrier(owner, &batch_id, execute, after_commit, settle_children)?;
            let (history, assistant, tools) = self.stored_batch_parts(&batch_id)?;
            let config = self.database.kernel_model_config(&self.binding.run_id)?;
            let extra = crate::kernel_compaction::bytes(&assistant)?
                .saturating_add(crate::kernel_compaction::bytes(&Self::tool_result_messages(
                    &assistant, &tools,
                ))?)
                .saturating_add(4096);
            let view = self.context_view(&batch_id, &history)?;
            let limit = crate::kernel_compaction::history_limit(
                &config,
                crate::kernel_compaction::bytes(&config.proposal_tools)?
                    .saturating_add(config.system_prompt.len()),
            );
            if crate::kernel_compaction::bytes(&view)?.saturating_add(extra) > limit {
                // Detach before leasing: the drive loop delivers this pending
                // batch through the compaction-aware replacement transport.
                return Err(LIVE_DETACHED.to_string());
            }
            let cursor = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?
                .last_event_seq();
            self.apply(
                Some(DecisionLease::BatchDispatch(&batch_id, owner)),
                |controller, now| {
                    controller
                        .begin_batch_model_request(
                            &batch_id,
                            now.monotonic_ms,
                            now.wall_ms,
                        )
                },
            )?;
            *stage = Stage::Batch { batch_id: batch_id.clone(), cursor };
            return Ok(fox_engine_protocol::KernelRoundDirective {
                schema_version: 1,
                kind: fox_engine_protocol::KernelRoundDirectiveKind::Batch,
                batch_id: Some(batch_id),
                checkpoint_seq: Some(cursor),
                tools,
                prompt: None,
            });
        }

        if continuation_allowed {
            let mut next_pre = pre_history;
            next_pre.push(output);
            next_pre.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": CONTINUATION_PROMPT}],
                "timestamp": 0,
            }));
            *stage = Stage::Continuation { pre_history: next_pre };
            return Ok(fox_engine_protocol::KernelRoundDirective {
                schema_version: 1,
                kind: fox_engine_protocol::KernelRoundDirectiveKind::Continuation,
                batch_id: None,
                checkpoint_seq: None,
                tools: Vec::new(),
                prompt: Some(CONTINUATION_PROMPT.into()),
            });
        }

        Ok(fox_engine_protocol::KernelRoundDirective {
            schema_version: 1,
            kind: fox_engine_protocol::KernelRoundDirectiveKind::Final,
            batch_id: None,
            checkpoint_seq: None,
            tools: Vec::new(),
            prompt: None,
        })
    }

    /// Run the whole authoritative Run through one live engine session.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn dispatch_initial_live(
        &self,
        owner: &str,
        policy: &dyn kernel::PolicyDecisionPort,
        runtime: &super::super::RuntimeCommand,
        api_key: &str,
        execute: ExecuteFn,
        after_commit: AfterCommitFn,
        settle_children: SettleChildrenFn,
    ) -> Result<(), String> {
        self.ensure_context_with_worker("initial", 4096, owner, runtime, api_key)?;
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        // Spawn and initialize before the durable dispatch commit: the ready
        // handshake advances no controller state, and an old runtime without
        // the loop capability must fall back to the per-round transport.
        let mut session = super::super::kernel_model_worker::LiveKernelSession::spawn(
            runtime,
            &config,
            api_key,
            &self.binding,
            &token,
            self.live_deadline()?,
        )?;
        self.tick()?;
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
        input.messages =
            self.model_retry_context("initial", self.context_view("initial", &input.messages)?)?;
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
                checkpoint_seq: guard.last_event_seq(),
            };
            frame.validate()?;
            let mut candidate = guard.clone();
            let now = self.clock.read();
            let effects = candidate
                .begin_initial_model_request(now.monotonic_ms, now.wall_ms)
                .map_err(|error| error.to_string())?;
            self.database.kernel_commit_initial_model(
                &self.binding.run_id,
                now.wall_ms,
                &candidate.persist_command(&effects),
                owner,
                false,
            )?;
            *guard = candidate;
            frame
        };
        token.check()?;
        let mut stage = Stage::Initial { cursor: frame.checkpoint_seq };
        let mut service = |frame: fox_engine_protocol::KernelRoundOutputFrame| {
            self.service_round_output(
                &mut stage,
                owner,
                policy,
                frame,
                execute,
                after_commit,
                settle_children,
            )
        };
        let request_payload = json!({"controlBinding": self.binding, "initialModel": frame});
        match session.run(
            "kernel.start_initial",
            request_payload,
            &self.binding,
            &token,
            self.live_deadline()?,
            self.preview,
            &mut service,
        ) {
            Ok(_) => Ok(()),
            Err(error) if error == LIVE_CANCEL || error == LIVE_DETACHED => Err(error),
            // A settled model failure keeps the frozen retry semantics: the
            // drive loop schedules the durable retry and a replacement session
            // re-runs the round from durable facts. Continuation rounds carry
            // no lease, so their failures are not retryable and fail closed.
            Err(error) => match &stage {
                Stage::Initial { cursor } => {
                    self.retry_settled_model(
                        kernel::INITIAL_MODEL_EFFECT_KEY,
                        owner,
                        *cursor,
                        error,
                    )
                }
                Stage::Batch { batch_id, cursor } => self.retry_settled_model(
                    &kernel::batch_delivery_effect_key(batch_id),
                    owner,
                    *cursor,
                    error,
                ),
                Stage::Continuation { .. } => Err(error),
            },
        }
    }
}

fn kernel_state_is_terminal(state: &str) -> bool {
    matches!(state, "completed" | "failed" | "cancelled" | "budget_exhausted")
}

#[allow(clippy::too_many_arguments)]
fn commit_tail(
    controller: &mut kernel::RunController,
    now: kernel::ClockReading,
    policy: &dyn kernel::PolicyDecisionPort,
    next_checkpoint: &Option<(String, fox_engine_protocol::KernelEngineBatchCheckpoint)>,
    continuation_allowed: bool,
) -> Result<Vec<kernel::Effect>, kernel::KernelError> {
    if let Some((batch_id, checkpoint)) = next_checkpoint {
        let value = serde_json::to_value(checkpoint)
            .map_err(|error| kernel::KernelError::FailClosed(error.to_string()))?;
        let stored = serde_json::json!({"hash": super::checkpoint_hash(&value), "value": value});
        let calls = checkpoint.assistant_message["content"]
            .as_array()
            .ok_or_else(|| kernel::KernelError::FailClosed("missing engine proposal".into()))?
            .iter()
            .filter(|block| block["type"] == "toolCall")
            .enumerate()
            .map(|(source_order, call)| kernel::ToolCallRequest {
                tool_call_id: call["id"].as_str().unwrap_or_default().into(),
                tool: call["name"].as_str().unwrap_or_default().into(),
                canonical_input_json: call["arguments"].to_string(),
                source_order,
            })
            .collect();
        let mut effects = controller.propose_tool_batch(
            batch_id,
            calls,
            policy,
            now.monotonic_ms,
            now.wall_ms,
        )?;
        effects.push(controller.checkpoint_tool_batch(batch_id, &stored.to_string())?);
        return Ok(effects);
    }
    if continuation_allowed {
        return Ok(vec![controller.note_continuation_request(CONTINUATION_PROMPT)?]);
    }
    Ok(controller.terminate(kernel::RunOutcome::Completed))
}
