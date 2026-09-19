//! One owning loop per durable Run. Model workers and resource executors finish
//! (including cancellation cleanup) before the loop can publish a terminal.
use super::{
    kernel_coordinator::KernelCoordinator, kernel_run_lock::KernelRunLock, RuntimeCommand,
};
use crate::{
    database::Database,
    kernel::{
        self, CancellationPort, CancellationRegistry, Clock, OutboxEffectKind, OutboxStatus,
        PolicyDecisionPort,
    },
};
use fox_engine_protocol::RunControlBinding;
use serde_json::Value;
use std::{path::Path, time::Duration};

fn terminal(state: &str) -> bool {
    matches!(
        state,
        "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired"
    )
}

/// Record the scope a conversation-level approval actually granted (#5).
///
/// `allow_once` is deliberately excluded: a one-shot decision must never become
/// a reusable permission. The scope is recomputed from the call's own input with
/// the same helper the policy uses, and it is only registered when the current
/// frozen policy would still have *asked* for these exact parameters - so the
/// approval can never widen anything (frozen read-only, an out-of-catalog tool,
/// a lifecycle hook and a denied scope all refuse).
fn register_approval_grant(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    decision: kernel::ApprovalDecision,
) -> Result<(), String> {
    if decision != kernel::ApprovalDecision::AllowConversation {
        // A one-shot approval is auditable as a decision but must never be
        // registered as a durable permission.
        return database
            .kernel_register_authorization_grant(
                run_id,
                tool_call_id,
                decision.as_str(),
                "",
                None,
            )
            .map(|_| ());
    }
    // The approval row itself is the durable record of what was asked. Nothing
    // is registered unless a decision for exactly this call exists.
    let Some((tool, input_json)) = database.kernel_decided_approval_call(run_id, tool_call_id)? else {
        return Ok(());
    };
    let binding = database.run_control_binding(run_id)?;
    let Some(binding) = binding else {
        return Ok(());
    };
    // Only a call the frozen policy still asks about can be covered by a grant.
    // Everything else (frozen read-only, an out-of-catalog tool, a lifecycle
    // hook, an already-granted scope) refuses without registering anything.
    if super::shadow_reconcile::frozen_kernel_tool_policy(
        &serde_json::json!({
            "mode": binding.permission.mode.as_str(),
            "projectRoot": binding.permission.project_root,
            "grants": binding.permission.grants.iter()
                .map(|grant| serde_json::json!([grant.tool, grant.scope]))
                .collect::<Vec<_>>(),
        }),
        &tool,
        &input_json,
    ) != kernel::PolicyDecision::RequireApproval
    {
        return Ok(());
    }
    let Ok(input) = serde_json::from_str::<Value>(&input_json) else {
        return Ok(());
    };
    let scope = super::shadow_reconcile::tool_operation_scope(
        &tool,
        &input,
        binding.permission.project_root.as_deref(),
    );
    database
        .kernel_register_authorization_grant(
            run_id,
            tool_call_id,
            decision.as_str(),
            &tool,
            scope.as_deref(),
        )
        .map(|_| ())
}

pub(crate) fn resource_failure_result(tool: &str, error: &str) -> Value {
    // Reader errors describe only the authorized path operation (missing path,
    // nonexistent file, OS access failure, scope escape). Keep them actionable.
    // Other executors may return remote bodies or credentials; do not forward.
    let message = if crate::resource_gateway::is_reader(tool) || tool == "attachment_compute" {
        format!("Project file operation failed: {}", error.chars().take(600).collect::<String>())
    } else {
        "The resource request failed. No successful result is available.".to_owned()
    };
    serde_json::json!({"content":[{"type":"text","text":message}],
        "details":{"code":"kernel.resource_failed","tool":tool}})
}

#[cfg(test)]
mod failure_tests {
    #[test]
    fn kernel_reader_failure_preserves_reason_without_claiming_policy_denial() {
        let result = super::resource_failure_result("ls", "tool path cannot be resolved: file not found");
        assert!(result["content"][0]["text"].as_str().unwrap().contains("file not found"));
        assert!(!result.to_string().contains("frozen policy"));
        let remote = super::resource_failure_result("http_request", "response includes private token");
        assert!(!remote.to_string().contains("private token"));
    }
}

/// The lock is acquired before preparation by startup, and before recovery by
/// restart. Passing it here keeps ownership until every executor has returned.
#[cfg(test)]
pub(super) fn drive(
    ownership: KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    // Sync lets the live batch loop run Host-classified independent read-only
    // calls concurrently; classification and leases stay Host-side.
    execute: impl Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
        + Sync,
) -> Result<(), String> {
    drive_with_actions(
        &ownership,
        database,
        clock,
        cancellation,
        run_id,
        runtime,
        api_key,
        policy,
        execute,
        |_| Ok(()),
        |_| Ok(()),
        &|_| {},
    )
}

pub(super) fn drive_with_actions(
    _ownership: &KernelRunLock,
    database: &Database,
    clock: &dyn Clock,
    cancellation: &CancellationRegistry,
    run_id: &str,
    runtime: &RuntimeCommand,
    api_key: &str,
    policy: &dyn PolicyDecisionPort,
    // Sync: the live loop may fan out Host-classified independent read-only
    // tool calls onto bounded worker threads.
    execute: impl Fn(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>
        + Sync,
    after_commit: impl Fn(&str) -> Result<(), String>,
    settle_children: impl Fn(bool) -> Result<(), String>,
    preview: &super::kernel_model_worker::PreviewSink,
) -> Result<(), String> {
    // Validate all frozen inputs before any recovery dispatch. Never reconstruct
    // a missing input/scope/model from current settings.
    database.kernel_host_scope(run_id)?;
    database.kernel_initial_input(run_id)?;
    let coordinator = if database.kernel_host_run_state(run_id)?.as_deref() == Some("created") {
        KernelCoordinator::start_prepared(database, clock, run_id, cancellation)?
    } else {
        KernelCoordinator::reopen(database, clock, run_id, cancellation)?
    }
    .with_preview(preview);
    let owner = format!("kernel-host:{}", uuid::Uuid::new_v4());

    loop {
        // Drain durable post-result actions and all child ownership before any
        // operation below can publish a parent terminal (including a timeout).
        // The action CAS independently refuses dispatch after queued cancel.
        for tool_id in database.pending_kernel_host_action_ids(run_id)? {
            after_commit(&tool_id)?;
        }
        settle_children(false)?;
        let commands = database.pending_kernel_host_commands(run_id)?;
        for command in commands {
            let snapshot = coordinator.snapshot()?;
            if !terminal(&snapshot.state) {
                if command.kind == "cancel" {
                    coordinator.cancel()?;
                    // This loop executes resources synchronously; no resource is
                    // still in flight once control has returned to this point.
                    coordinator.settle_cancellation()?;
                } else {
                    coordinator.tick()?;
                    let current = coordinator.snapshot()?;
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
                        coordinator.resolve_approval(tool_id, decision)?;
                        // #5: a conversation-level allow becomes an auditable
                        // additional grant; allow_once never does.
                        register_approval_grant(database, run_id, tool_id, decision)?;
                    }
                }
            }
            database.complete_kernel_host_command(run_id, command.seq)?;
        }
        coordinator.tick()?;
        let snapshot = coordinator.snapshot()?;
        if terminal(&snapshot.state) {
            return Ok(());
        }
        if snapshot.state == "cancelling" {
            coordinator.settle_cancellation()?;
            continue;
        }
        // Known pre-execution rejection, not uncertain executor work. Read its
        // durable typed result (also after restart), never arbitrary error text.
        if super::kernel_delegation::correction_limit_reached(&snapshot) {
            settle_children(true)?;
            coordinator.tick()?;
            coordinator.fail(
                "kernel.child_arguments_exhausted",
                "子任务参数连续校验失败，已提供 3 次纠错机会并停止继续派发。请检查工具参数后重试；此错误不是 API 额度不足。",
            )?;
            continue;
        }
        if snapshot.state == "retry_scheduled" {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        // Exclusive OS ownership establishes that no previous process can still
        // complete its lease. An uncertain action is never replayed automatically.
        if snapshot.pending_effects.iter().any(|effect| {
            effect.status == OutboxStatus::Leased
                && matches!(
                    effect.kind,
                    OutboxEffectKind::DispatchTool
                        | OutboxEffectKind::InitialModel
                        | OutboxEffectKind::ContinuationModel
                        | OutboxEffectKind::DeliverToolBatch
                )
        }) {
            coordinator.fail(
                "kernel.uncertain_execution",
                "Execution was interrupted after dispatch; it was not replayed.",
            )?;
            continue;
        }
        let next = snapshot.pending_effects.iter().find(|effect| {
            effect.status == OutboxStatus::Pending
                // An approved write must wait for other outstanding approvals,
                // rather than reaching a strict resource gate and failing forever.
                && !super::kernel_coordinator::live::dispatch_waits_for_approval(&snapshot.state, effect)
                && matches!(
                    effect.kind,
                    OutboxEffectKind::InitialModel
                        | OutboxEffectKind::ContinuationModel
                        | OutboxEffectKind::DispatchTool
                        | OutboxEffectKind::DeliverToolBatch
                )
        });
        let result = if snapshot.state == "compacting" {
            coordinator.resume_context_compaction(&owner, runtime, api_key)
        } else if let Some(effect) = next {
            match effect.kind {
                OutboxEffectKind::InitialModel => {
                    let binding = database
                        .run_control_binding(run_id)?
                        .ok_or("authoritative Run has no frozen control binding")?;
                    if binding.engine_id == "pi" {
                        // Loop-capable engine session: the worker drives the
                        // engine's own tool loop and the Host services each
                        // round output durably. Old runtimes fall back to the
                        // per-round transport.
                        match coordinator.dispatch_initial_live(
                            &owner,
                            policy,
                            runtime,
                            api_key,
                            &execute,
                            &after_commit,
                            &settle_children,
                        ) {
                            Err(error)
                                if error
                                    .contains("lacks the durable round-loop capability") =>
                            {
                                coordinator
                                    .dispatch_initial_with_worker(&owner, policy, runtime, api_key)
                            }
                            other => other,
                        }
                    } else {
                        coordinator
                            .dispatch_initial_with_worker(&owner, policy, runtime, api_key)
                    }
                }
                OutboxEffectKind::ContinuationModel => coordinator.dispatch_continuation_live(
                    &effect.effect_key, &owner, policy, runtime, api_key, &execute, &after_commit, &settle_children),
                OutboxEffectKind::DeliverToolBatch => coordinator
                    .dispatch_stored_batch_with_worker(
                        effect
                            .batch_id
                            .as_deref()
                            .ok_or("missing model batch identity")?,
                        &owner,
                        policy,
                        runtime,
                        api_key,
                    ),
                OutboxEffectKind::DispatchTool => coordinator
                    .dispatch_tool(
                        effect
                            .tool_call_id
                            .as_deref()
                            .ok_or("missing dispatch tool identity")?,
                        &owner,
                        &execute,
                    )
                    .map(|_| ()),
                _ => unreachable!(),
            }
        } else if snapshot.state == "waiting_approval" || snapshot.state == "retry_scheduled" {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        } else if database.kernel_last_event_type(run_id)?.as_deref() == Some("engine.continuation_requested") {
            coordinator.fail(
                "kernel.continuation_interrupted",
                "停止前续答检查已发出，但引擎在下一轮输出前中断。原始历史与已完成的工具结果保持不变；为避免重复执行或重复请求，任务已停止，请重新发起新任务继续。",
            )?;
            continue;
        } else {
            coordinator.fail(
                "kernel.no_progress",
                "No dispatchable work exists for this active Run.",
            )?;
            continue;
        };
        if let Err(error) = result {
            if error == super::kernel_coordinator::live::LIVE_DETACHED
                || error == super::kernel_coordinator::STEERING_REPLAN
            {
                // The transport detached after committing durable state, or a
                // terminal decision lost a race with a freshly accepted
                // additional request. Either way the next iteration re-reads
                // durable facts (pending deliveries, retries, terminal states)
                // and plans the safe next step; nothing here may replay engine
                // work, and neither case is an execution failure.
                continue;
            }
            settle_children(true)?;
            // Prefer a durable UI cancellation over classifying the interrupted
            // worker as an engine error. The executor has already cleaned up.
            if database
                .pending_kernel_host_commands(run_id)?
                .iter()
                .any(|command| command.kind == "cancel")
            {
                continue;
            }
            coordinator.tick()?;
            if !terminal(&coordinator.snapshot()?.state) {
                // Do not persist arbitrary adapter errors: they may contain
                // request bodies or credentials. Detailed diagnostics stay local.
                let (code, message) = match error.as_str() {
                    "kernel.jobs_pending" => ("kernel.jobs_pending", "后台计算尚未完成；任务已保留进度，请检查作业状态后继续。"),
                    crate::kernel_compaction::UNCERTAIN => (crate::kernel_compaction::UNCERTAIN,
                        "上下文压缩请求已发出，但结果未确认。原始历史保留，未自动重复请求；请检查后重新发起任务。"),
                    crate::kernel_compaction::INSUFFICIENT => (crate::kernel_compaction::INSUFFICIENT,
                        "保留工具结果、执行凭据和最近消息后，上下文仍超出安全容量。原始历史未删改，请缩小任务或新建对话。"),
                    crate::kernel_compaction::NO_CANDIDATES => (crate::kernel_compaction::NO_CANDIDATES, "没有可安全折叠的旧历史；已保留进度，可调整输入后续做。"),
                    crate::kernel_compaction::NO_REDUCTION => (crate::kernel_compaction::NO_REDUCTION, "摘要没有减少上下文；已保留进度，可调整输入后续做。"),
                    crate::kernel_compaction::SINGLE_TOO_LARGE => (crate::kernel_compaction::SINGLE_TOO_LARGE, "单项输入过大；请分页或引用该结果，再续做。"),
                    crate::kernel_compaction::FAILED => (crate::kernel_compaction::FAILED,
                        "上下文压缩未取得有效结果，任务已停止。原始历史保留，未自动重复请求。"),
                    _ => ("kernel.execution_failed", "The owned model or resource executor failed; uncertain work was not replayed."),
                };
                let reason=match code {
                    crate::kernel_compaction::NO_CANDIDATES => Some("no_candidates"),
                    crate::kernel_compaction::NO_REDUCTION => Some("no_reduction"),
                    crate::kernel_compaction::SINGLE_TOO_LARGE => Some("single_item_too_large"),
                    crate::kernel_compaction::INSUFFICIENT => Some("insufficient"),
                    crate::kernel_compaction::FAILED => Some("summary_failed"),
                    _ => None,
                };
                if let Some(reason)=reason {let _=database.kernel_context_budget_stopped(run_id,reason);}
                coordinator.fail(code, message)?;
            }
        }
    }
}

pub(super) fn acquire(sessions_dir: &Path, run_id: &str) -> Result<KernelRunLock, String> {
    KernelRunLock::acquire(sessions_dir, run_id)
}

pub(super) fn initial_input(
    binding: &RunControlBinding,
    prompt: &Value,
    prompt_hash: &str,
) -> Result<fox_engine_protocol::KernelInitialModelInput, String> {
    let mut history = prompt["messages"]
        .as_array()
        .ok_or("missing initial history")?
        .clone();
    // The database history contains the just-created user message. Replace that
    // final entry with the original submitted text/images, not a truncated copy.
    if history
        .last()
        .is_some_and(|message| message["role"] == "user")
    {
        history.pop();
    }
    for message in &mut history {
        if message["role"] == "assistant" {
            let text = message["content"]
                .as_str()
                .ok_or("invalid desktop assistant history")?;
            *message = serde_json::json!({"role":"assistant","content":[{"type":"text","text":text}],"stopReason":"stop"});
        }
    }
    let mut content = vec![
        serde_json::json!({"type":"text","text":prompt["text"].as_str().ok_or("missing submitted text")?}),
    ];
    content.extend(
        prompt["images"]
            .as_array()
            .ok_or("missing initial images")?
            .iter()
            .cloned(),
    );
    history.push(serde_json::json!({"role":"user","content":content}));
    let input = fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1,
        run_id: binding.run_id.clone(),
        turn_id: format!("kernel-turn:{}", binding.run_id),
        prompt_config_hash: prompt_hash.into(),
        messages: history,
    };
    input.validate()?;
    Ok(input)
}

/// The Host-verified managed write behind one frozen dispatch.
///
/// A managed write is identified from Host facts only: the real dispatch tool,
/// the frozen connector identity and scope, and the target the Host itself
/// admits for this dispatch — never from a tool result. The classification
/// itself lives in [`super::managed_files`], where it is unit-tested directly.
impl super::RuntimeHost {
    pub(super) fn stop_kernel_runs(&self) -> Result<(), String> {
        let active = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?;
            // The registration and dispatch checks use the same lock. A cleanup
            // callback setting the display state to ready cannot reopen admission.
            state.shutting_down = true;
            state.kernel_active_runs.iter().cloned().collect::<Vec<_>>()
        };
        for run_id in active {
            if let Err(error) = self.database.queue_kernel_host_command(&run_id, None) {
                if !self
                    .database
                    .kernel_host_run_state(&run_id)?
                    .as_deref()
                    .is_some_and(terminal)
                {
                    return Err(error);
                }
            }
            self.state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?
                .cancellation
                .request_run_cancel(&run_id);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?
            .kernel_active_runs
            .is_empty()
        {
            if std::time::Instant::now() >= deadline {
                return Err("Kernel shutdown is still waiting for owned resource cleanup".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }

    pub(crate) fn recover_kernel_runs_detached(&self) -> Result<(), String> {
        let runs = self.database.kernel_host_recoverable_runs()?;
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?;
            if state.shutting_down {
                return Err("Runtime Host is shutting down".into());
            }
            if !runs.is_empty() {
                state.state = "recovering".into();
            }
            for run_id in &runs {
                state.cancellation.register_run(run_id)?;
                state.kernel_active_runs.insert(run_id.clone());
            }
        }
        for run_id in runs {
            let host = self.clone();
            std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
                let result = (|| {
                    let ownership = acquire(&host.sessions_dir, &run_id)?;
                    let binding = host
                        .database
                        .run_control_binding(&run_id)?
                        .ok_or("missing Kernel recovery binding")?;
                    let cancellation = host
                        .state
                        .lock()
                        .map_err(|_| "runtime state lock poisoned")?
                        .cancellation
                        .clone();
                    let runtime = host.runtime_command()?;
                    host.drive_kernel_run(ownership, &binding, &runtime, &cancellation)
                })();
                if result.is_err() {
                    // Failure recording reacquires ownership; it cannot terminate
                    // a Run currently held by another process.
                    let _ = host.record_kernel_start_failure(&run_id);
                }
                if let Ok(mut state) = host.state.lock() {
                    state.kernel_active_runs.remove(&run_id);
                    state.cancellation.retire_run(&run_id);
                    if state.kernel_active_runs.is_empty() {
                        state.state = "ready".into();
                    }
                    if result.is_err() {
                        state.last_error = Some(
                            "A Kernel Run could not be recovered; no fallback execution was used."
                                .into(),
                        );
                    }
                }
                host.dispatch_next_queued_run();
            }));
        }
        Ok(())
    }

    pub(super) fn record_kernel_start_failure(&self, run_id: &str) -> Result<(), String> {
        let _ownership = acquire(&self.sessions_dir, run_id)?;
        if self.database.kernel_fail_before_aggregate(run_id)? {
            return Ok(());
        }
        if self.database.kernel_host_run_state(run_id)?.as_deref() == Some("created") {
            let (config, turn) = self.database.kernel_prepared_failure_config(run_id)?;
            let clock = super::shadow_reconcile::ReconcilerClock;
            let (mut controller, mut effects) =
                kernel::RunController::start(run_id, &turn, config, &clock)
                    .map_err(|error| error.to_string())?;
            if self
                .database
                .pending_kernel_host_commands(run_id)?
                .iter()
                .any(|command| command.kind == "cancel")
            {
                effects.extend(controller.request_cancel());
                effects.extend(controller.settle_cancellation());
            } else {
                effects.extend(controller.terminate(kernel::RunOutcome::Failed { code: "kernel.preparation_failed".into(),
                    message: "The frozen preparation was incomplete; no model or resource execution was started.".into() }));
            }
            return self.database.kernel_commit_decision(
                run_id,
                clock.now_wall_ms(),
                &controller.persist_command(&effects),
            );
        }
        let cancellation = self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?
            .cancellation
            .clone();
        let coordinator = KernelCoordinator::reopen(
            &self.database,
            &super::shadow_reconcile::ReconcilerClock,
            run_id,
            &cancellation,
        )?;
        if self
            .database
            .pending_kernel_host_commands(run_id)?
            .iter()
            .any(|command| command.kind == "cancel")
        {
            coordinator.cancel()?;
            coordinator.settle_cancellation()?;
        } else {
            coordinator.fail("kernel.preparation_failed", "The frozen Kernel preparation or recovery could not be validated; no fallback was used.")?;
        }
        cancellation.retire_run(run_id);
        Ok(())
    }

    pub(super) fn start_kernel_run(
        &self,
        ownership: KernelRunLock,
        binding: &RunControlBinding,
        prompt: Value,
        service: Value,
    ) -> Result<(), String> {
        let cancellation = {
            let _transition = self
                .run_transition
                .lock()
                .map_err(|_| "runtime transition lock poisoned")?;
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock poisoned")?;
            if state.shutting_down {
                return Err("Runtime Host is shutting down".into());
            }
            if state.cancelled_dispatches.remove(&binding.run_id) {
                return Err(super::CANCELLED_BEFORE_SUBMISSION.into());
            }
            state.cancellation.register_run(&binding.run_id)?;
            state.kernel_active_runs.insert(binding.run_id.clone());
            state.state = "busy".into();
            state.execution_profile_id = binding.execution_profile_id.clone();
            state.cancellation.clone()
        };
        let result = (|| {
            let runtime = self.runtime_command()?;
            if self
                .database
                .kernel_host_run_state(&binding.run_id)?
                .is_some()
            {
                // A pre-existing aggregate must use all of its persisted inputs.
                return self.drive_kernel_run(ownership, binding, &runtime, &cancellation);
            }
            let token = cancellation.run_token(&binding.run_id)?;
            let mut supported = super::kernel_gateway::supported_tools();
            if let Some(child) = self.database.child_run(&binding.run_id)? {
                if self
                    .database
                    .run_control_binding(&child.parent_run_id)?
                    .is_some_and(|parent| {
                        parent.authority == fox_engine_protocol::ExecutionAuthority::Authoritative
                    })
                {
                    let parent_scope = self.database.kernel_host_scope(&child.parent_run_id)?;
                    supported.retain(|tool| parent_scope.tool_names.contains(*tool));
                }
            }
            let config = super::kernel_model_worker::describe(
                &runtime,
                binding,
                service,
                prompt.clone(),
                supported,
                &token,
            )?;
            let hash = config.hash()?;
            let input = initial_input(binding, &prompt, &hash)?;
            let scope =
                super::kernel_gateway::freeze_scope(&self.database, binding, &prompt, &config)?;
            let frozen = kernel::RunFrozenConfig {
                engine_id: binding.engine_id.clone(),
                kernel_mode: "authoritative".into(),
                capability_manifest_version: 2,
                capability_manifest_hash: super::runtime_shadow_hash(
                    &serde_json::to_string(&scope).map_err(|_| "invalid resource scope")?,
                ),
                permission_snapshot_id: binding.permission_snapshot_id.clone(),
                execution_profile_id: binding.execution_profile_id.clone(),
                prompt_config_hash: hash.clone(),
                model_request_timeout_ms: binding.budgets.model_request_ms,
                model_first_response_ms: binding.budgets.model_first_response_ms,
                model_idle_ms: binding.budgets.model_idle_ms,
                tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
                run_execution_budget_ms: binding.budgets.run_execution_ms,
                run_execution_limited: binding.budgets.run_execution_limited,
                approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
                provider_max_retries: 2,
                turn_max_retries: 1,
            };
            self.database.kernel_create_run(
                &binding.run_id,
                &binding.engine_id,
                "authoritative",
                2,
                &binding.permission_snapshot_id,
                &binding.execution_profile_id,
                &hash,
                &serde_json::to_string(&frozen).map_err(|_| "invalid frozen Kernel Run")?,
            )?;
            self.database
                .freeze_kernel_model_config(&binding.run_id, &config)?;
            self.database.freeze_kernel_initial_input(&input)?;
            self.database
                .freeze_kernel_host_scope(&binding.run_id, &scope)?;
            self.drive_kernel_run(ownership, binding, &runtime, &cancellation)
        })();
        if let Ok(mut state) = self.state.lock() {
            state.kernel_active_runs.remove(&binding.run_id);
            state.cancellation.retire_run(&binding.run_id);
            if state.kernel_active_runs.is_empty() {
                state.state = "ready".into();
            }
        }
        result
    }

    fn drive_kernel_run(
        &self,
        ownership: KernelRunLock,
        binding: &RunControlBinding,
        runtime: &RuntimeCommand,
        cancellation: &CancellationRegistry,
    ) -> Result<(), String> {
        let config = self.database.kernel_model_config(&binding.run_id)?;
        let base_url = config.model_service["baseUrl"]
            .as_str()
            .ok_or("missing frozen model URL")?;
        let api_key = crate::model_service::get_api_key(base_url).unwrap_or_default();
        let policy = super::kernel_gateway::GatewayPolicy {
            binding: binding.clone(),
            scope: self.database.kernel_host_scope(&binding.run_id)?,
            // #5: production may also read the run's additional authorization
            // grants, so a conversation approval is reusable without rewriting
            // the frozen binding.
            database: Some(self.database.clone()),
            // Office artifact references (office_import_data.artifactId) are
            // resolved against this conversation's own compute store.
            sessions_dir: Some(self.sessions_dir.clone()),
            // Host-private placement for rendered previews, working copies and
            // commit staging. Derived from the Host's own state, so the model
            // cannot redirect these areas.
            artifacts_dir: self
                .sessions_dir
                .parent()
                .map(std::path::Path::to_path_buf),
        };
        let proposal_policy = super::kernel_gateway::GatewayProposalPolicy {
            gateway: &policy,
            database: &self.database,
        };
        let app = self.app.clone();
        let display_database = self.database.clone();
        let preview = move |notice: &fox_engine_protocol::KernelModelPreview| {
            use tauri::Emitter;
            if display_database.save_kernel_model_display(notice).unwrap_or(false) {
                // Accepted previews (including tool-parameter-only progress)
                // feed the controller's idle bound via the volatile registry.
                // Stale/foreign frames are ignored by the save above and never
                // become progress.
                display_database.note_kernel_model_progress(
                    &notice.run_id,
                    crate::database::now_ms(),
                );
                let _ = app.emit("fox://kernel-model-preview", notice);
            }
        };
        let result = drive_with_actions(
            &ownership,
            &self.database,
            &super::shadow_reconcile::ReconcilerClock,
            cancellation,
            &binding.run_id,
            runtime,
            &api_key,
            &proposal_policy,
            |_, effect, token| {
                let payload: Value = serde_json::from_str(&effect.payload_json)
                    .map_err(|_| "invalid frozen tool dispatch")?;
                let tool = payload["tool"].as_str().ok_or("missing tool name")?;
                let result = if super::work_tools::is_work_tool(tool) {
                    policy.execute_work(
                        &self.database,
                        effect
                            .tool_call_id
                            .as_deref()
                            .ok_or("missing work tool identity")?,
                        tool,
                        &payload["input"],
                        token,
                    )
                } else if super::kernel_delegation::TOOLS.contains(&tool) {
                    policy.execute_delegation(
                        &self.database,
                        effect
                            .tool_call_id
                            .as_deref()
                            .ok_or("missing delegation identity")?,
                        tool,
                        &payload["input"],
                        token,
                    )
                } else if super::kernel_gateway::is_context_resource(tool) {
                    policy.execute_context_resource(
                        &self.database,
                        &self.attachments_dir,
                        &self.sessions_dir,
                        &self.skills_dir,
                        tool,
                        &payload["input"],
                        token,
                    )
                } else if super::kernel_gateway::is_knowledge(tool) {
                    use tauri::Manager;
                    let app_state = self.app.state::<crate::app_state::AppState>();
                    policy.execute_knowledge(
                        &self.database,
                        &self.yuxi_client,
                        &app_state.local_knowledge,
                        tool,
                        &payload["input"],
                        token,
                    )
                } else {
                    // A managed write is identified from Host facts only: the
                    // real dispatch tool (including the frozen Office wrapper)
                    // plus the target the Host itself admitted for this frozen
                    // dispatch. The shared seam protects the current bytes before
                    // the write and registers the resulting content version, so
                    // the desktop Host and the real-task evaluation cannot drift
                    // apart on what "a managed write" means.
                    let outcome = super::managed_files::execute_with_managed_versions(
                        super::managed_files::ManagedExecutionContext {
                            database: &self.database,
                            backups_dir: &self.managed_files_dir,
                            conversation_id: &binding.conversation_id,
                            run_id: &binding.run_id,
                            project_root: binding.permission.project_root.as_deref(),
                            permission_mode: binding.permission.mode.as_str(),
                            scope: &policy.scope,
                            sessions_dir: Some(&self.sessions_dir),
                        },
                        tool,
                        &payload["input"],
                        effect.tool_call_id.as_deref(),
                        || policy.execute(&self.database, tool, &payload["input"], token),
                    );
                    outcome
                };
                match result {
                    Ok(result) => Ok((
                        result.get("isError").and_then(Value::as_bool) != Some(true),
                        result,
                    )),
                    // A work transaction may already have created a child or
                    // durable intent. Do not turn a staging failure into a
                    // retryable ordinary tool result and advance the model.
                    Err(error)
                        if super::work_tools::is_work_tool(tool)
                            || super::kernel_delegation::TOOLS.contains(&tool) =>
                    {
                        Err(error)
                    }
                    Err(error) if token.check().is_ok() => Ok((
                        false,
                        resource_failure_result(tool, &error),
                    )),
                    Err(error) => Err(error),
                }
            },
            |tool_id| self.dispatch_kernel_host_action(binding, tool_id),
            |force_cancel| self.wait_kernel_action_children(binding, force_cancel),
            &preview,
        );
        if result.is_err() {
            self.wait_kernel_action_children(binding, true)?;
        }
        result
    }

    fn dispatch_kernel_host_action(
        &self,
        binding: &RunControlBinding,
        tool_id: &str,
    ) -> Result<(), String> {
        use super::work_tools::WorkToolPostFinalizeDirective;
        use tauri::Emitter;
        let Some(body) = self
            .database
            .claim_kernel_host_action(&binding.run_id, tool_id)?
        else {
            return Ok(());
        };
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Action {
            Work(WorkToolPostFinalizeDirective),
            Delegation(super::kernel_delegation::CancelAction),
        }
        let action: Action =
            serde_json::from_str(&body).map_err(|_| "invalid frozen Kernel Host action")?;
        let directive = match action {
            Action::Work(directive) => directive,
            Action::Delegation(action) => {
                match action {
                    super::kernel_delegation::CancelAction::Child { child_run_id } => {
                        let child = self
                            .database
                            .child_run(&child_run_id)?
                            .ok_or("missing child")?;
                        if child.parent_run_id != binding.run_id {
                            return Err("Kernel child cancel identity mismatch".into());
                        }
                        super::ensure_child_cancel_not_graph_bound(&self.database, &child_run_id)?;
                        self.cancel_child_runtime(&child_run_id)?;
                    }
                    super::kernel_delegation::CancelAction::Team {
                        team_run_id,
                        reason,
                    } => {
                        let team = self
                            .database
                            .get_expert_team(&team_run_id)?
                            .ok_or("missing team")?;
                        if team.run.parent_run_id != binding.run_id {
                            return Err("Kernel team cancel identity mismatch".into());
                        }
                        for child in team.members {
                            if !super::run_status_is_terminal(&child.status) {
                                self.cancel_child_runtime(&child.child_run_id)?;
                            }
                        }
                        self.wait_kernel_action_children(binding, false)?;
                        self.database.cancel_expert_team(&team_run_id, &reason)?;
                    }
                }
                self.wait_kernel_action_children(binding, false)?;
                return self
                    .database
                    .complete_kernel_host_action(&binding.run_id, tool_id);
            }
        };
        match directive {
            WorkToolPostFinalizeDirective::StartChild(dispatch) => {
                if dispatch.child_run.parent_run_id != binding.run_id
                    || dispatch.started.run.id != dispatch.child_run.child_run_id
                {
                    return Err("Kernel child action identity mismatch".into());
                }
                self.start_child_runtime(
                    dispatch.started,
                    dispatch.child_run,
                    binding.conversation_id.clone(),
                )?;
            }
            WorkToolPostFinalizeDirective::CancelGraphChild(directive) => {
                let activation = self
                    .database
                    .activate_graph_node_cancel_intent(&directive.intent_id)
                    .map_err(|error| error.to_string())?;
                if activation.child_run_id != directive.child_run_id {
                    return Err("Kernel graph cancel identity mismatch".into());
                }
                if activation.activated {
                    self.signal_graph_child_cancel(&activation.child_run_id, false)?;
                }
            }
            WorkToolPostFinalizeDirective::StartGraphReviewer(directive) => {
                let activation = self
                    .database
                    .activate_graph_node_review_request(&directive.request_id)
                    .map_err(|error| error.to_string())?;
                if activation.activated {
                    let agent_id = self
                        .database
                        .conversation_agent_id(&binding.conversation_id)?;
                    super::dispatch_graph_reviewer_child(self, &directive.request_id, &agent_id)?;
                }
            }
            WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(directive) => {
                let acceptance = self
                    .database
                    .activate_read_only_graph_acceptance(&directive.acceptance_id)
                    .map_err(|error| error.to_string())?;
                if acceptance.accepted {
                    let _ = self.app.emit("fox://graph-acceptance", acceptance);
                }
            }
        }
        // Keep the owning parent loop inside this barrier until child executors
        // and their cleanup have returned. No parent model/terminal is published
        // while this action still owns running child resources.
        self.wait_kernel_action_children(binding, false)?;
        self.database
            .complete_kernel_host_action(&binding.run_id, tool_id)
    }

    fn wait_kernel_action_children(
        &self,
        binding: &RunControlBinding,
        force_cancel: bool,
    ) -> Result<(), String> {
        self.wait_kernel_selected_children(binding, force_cancel, None)
    }

    pub(super) fn wait_kernel_selected_children(
        &self,
        binding: &RunControlBinding,
        force_cancel: bool,
        selected: Option<&std::collections::BTreeSet<String>>,
    ) -> Result<(), String> {
        let elapsed = self
            .database
            .kernel_build_full_snapshot(&binding.run_id)?
            .running_elapsed_ms;
        let deadline = binding.budgets.remaining_run_ms(elapsed)
            .map(|remaining| std::time::Instant::now() + Duration::from_millis(remaining.max(0) as u64));
        loop {
            let active = self
                .database
                .active_child_run_ids(&binding.run_id)?
                .into_iter()
                .filter(|id| selected.is_none_or(|selected| selected.contains(id)))
                .collect::<Vec<_>>();
            let owned = self
                .child_hosts
                .lock()
                .map_err(|_| "child runtime map is poisoned")?
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            let mut owned_children = Vec::new();
            for id in owned {
                if selected.is_none_or(|selected| selected.contains(&id))
                    && self
                        .database
                        .child_run(&id)?
                        .is_some_and(|child| child.parent_run_id == binding.run_id)
                {
                    owned_children.push(id);
                }
            }
            if active.is_empty() && owned_children.is_empty() {
                return Ok(());
            }
            let cancelling = force_cancel
                || deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
                || self
                    .database
                    .pending_kernel_host_commands(&binding.run_id)?
                    .iter()
                    .any(|command| command.kind == "cancel");
            for id in active {
                if !owned_children.contains(&id)
                    && self.database.run_control_binding(&id)?.is_none()
                {
                    let _ownership = match acquire(&self.sessions_dir, &id) {
                        Ok(ownership) => ownership,
                        Err(error)
                            if error
                                .starts_with("Kernel Run already owned or cannot be locked:") =>
                        {
                            continue
                        }
                        Err(error) => return Err(error),
                    };
                    let child = self
                        .database
                        .child_run(&id)?
                        .ok_or("missing Kernel child record")?;
                    let settled = if cancelling {
                        self.database
                            .kernel_cancel_unstarted_child(&child.parent_run_id, &id)?
                    } else {
                        self.database
                            .kernel_fail_unstarted_child(&child.parent_run_id, &id)?
                    };
                    if settled {
                        continue;
                    }
                    return Err(
                        "Unowned child has no frozen Kernel authority; execution was not resumed"
                            .into(),
                    );
                }
                if cancelling {
                    if let Err(error) = self.cancel_child_runtime(&id) {
                        if !self
                            .database
                            .child_run_status(&id)?
                            .as_deref()
                            .is_some_and(super::run_status_is_terminal)
                        {
                            return Err(error);
                        }
                    }
                }
                if !owned_children.contains(&id) {
                    // Failed preparation can leave only a frozen binding. Under
                    // the child's exclusive lock, settle it without model I/O.
                    if cancelling
                        || self
                            .database
                            .kernel_host_run_state(&id)?
                            .as_deref()
                            .is_none_or(|state| state == "created")
                    {
                        if let Err(error) = self.record_kernel_start_failure(&id) {
                            if !error.starts_with("Kernel Run already owned or cannot be locked:") {
                                return Err(error);
                            }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
