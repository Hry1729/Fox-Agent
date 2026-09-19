//! Durable round loop for one live loop-capable engine session (Pi). The
//! engine drives its own tool loop inside the worker; every round output is
//! committed with the same transaction shape as the per-round transport, and
//! settled batches are handed back to the same session. Any failure detaches
//! the session and lets the drive loop recover through the existing durable
//! delivery, retry and restart paths.

use super::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(super) const CONTINUATION_LIMIT: i64 = 2;

/// Bounded number of follow-up rounds the Host will dedicate to additional user
/// input accepted mid-run. This is a separate budget from the stop-review: new
/// user input must not consume (or be starved by) the review of the model's own
/// stop. It still bounds how far one queue of additions can extend a Run.
pub(super) const STEERING_FOLLOWUP_LIMIT: i64 = 4;

/// Generic bounded stop-review prompt. It references only durable facts
/// (original request, actual tool results) and never grants permission.
pub(super) const CONTINUATION_PROMPT: &str = "Fox 续答检查：在结束前，请对照上方原始用户请求与本次任务中实际发生的工具结果，检查尚未完成的部分。若仍有可在授权工具范围内继续的工作，请立即发出下一个工具调用继续执行，不要只说明你将开始分析。若工作确实已完成、遇到具体阻碍、或必须由用户补充信息，请直接给出有用的最终答复。";

/// Prompt for a round that answers additional user input received while the Run
/// was in flight. It is new user work, not a review of the model's own stop.
pub(super) const STEERING_PROMPT: &str = "Fox 补充要求处理：用户在本次任务运行期间提交了新的补充要求（见上方）。请在既有授权与工具策略范围内处理这些补充要求，然后给出最终答复；不要重放已经完成的写入操作。";

pub(crate) const LIVE_CANCEL: &str = "kernel.live.cancel";
pub(crate) const LIVE_DETACHED: &str = "kernel.live.detached";

/// Bounded number of follow-up attempts when a terminal decision loses a race
/// with a freshly accepted request. Each attempt re-plans the additional-input
/// round with adopt-all semantics, so one attempt is normally enough; the bound
/// exists so a pathological stream of arrivals cannot spin this round forever.
pub(super) const STEERING_COMPETITION_ATTEMPTS: u8 = 4;

/// Which durable delivery the next engine output answers.
enum Stage {
    Initial { cursor: u64 },
    Batch { batch_id: String, cursor: u64 },
    Continuation { effect_key: String, pre_history: Vec<Value>, cursor: u64 },
}

/// What happens after a non-tool model stop. The end-of-turn states stay
/// distinct: a plain completion, the generic bounded stop-review, the bounded
/// business-delivery repair, and answering additional user input that was
/// accepted while this round was in flight.
enum StopFollowup {
    /// Accept the stop as the Run's final answer.
    Final,
    /// Generic stop-review via the `stop_review` continuation lane.
    Review(fox_engine_protocol::KernelInitialModelInput),
    /// Business delivery repair via the `delivery_repair` continuation lane.
    Repair {
        prompt: String,
        input: fox_engine_protocol::KernelInitialModelInput,
    },
    /// Additional user input the Host already accepted: it gets its own
    /// `steering` continuation lane and round, so a normal completion can never
    /// silently discard a request the user was told was received.
    Steering(fox_engine_protocol::KernelInitialModelInput),
}

/// The Host execute callback must be shareable across the bounded read-only
/// worker threads (see [`dispatch_read_only_group`]); it stays a shared
/// reference and never owns coordinator state.
///
/// [`dispatch_read_only_group`]: KernelCoordinator::dispatch_read_only_group
type ExecuteFn<'a> = &'a (dyn Fn(
    &RunControlBinding,
    &kernel::OutboxEffect,
    &kernel::CancellationToken,
) -> Result<(bool, Value), String>
     + Sync);

/// Upper bound on independent read-only tool calls run concurrently within one
/// proposed batch. Independence is decided by the Host from durable facts —
/// never from the model's own ordering or an MCP read-only claim.
const MAX_PARALLEL_READ_ONLY: usize = 4;

/// Host-side parallel-safety classification for one durable dispatch. Only
/// Host-native, side-effect-free readers may run concurrently:
///  * `read`/`ls`/`find`/`grep` are bounded frozen-root readers with shared
///    read handles, so even identical targets cannot conflict;
///  * `skill_load` returns read-only skill text; its only persistence is an
///    idempotent `INSERT OR IGNORE` activation row.
/// Everything else — `write_file`/`edit_file`/`run_command`, generic MCP tools
/// (a manifest read-only flag is not proof of safety), knowledge/work/
/// delegation tools and unknown names — is a serial barrier.
pub(super) fn parallel_read_only_dispatch(effect: &kernel::OutboxEffect) -> bool {
    if effect.kind != kernel::OutboxEffectKind::DispatchTool {
        return false;
    }
    let Ok(payload) = serde_json::from_str::<Value>(&effect.payload_json) else {
        return false;
    };
    matches!(
        payload.get("tool").and_then(Value::as_str),
        Some("read" | "ls" | "find" | "grep" | "skill_load")
    )
}

/// Source-ordered ids of the Run's still-pending tool dispatches. The outbox
/// projection is key-ordered (`created_at`, `effect_key`), not source-ordered;
/// the durable tool-call table (`batch_id`, `source_order`) is the model's
/// actual ordering and is what the engine batch barrier projects back. A
/// dispatch without a tool row should not exist and is appended so the serial
/// path keeps rejecting it instead of silently skipping it.
pub(crate) fn dispatch_waits_for_approval(state: &str, effect: &kernel::OutboxEffect) -> bool {
    if state != "waiting_approval" || effect.kind != kernel::OutboxEffectKind::DispatchTool {
        return false;
    }
    let payload: Value = serde_json::from_str(&effect.payload_json).unwrap_or(Value::Null);
    !crate::database::Database::approval_independent(payload["tool"].as_str())
}

pub(super) fn pending_dispatch_ids(snapshot: &kernel::KernelSnapshot) -> Vec<String> {
    let mut pending = std::collections::HashSet::new();
    for effect in &snapshot.pending_effects {
        if effect.status == kernel::OutboxStatus::Pending
            && effect.kind == kernel::OutboxEffectKind::DispatchTool
            && !dispatch_waits_for_approval(&snapshot.state, effect)
        {
            if let Some(id) = effect.tool_call_id.as_deref() {
                pending.insert(id);
            }
        }
    }
    let mut ordered: Vec<String> = snapshot
        .tool_calls
        .iter()
        .filter(|tool| tool.state == "running" && pending.contains(tool.tool_call_id.as_str()))
        .map(|tool| tool.tool_call_id.clone())
        .collect();
    for id in &pending {
        if !ordered.iter().any(|existing| existing == id) {
            ordered.push((*id).to_owned());
        }
    }
    ordered
}
type AfterCommitFn<'a> = &'a dyn Fn(&str) -> Result<(), String>;
type SettleChildrenFn<'a> = &'a dyn Fn(bool) -> Result<(), String>;

/// Test-only barrier: a callback invoked at the last moment before a round
/// decision is committed — after every queue read, before the write-set.
///
/// It lets a test place an additional request exactly inside the window the
/// terminal guard exists for, deterministically and once per Run, instead of
/// relying on timing. Hooks are keyed by Run id and removed when they fire, so
/// tests in the same process cannot interfere with each other.
#[cfg(test)]
pub(super) mod test_barrier {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    pub(crate) type Hook = Box<dyn Fn() + Send + 'static>;

    fn hooks() -> &'static Mutex<HashMap<String, Hook>> {
        static HOOKS: OnceLock<Mutex<HashMap<String, Hook>>> = OnceLock::new();
        HOOKS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn install(run_id: &str, hook: Hook) {
        hooks().lock().expect("barrier lock").insert(run_id.to_owned(), hook);
    }

    pub(crate) fn fire(run_id: &str) {        let hook = hooks().lock().expect("barrier lock").remove(run_id);
        if let Some(hook) = hook {
            hook();
        }
    }
}

impl KernelCoordinator<'_> {
    /// Bytes the next model request adds for the Run's outstanding additional
    /// input, including the notice wrapper the model actually sees.
    ///
    /// The context gate must be asked about the request that will really be sent,
    /// so this is what the dispatch adds to its fixed reserve.
    pub(super) fn active_steering_bytes(&self) -> Result<usize, String> {
        let active = self.database.active_run_steering(&self.binding.run_id)?;
        if active.is_empty() {
            // An empty notice list adds nothing to the request.
            return Ok(0);
        }
        // Measure the messages the model actually receives (wrapper text plus
        // the message envelope), not the compact notice the directive carries:
        // the budget is about the request, and the envelope is part of it.
        let messages: Vec<Value> = active
            .iter()
            .map(super::steering::steering_user_message)
            .collect();
        crate::kernel_compaction::bytes(&messages)
    }

    pub(super) fn stored_continuation_input(&self, key: &str) -> Result<fox_engine_protocol::KernelInitialModelInput, String> {
        self.database.kernel_continuation_input(&self.binding.run_id, key)
    }

    /// Remaining whole-Run execution budget as a wall deadline. The running
    /// clock excludes approval/execution waits, so this stays valid across a
    /// long approval whereas a fixed Instant captured at dispatch would not.
    fn live_deadline(&self) -> Result<Instant, String> {
        let remaining = self.live_remaining_ms()?;
        Ok(Instant::now() + Duration::from_millis(remaining.max(1) as u64))
    }

    /// Remaining budget for the next blocking transport wait, in milliseconds.
    /// This is the live-loop counterpart of the per-round formula: the minimum
    /// of the remaining whole-Run execution budget and the active model-request
    /// window. Recomputed before every worker wait so the bound tracks durable
    /// Host facts (and naturally pauses across approvals). When no model
    /// request is armed (for example during startup) only the Run budget applies.
    /// A bounded transport drain follows model expiry; late output cannot commit.
    /// Whether this Run was frozen with at least one tool that can actually
    /// change something. A conversational/read-only Run can be complete after a
    /// single answer, so the bounded stop-review (#17) must not spend a round on
    /// it; a Run that owns writable or process tools may still have open work.
    fn round_can_still_act(&self) -> Result<bool, String> {
        Ok(self
            .database
            .kernel_model_config(&self.binding.run_id)?
            .proposal_tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .any(|name| {
                fox_engine_protocol::canonical_runtime_tool_contract(name)
                    .is_some_and(|(category, _, _)| category != "project-read")
            }))
    }

    fn live_remaining_ms(&self) -> Result<i64, String> {
        let now = self.clock.read();
        let facts = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(now.monotonic_ms);
        let run_remaining = self.binding.budgets.remaining_run_ms(facts.running_elapsed_ms);
        if run_remaining.is_some_and(|remaining| remaining <= 0) {
            return Err("Kernel Run execution budget exhausted".into());
        }
        let model_remaining = match facts.model_request_since_wall_ms {
            Some(since) if now.wall_ms >= since => self
                .binding
                .budgets
                .model_request_ms
                .saturating_sub(now.wall_ms - since)
                .saturating_add(super::super::kernel_model_worker::MODEL_SETTLE_GRACE_MS),
            // Startup still has a finite transport window. Each subsequent
            // request gets its own deadline, independent of total task duration.
            _ => self.binding.budgets.model_request_ms,
        };
        Ok(run_remaining.map_or(model_remaining, |remaining| remaining.min(model_remaining)))
    }

    /// Recomputed deadline source passed to the live worker transport.
    fn live_deadline_for(&self) -> impl FnMut() -> Result<Instant, String> + '_ {
        move || self.live_deadline()
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

    pub(super) fn tool_result_messages(
        run_id: &str,
        assistant: &Value,
        tools: &[fox_engine_protocol::KernelSettledToolResult],
    ) -> Vec<Value> {
        tools
            .iter()
            .map(|tool| {
                let is_error = tool.state == fox_engine_protocol::KernelSettledToolState::Failed;
                // The durable result (tool.result) stays complete; only the
                // content projected into the model context is bounded for
                // re-readable reference tools. Errors and receipts pass through.
                // The bounded view names the persisted result so omitted content
                // stays reachable without re-running the tool.
                let reference =
                    crate::kernel_compaction::tool_result_ref(run_id, &tool.tool_call_id);
                // The boundable-tool rule must be judged on the operation that
                // actually ran. The Host dispatches every Office operation through
                // the `call_mcp_tool` wrapper, so matching the whitelist on the
                // wrapper name made its `office_read` entry unreachable: a
                // 927-row `office_read` then passed through verbatim at 2.29 MB,
                // which is past the model window and detaches the live loop.
                let boundable_tool =
                    effective_boundable_tool(&tool.tool, &tool.canonical_input);
                let mut content =
                    crate::kernel_compaction::bound_tool_result_content_with_storage(
                        boundable_tool,
                        is_error,
                        &tool.result["content"],
                        reference.as_deref(),
                        &tool.storage.clone().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default(),
                    )
                    .unwrap_or_else(|| tool.result["content"].clone());
                // `read_tool_result` carries its range cursor in `details`, which
                // the durable history strips. Surface the whitelisted navigation
                // facts as a trailing text block so a replayed batch still lets
                // the model continue a multi-page read.
                if tool.tool == crate::kernel_compaction::RESULT_REF_TOOL && !is_error {
                    if let Some(view) = crate::kernel_compaction::read_tool_result_model_view(&tool.result) {
                        content = view;
                    }
                }
                serde_json::json!({
                    "role":"toolResult", "toolCallId":tool.tool_call_id, "toolName":tool.tool,
                    "content":content, "details":{},
                    "isError":is_error,
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
            // Schedule pending dispatches in the model's source order. A
            // bounded maximal prefix of Host-classified independent read-only
            // calls executes concurrently; the first writer, generic MCP call
            // or dependent call ends the prefix and is handled serially after
            // the readers have all settled — a write never overlaps any call
            // that precedes it in source order, even when the model grouped
            // them in one proposal.
            let ordered_ids = pending_dispatch_ids(&snapshot);
            if let Some(first_id) = ordered_ids.first().cloned() {
                let pending_by_id: std::collections::HashMap<&str, &kernel::OutboxEffect> = snapshot
                    .pending_effects
                    .iter()
                    .filter(|effect| {
                        effect.status == kernel::OutboxStatus::Pending
                            && effect.kind == kernel::OutboxEffectKind::DispatchTool
                    })
                    .filter_map(|effect| {
                        effect.tool_call_id.as_deref().map(|id| (id, effect))
                    })
                    .collect();
                let group: Vec<kernel::OutboxEffect> = ordered_ids
                    .iter()
                    .take(MAX_PARALLEL_READ_ONLY)
                    .map_while(|id| {
                        let effect = (*pending_by_id.get(id.as_str())?).clone();
                        parallel_read_only_dispatch(&effect).then_some(effect)
                    })
                    .collect();
                if group.len() >= 2 {
                    self.dispatch_read_only_group(&group, owner, execute)?;
                } else {
                    // Empty prefix: the first call is a writer/unknown and is a
                    // serial barrier. One reader: no fan-out, keep serial path.
                    self.dispatch_tool(&first_id, owner, execute)?;
                }
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
        self.tick_settled_model()?;
        if self.model_request_window_expired()? {
            return Err(super::super::kernel_model_worker::MODEL_WINDOW_EXPIRED.into());
        }
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check().map_err(|_| LIVE_CANCEL.to_string())?;
        let output = frame.assistant_message;
        let tool_use = output["stopReason"] == "toolUse";

        // Mid-run steering is consumed only at dispatch boundaries: rows
        // already delivered to the request this round answers are appended to
        // the durable history in the exact order the live engine saw them,
        // while still-`received` rows wait until this round's decision arms the
        // next directive.
        let delivered_rows = self
            .database
            .delivered_run_steering(&self.binding.run_id)?;
        // Mutable: the end-of-turn decision re-reads this queue, and the rows it
        // finally delivers must be the same snapshot the directive, the frozen
        // follow-up input and the status flips are built from.
        let mut pending_rows = self
            .database
            .pending_run_steering(&self.binding.run_id)?;
        let delivered_messages: Vec<Value> = delivered_rows
            .iter()
            .map(super::steering::steering_user_message)
            .collect();
        let mut pending_messages: Vec<Value> = pending_rows
            .iter()
            .map(super::steering::steering_user_message)
            .collect();

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
                let mut pre = self.processed_initial_input()?;
                pre.extend(delivered_messages.iter().cloned());
                (
                    pre,
                    serde_json::to_string(&response).map_err(|_| "invalid response")?,
                    None,
                )
            }
            Stage::Batch { batch_id, cursor } => {
                let (history, assistant, tools) = self.stored_batch_parts(batch_id)?;
                let mut pre = history;
                pre.push(assistant.clone());
                pre.extend(Self::tool_result_messages(&self.binding.run_id, &assistant, &tools));
                // After settled tool results, matching the worker transcript
                // where the directive splices steering into the next request.
                pre.extend(delivered_messages.iter().cloned());
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
            Stage::Continuation { pre_history, cursor, .. } => {
                let response = serde_json::json!({
                    "schemaVersion": 1, "runId": self.binding.run_id, "turnId": frame.turn_id,
                    "checkpointSeq": cursor,
                    "assistantMessage": output,
                });
                (
                    pre_history.clone(),
                    serde_json::to_string(&response).map_err(|_| "invalid response")?,
                    None,
                )
            }
        };

        // At a non-tool stop, the end-of-turn state is decided from durable
        // facts before the commit that records the decision:
        //  * additional user input accepted while this round was in flight is
        //    answered first — it gets its own `steering` lane, prompt and round,
        //    so a normal completion can never discard a request the user was
        //    told Fox had received;
        //  * business delivery verification for tasks that explicitly promised
        //    files (Host checks real artifacts; bounded repairs use their own
        //    lane and counter, separate from this review and from model-failure
        //    retries);
        //  * the generic bounded stop-review, which gets one more round when
        //    work is plausibly unfinished — at least one tool already ran, or
        //    this is the first round of a Run frozen with an executable
        //    (non-read-only) tool but the model only produced an unbacked
        //    "I will start" preface;
        //  * a real final stop.
        // A genuinely complete no-tool answer to a read-only/conversational
        // request completes immediately, so a plain question is never charged
        // an extra model round. The executable-preface nudge fires once; the
        // post-review stop is accepted as the final answer.
        let mut delivery_outcome: Option<super::super::delivery::DeliveryStop> = None;
        let base_followup = if tool_use {
            StopFollowup::Final
        } else {
            let delivery = super::super::delivery::evaluate_stop(
                &self.database,
                self.binding.permission.project_root.as_deref(),
                &self.binding.run_id,
            )?;
            match delivery {
                super::super::delivery::DeliveryStop::NoChecklist => {
                    // #17/R8: a review is a correction for a stop that left
                    // explicit work open. "A tool ran at some point" is NOT
                    // evidence of unfinished work, so it cannot buy another model
                    // round - that is exactly how a complete answer used to cost
                    // an extra round.
                    //
                    // With no delivery checklist there is no machine-checkable
                    // demand left (the delivery lane owns those with its own
                    // evidence and counter). The one remaining case that needs a
                    // bounded nudge is a run that has already acted but whose stop
                    // only *promises* further work instead of delivering a result.
                    // Everything else completes as-is.
                    let review_budget_left = self
                        .database
                        .kernel_count_continuations(&self.binding.run_id)?
                        < CONTINUATION_LIMIT;
                    let draft = assistant_plain_text(&output);
                    // The nudge is for a model that stopped while *promising*
                    // further work - it says it will act (often right after
                    // reading something) but proposed no concrete step and
                    // delivered no result. That is the explicit unfinished work
                    // this lane corrects.
                    //
                    // It is deliberately NOT triggered by tool use: a run that
                    // executed a tool and then produced a complete answer has
                    // nothing open here (the delivery lane owns machine-checkable
                    // demands), so it must not be charged another model round.
                    let pending_jobs=self.database.kernel_jobs_for_run(&self.binding.run_id)?.iter().any(|job| !job.state.is_terminal());
                    if pending_jobs && !review_budget_left {return Err("kernel.jobs_pending".into());}
                    let promised_but_unstarted = pending_jobs || !looks_like_final_answer(&draft);
                    // Never while tools are still in flight: an unresolved call is
                    // not a promise, and the barrier (not this lane) owns it.
                    let no_tool_in_flight = self
                        .snapshot()?
                        .tool_calls
                        .iter()
                        .all(|tool| matches!(tool.state.as_str(), "completed" | "failed" | "cancelled" | "expired"));
                    let continuation_allowed =
                        review_budget_left && no_tool_in_flight && promised_but_unstarted;
                    if !continuation_allowed {
                        StopFollowup::Final
                    } else {
                        let mut input =
                            self.database.kernel_initial_input(&self.binding.run_id)?;
                        input.messages = pre_history.clone();
                        input.messages.push(output.clone());
                        // The frozen continuation payload deliberately omits
                        // queued steering: it stays sourced from
                        // run_steering_messages, so an external replacement
                        // dispatch appends each active row exactly once and the
                        // in-worker directive carries the same rows alongside
                        // this prompt.
                        input.messages.push(json!({"role":"user","content":[{"type":"text","text":CONTINUATION_PROMPT}],"timestamp":0}));
                        input.validate()?;
                        StopFollowup::Review(input)
                    }
                }
                super::super::delivery::DeliveryStop::Repair {
                    items,
                    prompt,
                    findings_json,
                } => {
                    let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
                    input.messages = pre_history.clone();
                    input.messages.push(output.clone());
                    input.messages.push(json!({"role":"user","content":[{"type":"text","text":prompt.clone()}],"timestamp":0}));
                    input.validate()?;
                    delivery_outcome = Some(super::super::delivery::DeliveryStop::Repair {
                        items,
                        prompt: prompt.clone(),
                        findings_json,
                    });
                    StopFollowup::Repair { prompt, input }
                }
                other => {
                    // Kept only until the steering re-check below decides this
                    // round's actual end state: a delivery ledger row is written
                    // once, for the decision that really commits.
                    delivery_outcome = Some(other);
                    StopFollowup::Final
                }
            }
        };

        // Accepted-but-unanswered user input outranks every other follow-up
        // except a tool proposal. Its own lane and its own bounded counter keep
        // it from consuming the stop-review budget; the ceiling below keeps one
        // queue of additions from extending a Run without limit. The rows that
        // are delivered by this decision are decided after the re-read so the
        // steering notices, the dispatch row and the lane all agree.
        let mut stop_followup = if !tool_use
            && !pending_rows.is_empty()
            && self
                .database
                .kernel_count_steering_followups(&self.binding.run_id)?
                < STEERING_FOLLOWUP_LIMIT
        {
            let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
            input.messages = pre_history.clone();
            input.messages.push(output.clone());
            input.messages.push(json!({"role":"user","content":[{"type":"text","text":STEERING_PROMPT}],"timestamp":0}));
            input.validate()?;
            delivery_outcome = None;
            StopFollowup::Steering(input)
        } else {
            base_followup
        };
        if !matches!(stop_followup, StopFollowup::Steering(_)) {
            // Re-read at the authoritative boundary: a request accepted between
            // the gate above and this point must still be answered, never
            // silently finished. Adopting the re-read snapshot (not just acting
            // on it) keeps the delivered set, the frozen follow-up input and the
            // directive on one queue version; the terminal decision re-checks the
            // same condition inside its write-set.
            let still_pending = self
                .database
                .pending_run_steering(&self.binding.run_id)?;
            if !tool_use
                && !still_pending.is_empty()
                && self
                    .database
                    .kernel_count_steering_followups(&self.binding.run_id)?
                    < STEERING_FOLLOWUP_LIMIT
                && !matches!(
                    stop_followup,
                    StopFollowup::Review(_) | StopFollowup::Repair { .. }
                )
            {
                pending_messages = still_pending
                    .iter()
                    .map(super::steering::steering_user_message)
                    .collect();
                pending_rows = still_pending;
                let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
                input.messages = pre_history.clone();
                input.messages.push(output.clone());
                input.messages.push(json!({"role":"user","content":[{"type":"text","text":STEERING_PROMPT}],"timestamp":0}));
                input.validate()?;
                delivery_outcome = None;
                stop_followup = StopFollowup::Steering(input);
            }
        }

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

        // Queued rows are bound to the directive this decision arms (a batch
        // directive after a tool proposal, a continuation directive after a
        // non-tool follow-up). A Final response arms nothing: the terminal
        // write-set cancels open rows instead.
        let response_event_seq = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .last_event_seq()
            + 1;
        let directive_follows = tool_use
            || matches!(
                stop_followup,
                StopFollowup::Review(_)
                    | StopFollowup::Repair { .. }
                    | StopFollowup::Steering(_)
            );
        let steering_dispatch_key = format!("round-response:{response_event_seq}");
        // Any decision that arms a directive carries the notices, so it adopts
        // every row that is `received` at commit time — including one accepted
        // after the reads above. That is what closes the window instead of
        // failing the whole round for a race the user cannot see.
        let mut steering_decision = directive_follows.then(|| crate::database::SteeringDecision {
            deliver_seqs: pending_rows.iter().map(|row| row.seq).collect(),
            dispatch_key: steering_dispatch_key.clone(),
            adopt_all_received: true,
        });

        // Commit the recorded response together with its next-step decision.
        //
        // A request accepted between the re-read above and this write-set can
        // still make a **Final** decision refuse (a Final arms no directive, so
        // nothing can carry the new row). That is a scheduling condition, not an
        // execution failure: the Host still holds this round's model result, the
        // write-set rolled back with the model request still leased, so the only
        // safe and useful recovery is to answer the new request *in this round*
        // and commit again. Detaching here would leave a leased model dispatch
        // behind, which the drive loop must treat as uncertain — turning "the
        // user added one sentence" into a failed task.
        let mut attempts = 0u8;
        let next_batch = loop {
            #[cfg(test)]
            test_barrier::fire(&self.binding.run_id);
            match self.commit_round_decision(
                stage,
                answered_batch.as_deref(),
                owner,
                policy,
                &response_json,
                &next_checkpoint,
                &stop_followup,
                steering_decision.as_ref(),
            ) {
                Ok(next_batch) => break next_batch,
                Err(error)
                    if error.starts_with(kernel::STEERING_COMPETITION)
                        && attempts < STEERING_COMPETITION_ATTEMPTS =>
                {
                    attempts += 1;
                    let arrived = self
                        .database
                        .pending_run_steering(&self.binding.run_id)?;
                    if arrived.is_empty() {
                        // The queue is empty again (an explicit cancel raced us);
                        // nothing can be planned for it, so this is a real
                        // refusal.
                        return Err(error);
                    }
                    // Answer the accepted request with its own lane round, built
                    // from the same history this round saw plus its reply.
                    let mut input = self.database.kernel_initial_input(&self.binding.run_id)?;
                    input.messages = pre_history.clone();
                    input.messages.push(output.clone());
                    input.messages.push(json!({"role":"user","content":[{"type":"text","text":STEERING_PROMPT}],"timestamp":0}));
                    input.validate()?;
                    pending_messages = arrived
                        .iter()
                        .map(super::steering::steering_user_message)
                        .collect();
                    pending_rows = arrived;
                    delivery_outcome = None;
                    stop_followup = StopFollowup::Steering(input);
                    // Adopt-all: whatever else arrives before the retry commits
                    // rides the same round instead of refusing it again.
                    steering_decision = Some(crate::database::SteeringDecision {
                        deliver_seqs: pending_rows.iter().map(|row| row.seq).collect(),
                        dispatch_key: steering_dispatch_key.clone(),
                        adopt_all_received: true,
                    });
                }
                Err(error) => return Err(error),
            }
        };

        // The directive and the next round's history must describe the rows that
        // were durably bound by this decision — including any that arrived in
        // the window above — so read them back instead of trusting the snapshot.
        if let Some(decision) = steering_decision.as_ref() {
            let bound = self
                .database
                .bound_run_steering(&self.binding.run_id, &decision.dispatch_key)?;
            pending_messages = bound
                .iter()
                .map(super::steering::steering_user_message)
                .collect();
            pending_rows = bound;
        }

        // The business delivery ledger is written only after the stop decision
        // is durable: a crash in between fails closed (item stays pending)
        // instead of recording a phantom pass or rewriting history.
        if let Some(outcome) = &delivery_outcome {
            super::super::delivery::persist_outcome(
                &self.database,
                &self.binding.run_id,
                outcome,
                crate::database::now_ms(),
            )?;
        }

        if let Some((batch_id, _checkpoint)) = next_batch {
            // Drive the just-proposed batch to the durable barrier, hand the
            // settled results to the live session and arm the next round.
            self.drive_batch_to_barrier(owner, &batch_id, execute, after_commit, settle_children)?;
            let (history, assistant, tools) = self.stored_batch_parts(&batch_id)?;
            let config = self.database.kernel_model_config(&self.binding.run_id)?;
            // The budget must cover the bytes this request really sends: the
            // assistant turn, the settled tool results, the steering notices the
            // directive carries, and the fixed per-request reserve. Leaving
            // steering out here let a long task overshoot the frozen window.
            let steering_bytes = crate::kernel_compaction::bytes(
                &super::steering::steering_notices(&pending_rows),
            )?;            let extra = crate::kernel_compaction::bytes(&assistant)?
                .saturating_add(crate::kernel_compaction::bytes(&Self::tool_result_messages(
                    &self.binding.run_id,
                    &assistant,
                    &tools,
                ))?)
                .saturating_add(steering_bytes)
                .saturating_add(4096);
            let view = self.context_view(&batch_id, &history)?;
            if !self.context_fits(&config, &view, extra)? {
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
                preview_seq: None,
                tools,
                prompt: None,
                steering: super::steering::steering_notices(&pending_rows),
            });
        }

        // Every continuation lane sends the same thing: the follow-up prompt,
        // the durable history, and the queued steering rows. The budget below
        // therefore measures the exact bytes of the next request — including the
        // steering notices appended to it — before the dispatch is armed.
        let armed_followup = match &stop_followup {
            StopFollowup::Final => None,
            StopFollowup::Review(input) => Some((CONTINUATION_PROMPT.to_owned(), input)),
            StopFollowup::Repair { prompt, input } => Some((prompt.clone(), input)),
            StopFollowup::Steering(input) => Some((STEERING_PROMPT.to_owned(), input)),
        };
        if let Some((prompt, input)) = armed_followup {
            let config = self.database.kernel_model_config(&self.binding.run_id)?;
            // The steering notices travel in the directive and the worker splices
            // them into this very model request, so they belong to this request's
            // byte budget. Leaving them out let a Run overshoot the frozen window
            // and skip the compaction/replacement transport it should have used.
            let mut next_messages = input.messages.clone();
            next_messages.extend(pending_messages.iter().cloned());
            // `next_messages` already contains the notices the worker will splice
            // into this request, so only the fixed per-request reserve is added.
            if !self.context_fits(
                &config,
                &next_messages,
                4096,
            )? {
                return Err(LIVE_DETACHED.into());
            }
            let cursor = self.controller.lock().map_err(|_| "Kernel coordinator lock poisoned")?.last_event_seq();
            let effect_key = format!("continuation:{cursor}");
            self.apply(Some(DecisionLease::ContinuationDispatch(&effect_key, owner)), |controller, now|
                controller.begin_continuation_model_request(&effect_key, now.monotonic_ms, now.wall_ms))?;
            // The frozen payload stored by the continuation request excludes
            // steering (single source: run_steering_messages); mirror the
            // worker transcript by appending the just-delivered rows so the
            // next round's durable history matches what the live model sees.
            let mut continuation_pre = input.messages.clone();
            continuation_pre.extend(pending_messages.iter().cloned());
            *stage = Stage::Continuation { effect_key, pre_history: continuation_pre, cursor };
            return Ok(fox_engine_protocol::KernelRoundDirective {
                schema_version: 1,
                kind: fox_engine_protocol::KernelRoundDirectiveKind::Continuation,
                batch_id: None,
                checkpoint_seq: None,
                preview_seq: Some(cursor),
                tools: Vec::new(),
                prompt: Some(prompt),
                steering: super::steering::steering_notices(&pending_rows),
            });
        }

        Ok(fox_engine_protocol::KernelRoundDirective {
            schema_version: 1,
            kind: fox_engine_protocol::KernelRoundDirectiveKind::Final,
            batch_id: None,
            checkpoint_seq: None,
            preview_seq: None,
            tools: Vec::new(),
            prompt: None,
            // A Final directive can never carry steering: open rows were
            // cancelled inside the terminal write-set.
            steering: Vec::new(),
        })
    }

    /// Commit one round's response together with its next-step decision.
    ///
    /// Extracted from the live round handler so a lost terminal race can be
    /// re-planned and committed again in the same call, with the model response
    /// the Host already holds — instead of leaving the model request leased and
    /// the Run un-recoverable.
    #[allow(clippy::too_many_arguments)]
    fn commit_round_decision(
        &self,
        stage: &Stage,
        answered_batch: Option<&str>,
        owner: &str,
        policy: &dyn kernel::PolicyDecisionPort,
        response_json: &str,
        next_checkpoint: &Option<(String, fox_engine_protocol::KernelEngineBatchCheckpoint)>,
        stop_followup: &StopFollowup,
        steering_decision: Option<&crate::database::SteeringDecision>,
    ) -> Result<Option<(String, fox_engine_protocol::KernelEngineBatchCheckpoint)>, String> {
        let next_checkpoint = next_checkpoint.clone();
        match (answered_batch, stage) {
            (Some(batch_id), _) => {
                let batch_id = batch_id.to_owned();
                self.apply_decision(
                    Some(DecisionLease::Batch(&batch_id, owner)),
                    |controller, now| {
                        let mut effects =
                            vec![controller.record_batch_model_response(&batch_id, response_json)?];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            stop_followup,
                        )?);
                        Ok(effects)
                    },
                    steering_decision,
                )?;
            }
            (None, Stage::Initial { .. }) => {
                self.apply_decision(
                    Some(DecisionLease::Initial(owner)),
                    |controller, now| {
                        let mut effects =
                            vec![controller.record_initial_model_response(response_json)?];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            stop_followup,
                        )?);
                        Ok(effects)
                    },
                    steering_decision,
                )?;
            }
            (None, Stage::Continuation { effect_key, .. }) => {
                self.apply_decision(
                    Some(DecisionLease::Continuation(effect_key, owner)),
                    |controller, now| {
                        let mut effects =
                            vec![controller.record_continuation_model_response(response_json)?];
                        effects.extend(commit_tail(
                            controller,
                            now,
                            policy,
                            &next_checkpoint,
                            stop_followup,
                        )?);
                        Ok(effects)
                    },
                    steering_decision,
                )?;
            }
            (None, Stage::Batch { .. }) => return Err("continuation stage mismatch".into()),
        }
        Ok(next_checkpoint)
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
        self.dispatch_live(owner, policy, runtime, api_key, execute, after_commit, settle_children, None)
    }

    pub(crate) fn dispatch_continuation_live(
        &self,
        effect_key: &str,
        owner: &str,
        policy: &dyn kernel::PolicyDecisionPort,
        runtime: &super::super::RuntimeCommand,
        api_key: &str,
        execute: ExecuteFn,
        after_commit: AfterCommitFn,
        settle_children: SettleChildrenFn,
    ) -> Result<(), String> {
        self.dispatch_live(owner, policy, runtime, api_key, execute, after_commit, settle_children, Some(effect_key))
    }

    fn dispatch_live(
        &self,
        owner: &str,
        policy: &dyn kernel::PolicyDecisionPort,
        runtime: &super::super::RuntimeCommand,
        api_key: &str,
        execute: ExecuteFn,
        after_commit: AfterCommitFn,
        settle_children: SettleChildrenFn,
        continuation_key: Option<&str>,
    ) -> Result<(), String> {
        // The request this dispatch will actually send includes the queued
        // additional-input rows. Their wrapper bytes therefore belong to the
        // context budget the compaction gate is asked about: checking only the
        // frozen history (plus the fixed reserve) let a Run whose history fits
        // but whose history-plus-queue does not detach on every attempt without
        // ever planning a compaction, so no attempt could make progress.
        //
        // The active rows are read before delivery flips them, and are exactly
        // the rows `deliver_steering_for_dispatch` returns below.
        let planned_steering_bytes = self.active_steering_bytes()?;
        let context_key = continuation_key.unwrap_or("initial");
        self.ensure_context_with_worker(
            context_key,
            planned_steering_bytes.saturating_add(4096),
            owner,
            runtime,
            api_key,
        )?;
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
        let mut input = if let Some(key) = continuation_key { self.stored_continuation_input(key)? }
            else { self.database.kernel_initial_input(&self.binding.run_id)? };
        input.messages = self.model_retry_context(context_key, self.context_view(context_key, &input.messages)?)?;
        // Consume accepted mid-run additions at the model-dispatch boundary:
        // every active row (new `received` plus rows already delivered to a
        // failed attempt of this same dispatch) is spliced into the frozen
        // input exactly once and the status flip commits before the frame is
        // handed to the worker. A reopen/replacement dispatch re-collects the
        // same rows instead of rewriting the already-sent request.
        let steering_dispatch_key = match continuation_key {
            Some(key) => format!("continuation-delivery:{key}"),
            None => kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.to_string(),
        };
        let steering_rows = self
            .database
            .deliver_steering_for_dispatch(&self.binding.run_id, &steering_dispatch_key)?;
        let steering_messages: Vec<Value> = steering_rows
            .iter()
            .map(super::steering::steering_user_message)
            .collect();
        // Post-compaction confirmation against the final assembled input. The
        // steering notices are already inside `planned_messages`, so only the
        // fixed per-request reserve is added here — counting them twice would
        // detach a request that does fit.
        let mut planned_messages = input.messages.clone();
        planned_messages.extend(steering_messages.iter().cloned());
        let config_for_budget = self.database.kernel_model_config(&self.binding.run_id)?;
        if !self.context_fits(
            &config_for_budget,
            &planned_messages,
            4096,
        )? {
            // The compaction-aware replacement transport has already built the
            // final request. Re-detaching the same input cannot make it smaller.
            return Err(crate::kernel_compaction::INSUFFICIENT.into());
        }
        input.messages.extend(steering_messages);
        let continuation_history = input.messages.clone();
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
                idempotency_key: continuation_key.map(|key| format!("continuation-delivery:{key}"))
                    .unwrap_or_else(|| kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into()),
                continuation_key: continuation_key.map(str::to_owned),
                checkpoint_seq: guard.last_event_seq(),
            };
            frame.validate()?;
            let mut candidate = guard.clone();
            let now = self.clock.read();
            if let Some(key) = continuation_key {
                let effects = candidate.begin_continuation_model_request(key, now.monotonic_ms, now.wall_ms)
                    .map_err(|error| error.to_string())?;
                self.database.kernel_commit_continuation_model(&self.binding.run_id, now.wall_ms,
                    &candidate.persist_command(&effects), key, owner, false, None)?;
            } else {
                let effects = candidate.begin_initial_model_request(now.monotonic_ms, now.wall_ms)
                    .map_err(|error| error.to_string())?;
                self.database.kernel_commit_initial_model(&self.binding.run_id, now.wall_ms,
                    &candidate.persist_command(&effects), owner, false, None)?;
            }
            *guard = candidate;
            frame
        };
        token.check()?;
        let mut stage = match continuation_key {
            Some(key) => Stage::Continuation { effect_key: key.into(), pre_history: continuation_history, cursor: frame.checkpoint_seq },
            None => Stage::Initial { cursor: frame.checkpoint_seq },
        };
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
        let request_payload =
            json!({"controlBinding": self.binding, "initialModel": frame, "streamPreview": true});
        let mut deadline_for = self.live_deadline_for();
        let outcome = session.run(
            "kernel.start_initial",
            request_payload,
            &self.binding,
            &token,
            &mut deadline_for,
            self.preview,
            &mut service,
        );
        // Killing/reaping the old worker is a prerequisite for admitting any retry.
        drop(session);
        match outcome {
            Ok(_) => Ok(()),
            Err(error) if error == LIVE_CANCEL || error == LIVE_DETACHED => Err(error),
            // A settled model failure keeps the frozen retry semantics: the
            // drive loop schedules the durable retry and a replacement session
            // re-runs only the leased model round from durable facts, including
            // continuations. Tool execution effects are never reset here.
            Err(error) => match &stage {
                Stage::Initial { cursor } => self.retry_settled_model(
                    kernel::INITIAL_MODEL_EFFECT_KEY,
                    owner,
                    *cursor,
                    error,
                ),
                Stage::Batch { batch_id, cursor } => self.retry_settled_model(
                    &kernel::batch_delivery_effect_key(batch_id),
                    owner,
                    *cursor,
                    error,
                ),
                Stage::Continuation { effect_key, cursor, .. } => {
                    self.retry_settled_model(effect_key, owner, *cursor, error)
                }
            },
        }
    }
}

fn kernel_state_is_terminal(state: &str) -> bool {
    matches!(
        state,
        "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired"
    )
}

/// Whether a first-round plain stop already reads as a delivered answer rather
/// than a promise to start working.
///
/// This exists so the bounded stop-review (#17) is not spent on a task that never
/// needed a tool: a complete read-only answer must finish in one round. It only
/// *suppresses* the review for a draft that carries no forward-looking intent, so
/// a draft that announces future work still gets its one correction.
fn assistant_plain_text(message: &Value) -> String {
    match &message["content"] {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks.iter().filter(|b| b["type"]=="text")
            .filter_map(|b| b["text"].as_str()).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn looks_like_final_answer(draft: &str) -> bool {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return false;
    }
    let lowered = trimmed.to_lowercase();
    // Correct only a stand-alone promise, never a complete answer merely
    // containing sequence words or recommendations. Delivery/steering retain
    // their own authoritative checks for specific outstanding requirements.
    let sentence = lowered.trim_end_matches(['.', '!', '。', '！', '…']);
    let standalone = !sentence.contains(['\n', '.', '。', '!', '！', '?', '？']);
    let promise = ["我将", "我会", "让我", "接下来我", "下一步我", "现在我", "我先",
        "i will ", "i'll ", "let me ", "i am going to ", "i’m going to "]
        .iter().any(|prefix| sentence.starts_with(prefix));
    !(standalone && promise)
}

#[cfg(test)]
impl<'a> KernelCoordinator<'a> {
    /// Test accessor for the recomputed live execution budget.
    pub(super) fn live_remaining_ms_for_test(&self) -> i64 {
        self.live_remaining_ms().expect("live remaining budget")
    }

    /// Commit a plain "completed" decision, deliberately skipping the transport
    /// step that defers a non-tool stop into the `steering` lane. The terminal
    /// decision itself must still refuse while accepted input is unanswered, so
    /// this accessor proves the guard is not merely a caller convention.
    pub(super) fn complete_bypassing_the_steering_lane_for_test(&self) -> Result<(), String> {
        self.apply(None, |controller, _| {
            Ok(controller.terminate(kernel::RunOutcome::Completed))
        })
    }

    /// Durable event sequence this Run has committed.
    pub(super) fn last_event_seq_for_test(&self) -> u64 {
        self.controller
            .lock()
            .expect("Kernel coordinator lock")
            .last_event_seq()
    }
}

#[allow(clippy::too_many_arguments)]
fn commit_tail(
    controller: &mut kernel::RunController,
    now: kernel::ClockReading,
    policy: &dyn kernel::PolicyDecisionPort,
    next_checkpoint: &Option<(String, fox_engine_protocol::KernelEngineBatchCheckpoint)>,
    stop_followup: &StopFollowup,
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
    match stop_followup {
        StopFollowup::Final => Ok(controller.terminate(kernel::RunOutcome::Completed)),
        StopFollowup::Review(input) => controller.request_continuation(
            CONTINUATION_PROMPT,
            &serde_json::to_string(input)
                .map_err(|error| kernel::KernelError::FailClosed(error.to_string()))?,
        ),
        StopFollowup::Repair { prompt, input } => controller.request_delivery_repair(
            prompt,
            &serde_json::to_string(input)
                .map_err(|error| kernel::KernelError::FailClosed(error.to_string()))?,
        ),
        StopFollowup::Steering(input) => controller.request_steering_followup(
            STEERING_PROMPT,
            &serde_json::to_string(input)
                .map_err(|error| kernel::KernelError::FailClosed(error.to_string()))?,
        ),
    }
}

/// The operation a settled call is judged by when bounding the model view.
///
/// `office_read` is named in `BOUNDABLE_TOOLS`, but the Host never dispatches it
/// under that name: the built-in connector is always reached through the
/// `call_mcp_tool` wrapper, so a match on the wrapper name made the whitelist
/// entry dead code and let a 927-row sheet read pass through verbatim.
///
/// Only the built-in Office connector is unwrapped, and only for the read-only
/// operations the whitelist already accepts. A *generic* MCP call keeps
/// `call_mcp_tool`: its side-effect and re-read semantics are unknown, which is
/// what the whitelist's own reasoning requires. An unrecognised or write-form
/// inner operation also keeps the wrapper name, so it is never bounded.
pub(super) fn effective_boundable_tool<'a>(tool: &'a str, canonical_input: &'a Value) -> &'a str {
    const UNWRAPPABLE: &[&str] = &["office_read", "office_help", "office_validate"];
    if tool != "call_mcp_tool" {
        return tool;
    }
    if canonical_input["serverId"].as_str() != Some(crate::office::SERVER_ID) {
        return tool;
    }
    match canonical_input["tool"].as_str() {
        Some(inner) if UNWRAPPABLE.contains(&inner) => {
            crate::kernel_compaction::boundable_tool_name(inner).unwrap_or(tool)
        }
        _ => tool,
    }
}

#[cfg(test)]
mod range_model_view_tests {
    use super::{effective_boundable_tool, KernelCoordinator};
    use serde_json::{json, Value};

    /// The Host reaches every Office operation through `call_mcp_tool`, so the
    /// boundable-tool whitelist has to be judged on the inner operation. Judging
    /// it on the wrapper made the whitelist's `office_read` entry unreachable and
    /// let a 927-row sheet read through at 2.29 MB, past the model window.
    #[test]
    fn the_office_wrapper_is_unwrapped_for_view_bounding_but_a_generic_mcp_call_is_not() {
        // A built-in read-only Office operation: bounded, by its own name.
        for inner in ["office_read", "office_help", "office_validate"] {
            let input = json!({"serverId": crate::office::SERVER_ID, "tool": inner, "arguments": {}});
            assert_eq!(
                effective_boundable_tool("call_mcp_tool", &input),
                inner,
                "{inner} must be judged by its own name"
            );
        }
        // A write: never bounded, so the wrapper name stands.
        let write = json!({"serverId": crate::office::SERVER_ID, "tool": "office_create", "arguments": {}});
        assert_eq!(effective_boundable_tool("call_mcp_tool", &write), "call_mcp_tool");
        // An unknown inner operation: the wrapper name stands.
        let unknown = json!({"serverId": crate::office::SERVER_ID, "tool": "office_future", "arguments": {}});
        assert_eq!(
            effective_boundable_tool("call_mcp_tool", &unknown),
            "call_mcp_tool"
        );
        // A generic MCP server keeps its wrapper: re-read semantics are unknown,
        // which is exactly what the whitelist documents.
        let generic = json!({"serverId": "some-other-server", "tool": "office_read", "arguments": {}});
        assert_eq!(
            effective_boundable_tool("call_mcp_tool", &generic),
            "call_mcp_tool"
        );
        // A directly dispatched tool is untouched.
        assert_eq!(effective_boundable_tool("read", &json!({})), "read");
    }

    #[test]
    fn settled_history_exposes_range_cursor_without_rewriting_host_results() {
        let tools: Vec<fox_engine_protocol::KernelSettledToolResult> = serde_json::from_value(json!([{
            "toolCallId": "range-read", "tool": "read_tool_result", "sourceOrder": 0,
            "canonicalInput": {"reference": "fox-result://source-run/read-once"}, "state": "completed",
            "result": {
                "content": [{"type": "text", "text": "中A"},
                    {"type": "text", "text": "FOX_EXECUTION_RECEIPT_V1\n{\"executionState\":\"completed\"}"}],
                "details": {"reference": "fox-result://source-run/read-once", "offset": 0,
                    "returnedBytes": 4, "nextOffset": 4, "complete": false, "originalBytes": 8,
                    "retrievable": true, "truncated": false, "privateDiagnostic": "must-not-leak"}
            }
        }])).unwrap();
        let before = serde_json::to_value(&tools).unwrap();
        let messages = KernelCoordinator::tool_result_messages("reading-run", &json!({"timestamp": 1}), &tools);
        assert_eq!(serde_json::to_value(&tools).unwrap(), before);
        assert_eq!(messages[0]["details"], json!({}));
        assert_eq!(messages[0]["content"][0], tools[0].result["content"][0]);
        assert_eq!(messages[0]["content"][1], tools[0].result["content"][1]);
        let cursor: Value = serde_json::from_str(messages[0]["content"][2]["text"].as_str().unwrap()
            .strip_prefix("FOX_RESULT_CURSOR_V1 ").unwrap()).unwrap();
        assert_eq!(cursor["nextOffset"], json!(4));
        assert_eq!(cursor["complete"], json!(false));
        assert!(!messages[0].to_string().contains("must-not-leak"));
    }
}

#[cfg(test)]
mod stop_completion_tests {
    use super::looks_like_final_answer;
    #[test]
    fn short_answers_and_delivered_recommendations_do_not_trigger_a_review() {
        for answer in ["完成。", "42", "首先核对源数据，然后按原因统计。", "下一步建议优化调度优先级。",
            "分析完成。\n我会建议先处理高频原因。"] {
            assert!(looks_like_final_answer(answer), "{answer}");
        }
        for preface in ["", "我先查看附件结构。", "I will now summarize the stored result.", "让我继续分析。"] {
            assert!(!looks_like_final_answer(preface), "{preface}");
        }
    }
}
