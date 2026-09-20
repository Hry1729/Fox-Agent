#[path = "unified_tests.rs"]
mod unified_tests;
#[path = "limit_fix_tests.rs"]
mod limit_fix_tests;
use super::*;
#[path = "reconciliation_tests.rs"]
mod reconciliation_tests;
#[path = "native_engine_tests.rs"]
mod native_engine_tests;
#[test]
fn acceptance_task4_external_marker_survives_lost_result_without_duplicate_execution() {
    use std::io::Write;
    for commit_failure in [false, true] {
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, root, run_id) = fixture(&clock);
        let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator.propose_tools("batch", calls(), &Allow).unwrap();
        let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        if commit_failure {
            conn.execute_batch("CREATE TRIGGER acceptance_drop_result BEFORE INSERT ON kernel_events WHEN NEW.event_type='tool.completed' BEGIN SELECT RAISE(ABORT,'acceptance result loss'); END;").unwrap();
        }
        let marker = root.join("external-effect-marker.txt");
        assert!(coordinator.dispatch_tool("read-a", "original-owner", |_, _, _| {
            // Fault-injection executor: a real isolated marker represents an external side effect.
            // This does not exercise a production write tool or a business connector.
            std::fs::OpenOptions::new().create(true).append(true).open(&marker).unwrap().write_all(b"executed-once\n").unwrap();
            if commit_failure { Ok((true, json!({"proof":"executed"}))) }
            else { Err("injected response loss after external marker".into()) }
        }).is_err());
        drop(coordinator); drop(conn); drop(db);
        for _ in 0..3 {
            let db = Database::open(root.join("facts.db")).unwrap();
            let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
            assert!(!coordinator.dispatch_tool("read-a", "replacement-owner", |_, _, _| panic!("must not repeat external effect")).unwrap());
            let plan = kernel::plan_recovery(vec![db.kernel_recovery_facts(&run_id).unwrap().unwrap()]);
            assert_eq!(plan.uncertain_leased.len(), 1);
            assert_eq!(std::fs::read_to_string(&marker).unwrap(), "executed-once\n");
        }
        println!("TASK4 commit_failure={commit_failure} reopen_count=3 marker_count=1 uncertain_count=1");
    }
}
#[path = "compaction_tests.rs"]
mod compaction_tests;
#[path = "display_tests.rs"]
mod display_tests;
#[path = "tool_result_read_tests.rs"]
mod tool_result_read_tests;
#[path = "skill_load_tests.rs"]
mod skill_load_tests;
#[path = "delivery_live_tests.rs"]
mod delivery_live_tests;
#[path = "concurrency_tests.rs"]
mod concurrency_tests;
#[path = "steering_tests.rs"]
mod steering_tests;
#[path = "steering_live_tests.rs"]
mod steering_live_tests;
#[path = "real_eval_tests.rs"]
mod real_eval_tests;
#[path = "harness_security_tests.rs"]
mod harness_security_tests;
#[path = "kernel_artifact_gate_tests.rs"]
mod kernel_artifact_gate_tests;
use crate::kernel::{CancellationRegistry, PolicyDecision, TestClock};
use fox_engine_protocol::{FrozenPermission, PermissionMode, ResourceExecutor, TimeBudgets};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Ask;
impl PolicyDecisionPort for Ask {
    fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
        PolicyDecision::RequireApproval
    }
}
struct Allow;
impl PolicyDecisionPort for Allow {
    fn decide(&self, _: &str, _: &str, _: &str, _: &str) -> PolicyDecision {
        PolicyDecision::Allow
    }
}

fn fixture(clock: &TestClock) -> (Database, PathBuf, String) {
    fixture_with_prompt(clock, "test-prompt")
}

fn fixture_with_prompt(clock: &TestClock, prompt_hash: &str) -> (Database, PathBuf, String) {
    fixture_with_model_opt(clock, prompt_hash, None)
}

fn fixture_with_model(clock: &TestClock, config: &crate::kernel_model_config::KernelModelConfig) -> (Database, PathBuf, String) {
    fixture_with_model_opt(clock, &config.hash().unwrap(), Some(config))
}

fn fixture_with_model_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>) -> (Database, PathBuf, String) {
    fixture_with_initial_opt(clock, prompt_hash, model, false)
}

fn initial_input(run_id: &str, prompt_hash: &str) -> fox_engine_protocol::KernelInitialModelInput {
    fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1, run_id: run_id.into(), turn_id: "turn-1".into(), prompt_config_hash: prompt_hash.into(),
        messages: vec![json!({"role":"user","content":"初始问题：保留原文 😀"})],
    }
}

fn fixture_with_initial_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>, initial: bool) -> (Database, PathBuf, String) {
    fixture_with_start_opt(clock, prompt_hash, model, initial, false)
}

fn fixture_with_start_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>, initial: bool, prepared: bool) -> (Database, PathBuf, String) {
    fixture_with_retry_opt(clock,prompt_hash,model,initial,prepared,(0,0))
}

fn fixture_with_retry_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>, initial: bool, prepared: bool, retries: (u32,u32)) -> (Database, PathBuf, String) {
    fixture_with_project_opt(clock,prompt_hash,model,initial,prepared,retries,true)
}

fn fixture_with_project_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>, initial: bool, prepared: bool, retries: (u32,u32), has_project: bool) -> (Database, PathBuf, String) {
    fixture_with_budgets_opt(clock, prompt_hash, model, initial, prepared, retries, has_project, TimeBudgets::default())
}

fn fixture_with_budgets_opt(clock: &TestClock, prompt_hash: &str, model: Option<&crate::kernel_model_config::KernelModelConfig>, initial: bool, prepared: bool, retries: (u32,u32), has_project: bool, budgets: TimeBudgets) -> (Database, PathBuf, String) {
    let engine = model.map(|model| model.engine_id.as_str()).unwrap_or("pi");
    let root = std::env::temp_dir().join(format!("fox-coordinator-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("proof.txt"), "durable coordinator 中文 😀").unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            if has_project {Some(root.to_str().unwrap())} else {None},
            Some("read_only"),
        )
        .unwrap();
    let run_id = db
        .create_run(&conversation.id, "test", None)
        .unwrap()
        .run
        .id;
    let permission = FrozenPermission {
        mode: PermissionMode::ReadOnly,
        project_root: has_project.then(||root.to_string_lossy().into_owned()),
        grants: vec![],
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id,
        engine_id: engine.into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets,
    };
    db.freeze_run_control(&binding).unwrap();
    let config = kernel::RunFrozenConfig {
        engine_id: engine.into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "coordinator-test-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: prompt_hash.into(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: retries.0,
        turn_max_retries: retries.1,
    };
    db.kernel_create_run(
        &run_id,
        engine,
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        prompt_hash,
        &serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    if let Some(model) = model { db.freeze_kernel_model_config(&run_id, model).unwrap(); }
    if initial {
        let original = initial_input(&run_id, prompt_hash);
        let mut wrong = original.clone(); wrong.prompt_config_hash = "wrong-model".into();
        assert!(db.freeze_kernel_initial_input(&wrong).is_err());
        wrong = original.clone(); wrong.run_id = "wrong-run".into();
        assert!(db.freeze_kernel_initial_input(&wrong).is_err());
        db.freeze_kernel_initial_input(&original).unwrap();
        assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
        let (wrong_controller, wrong_effects) = RunController::start(&run_id, "wrong-turn", config.clone(), clock).unwrap();
        assert!(db.kernel_commit_decision(&run_id, clock.now_wall_ms(), &wrong_controller.persist_command(&wrong_effects)).is_err());
    }
    if prepared { return (db, root, run_id); }
    let (controller, effects) = RunController::start(&run_id, "turn-1", config, clock).unwrap();
    db.kernel_commit_decision(
        &run_id,
        clock.now_wall_ms(),
        &controller.persist_command(&effects),
    )
    .unwrap();
    (db, root, run_id)
}

fn calls() -> Vec<ToolCallRequest> {
    ["read-a", "read-b"]
        .iter()
        .enumerate()
        .map(|(source_order, id)| ToolCallRequest {
            tool_call_id: (*id).into(),
            tool: "read".into(),
            canonical_input_json: json!({"path":"proof.txt"}).to_string(),
            source_order,
        })
        .collect()
}

fn timeout_failure(run_id: &str, turn_id: &str, checkpoint_seq: u64) -> String {
    format!(
        "kernel.settled_model_failure:{}",
        json!({
            "schemaVersion": 1, "runId": run_id, "turnId": turn_id,
            "checkpointSeq": checkpoint_seq, "category": "model_timeout",
            "httpStatus": null, "retryAfterMs": null,
            "telemetry": {"elapsedMs": 121_000, "idleElapsedMs": 120_000,
                "firstResponseMs": 2_000, "textBytes": 449,
                "reasoningBytes": 0, "toolParamBytes": 82_000},
        })
    )
}

fn retry_reason(root: &std::path::Path, run_id: &str) -> String {
    // Read-only inspection through a separate connection; never the live handle.
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.retrying' ORDER BY seq DESC LIMIT 1",
        [run_id],
        |row| row.get::<_, String>(0),
    ).unwrap()
}

#[test]
fn worker_model_timeout_retries_only_the_model_request_without_replaying_tools() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "timeout-prompt", None, false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    // The real dispatch flow: the worker returns settled model_timeout
    // evidence. Nothing outstanding was dispatched in the failed round, so
    // the Host schedules a bounded retry that only re-issues the model
    // request; no tool is executed here.
    coordinator.dispatch_batch("worker-batch", "first", &Allow, |binding, frame, _| {
        Err(timeout_failure(&binding.run_id, &frame.turn_id, frame.checkpoint_seq))
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
    assert_eq!(coordinator.snapshot().unwrap().retry.turn_attempts, 1);
    // The retry is not due yet: an early claim is refused.
    assert!(coordinator.dispatch_batch("worker-batch", "early", &Allow, |_, _, _| panic!("retry is not due")).is_err());
    drop(coordinator);
    drop(db);
    clock.advance(2000);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.tick().unwrap();
    // After resume the same batch completes without re-executing any tool.
    coordinator.dispatch_batch("worker-batch", "second", &Allow, |binding, frame, _| {
        Ok(fox_engine_protocol::KernelModelResponse {
            schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(),
            batch_id: frame.batch_id.clone(), checkpoint_seq: frame.checkpoint_seq,
            assistant_message: json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"已恢复并完成。\n<fox-final/>"}]}),
        })
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
}

#[test]
fn worker_model_timeout_with_an_unsettled_tool_fails_closed_without_retry() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = fixture_with_retry_opt(&clock, "timeout-uncertain", None, false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    assert!(coordinator.snapshot().unwrap().tool_calls.iter().any(|tool| tool.state == "running"));
    // The timeout evidence path refuses to retry while a tool is unsettled:
    // the uncertain execution must be reconciled, never replayed.
    assert!(coordinator.dispatch_batch("batch", "stalled", &Allow, |binding, frame, _| {
        Err(timeout_failure(&binding.run_id, &frame.turn_id, frame.checkpoint_seq))
    }).is_err());
    // No retry was scheduled and the uncertain tool was left for reconciliation.
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    assert_eq!(coordinator.snapshot().unwrap().retry.turn_attempts, 0);
}

#[test]
fn noted_worker_progress_arms_first_response_and_idle_is_bounded() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "progress-prompt", None, false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    // The request was armed at wall 1000. A preview at wall 2000 arms
    // first-output; without it the first-response bound would fire at 61000.
    db.note_kernel_model_progress(&run_id, 2000);
    clock.advance(59_999);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    // 119s of further silence stays under the 120s idle bound...
    clock.advance(60_000);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "running");
    // ...but the full idle span fails closed through the Host backstop (the
    // retry path belongs to the settled worker-evidence flow).
    clock.advance(1_001);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "failed");
    let reason = rusqlite::Connection::open(root.join("facts.db")).unwrap().query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.failed' ORDER BY seq DESC LIMIT 1",
        [&run_id], |row| row.get::<_, String>(0)).unwrap();
    assert!(reason.contains("model.idle_timeout"));
}

#[test]
fn silent_request_trips_first_response_before_the_whole_round_bound() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "silent-prompt", None, false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    // Default budgets: 300s whole-round / 60s first-response / 120s idle.
    // No preview ever arrives: the first-response bound fires at 61s, long
    // before the whole-round bound, and the backstop fails the run closed.
    clock.advance(60_000);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "failed");
    let reason = rusqlite::Connection::open(root.join("facts.db")).unwrap().query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.failed' ORDER BY seq DESC LIMIT 1",
        [&run_id], |row| row.get::<_, String>(0)).unwrap();
    assert!(reason.contains("model.first_response_timeout"));
}

#[test]
fn model_progress_registry_is_consume_once_and_bounded() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = fixture(&clock);
    assert_eq!(db.consume_kernel_model_progress(&run_id), None);
    db.note_kernel_model_progress(&run_id, 1500);
    db.note_kernel_model_progress(&run_id, 1600);
    assert_eq!(db.consume_kernel_model_progress(&run_id), Some(1600));
    assert_eq!(db.consume_kernel_model_progress(&run_id), None);
    db.note_kernel_model_progress(&run_id, 1700);
    db.clear_kernel_model_progress(&run_id);
    assert_eq!(db.consume_kernel_model_progress(&run_id), None);
}

#[test]
fn restart_approval_dispatches_real_reads_once_and_preserves_batch_order() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    {
        let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator.propose_tools("batch", calls(), &Ask).unwrap();
        assert_eq!(coordinator.snapshot().unwrap().state, "waiting_approval");
    }
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        db.kernel_recovery_facts(&run_id)
            .unwrap()
            .unwrap()
            .pending_approvals
            .len(),
        2
    );
    coordinator
        .resolve_approval("read-a", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator
        .resolve_approval("read-b", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    // Finish in reverse order using the real Rust file executor, not a mock result.
    for id in ["read-b", "read-a"] {
        assert!(coordinator
            .dispatch_tool(id, "reader-owner", |binding, effect, token| {
                assert_eq!(effect.idempotency_key, kernel::dispatch_idempotency_key(id));
                let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
                Ok((
                    true,
                    crate::resource_gateway::execute(binding, "read", &payload["input"], &token)?,
                ))
            })
            .unwrap());
    }
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(
        snapshot
            .tool_calls
            .iter()
            .map(|tool| tool.tool_call_id.as_str())
            .collect::<Vec<_>>(),
        vec!["read-a", "read-b"]
    );
    for tool in &snapshot.tool_calls {
        assert_eq!(tool.state, "completed");
        let result: Value = serde_json::from_str(tool.result_json.as_ref().unwrap()).unwrap();
        assert_eq!(result["content"][0]["text"], "durable coordinator 中文 😀");
    }
    assert_eq!(
        snapshot
            .pending_effects
            .iter()
            .filter(|effect| effect.kind == kernel::OutboxEffectKind::DeliverToolBatch)
            .count(),
        1
    );
    drop(coordinator);
    drop(db);
    std::fs::write(root.join("proof.txt"), "changed after committed results").unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator
        .dispatch_tool("read-a", "new-owner", |_, _, _| panic!(
            "must not read twice"
        ))
        .unwrap());
    assert_eq!(
        coordinator.snapshot().unwrap().tool_calls[0].result_json,
        snapshot.tool_calls[0].result_json
    );
    let frame = coordinator.prepare_batch_resume("batch", vec![json!({"role":"user","content":"read"})],
        json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}},
            {"type":"toolCall","id":"read-b","name":"read","arguments":{"path":"proof.txt"}}
        ]}), Vec::new()).unwrap();
    for item in &frame.tools {
        let text = item.result["content"].as_array().unwrap().last().unwrap()["text"].as_str().unwrap();
        let receipt: Value = serde_json::from_str(text.strip_prefix("FOX_EXECUTION_RECEIPT_V1\n").unwrap()).unwrap();
        assert_eq!(receipt["approvalDecision"],"allow_once");
        assert_eq!(receipt["executionState"],"completed");
        assert_eq!(receipt["runId"],run_id);
        assert_eq!(receipt["toolCallId"],item.tool_call_id);
        assert_eq!(item.result["content"][0]["text"],"durable coordinator 中文 😀");
    }
    // Projection never changes durable tool facts or rereads the changed file.
    assert_eq!(coordinator.snapshot().unwrap(), snapshot);
}

#[test]
fn uncertain_execution_is_not_retried_and_siblings_stay_pending() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    let executions = AtomicUsize::new(0);
    assert!(coordinator
        .dispatch_tool("read-a", "dead-owner", |_, _, _| {
            executions.fetch_add(1, Ordering::SeqCst);
            Err("transport lost after possible side effect".into())
        })
        .is_err());
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator
        .dispatch_tool("read-a", "new-owner", |_, _, _| panic!(
            "uncertain effect must not rerun"
        ))
        .unwrap());
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    let facts = db.kernel_recovery_facts(&run_id).unwrap().unwrap();
    let plan = kernel::plan_recovery(vec![facts]);
    assert_eq!(plan.uncertain_leased.len(), 1);
    assert!(plan
        .redispatch_pending
        .iter()
        .any(|(_, effect)| effect.tool_call_id.as_deref() == Some("read-b")));
}

#[test]
fn expired_approval_and_cancel_never_reach_executor() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Ask).unwrap();
    clock.advance(300_000);
    assert!(coordinator
        .resolve_approval("read-a", kernel::ApprovalDecision::AllowOnce)
        .is_err());
    assert!(!coordinator
        .dispatch_tool("read-a", "owner", |_, _, _| panic!("expired"))
        .unwrap());
    let (db2, _, run2) = fixture(&clock);
    let cancelled = KernelCoordinator::reopen(&db2, &clock, &run2, &cancellation).unwrap();
    cancelled.propose_tools("batch", calls(), &Allow).unwrap();
    cancelled.cancel().unwrap();
    assert!(db2
        .kernel_outbox_lease_all_pending("cancel-drain")
        .unwrap()
        .iter()
        .all(|(_, effect)| matches!(
            effect.kind,
            kernel::OutboxEffectKind::CancelEngineTurn | kernel::OutboxEffectKind::CancelToolCall
        )));
    assert!(!cancelled
        .dispatch_tool("read-a", "owner", |_, _, _| panic!("cancelled"))
        .unwrap());
}

#[test]
fn failed_commit_rolls_back_memory_and_leaves_executed_effect_uncertain() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_kernel_tool BEFORE INSERT ON kernel_tool_calls BEGIN SELECT RAISE(ABORT,'injected proposal failure'); END;").unwrap();
    assert!(coordinator.propose_tools("batch", calls(), &Allow).is_err());
    assert!(coordinator.snapshot().unwrap().tool_calls.is_empty());
    connection
        .execute_batch("DROP TRIGGER reject_kernel_tool;")
        .unwrap();
    // Same in-memory coordinator can retry a rolled-back proposal without a
    // speculative duplicate batch or event-sequence gap.
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_kernel_result BEFORE INSERT ON kernel_events WHEN NEW.event_type='tool.completed' BEGIN SELECT RAISE(ABORT,'injected result failure'); END;").unwrap();
    assert!(coordinator
        .dispatch_tool("read-a", "owner", |_, _, _| Ok((
            true,
            json!({"proof":"executed"})
        )))
        .is_err());
    connection
        .execute_batch("DROP TRIGGER reject_kernel_result;")
        .unwrap();
    assert!(!coordinator
        .dispatch_tool("read-a", "retry-owner", |_, _, _| panic!(
            "executed but uncommitted is uncertain"
        ))
        .unwrap());
    assert_eq!(
        coordinator.snapshot().unwrap().tool_calls[0].state,
        "running"
    );
    assert!(coordinator
        .dispatch_tool("read-b", "owner-b", |_, _, _| Ok((
            true,
            json!({"proof":"sibling"})
        )))
        .unwrap());
    assert_eq!(
        coordinator.snapshot().unwrap().tool_calls[1].state,
        "completed"
    );
}

#[test]
fn foreign_lease_owner_cannot_commit_results_and_cancel_wins_late_result() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    assert!(coordinator
        .dispatch_tool("read-a", "real-owner", |_, _, token| {
            assert!(token.check().is_ok());
            let mut controller =
                RunController::rehydrate(db.kernel_rehydrate(&run_id)?.unwrap()).unwrap();
            let effects = controller.tool_settled("read-a", true, "{}").unwrap();
            assert!(db
                .kernel_commit_tool_result(
                    &run_id,
                    clock.now_wall_ms(),
                    &controller.persist_command(&effects),
                    "read-a",
                    "foreign-owner"
                )
                .is_err());
            coordinator.cancel()?;
            assert!(
                token.check().is_err(),
                "durable cancel must reach the running executor"
            );
            Ok((true, json!({"late":"success"})))
        })
        .unwrap());
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "cancelling");
    assert_eq!(snapshot.tool_calls[0].state, "cancelled");
}

#[test]
fn corrupt_frozen_binding_prevents_execution_after_coordinator_open() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Allow).unwrap();
    rusqlite::Connection::open(root.join("facts.db"))
        .unwrap()
        .execute(
            "UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1",
            [&run_id],
        )
        .unwrap();
    assert!(coordinator
        .dispatch_tool("read-a", "owner", |_, _, _| panic!("invalid binding"))
        .is_err());
    assert!(KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).is_err());
}

#[test]
fn independent_reads_can_execute_concurrently_without_holding_the_controller_lock() {
    use std::{sync::mpsc, time::Duration};
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .propose_tools("parallel", calls(), &Allow)
        .unwrap();
    let (started, arrivals) = mpsc::channel();
    let (release_a, wait_a) = mpsc::channel();
    let (release_b, wait_b) = mpsc::channel();
    let coordinator = &coordinator;
    let started_a = started.clone();
    std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            coordinator.dispatch_tool("read-a", "owner-a", |_, _, token| {
                started_a.send("a").unwrap();
                wait_a
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| error.to_string())?;
                token.check()?;
                Ok((true, json!({"source":"a"})))
            })
        });
        let second = scope.spawn(move || {
            coordinator.dispatch_tool("read-b", "owner-b", |_, _, token| {
                started.send("b").unwrap();
                wait_b
                    .recv_timeout(Duration::from_secs(5))
                    .map_err(|error| error.to_string())?;
                token.check()?;
                Ok((true, json!({"source":"b"})))
            })
        });
        let one = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
        let two = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(one, two, "both callbacks must enter before either returns");
        release_b.send(()).unwrap();
        release_a.send(()).unwrap();
        assert!(first.join().unwrap().unwrap());
        assert!(second.join().unwrap().unwrap());
    });
    assert!(coordinator
        .snapshot()
        .unwrap()
        .tool_calls
        .iter()
        .all(|tool| tool.state == "completed"));
}

fn pi_batch_probe(mode: &str, input: Value) -> Value {
    use std::{
        io::Write,
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct ProbeChild(Option<Child>);
    impl Drop for ProbeChild {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut command = Command::new("node");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = ProbeChild(Some(
        command
            .arg(repository.join("services/agent-runtime/test/fixtures/pi-kernel-batch-child.mjs"))
            .arg(mode)
            .current_dir(&repository)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    child
        .0
        .as_mut()
        .unwrap()
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.0.as_mut().unwrap().try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "Pi batch probe timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.0.take().unwrap().wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Pi batch probe failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn pi_checkpoint_sqlite_reopen_rust_readers_and_replacement_pi_share_one_result_batch() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    std::fs::write(root.join("a.txt"), "Rust persisted result A 中文").unwrap();
    std::fs::write(root.join("b.txt"), "Rust persisted result B 😀").unwrap();
    std::fs::write(root.join("c.txt"), "Rust persisted next result C").unwrap();
    let captured = pi_batch_probe("capture", json!({}));
    assert_eq!(captured["executions"], 0);
    let messages = captured["messages"].as_array().unwrap();
    let original = messages.last().unwrap().clone();
    let history = messages[..messages.len() - 1].to_vec();
    {
        let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator
            .propose_engine_batch(
                fox_engine_protocol::KernelEngineBatchCheckpoint {
                    schema_version: 1,
                    batch_id: "pi-batch".into(),
                    history,
                    assistant_message: original,
                },
                &Ask,
            )
            .unwrap();
        assert!(coordinator.prepare_stored_batch_resume("pi-batch").is_err());
    }
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    for id in ["read-b", "read-a"] {
        coordinator
            .resolve_approval(id, kernel::ApprovalDecision::AllowOnce)
            .unwrap();
        coordinator
            .dispatch_tool(id, "rust-reader", |binding, effect, token| {
                let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
                Ok((
                    true,
                    crate::resource_gateway::execute(binding, "read", &payload["input"], token)?,
                ))
            })
            .unwrap();
    }
    coordinator.dispatch_batch("pi-batch", "pi-delivery", &Ask, |binding, frame, token| {
        token.check()?;
        assert!(coordinator.prepare_stored_batch_resume("pi-batch").is_err());
        let facts = db.kernel_rehydrate(&run_id)?.unwrap();
        assert_eq!(facts.model_request_since_wall_ms, Some(clock.now_wall_ms()));
        let identity = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"replacement-pi","executionProfileId":binding.execution_profile_id});
        let request = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"replacement-pi",
            "payload":{"controlBinding":binding,"batchResume":frame}});
        let resumed = pi_batch_probe("resume-propose", json!({"request":request,"identity":identity,
            "expectedResults":["Rust persisted result A 中文","Rust persisted result B 😀"]}));
        assert_eq!(resumed["executions"], 0);
        assert_eq!(resumed["consumed"], json!(["read-a", "read-b"]));
        assert_eq!(resumed["providerRequests"], 1);
        assert_eq!(resumed["toolExecutionStarts"], 0);
        assert_eq!(resumed["response"]["assistantMessage"]["stopReason"], "toolUse");
        serde_json::from_value(resumed["response"].clone()).map_err(|error| error.to_string())
    }).unwrap();
    assert!(coordinator.prepare_stored_batch_resume("pi-batch").is_err());
    let facts = db.kernel_rehydrate(&run_id).unwrap().unwrap();
    assert_eq!(facts.state, kernel::RunState::WaitingApproval);
    assert_eq!(facts.model_request_since_wall_ms, None);
    let next_batch = facts.tools.iter().find(|tool| tool.tool_call_id == "read-c").unwrap().batch_id.clone();
    let next_checkpoint = db.kernel_engine_batch_checkpoint(&run_id, &next_batch).unwrap().unwrap();
    assert_eq!(next_checkpoint["checkpoint"]["value"]["history"].as_array().unwrap().iter()
        .filter(|message| message["role"] == "toolResult").count(), 2);
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.resolve_approval("read-c", kernel::ApprovalDecision::AllowOnce).unwrap();
    coordinator.dispatch_tool("read-c", "next-reader", |binding, effect, token| {
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        Ok((true, crate::resource_gateway::execute(binding, "read", &payload["input"], token)?))
    }).unwrap();
    coordinator.dispatch_batch(&next_batch, "next-pi-delivery", &Ask, |binding, frame, token| {
        token.check()?;
        let identity = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"next-pi","executionProfileId":binding.execution_profile_id});
        let request = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"next-pi",
            "payload":{"controlBinding":binding,"batchResume":frame}});
        let resumed = pi_batch_probe("resume", json!({"request":request,"identity":identity,
            "expectedResults":["Rust persisted result A 中文","Rust persisted result B 😀","Rust persisted next result C"]}));
        assert_eq!(resumed["executions"], 0);
        assert_eq!(resumed["providerRequests"], 1);
        assert_eq!(resumed["answer"], "durable batch consumed");
        serde_json::from_value(resumed["response"].clone()).map_err(|error| error.to_string())
    }).unwrap();
    assert!(coordinator.snapshot().unwrap().pending_effects.is_empty());
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Completed);
}

#[test]
fn original_engine_checkpoint_and_proposal_commit_atomically_and_replay_exactly() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let checkpoint = fox_engine_protocol::KernelEngineBatchCheckpoint {
        schema_version: 1,
        batch_id: "checkpoint-batch".into(),
        history: vec![json!({"role":"user","content":"read proof"})],
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}]}),
    };
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_checkpoint BEFORE INSERT ON kernel_events WHEN NEW.event_type='engine.batch_checkpoint' BEGIN SELECT RAISE(ABORT,'injected checkpoint failure'); END;").unwrap();
    assert!(coordinator
        .propose_engine_batch(checkpoint.clone(), &Ask)
        .is_err());
    assert!(coordinator.snapshot().unwrap().tool_calls.is_empty());
    assert!(db
        .kernel_engine_batch_checkpoint(&run_id, "checkpoint-batch")
        .unwrap()
        .is_none());
    connection
        .execute_batch("DROP TRIGGER reject_checkpoint;")
        .unwrap();
    coordinator
        .propose_engine_batch(checkpoint.clone(), &Ask)
        .unwrap();
    let before = coordinator.snapshot().unwrap();
    coordinator
        .propose_engine_batch(checkpoint.clone(), &Ask)
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap(), before);
    let mut changed = checkpoint;
    changed.history[0]["content"] = json!("different history");
    assert!(coordinator.propose_engine_batch(changed, &Ask).is_err());
    connection.execute("UPDATE kernel_events SET payload_json=json_set(payload_json,'$.checkpoint.hash','corrupt') WHERE run_id=?1 AND event_type='engine.batch_checkpoint'", [&run_id]).unwrap();
    assert!(coordinator
        .prepare_stored_batch_resume("checkpoint-batch")
        .unwrap_err()
        .contains("hash"));
}

#[test]
fn batch_dispatch_claim_and_deadline_are_atomic_and_uncertain_delivery_is_not_replayed() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_engine_batch(fox_engine_protocol::KernelEngineBatchCheckpoint {
        schema_version: 1, batch_id: "batch".into(),
        history: vec![json!({"role":"user","content":"read"})],
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}]}),
    }, &Allow).unwrap();
    coordinator.dispatch_tool("read-a", "reader", |_, _, _| Ok((true,
        json!({"content":[{"type":"text","text":"durable"}]})))).unwrap();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_model_dispatch BEFORE INSERT ON kernel_events WHEN NEW.event_type='engine.batch_dispatched' BEGIN SELECT RAISE(ABORT,'injected model dispatch failure'); END;").unwrap();
    let executions = AtomicUsize::new(0);
    assert!(coordinator.dispatch_batch("batch", "model", &Allow, |_, _, _| {
        executions.fetch_add(1, Ordering::SeqCst); Err("must not send".into())
    }).is_err());
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms, None);
    assert!(coordinator.prepare_stored_batch_resume("batch").is_ok());
    connection.execute_batch("DROP TRIGGER reject_model_dispatch;").unwrap();
    assert!(coordinator.dispatch_batch("batch", "model", &Allow, |_, _, token| {
        token.check()?;
        executions.fetch_add(1, Ordering::SeqCst);
        Err("connection lost after sending request".into())
    }).is_err());
    let started_at = db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms;
    assert_eq!(started_at, Some(1_000));
    drop(coordinator);
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(reopened.dispatch_batch("batch", "new-owner", &Allow, |_, _, _| {
        executions.fetch_add(1, Ordering::SeqCst); Err("must not resend".into())
    }).is_err());
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms, started_at);
    clock.advance(120_001);
    reopened.tick().unwrap();
    assert!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state.is_terminal());
    assert!(reopened.dispatch_batch("batch", "expired", &Allow, |_, _, _| panic!("expired model dispatched")).is_err());
}

#[test]
fn model_response_failure_never_partially_commits_terminal_or_next_batch() {
    for failure in ["commit", "foreign-owner", "foreign-run", "foreign-cursor", "cancel"] {
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db, root, run_id) = fixture(&clock);
        let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator.propose_engine_batch(fox_engine_protocol::KernelEngineBatchCheckpoint {
            schema_version: 1, batch_id: "batch".into(),
            history: vec![json!({"role":"user","content":"read"})],
            assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
                {"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}]}),
        }, &Allow).unwrap();
        coordinator.dispatch_tool("read-a", "reader", |_, _, _| Ok((true,
            json!({"content":[{"type":"text","text":"durable"}]})))).unwrap();
        let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        if failure == "commit" {
            connection.execute_batch("CREATE TRIGGER reject_model_response BEFORE INSERT ON kernel_events WHEN NEW.event_type='engine.batch_response' BEGIN SELECT RAISE(ABORT,'injected model response failure'); END;").unwrap();
        }
        let result = coordinator.dispatch_batch("batch", "model", &Ask, |binding, frame, _| {
            if failure == "foreign-owner" {
                connection.execute("UPDATE kernel_effect_outbox SET lease_owner='foreign' WHERE run_id=?1 AND effect_type='deliver_tool_batch'", [&run_id]).unwrap();
            }
            if failure == "cancel" { coordinator.cancel()?; }
            Ok(fox_engine_protocol::KernelModelResponse {
                schema_version: 1,
                run_id: if failure == "foreign-run" { "different-run".into() } else { binding.run_id.clone() },
                turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(),
                checkpoint_seq: frame.checkpoint_seq + u64::from(failure == "foreign-cursor"),
                assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
                    {"type":"toolCall","id":"read-next","name":"read","arguments":{"path":"proof.txt"}}]}),
            })
        });
        assert!(result.is_err(), "{failure}");
        let facts = db.kernel_rehydrate(&run_id).unwrap().unwrap();
        assert_eq!(facts.tools.len(), 1, "{failure}");
        assert_ne!(facts.state, kernel::RunState::Completed, "{failure}");
        let responses: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.batch_response'", [&run_id], |row| row.get(0)).unwrap();
        assert_eq!(responses, 0, "{failure}");
        let delivered: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='deliver_tool_batch' AND status='completed'", [&run_id], |row| row.get(0)).unwrap();
        assert_eq!(delivered, 0, "{failure}");
    }
}

fn worker_configuration() -> super::super::kernel_model_worker::KernelModelConfig {
    super::super::kernel_model_worker::KernelModelConfig {
        engine_id: "pi".into(), native_adapter: None,
        execution_profile_id: "legacy".into(),
        model_service: json!({"apiType":"faux","modelId":"kernel-host-test","baseUrl":"http://localhost",
            "fauxResponses":["Host consumed the durable batch"]}),
        system_prompt: "Use the durable supplied history only.".into(),
        proposal_tools: vec![json!({"name":"read","description":"Propose a Host-owned read",
            "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}})],
    }
}

fn real_worker_command() -> super::super::RuntimeCommand {
    super::super::RuntimeCommand {
        program: "node".into(),
        script: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../services/agent-runtime/src/pi-runtime.mjs")),
    }
}

#[test]
fn kernel_repair_override_cannot_be_auto_approved_or_prompt_for_ineligible_work() {
    let clock=TestClock::new(1_000);
    let config = worker_configuration();
    let (db,_,run_id)=fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let mut scope=freeze_host_scope(&db,&run_id);
    scope.tool_names.insert("task_repair_escalate_start".into());
    let mut binding=db.run_control_binding(&run_id).unwrap().unwrap();
    binding.execution_profile_id="durable_v2".into();
    binding.permission.mode=PermissionMode::Allow;
    binding.permission.grants.push(fox_engine_protocol::PermissionGrant {tool:"task_repair_escalate_start".into(),scope:"project".into()});
    binding.permission_snapshot_id=Database::run_control_permission_hash(&binding.permission).unwrap();
    let policy=super::super::kernel_gateway::GatewayPolicy {binding,scope, database: None, sessions_dir: None, artifacts_dir: None };
    let input=json!({"taskId":"missing-task","attemptId":"repair-attempt","expectedVersion":1,
        "rootCause":"Confirmed root cause","findingIds":["missing-finding"],"escalationReason":"One bounded repair"}).to_string();
    assert!(matches!(policy.decide(&run_id,"repair","task_repair_escalate_start",&input),PolicyDecision::RequireApproval));
    let proposals=super::super::kernel_gateway::GatewayProposalPolicy {gateway:&policy,database:&db};
    assert!(matches!(proposals.decide(&run_id,"repair","task_repair_escalate_start",&input),PolicyDecision::Deny {..}));
}

fn freeze_host_scope(db: &Database, run_id: &str) -> crate::database::KernelHostScope {
    let scope = crate::database::KernelHostScope { schema_version: 1, tool_names: ["read".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(), knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks:Vec::new() };
    db.freeze_kernel_host_scope(run_id, &scope).unwrap();
    scope
}

#[test]
fn owning_host_loop_starts_real_model_and_commits_one_visible_final() {
    let config = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    freeze_host_scope(&db, &run_id);
    super::super::kernel_host::drive(super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &cancellation, &run_id, &real_worker_command(), "test-key", &Allow,
        |_,_,_| panic!("plain model answer must not execute resources")).unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "completed");
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let output: (i64,String) = connection.query_row("SELECT COUNT(*),MAX(content) FROM messages WHERE run_id=?1 AND role='assistant'",
        [&run_id], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(output, (1,"Host consumed the durable batch".into()));
}

#[test]
fn owning_host_recovery_consumes_queued_approvals_then_real_reads_and_model() {
    let config = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let scope = freeze_host_scope(&db, &run_id);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("proposal", &Ask, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}]}),
    })).unwrap();
    db.queue_kernel_host_command(&run_id, Some(("read-a","allow_once"))).unwrap();
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let policy = super::super::kernel_gateway::GatewayPolicy { binding: db.run_control_binding(&run_id).unwrap().unwrap(), scope , database: None, sessions_dir: None, artifacts_dir: None };
    let count = AtomicUsize::new(0);
    super::super::kernel_host::drive(super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &cancellation, &run_id, &real_worker_command(), "test-key", &policy,
        |_,effect,token| {
            count.fetch_add(1, Ordering::SeqCst);
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            Ok((true,policy.execute(&db, "read", &payload["input"], token)?))
        }).unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "completed");
    assert!(db.pending_kernel_host_commands(&run_id).unwrap().is_empty());
}

#[test]
fn owning_host_recovery_never_replays_an_uncertain_model_request() {
    let config = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    freeze_host_scope(&db, &run_id);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator.dispatch_initial("uncertain", &Allow, |_,_,_| Err("simulated lost response".into())).is_err());
    drop(coordinator);
    let invalid_worker = super::super::RuntimeCommand { program: "must-not-start-a-model".into(), script: None };
    super::super::kernel_host::drive(super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &cancellation, &run_id, &invalid_worker, "test-key", &Allow,
        |_,_,_| panic!("uncertain recovery must not execute resources")).unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "failed");
}

pub(super) fn settle_worker_batch(coordinator: &KernelCoordinator<'_>) {    coordinator.propose_engine_batch(fox_engine_protocol::KernelEngineBatchCheckpoint {
        schema_version: 1, batch_id: "worker-batch".into(),
        history: vec![json!({"role":"user","content":"read the file"})],
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"worker-read","name":"read","arguments":{"path":"proof.txt"}}]}),
    }, &Allow).unwrap();
    coordinator.dispatch_tool("worker-read", "real-reader", |binding, effect, token| {
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        Ok((true, crate::resource_gateway::execute(binding, "read", &payload["input"], token)?))
    }).unwrap();
}

#[test]
fn formal_worker_transport_checks_frozen_configuration_before_lease_and_commits_real_response() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_prompt(&clock, &config.hash().unwrap());
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    for part in ["prompt", "model", "schema", "profile", "credential"] {
        let mut changed = config.clone();
        match part {
            "prompt" => changed.system_prompt.push_str(" changed"),
            "model" => changed.model_service["modelId"] = json!("different"),
            "schema" => changed.proposal_tools.clear(),
            "profile" => changed.execution_profile_id = "shadow".into(),
            _ => changed.model_service["apiKey"] = json!("must-not-freeze"),
        }
        let before = coordinator.snapshot().unwrap();
        assert!(coordinator.dispatch_batch_with_worker("worker-batch", "worker", &Allow,
            &real_worker_command(), &changed, "test-key").is_err(), "{part}");
        assert_eq!(coordinator.snapshot().unwrap(), before, "{part}");
        assert!(coordinator.prepare_stored_batch_resume("worker-batch").is_ok());
    }
    coordinator.dispatch_batch_with_worker("worker-batch", "worker", &Allow,
        &real_worker_command(), &config, "test-key").unwrap();
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Completed);
    assert!(coordinator.dispatch_batch_with_worker("worker-batch", "worker-again", &Allow,
        &real_worker_command(), &config, "test-key").is_err());
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let responses: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.batch_response'",
        [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(responses, 1);
}

#[test]
fn formal_worker_protocol_failure_retains_uncertain_delivery_across_reopen() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_prompt(&clock, &config.hash().unwrap());
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let mut wrong = real_worker_command();
    // The fake Legacy runtime is a real process but cannot speak this protocol.
    wrong.script = Some(wrong.script.unwrap().with_file_name("fake-runtime.mjs"));
    assert!(coordinator.dispatch_batch_with_worker("worker-batch", "worker", &Allow,
        &wrong, &config, "test-key").is_err());
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator.dispatch_batch_with_worker("worker-batch", "new-worker", &Allow,
        &real_worker_command(), &config, "test-key").is_err());
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let status: String = connection.query_row("SELECT status FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='deliver_tool_batch'",
        [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(status, "leased");
    assert_ne!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Completed);
}

#[test]
fn formal_worker_can_be_cancelled_without_holding_the_coordinator_lock() {
    let mut config = worker_configuration();
    config.model_service["fauxTokensPerSecond"] = json!(1);
    config.model_service["fauxResponses"] = json!(["This model response must be cancelled before completion"]);
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture_with_prompt(&clock, &config.hash().unwrap());
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        let running = scope.spawn(|| coordinator.dispatch_batch_with_worker("worker-batch", "worker", &Allow,
            &real_worker_command(), &config, "test-key"));
        // Wait for the durable claim, not an arbitrary delay before cancellation.
        while db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms.is_none() {
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        coordinator.cancel().unwrap();
        assert!(running.join().unwrap().is_err());
    });
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_ne!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Completed);
}

#[test]
fn formal_worker_uses_remaining_run_budget_instead_of_a_fresh_full_window() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture_with_prompt(&clock, &config.hash().unwrap());
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    clock.advance(1_800_000 - 1);
    let started = std::time::Instant::now();
    coordinator.dispatch_batch_with_worker("worker-batch", "worker", &Allow,
        &real_worker_command(), &config, "test-key").unwrap();
    // Transport expiry is now settled durably, rather than escaping as a plain
    // deadline string. There is neither enough remaining time nor retry budget.
    let facts = db.kernel_rehydrate(&run_id).unwrap().unwrap();
    assert_eq!(facts.state, kernel::RunState::Failed);
    assert_eq!(facts.retry.turn_attempts, 0);
    assert!(facts.tools.iter().all(|tool| tool.state == kernel::ToolCallState::Completed));
    assert!(started.elapsed() < std::time::Duration::from_secs(5));
    assert!(coordinator.prepare_stored_batch_resume("worker-batch").is_err());
    assert_ne!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Completed);
}

#[test]
fn stored_model_configuration_reopens_and_drives_real_pi_without_caller_configuration() {
    let mut config = worker_configuration();
    let frozen = config.clone();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_model(&clock, &config);
    db.freeze_kernel_model_config(&run_id, &config).unwrap(); // Exact replay only.
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    config.model_service["fauxResponses"] = json!(["Changed current configuration must not be used"]);
    assert!(db.freeze_kernel_model_config(&run_id, &config).is_err());
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_model_config(&run_id).unwrap(), frozen);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_stored_batch_with_worker("worker-batch", "stored-worker", &Allow,
        &real_worker_command(), "test-key").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let response: String = connection.query_row("SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='engine.batch_response'",
        [&run_id], |row| row.get(0)).unwrap();
    assert!(response.contains("Host consumed the durable batch"));
    assert!(!response.contains("Changed current configuration"));
}

#[test]
fn model_snapshot_is_immutable_and_tampering_fails_before_any_dispatch_lease() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_model(&clock, &config);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    assert!(connection.execute("UPDATE kernel_model_configs SET config_json=config_json WHERE run_id=?1", [&run_id]).is_err());
    connection.execute_batch("DROP TRIGGER kernel_model_config_immutable;").unwrap();
    let original = serde_json::to_string(&config).unwrap();
    for (column, changed) in [
        ("config_json", "{}"), ("config_hash", "wrong-hash"), ("adapter_version", "pi-future-incompatible"),
    ] {
        connection.execute(&format!("UPDATE kernel_model_configs SET {column}=?1 WHERE run_id=?2"),
            rusqlite::params![changed,run_id]).unwrap();
        let before = coordinator.snapshot().unwrap();
        assert!(db.kernel_model_config(&run_id).is_err(), "{column}");
        assert!(coordinator.dispatch_stored_batch_with_worker("worker-batch", "must-not-dispatch", &Allow,
            &real_worker_command(), "test-key").is_err(), "{column}");
        assert_eq!(coordinator.snapshot().unwrap(), before);
        assert!(coordinator.prepare_stored_batch_resume("worker-batch").is_ok());
        connection.execute("UPDATE kernel_model_configs SET config_json=?1,config_hash=?2,adapter_version=?3 WHERE run_id=?4",
            rusqlite::params![original,config.hash().unwrap(),crate::kernel_model_config::KERNEL_MODEL_ADAPTER,run_id]).unwrap();
    }
    connection.execute("DELETE FROM kernel_model_configs WHERE run_id=?1", [&run_id]).unwrap();
    assert!(coordinator.dispatch_stored_batch_with_worker("worker-batch", "missing", &Allow,
        &real_worker_command(), "test-key").is_err());
    // An active historical Run may not backfill configuration from today's settings.
    assert!(db.freeze_kernel_model_config(&run_id, &config).is_err());
}

#[test]
fn v54_model_snapshot_upgrade_preserves_existing_runs_without_backfilling() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let (db, root, run_id) = fixture_with_model(&clock, &config);
    let expected_binding = db.run_control_binding(&run_id).unwrap().unwrap();
    drop(db);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("DROP TABLE kernel_model_configs; DELETE FROM schema_migrations WHERE version=55;").unwrap();
    drop(connection);
    for _ in 0..2 {
        let db = Database::open(root.join("facts.db")).unwrap();
        assert_eq!(db.run_control_binding(&run_id).unwrap().unwrap(), expected_binding);
        assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Running);
        assert!(db.kernel_model_config(&run_id).unwrap_err().contains("missing"));
        assert!(db.freeze_kernel_model_config(&run_id, &config).is_err());
    }
}

#[test]
fn initial_input_reopens_exactly_and_does_not_follow_current_messages() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let (db, root, run_id) = fixture_with_initial_opt(&clock, &config.hash().unwrap(), Some(&config), true);
    let original = initial_input(&run_id, &config.hash().unwrap());
    db.freeze_kernel_initial_input(&original).unwrap();
    let mut changed = original.clone();
    changed.messages[0]["content"] = json!("Changed current conversation");
    assert!(db.freeze_kernel_initial_input(&changed).is_err());
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state, kernel::RunState::Running);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    assert!(connection.execute("UPDATE kernel_initial_inputs SET input_json=input_json WHERE run_id=?1", [&run_id]).is_err());
    assert!(connection.execute("UPDATE kernel_runs SET turn_id='different-turn' WHERE run_id=?1", [&run_id]).is_err());
    // Snapshot storage must not claim or create a model dispatch.
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND status='leased'", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn initial_input_missing_or_changed_configuration_never_backfills_after_start() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let (db, root, run_id) = fixture_with_model(&clock, &config);
    assert!(db.kernel_initial_input(&run_id).unwrap_err().contains("missing"));
    assert!(db.freeze_kernel_initial_input(&initial_input(&run_id, &config.hash().unwrap())).is_err());
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let count: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_initial_inputs", [], |row| row.get(0)).unwrap();
    assert_eq!(count, 0);
}

#[test]
fn initial_input_tampering_is_rejected_without_exposing_message_content() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let (db, root, run_id) = fixture_with_initial_opt(&clock, &config.hash().unwrap(), Some(&config), true);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let original = initial_input(&run_id, &config.hash().unwrap());
    let encoded = serde_json::to_string(&original).unwrap();
    let original_hash = format!("sha256:{}", hex::encode(Sha256::digest(encoded.as_bytes())));
    connection.execute_batch("DROP TRIGGER kernel_initial_input_immutable").unwrap();
    for (column, changed) in [("input_json", "{}"), ("input_hash", "wrong"), ("turn_id", "wrong"), ("prompt_config_hash", "wrong")] {
        connection.execute(&format!("UPDATE kernel_initial_inputs SET {column}=?1 WHERE run_id=?2"), rusqlite::params![changed,run_id]).unwrap();
        let error = db.kernel_initial_input(&run_id).unwrap_err();
        assert!(!error.contains("初始问题"));
        assert!(db.freeze_kernel_initial_input(&original).is_err());
        connection.execute("UPDATE kernel_initial_inputs SET input_json=?1,input_hash=?2,turn_id=?3,prompt_config_hash=?4 WHERE run_id=?5",
            rusqlite::params![encoded,original_hash,original.turn_id,original.prompt_config_hash,run_id]).unwrap();
    }
    connection.execute_batch("DROP TRIGGER kernel_model_config_immutable").unwrap();
    connection.execute("UPDATE kernel_model_configs SET config_hash='wrong' WHERE run_id=?1", [&run_id]).unwrap();
    assert!(db.kernel_initial_input(&run_id).is_err());
    assert!(db.freeze_kernel_initial_input(&original).is_err());
}

#[test]
fn v55_initial_input_upgrade_preserves_runs_without_inventing_old_context() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let (db, root, run_id) = fixture_with_model(&clock, &config);
    let binding = db.run_control_binding(&run_id).unwrap().unwrap();
    drop(db);
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("DROP TABLE kernel_initial_inputs; DROP TRIGGER kernel_initial_input_start_guard; DELETE FROM schema_migrations WHERE version=56").unwrap();
    drop(connection);
    for _ in 0..2 {
        let db = Database::open(root.join("facts.db")).unwrap();
        assert_eq!(db.run_control_binding(&run_id).unwrap().unwrap(), binding);
        assert_eq!(db.kernel_model_config(&run_id).unwrap(), config);
        assert!(db.kernel_initial_input(&run_id).unwrap_err().contains("missing"));
        assert!(db.freeze_kernel_initial_input(&initial_input(&run_id, &config.hash().unwrap())).is_err());
    }
}

#[test]
fn initial_model_real_worker_after_reopen_commits_final_and_consumes_once() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().pending_effects.iter().filter(|e| e.kind == kernel::OutboxEffectKind::InitialModel).count(), 1);
    assert!(db.kernel_outbox_lease_effect(&run_id, kernel::INITIAL_MODEL_EFFECT_KEY, "wrong-path").unwrap().is_none());
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial_with_worker("first", &Allow, &real_worker_command(), "test-key").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert!(coordinator.dispatch_initial_with_worker("replay", &Allow, &real_worker_command(), "test-key").is_err());
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let (status, attempts): (String, i64) = connection.query_row("SELECT status,attempts FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='initial-model'", [&run_id], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!((status, attempts), ("completed".into(), 1));
    let records:Vec<String>={let mut q=connection.prepare("SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.request'").unwrap();
        q.query_map([&run_id],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap()};
    assert_eq!(records.len(),1,"the real worker request is durably accounted once, including after attempted replay");
    let record:Value=serde_json::from_str(&records[0]).unwrap();
    assert_eq!(record["record"]["schemaVersion"],"usage-v1");
    assert_eq!(record["record"]["runId"],run_id);
    assert_eq!(record["record"]["outcome"],"success");
    assert!(record["record"]["cost"]["knownCost"].is_null());
    assert!(!records[0].contains("test-key"));
}

fn settled_provider_rejection(run_id: &str, turn_id: &str, checkpoint_seq: u64) -> String {
    format!("kernel.settled_model_failure:{}", json!({
        "schemaVersion": 1, "runId": run_id, "turnId": turn_id,
        "checkpointSeq": checkpoint_seq, "category": "provider_unavailable",
        "httpStatus": 429, "retryAfterMs": 2000,
    }))
}

fn start_http_model_fixture(replies: Vec<Value>) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        while requests.len() < replies.len() {
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "model request was not received");
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            let (header_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                assert!(bytes.len() < 1_048_576);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length: usize = headers.lines().find_map(|line| line.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                    assert!(length < 1_048_576);
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            let body: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let delta = replies[requests.len()].clone();
            let finish = if delta["tool_calls"].is_array() { "tool_calls" } else { "stop" };
            requests.push(body);
            let chunks = [
                json!({"id":"local-pilot-test","object":"chat.completion.chunk","created":1,"model":"kernel-http-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                json!({"id":"local-pilot-test","object":"chat.completion.chunk","created":1,"model":"kernel-http-test","choices":[{"index":0,"delta":{},"finish_reason":finish}]}),
            ];
            let response = format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", chunks[0], chunks[1]);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", response.len(), response).unwrap();
        }
        requests
    });
    (address, server)
}

fn completion_contract_for_test() -> String {
    let prompt = std::process::Command::new("node").args(["--input-type=module", "-e", "import { KERNEL_COMPLETION_CONTRACT } from './services/agent-runtime/src/kernel-completion.mjs'; process.stdout.write(KERNEL_COMPLETION_CONTRACT)"])
        .current_dir(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))
        .output().unwrap();
    assert!(prompt.status.success());
    String::from_utf8(prompt.stdout).unwrap()
}

#[test]
fn agv_shaped_stalled_tool_argument_stream_times_out_idle_and_retries_without_replay() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    // One server, two sequential connections (same frozen config/hash):
    //  1. sends one chunk of partial tool-call JSON (as the AGV failure
    //     produced long Office arguments), then stalls forever — only the
    //     idle bound can classify this;
    //  2. answers the retried request with a completed final answer.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let read_request = |stream: &mut std::net::TcpStream| {
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.windows(4).position(|part| part == b"\r\n\r\n").is_some() {
                    break;
                }
            }
        };
        let sse = |stream: &mut std::net::TcpStream, body: String| {
            // One complete chunked response: data chunk + terminating chunk.
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n", body.len(), body).unwrap();
        };
        let sse_stall = |stream: &mut std::net::TcpStream, body: String| {
            // A partial chunk with NO terminating chunk: the stream stays
            // open, so only the worker's idle bound can classify the stall.
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{:x}\r\n{}\r\n", body.len(), body).unwrap();
        };

        // Connection 1: partial tool arguments, then stall with no further bytes.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut first = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "stalled request was not received");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        read_request(&mut first);
        let chunk = json!({"id":"agv-stall","object":"chat.completion.chunk","created":1,"model":"agv-stall","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"write-excel","type":"function","function":{"name":"read","arguments":"{\"path\":\"proof.t"}}]},"finish_reason":null}]});
        sse_stall(&mut first, format!("data: {chunk}\n\n"));
        // Stall past the 1.5s idle bound (the worker aborts on its own timer)
        // but release early enough that the retried request still arrives
        // inside its 1s first-response budget.
        std::thread::sleep(Duration::from_millis(2_500));
        drop(first);

        // Connection 2: the retry completes normally.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut second = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "retried request was not received");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("{error}"),
            }
        };
        read_request(&mut second);
        let delta = json!({"role":"assistant","content":"继续完成剩余表格并交付。\n<fox-final/>"});
        let chunks = [
            json!({"id":"agv-retry","object":"chat.completion.chunk","created":1,"model":"agv-stall","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
            json!({"id":"agv-retry","object":"chat.completion.chunk","created":1,"model":"agv-stall","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ];
        sse(&mut second, format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", chunks[0], chunks[1]));
    });

    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"agv-stall","baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let budgets = TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 1_000,
        model_idle_ms: 1_500,
        ..TimeBudgets::default()
    };
    let (db, root, run_id) = fixture_with_budgets_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (0, 1), true, budgets);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    coordinator.dispatch_batch_with_worker("worker-batch", "agv-stall", &Allow, &real_worker_command(), &config, "local-test-only").unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "retry_scheduled", "idle timeout must schedule a retry, not fail the run");
    assert_eq!(snapshot.tool_calls, tools, "completed tool results are kept; nothing is replayed");

    // The single turn-budget retry resumes with the SAME frozen configuration
    // and completes the run without re-executing any tool.
    drop(coordinator);
    drop(db);
    clock.advance(2_000);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.tick().unwrap();
    coordinator.dispatch_batch_with_worker("worker-batch", "agv-recovered", &Allow, &real_worker_command(), &config, "local-test-only").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
    server.join().unwrap();
}

#[test]
fn settled_incomplete_http_reply_continues_after_reopen_with_completed_tool_results() {
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","content":"I have the workbook with 5 sheets. Let me analyze the data comprehensively."}),
        json!({"role":"assistant","content":"现有工具结果已核对。\n<fox-final/>"}),
    ]);
    let mut config = worker_configuration();
    config.system_prompt = completion_contract_for_test();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test","baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    coordinator.dispatch_batch_with_worker("worker-batch", "first-http", &Allow, &real_worker_command(), &config, "local-test-only").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    drop(coordinator);
    drop(db);
    clock.advance(1000);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.tick().unwrap();
    coordinator.dispatch_batch_with_worker("worker-batch", "second-http", &Allow, &real_worker_command(), &config, "local-test-only").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
    assert!(db.kernel_reconciliation_view(&coordinator.binding.conversation_id, &run_id).unwrap().items.is_empty());
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(!requests[0]["messages"].to_string().contains("Fox 续答提示"));
    assert!(requests[1]["messages"].to_string().contains("Fox 续答提示"));
    assert!(requests.iter().all(|request| request["messages"].as_array().unwrap().iter().any(|message| message["role"] == "tool")));
}

#[test]
fn settled_incomplete_http_recovery_after_new_compute_error_keeps_prior_progress() {
    let invalid_code = "const rows = attachments[0].sheets[0].rows;\nreturn { 含\"为\"字样异常条数: rows.length - 1 };";
    let valid_code = "const count = attachments[0].sheets[0].rows.length - 1;\nsaveFile('summary.json', {count});\nreturn {count};";
    let tool_reply = |id: &str, code: &str| json!({"role":"assistant", "tool_calls":[{
        "index":0,"id":id,"type":"function","function":{"name":"attachment_compute",
        "arguments":json!({"attachmentIds":["a"],"code":code}).to_string()}}]});
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","content":"Let me analyze the data comprehensively."}),
        tool_reply("compute-invalid", invalid_code),
        json!({"role":"assistant","content":"Let me redo properly with correct indices."}),
        tool_reply("compute-corrected", valid_code),
        json!({"role":"assistant","content":"已修正脚本，核对 2 条记录并生成 summary.json。\n<fox-final/>"}),
    ]);
    let mut config = worker_configuration();
    config.system_prompt = completion_contract_for_test();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test","baseUrl":format!("http://{address}/v1")});
    config.proposal_tools.push(json!({"name":"attachment_compute","description":"Compute explicitly selected attachment data with JavaScript",
        "parameters":{"type":"object","properties":{"code":{"type":"string"},"attachmentIds":{"type":"array","items":{"type":"string"}}},"required":["code"]}}));
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let prior_read = coordinator.snapshot().unwrap().tool_calls[0].clone();
    let worker = real_worker_command();
    coordinator.dispatch_batch_with_worker("worker-batch", "preface", &Allow, &worker, &config, "local-test-only").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    clock.advance(1000);
    coordinator.dispatch_batch_with_worker("worker-batch", "after-preface", &Allow, &worker, &config, "local-test-only").unwrap();
    let batch = coordinator.snapshot().unwrap().tool_calls.iter().find(|tool| tool.tool_call_id == "compute-invalid").unwrap().batch_id.clone();
    let source = root.join("data.csv");
    std::fs::write(&source, "id,value\nA,1\nB,2\n").unwrap();
    let inputs = vec![("a".to_owned(), source.clone())];
    coordinator.dispatch_tool("compute-invalid", "compute-error", |_, effect, _| {
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        let error = crate::data_compute::execute(&inputs, &root.join("failed-outputs"), &payload["input"], || false).unwrap_err();
        assert!(error.contains("code line 2 (1-based"), "{error}");
        Ok((false, json!({"code":"kernel.resource_failed","message":error,"tool":"attachment_compute"})))
    }).unwrap();
    let after_error = coordinator.snapshot().unwrap().tool_calls;
    assert!(after_error.iter().any(|tool| tool == &prior_read));
    coordinator.dispatch_batch_with_worker(&batch, "repair-preface", &Allow, &worker, &config, "local-test-only").unwrap();
    // A previous completed request spent its retry. The new request still gets
    // its one repair opportunity; restarting cannot reset that allowance.
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    assert_eq!(coordinator.snapshot().unwrap().retry.completion_attempts, 1);
    assert_eq!(coordinator.snapshot().unwrap().retry.turn_attempts, 0);
    drop(coordinator);
    drop(db);
    clock.advance(1000);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_batch_with_worker(&batch, "repair-after-reopen", &Allow, &worker, &config, "local-test-only").unwrap();
    let next_batch = coordinator.snapshot().unwrap().tool_calls.iter().find(|tool| tool.tool_call_id == "compute-corrected").unwrap().batch_id.clone();
    coordinator.dispatch_tool("compute-corrected", "compute-fixed", |_, effect, _| {
        let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
        let result = crate::data_compute::execute(&inputs, &root.join("corrected-outputs"), &payload["input"], || false)?;
        assert_eq!(result["result"]["count"], 2);
        Ok((true, result))
    }).unwrap();
    coordinator.dispatch_batch_with_worker(&next_batch, "final", &Allow, &worker, &config, "local-test-only").unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state, "completed");
    for original in after_error { assert!(snapshot.tool_calls.contains(&original)); }
    assert!(!root.join("failed-outputs").exists());
    let saved: Value = serde_json::from_slice(&std::fs::read(root.join("corrected-outputs/summary.json")).unwrap()).unwrap();
    assert_eq!(saved, json!({"count":2}));
    assert_eq!(std::fs::read_to_string(source).unwrap(), "id,value\nA,1\nB,2\n");
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 5);
    for index in [2, 3, 4] {
        let messages = requests[index]["messages"].as_array().unwrap();
        assert!(messages.iter().any(|message| message["role"] == "tool" && message.to_string().contains("Syntax error")));
    }
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let replayed: i64 = connection.query_row("SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='dispatch_tool' AND attempts<>1", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(replayed, 0);
    assert!(db.kernel_reconciliation_view(&coordinator.binding.conversation_id, &run_id).unwrap().items.is_empty());
}

#[test]
fn settled_incomplete_answer_uses_one_request_retry_across_reopen_without_reexecuting_tools() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, "test-prompt", None, false, false, (0, 1));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    let reject = |binding: &RunControlBinding, frame: &fox_engine_protocol::KernelBatchResumeFrame, _: &kernel::CancellationToken| {
        Err(format!("kernel.settled_model_failure:{}", json!({
            "schemaVersion": 1, "runId": binding.run_id, "turnId": frame.turn_id,
            "checkpointSeq": frame.checkpoint_seq, "category": "incomplete_response",
            "httpStatus": null, "retryAfterMs": null,
        })))
    };
    coordinator.dispatch_batch("worker-batch", "first", &Allow, reject).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
    assert!(coordinator.dispatch_batch("worker-batch", "early", &Allow, |_, _, _| panic!("retry is not due")).is_err());
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().retry.completion_attempts, 1);
    clock.advance(1000);
    coordinator.dispatch_batch("worker-batch", "second", &Allow, |binding, frame, token| {
        assert!(frame.history.last().unwrap()["content"][0]["text"].as_str().unwrap().contains("Fox 续答提示"));
        reject(binding, frame, token)
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
    assert!(coordinator.dispatch_batch("worker-batch", "third", &Allow, |_, _, _| panic!("only one retry is allowed")).is_err());
    assert_eq!(coordinator.snapshot().unwrap().state, "failed");
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let (status, owner): (String, Option<String>) = connection.query_row("SELECT status,lease_owner FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='deliver-batch:worker-batch'", [&run_id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(status, "completed");
    assert_eq!(owner, None);
    let code: String = connection.query_row("SELECT json_extract(payload_json,'$.code') FROM kernel_events WHERE run_id=?1 AND event_type='run.failed'", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(code, "kernel.model_incomplete");
    assert!(db.kernel_reconciliation_view(&coordinator.binding.conversation_id, &run_id).unwrap().items.is_empty());
    drop(coordinator);
    let restarted_cancellation = CancellationRegistry::default();
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &restarted_cancellation);
    // All terminal facts and the settled model lease survive a restart.
    assert!(reopened.is_ok());
}

#[test]
fn settled_initial_retry_survives_reopen_and_honors_server_delay() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), true, true, (1, 0));
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("first", &Allow, |binding, frame, _| {
        Err(settled_provider_rejection(&binding.run_id, &frame.input.turn_id, frame.checkpoint_seq))
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    assert!(coordinator.dispatch_initial("early", &Allow, |_, _, _| panic!("cannot retry before due time")).is_err());
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    clock.advance(1999);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "retry_scheduled");
    clock.advance(1);
    coordinator.dispatch_initial("second", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"retry complete"}]}),
    })).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let attempts: i64 = connection.query_row("SELECT attempts FROM kernel_effect_outbox WHERE run_id=?1 AND effect_key='initial-model'", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(attempts, 2);
}

#[test]
fn settled_batch_retry_never_reexecutes_completed_resources() {
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture_with_retry_opt(&clock, "test-prompt", None, false, false, (1, 0));
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    settle_worker_batch(&coordinator);
    let tools = coordinator.snapshot().unwrap().tool_calls;
    coordinator.dispatch_batch("worker-batch", "first", &Allow, |binding, frame, _| {
        Err(settled_provider_rejection(&binding.run_id, &frame.turn_id, frame.checkpoint_seq))
    }).unwrap();
    clock.advance(2000);
    coordinator.dispatch_batch("worker-batch", "second", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.turn_id.clone(), batch_id: frame.batch_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"done"}]}),
    })).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, tools);
}

#[test]
fn settled_model_retry_cancellation_and_frozen_budget_block_more_dispatch() {
    for cancel in [false, true] {
        let config = worker_configuration();
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db, _, run_id) = fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), true, true, (1, 0));
        let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator.dispatch_initial("first", &Allow, |binding, frame, _| {
            Err(settled_provider_rejection(&binding.run_id, &frame.input.turn_id, frame.checkpoint_seq))
        }).unwrap();
        if cancel {
            coordinator.cancel().unwrap();
            coordinator.settle_cancellation().unwrap();
            clock.advance(2000);
            assert!(coordinator.dispatch_initial("cancelled", &Allow, |_, _, _| panic!("cancelled retry must not dispatch")).is_err());
            assert_eq!(coordinator.snapshot().unwrap().state, "cancelled");
        } else {
            clock.advance(2000);
            coordinator.dispatch_initial("last", &Allow, |binding, frame, _| {
                Err(settled_provider_rejection(&binding.run_id, &frame.input.turn_id, frame.checkpoint_seq))
            }).unwrap();
            assert_eq!(coordinator.snapshot().unwrap().state, "failed");
            assert!(coordinator.dispatch_initial("extra", &Allow, |_, _, _| panic!("retry budget cannot be reset")).is_err());
        }
    }
}

#[test]
fn initial_model_lost_response_stays_uncertain_after_reopen() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator.dispatch_initial("lost", &Allow, |_, _, _| Err("connection lost".into())).is_err());
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator.dispatch_initial("retry", &Allow, |_, _, _| panic!("must not replay uncertain initial request")).is_err());
    assert_eq!(coordinator.snapshot().unwrap().pending_effects.iter().find(|e| e.kind == kernel::OutboxEffectKind::InitialModel).unwrap().status, kernel::OutboxStatus::Leased);
}

#[test]
fn initial_model_proposal_enters_the_existing_approval_and_batch_pipeline() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("initial", &Ask, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id:binding.run_id.clone(), turn_id:frame.input.turn_id.clone(), checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"first-read","name":"read","arguments":{"path":"proof.txt"}}]}),
    })).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "waiting_approval");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls.len(), 1);
    assert!(coordinator.prepare_stored_batch_resume(&format!("initial-batch:{run_id}:2")).is_err());
}

#[test]
fn kernel_host_scope_and_commands_survive_reopen_without_double_decisions() {
    let config = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let scope = crate::database::KernelHostScope {
        schema_version: 1, tool_names: ["read".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(), knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks:Vec::new(),
    };
    let mut bad = scope.clone(); bad.tool_names.clear();
    assert!(db.freeze_kernel_host_scope(&run_id, &bad).is_err());
    db.freeze_kernel_host_scope(&run_id, &scope).unwrap();
    db.freeze_kernel_host_scope(&run_id, &scope).unwrap();
    assert_eq!(db.kernel_host_scope(&run_id).unwrap(), scope);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("initial", &Ask, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"text","text":"只投影可见文字"},
            {"type":"thinking","thinking":"private reasoning must never become chat text"},
            {"type":"toolCall","id":"first","name":"read","arguments":{"path":"proof.txt"}},
            {"type":"toolCall","id":"second","name":"read","arguments":{"path":"proof.txt"}}]}),
    })).unwrap();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let projected: (i64,String) = connection.query_row("SELECT COUNT(*),MAX(content) FROM messages WHERE run_id=?1 AND role='assistant'",
        [&run_id], |row| Ok((row.get(0)?,row.get(1)?))).unwrap();
    assert_eq!(projected, (1,"只投影可见文字".into()));
    let approvals: i64 = connection.query_row("SELECT COUNT(*) FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
        WHERE t.run_id=?1 AND a.status='pending' AND t.status='pending'", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(approvals, 2);
    db.repair_interrupted_runs().unwrap();
    assert!(!db.apply_runtime_event(&run_id, 99_999, &json!({"type":"run.failed","code":"legacy","message":"must not overwrite"})).unwrap());
    assert!(db.mark_run_failed(&run_id, "legacy", "must not overwrite").is_err());
    assert!(db.mark_run_interrupted(&run_id, "legacy", "must not overwrite").is_err());
    let still_pending: i64 = connection.query_row("SELECT COUNT(*) FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
        WHERE t.run_id=?1 AND a.status='pending' AND t.status='pending'", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(still_pending, 2);
    assert!(db.queue_kernel_host_command(&run_id, Some(("first", "allow_once"))).unwrap());
    assert!(!db.queue_kernel_host_command(&run_id, Some(("first", "allow_once"))).unwrap());
    assert!(db.queue_kernel_host_command(&run_id, Some(("first", "denied"))).is_err());
    assert!(db.queue_kernel_host_command(&run_id, Some(("missing", "allow_once"))).is_err());
    let command = db.pending_kernel_host_commands(&run_id).unwrap().remove(0);
    assert!(db.complete_kernel_host_command(&run_id, command.seq).is_err());
    connection.execute_batch("CREATE TRIGGER reject_kernel_projection BEFORE UPDATE ON approvals
        BEGIN SELECT RAISE(ABORT,'projection rollback test'); END;").unwrap();
    assert!(coordinator.resolve_approval("first", kernel::ApprovalDecision::AllowOnce).is_err());
    let approval: String = connection.query_row("SELECT state FROM kernel_approvals WHERE run_id=?1 AND tool_call_id='first'",
        [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(approval, "pending");
    connection.execute_batch("DROP TRIGGER reject_kernel_projection").unwrap();
    coordinator.resolve_approval("first", kernel::ApprovalDecision::AllowOnce).unwrap();
    // Crash window: the domain decision persisted, but queue acknowledgement did not.
    drop(coordinator); drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_host_scope(&run_id).unwrap(), scope);
    assert_eq!(db.pending_kernel_host_commands(&run_id).unwrap(), vec![command.clone()]);
    db.complete_kernel_host_command(&run_id, command.seq).unwrap();
    assert!(db.pending_kernel_host_commands(&run_id).unwrap().is_empty());
    assert_eq!(db.kernel_host_recoverable_runs().unwrap(), vec![run_id.clone()]);
    assert!(db.queue_kernel_host_command(&run_id, None).unwrap());
    assert!(!db.queue_kernel_host_command(&run_id, None).unwrap());
    assert!(db.queue_kernel_host_command(&run_id, Some(("second", "allow_once"))).is_err());
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.cancel().unwrap();
    coordinator.settle_cancellation().unwrap();
    let cancel = db.pending_kernel_host_commands(&run_id).unwrap().remove(0);
    db.complete_kernel_host_command(&run_id, cancel.seq).unwrap();
    assert!(db.kernel_host_recoverable_runs().unwrap().is_empty());
    let projected_status: String = connection.query_row("SELECT status FROM runs WHERE id=?1", [&run_id], |row| row.get(0)).unwrap();
    assert_eq!(projected_status, "cancelled");
    assert_eq!(db.kernel_host_scope(&run_id).unwrap(), scope);
    bad = scope.clone(); bad.office_tools.insert("new-live-tool".into());
    assert!(db.freeze_kernel_host_scope(&run_id, &bad).is_err());
}

#[test]
fn initial_model_dispatch_rollback_does_not_call_engine_or_arm_deadline() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute_batch("CREATE TRIGGER reject_initial_dispatch BEFORE INSERT ON kernel_events WHEN NEW.event_type='engine.initial_dispatched' BEGIN SELECT RAISE(ABORT,'injected initial dispatch failure'); END;").unwrap();
    assert!(coordinator.dispatch_initial("blocked", &Allow, |_, _, _| panic!("rolled-back dispatch must not call engine")).is_err());
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms, None);
    let pending = coordinator.snapshot().unwrap().pending_effects.into_iter().find(|e| e.kind == kernel::OutboxEffectKind::InitialModel).unwrap();
    assert_eq!(pending.status, kernel::OutboxStatus::Pending);
    connection.execute_batch("DROP TRIGGER reject_initial_dispatch").unwrap();
    coordinator.dispatch_initial_with_worker("allowed", &Allow, &real_worker_command(), "test-key").unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn initial_model_second_coordinator_cannot_claim_an_inflight_delivery() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("first", &Allow, |binding, frame, _| {
        let other_db = Database::open(root.join("facts.db"))?;
        let other_registry = CancellationRegistry::default();
        let other = KernelCoordinator::reopen(&other_db, &clock, &run_id, &other_registry)?;
        assert!(other.dispatch_initial("second", &Allow, |_, _, _| panic!("second owner cannot call engine")).is_err());
        Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
            assistant_message: json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"one delivery"}]}),
        })
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn kernel_initial_response_projects_usage_and_message_atomically() {
    for reject_usage in [false,true] {
        let config = worker_configuration();
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
        let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
        let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        if reject_usage {
            connection.execute_batch("CREATE TRIGGER reject_kernel_usage BEFORE INSERT ON run_events
                WHEN NEW.event_type='usage.updated' BEGIN SELECT RAISE(ABORT,'usage projection failure'); END;").unwrap();
        }
        let result = coordinator.dispatch_initial("usage-model",&Allow,|binding,frame,_| Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"usage and message"}],
                "usage":{"input":10,"output":4,"cacheRead":2,"cacheWrite":0,"totalTokens":16}}),
        }));
        assert_eq!(result.is_err(),reject_usage);
        let counts: (i64,i64) = connection.query_row("SELECT
            (SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'),
            (SELECT COUNT(*) FROM run_events WHERE run_id=?1 AND event_type='usage.updated')",[&run_id],|row|Ok((row.get(0)?,row.get(1)?))).unwrap();
        assert_eq!(counts,if reject_usage {(0,0)} else {(1,1)});
        assert_eq!(coordinator.snapshot().unwrap().state,if reject_usage {"running"} else {"completed"});
    }
}

#[test]
fn kernel_managed_child_quota_blocks_model_before_dispatch() {
    for (total, output) in [(256,0),(0,64),(0,0)] {
        let config = worker_configuration();
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
        let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
        let parent_conversation = db.create_conversation(db.default_agent_id(),None,None,None).unwrap();
        let parent_id = db.create_run(&parent_conversation.id,"quota parent",None).unwrap().run.id;
        let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        conn.execute("INSERT INTO child_run_delegations(id,parent_run_id,child_run_id,child_conversation_id,
            tool_call_id,worker_agent_id,objective,status,max_duration_ms,max_total_tokens,max_output_tokens,
            max_tool_calls,created_at,total_tokens,output_tokens)
            SELECT 'quota-test',?4,r.id,r.conversation_id,'parent-tool',c.agent_id,'test','running',1000,256,64,0,0,?2,?3
            FROM runs r JOIN conversations c ON c.id=r.conversation_id WHERE r.id=?1",
            rusqlite::params![run_id,total,output,parent_id]).unwrap();
        let calls = AtomicUsize::new(0);
        let result = coordinator.dispatch_initial("quota-model",&Allow,|_,_,_| {
            calls.fetch_add(1,Ordering::SeqCst);
            Err("test stops after admission".into())
        });
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst),usize::from(total==0 && output==0));
        if total>0 || output>0 {
            assert!(!coordinator.snapshot().unwrap().pending_effects.iter()
                .any(|effect| effect.kind==kernel::OutboxEffectKind::InitialModel && effect.status==kernel::OutboxStatus::Leased));
        }
    }
}

#[test]
fn kernel_host_action_waits_for_result_commit_and_never_replays_claimed_action() {
    for outcome in ["success","failure","uncertain","cancel"] {
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db,root,run_id) = fixture(&clock);
        let coordinator = KernelCoordinator::reopen(&db,&clock,&run_id,&cancellation).unwrap();
        coordinator.propose_tools("actions",calls(),&Allow).unwrap();
        let body = json!({"kind":"start_child","childRunId":"test-child"}).to_string();
        assert!(db.stage_kernel_host_action(&run_id,"read-a",&body).is_err());
        let result = coordinator.dispatch_tool("read-a","action-owner",|_,_,_| {
            db.stage_kernel_host_action(&run_id,"read-a",&body)?;
            db.stage_kernel_host_action(&run_id,"read-a",&body)?;
            assert!(db.stage_kernel_host_action(&run_id,"read-a","{}").is_err());
            assert!(db.claim_kernel_host_action(&run_id,"read-a")?.is_none());
            if outcome=="uncertain" { return Err("interrupted after action staged".into()); }
            Ok((outcome!="failure",json!({"content":[]})))
        });
        assert_eq!(result.is_err(),outcome=="uncertain");
        if outcome=="cancel" { db.queue_kernel_host_command(&run_id,None).unwrap(); }
        drop(coordinator);
        drop(db);
        let db = Database::open(root.join("facts.db")).unwrap();
        let claimed = db.claim_kernel_host_action(&run_id,"read-a").unwrap();
        assert_eq!(claimed,if outcome=="success" {Some(body)} else {None});
        if outcome=="success" {
            assert!(db.claim_kernel_host_action(&run_id,"read-a").is_err());
            db.complete_kernel_host_action(&run_id,"read-a").unwrap();
            assert!(db.claim_kernel_host_action(&run_id,"read-a").unwrap().is_none());
        } else {
            assert!(db.complete_kernel_host_action(&run_id,"read-a").is_err());
        }
    }
}

#[test]
fn kernel_parent_cancel_waits_for_unstarted_child_cleanup_without_model_io() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    db.freeze_kernel_host_scope(&run_id,&crate::database::KernelHostScope { schema_version:1,
        tool_names:config.proposal_tools.iter().map(|tool|tool["name"].as_str().unwrap().to_owned()).collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
        knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:Vec::new() }).unwrap();
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
    let child_conversation = db.create_conversation(db.default_agent_id(),None,None,None).unwrap();
    let child_id = db.create_run(&child_conversation.id,"unstarted child",None).unwrap().run.id;
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute("UPDATE runs SET parent_run_id=?2,depth=1 WHERE id=?1",rusqlite::params![child_id,run_id]).unwrap();
    conn.execute("INSERT INTO child_run_delegations(id,parent_run_id,child_run_id,child_conversation_id,
        tool_call_id,worker_agent_id,objective,status,max_duration_ms,max_total_tokens,max_output_tokens,max_tool_calls,created_at)
        VALUES('orphan',?1,?2,?3,'orphan-tool','fox-general','test','queued',1000,256,64,1,0)",
        rusqlite::params![run_id,child_id,child_conversation.id]).unwrap();
    assert!(!db.kernel_fail_unstarted_child("foreign-parent",&child_id).unwrap());
    db.queue_kernel_host_command(&run_id,None).unwrap();
    let calls = AtomicUsize::new(0);
    let ownership = super::super::kernel_host::acquire(&root,&run_id).unwrap();
    super::super::kernel_host::drive_with_actions(&ownership,&db,&clock,&cancellation,&run_id,
        &super::super::RuntimeCommand {program:"must-not-start-a-model".into(),script:None},"",&Allow,
        |_,_,_| panic!("cancelling parent cannot execute resources"),|_|Ok(()),|_| {
            calls.fetch_add(1,Ordering::SeqCst);
            assert_eq!(coordinator.snapshot()?.state,"running","parent must not be terminal before child cleanup");
            assert!(db.kernel_fail_unstarted_child(&run_id,&child_id)?);
            assert_eq!(db.child_run(&child_id)?.unwrap().status,"failed");
            assert!(db.active_child_run_ids(&run_id)?.is_empty());
            Ok(())
        },&|_|{}).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst),1);
    assert_eq!(coordinator.snapshot().unwrap().state,"cancelled");
    assert!(!db.kernel_fail_unstarted_child(&run_id,&child_id).unwrap());
    assert!(db.run_control_binding(&child_id).unwrap().is_none());
}

#[test]
fn kernel_work_snapshot_uses_projected_tool_identity_and_frozen_gateway() {
    let mut config = worker_configuration();
    config.proposal_tools = vec![json!({"name":"work_snapshot_get","description":"Read work state",
        "parameters":{"type":"object","properties":{}}})];
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db,_,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let scope = crate::database::KernelHostScope { schema_version:1,tool_names:["work_snapshot_get".into()].into_iter().collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:Vec::new() };
    db.freeze_kernel_host_scope(&run_id,&scope).unwrap();
    let policy = super::super::kernel_gateway::GatewayPolicy {binding:db.run_control_binding(&run_id).unwrap().unwrap(),scope, database: None, sessions_dir: None, artifacts_dir: None };
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
    coordinator.dispatch_initial("work-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"work-1","name":"work_snapshot_get","arguments":{}}]}),
    })).unwrap();
    assert!(coordinator.dispatch_tool("work-1","work-executor",|_,_,token| {
        let result = policy.execute_work(&db,"work-1","work_snapshot_get",&json!({}),token)?;
        assert!(result["content"].is_array());
        assert!(policy.execute_work(&db,"work-1","goal_propose",&json!({}),token).is_err());
        Ok((true,result))
    }).unwrap());
    assert_eq!(coordinator.snapshot().unwrap().tool_calls[0].state,"completed");
    assert!(db.pending_kernel_host_action_ids(&run_id).unwrap().is_empty());
}

#[test]
fn kernel_frozen_hooks_keep_block_and_approval_policy_and_transactional_audit() {
    for action in ["block","require_approval"] {
        let config = worker_configuration();
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
        let hook = db.save_lifecycle_hook("kernel-rule","Frozen guard","before_tool","read",action,"frozen reason",true,10).unwrap();
        let after = db.save_lifecycle_hook("kernel-after","Tool audit","after_tool","read","annotate","tool finished",true,20).unwrap();
        let before_run = db.save_lifecycle_hook("kernel-start","Run audit","before_run","primary","annotate","run started",true,20).unwrap();
        let after_run = db.save_lifecycle_hook("kernel-end","Run audit","after_run","run.*","annotate","run ended",true,20).unwrap();
        let scope = crate::database::KernelHostScope {schema_version:1,tool_names:["read".into()].into_iter().collect(),
            mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
            knowledge_connection_hashes:Default::default(),office_tools:Default::default(),
            lifecycle_hooks:vec![hook,after,before_run,after_run]};
        db.freeze_kernel_host_scope(&run_id,&scope).unwrap();
        db.save_lifecycle_hook("kernel-rule","Changed live rule","before_tool","*","annotate","changed",false,10).unwrap();
        let policy = super::super::kernel_gateway::GatewayPolicy {binding:db.run_control_binding(&run_id).unwrap().unwrap(),scope, database: None, sessions_dir: None, artifacts_dir: None };
        let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
        coordinator.dispatch_initial("hooks-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
                {"type":"toolCall","id":"guarded-read","name":"read","arguments":{"path":"proof.txt"}}]}),
        })).unwrap();
        if action=="block" {
            assert_eq!(coordinator.snapshot().unwrap().tool_calls[0].state,"failed");
            assert!(matches!(policy.decide(&run_id,"ignored","read","{\"path\":\"proof.txt\"}"),PolicyDecision::Deny {..}));
        } else {
            assert_eq!(coordinator.snapshot().unwrap().state,"waiting_approval");
            coordinator.resolve_approval("guarded-read",kernel::ApprovalDecision::AllowOnce).unwrap();
            coordinator.dispatch_tool("guarded-read","hook-resource",|_,_,token| {
                Ok((true,policy.execute(&db,"read",&json!({"path":"proof.txt"}),token)?))
            }).unwrap();
        }
        coordinator.fail("test.finished","Finish audit fixture").unwrap();
        coordinator.tick().unwrap();
        let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM lifecycle_hook_executions WHERE run_id=?1 AND id LIKE 'kernel-hook:%'",[&run_id],|row|row.get(0)).unwrap();
        assert_eq!(count,4,"one before/after audit per tool and run despite repeated commits");
        let frozen: String = conn.query_row("SELECT details_json FROM lifecycle_hook_executions WHERE run_id=?1 AND hook_id='kernel-rule'",[&run_id],|row|row.get(0)).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&frozen).unwrap()["reason"],"frozen reason");
    }
}

#[test]
fn kernel_delegation_stages_a_single_child_without_starting_an_executor() {
    let supported = super::super::kernel_gateway::supported_tools();
    // 71 = the frozen catalogue including the four compute job tools, `read_tool_result` (the reader that
    // keeps a bounded model view's promise that omitted content is reachable)
    // and `skill_load` (on-demand loading of catalog skills without growing
    // the frozen scope).
    assert_eq!(supported.len(),71);
    assert_eq!(supported.iter().collect::<std::collections::BTreeSet<_>>().len(),71);
    assert!(supported.iter().any(|tool| *tool == "read_tool_result"));
    assert!(supported.iter().any(|tool| *tool == "skill_load"));
    let mut config = worker_configuration();
    config.model_service["maxOutputTokens"] = json!(1024);
    config.proposal_tools = ["child_agent_list","child_run_start","child_run_collect","child_run_cancel"].iter()
        .map(|name|json!({"name":name,"description":"Host delegation","parameters":{"type":"object","properties":{}}})).collect();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db,_,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let scope = crate::database::KernelHostScope {schema_version:1,
        tool_names:config.proposal_tools.iter().map(|tool|tool["name"].as_str().unwrap().to_owned()).collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),
        office_tools:Default::default(),lifecycle_hooks:Vec::new()};
    db.freeze_kernel_host_scope(&run_id,&scope).unwrap();
    let policy = super::super::kernel_gateway::GatewayPolicy {binding:db.run_control_binding(&run_id).unwrap().unwrap(),scope, database: None, sessions_dir: None, artifacts_dir: None };
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
    let input = json!({"objective":"One bounded concern","context":"Explicit context only", "budget":{
        "maxDurationMs":1000,"maxTotalTokens":256,"maxOutputTokens":64,"maxToolCalls":0}});
    coordinator.dispatch_initial("child-model",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"invalid-delegate","name":"child_run_start","arguments":{"objctive":"typo must not start a child"}}]}),
    })).unwrap();
    coordinator.dispatch_tool("invalid-delegate","child-owner",|_,_,token| {
        for (tool, bad, field) in [
            ("child_run_start",json!({"objctive":"bounded task"}),"objective"),
            ("child_run_start",json!({"objective":" "}),"objective"),
            ("child_run_start",json!({"objective":"task","context":42}),"context"),
            ("child_run_start",json!({"objective":"task","mode":"invalid"}),"mode"),
            ("child_run_start",json!({"objective":"task","expertId":"expert"}),"expertId"),
            ("child_run_start",json!({"objective":"task","agentId":"unknown"}),"agentId"),
            ("child_run_start",json!({"objective":"task","budget":null}),"budget"),
            ("child_run_start",json!({"objective":"task","budget":{"maxDurationMs":1}}),"budget"),
            ("child_run_start",json!({"objective":"task","budget":{"maxOutputTokens":2048}}),"budget"),
            ("child_run_start",json!({"objective":"task","budget":{"maxTotalTokens":256,"maxOutputTokens":512}}),"budget"),
            ("child_run_collect",json!({"childRunIds":[]}),"childRunIds"),
            ("child_run_collect",json!({"childRunIds":["invented"],"waitMs":-1}),"waitMs"),
            ("child_run_collect",json!({"childRunIds":["invented"]}),"childRunIds"),
            ("child_run_cancel",json!({"childRunId":"invented"}),"childRunId"),
        ] {
            let result = policy.execute_delegation(&db,"invalid-delegate",tool,&bad,token)?;
            assert_eq!(result["isError"],true);
            assert_eq!(result["details"]["error"]["field"],field);
            assert_eq!(result["details"]["executionStarted"],false);
            if bad.get("objctive").is_some() {
                assert_eq!(result["details"]["suggestedArguments"]["objective"],bad["objctive"]);
                assert!(bad.get("objective").is_none(),"the canonical proposal must remain unchanged");
            }
            assert!(db.child_runs_for_parent(&run_id)?.is_empty());
            assert!(db.pending_kernel_host_action_ids(&run_id)?.is_empty());
        }
        let mut denied = super::super::kernel_gateway::GatewayPolicy { binding:policy.binding.clone(),scope:policy.scope.clone() , database: None, sessions_dir: None, artifacts_dir: None };
        denied.scope.tool_names.clear();
        assert!(denied.execute_delegation(&db,"invalid-delegate","child_run_start",&input,token).is_err());
        let result = policy.execute_delegation(&db,"invalid-delegate","child_run_start",&json!({"objctive":"typo"}),token)?;
        Ok((false,result))
    }).unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state,"running");
    assert_eq!(snapshot.tool_calls[0].state,"failed");
    let batch_id = snapshot.tool_calls[0].batch_id.clone();
    coordinator.dispatch_batch(&batch_id,"correct-child-model",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"delegate","name":"child_run_start","arguments":input}]}),
    })).unwrap();
    coordinator.dispatch_tool("delegate","child-owner",|_,_,token| {
        let result = policy.execute_delegation(&db,"delegate","child_run_start",&input,token)?;
        let child_id = result["details"]["childRun"]["childRunId"].as_str().unwrap();
        assert!(db.kernel_host_run_state(child_id)?.is_none());
        assert!(db.run_control_binding(child_id)?.is_none());
        assert!(db.claim_kernel_host_action(&run_id,"delegate")?.is_none());
        let invalid = policy.execute_delegation(&db,"delegate","child_run_collect",&json!({"childRunIds":["foreign-child"]}),token)?;
        assert_eq!(invalid["isError"],true);
        assert_eq!(invalid["details"]["executionStarted"],false);
        Ok((true,result))
    }).unwrap();
    let children = db.child_runs_for_parent(&run_id).unwrap();
    assert_eq!(children.len(),1);
    assert_eq!(children[0].status,"queued");
    assert_eq!(db.pending_kernel_host_action_ids(&run_id).unwrap(),vec!["delegate"]);
    let body = db.claim_kernel_host_action(&run_id,"delegate").unwrap().unwrap();
    let directive: super::super::work_tools::WorkToolPostFinalizeDirective = serde_json::from_str(&body).unwrap();
    let super::super::work_tools::WorkToolPostFinalizeDirective::StartChild(child) = directive else {panic!("expected child action")};
    assert_eq!(child.child_run.child_run_id,children[0].child_run_id);
    assert!(db.claim_kernel_host_action(&run_id,"delegate").is_err());
    assert!(db.kernel_fail_unstarted_child(&run_id,&child.child_run.child_run_id).unwrap());
    assert!(db.active_child_run_ids(&run_id).unwrap().is_empty());
}

#[test]
fn kernel_delegation_correction_limit_survives_reopen_and_staging_errors_stay_fatal() {
    for staging_failure in [false, true] {
        let mut config = worker_configuration();
        config.model_service["maxOutputTokens"] = json!(1024);
        config.proposal_tools = vec![json!({"name":"child_run_start","description":"Host delegation","parameters":{"type":"object","properties":{}}})];
        let clock = TestClock::new(1_000);
        let cancellation = CancellationRegistry::default();
        let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
        let scope = crate::database::KernelHostScope {schema_version:1,
            tool_names:["child_run_start".into()].into_iter().collect(),
            mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),
            office_tools:Default::default(),lifecycle_hooks:Vec::new()};
        db.freeze_kernel_host_scope(&run_id,&scope).unwrap();
        let policy = super::super::kernel_gateway::GatewayPolicy {binding:db.run_control_binding(&run_id).unwrap().unwrap(),scope, database: None, sessions_dir: None, artifacts_dir: None };
        let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
        let args = if staging_failure {json!({"objective":"bounded concern"})} else {json!({"objctive":"typo"})};
        let count = if staging_failure {1} else {4};
        coordinator.dispatch_initial("child-model",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":(0..count).map(|i|
                json!({"type":"toolCall","id":format!("child-{i}"),"name":"child_run_start","arguments":args})).collect::<Vec<_>>()}),
        })).unwrap();
        if staging_failure {
            let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
            conn.execute_batch("CREATE TRIGGER reject_child_action BEFORE INSERT ON kernel_host_actions BEGIN SELECT RAISE(ABORT,'injected staging failure'); END;").unwrap();
            let result = coordinator.dispatch_tool("child-0","child-owner",|_,_,token| {
                policy.execute_delegation(&db,"child-0","child_run_start",&args,token).map(|result|(true,result))
            });
            assert!(result.is_err(),"failure after child creation must not become a correctable parameter result");
            assert_eq!(db.child_runs_for_parent(&run_id).unwrap().len(),1);
            assert!(db.pending_kernel_host_action_ids(&run_id).unwrap().is_empty());
        } else {
            for i in 0..3 {
                let id = format!("child-{i}");
                coordinator.dispatch_tool(&id,"child-owner",|_,_,token| {
                    let result = policy.execute_delegation(&db,&id,"child_run_start",&args,token)?;
                    assert_eq!(result["details"]["error"]["code"],"child.invalid_arguments");
                    Ok((false,result))
                }).unwrap();
            }
            drop(coordinator);
            drop(db);
            let db = Database::open(root.join("facts.db")).unwrap();
            let reopened = KernelCoordinator::reopen(&db,&clock,&run_id,&cancellation).unwrap();
            let result = reopened.dispatch_tool("child-3","child-owner",|_,_,token| {
                policy.execute_delegation(&db,"child-3","child_run_start",&args,token).map(|result|(false,result))
            });
            assert!(result.unwrap());
            assert!(super::super::kernel_delegation::correction_limit_reached(&reopened.snapshot().unwrap()));
            drop(reopened);
            drop(db);
            // A settled limit result survives another reopen and stops the Host
            // before any model continuation, with a specific persisted UI error.
            let db = Database::open(root.join("facts.db")).unwrap();
            super::super::kernel_host::drive(super::super::kernel_host::acquire(&root,&run_id).unwrap(),
                &db,&clock,&cancellation,&run_id,&real_worker_command(),"test-key",&Allow,
                |_,_,_|panic!("exhausted correction must not execute another resource")).unwrap();
            assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state,"failed");
            let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
            let error: String = conn.query_row("SELECT error_code FROM runs WHERE id=?1",[&run_id],|row|row.get(0)).unwrap();
            assert_eq!(error,"kernel.child_arguments_exhausted");
            assert!(db.child_runs_for_parent(&run_id).unwrap().is_empty());
            assert!(db.pending_kernel_host_action_ids(&run_id).unwrap().is_empty());
        }
    }
}

#[test]
fn kernel_context_resources_preserve_conversation_scope_and_use_kernel_results() {
    let mut config = worker_configuration();
    config.proposal_tools = ["memory_search","memory_propose","read_attachment"].iter()
        .map(|name|json!({"name":name,"description":"Host context resource","parameters":{"type":"object","properties":{}}})).collect();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let scope = crate::database::KernelHostScope {schema_version:1,
        tool_names:config.proposal_tools.iter().map(|tool|tool["name"].as_str().unwrap().to_owned()).collect(),
        mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),
        office_tools:Default::default(),lifecycle_hooks:Vec::new()};
    db.freeze_kernel_host_scope(&run_id,&scope).unwrap();
    let policy = super::super::kernel_gateway::GatewayPolicy {binding:db.run_control_binding(&run_id).unwrap().unwrap(),scope, database: None, sessions_dir: None, artifacts_dir: None };
    let foreign = db.create_conversation(db.default_agent_id(),None,None,None).unwrap();
    for (id,conversation_id) in [("own-attachment",policy.binding.conversation_id.as_str()),("foreign-attachment",foreign.id.as_str())] {
        db.add_attachments(&[crate::database::AttachmentRecord {id:id.into(),conversation_id:conversation_id.into(),message_id:None,
            display_name:"proof.txt".into(),storage_path:root.join("proof.txt").to_string_lossy().into_owned(),media_type:Some("text/plain".into()),
            byte_size:0,sha256:None,status:"ready".into(),created_at:0}]).unwrap();
    }
    let inputs = [("memory_propose",json!({"kind":"preference","canonicalKey":"style","content":"Prefer concise answers",
        "evidenceExcerpt":"Test explicitly requests concise answers"})),("memory_search",json!({"query":"concise"})),
        ("read_attachment",json!({"attachmentId":"own-attachment"}))];
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
    coordinator.dispatch_initial("context-model",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":inputs.iter().map(|(tool,input)|
            json!({"type":"toolCall","id":tool,"name":tool,"arguments":input})).collect::<Vec<_>>()}),
    })).unwrap();
    for (tool,input) in inputs {
        coordinator.dispatch_tool(tool,"context-owner",|_,_,token| {
            let result = policy.execute_context_resource(&db,&root,&root,&root,tool,&input,token)?;
            if tool=="read_attachment" {
                let page: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
                assert_eq!(page["text"],"durable coordinator 中文 😀");
                assert_eq!(page["hasMore"],false);
                assert!(result["details"].get("text").is_none(), "the raw text must not be duplicated in UI metadata");
                assert!(policy.execute_context_resource(&db,&root,&root,&root,tool,&json!({"attachmentId":"foreign-attachment"}),token).is_err());
            }
            Ok((true,result))
        }).unwrap();
    }
    assert!(coordinator.snapshot().unwrap().tool_calls.iter().all(|tool|tool.state=="completed"));
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM tool_calls WHERE run_id=?1",[run_id],|row|row.get(0)).unwrap();
    assert_eq!(count,3,"only Kernel-projected tools, no parallel Legacy tools");
}

#[test]
fn kernel_real_model_previews_are_transient_until_the_response_transaction_commits() {
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db,root,run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let previews = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed = previews.clone();
    let path = root.join("facts.db");
    let sink = move |notice: &fox_engine_protocol::KernelModelPreview| {
        let conn = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",[&notice.run_id],|row|row.get(0)).unwrap();
        assert_eq!(count,0,"preview must not be written as a completed message");
        observed.lock().unwrap().push(notice.clone());
    };
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap().with_preview(&sink);
    coordinator.dispatch_initial_with_worker("preview-model",&Allow,&real_worker_command(),"").unwrap();
    let notices = previews.lock().unwrap();
    assert!(!notices.is_empty());
    assert_eq!(notices.last().unwrap().text,"Host consumed the durable batch");
    assert!(notices.iter().all(|notice|notice.run_id==run_id && notice.checkpoint_seq==2));
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",[&run_id],|row|row.get(0)).unwrap();
    assert_eq!(count,1);
}

// ---------------------------------------------------------------------------
// Live native-loop integration (2026-09 loop review): executable-preface stop
// review, read-only immediate completion, continuation final-answer projection.
// ---------------------------------------------------------------------------

#[test]
fn live_executable_preface_gets_one_bounded_review_then_projects_final_answer() {
    assert_live_continuation_projection(false);
}

#[test]
fn live_two_reviews_preserve_streamed_message_identity_after_tool_results() {
    assert_live_continuation_projection(true);
}

fn assert_live_continuation_projection(after_tool: bool) {
    let mut replies = Vec::new();
    if after_tool {
        replies.push(json!({"role":"assistant","tool_calls":[{"index":0,"id":"live-read","type":"function",
            "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]}));
        // R8: after this tool result the model delivers a complete answer. Tool
        // use alone is not evidence of unfinished work, so the run must NOT be
        // charged another model round; the answer below is the final round.
        replies.push(json!({"role":"assistant","content":"统计完成，共 5 条记录。","reasoning_content":"Checked the durable results."}));
    } else {
        // A first round that only promises work still gets its one bounded review.
        replies.push(json!({"role":"assistant","content":"I will now analyze the workbook."}));
        replies.push(json!({"role":"assistant","content":"统计完成，共 5 条记录。","reasoning_content":"Checked the durable results."}));
    }
    let expected_rounds = replies.len();
    let (address, server) = start_http_model_fixture(replies);
    let mut config = worker_configuration();
    config.system_prompt = "Use the durable history only.".into();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test","baseUrl":format!("http://{address}/v1")});
    if !after_tool {
        config.proposal_tools = vec![json!({"name":"attachment_compute","description":"Compute selected attachment data with JavaScript",
            "parameters":{"type":"object","properties":{"code":{"type":"string"},"attachmentIds":{"type":"array","items":{"type":"string"}}},"required":["code"]}})];
    }
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1));
    db.freeze_kernel_host_scope(&run_id, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: config.proposal_tools.iter().map(|tool|tool["name"].as_str().unwrap().into()).collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: Vec::new(),
    })
    .unwrap();
    let previews = std::sync::Arc::new(Mutex::new(Vec::new()));
    let observed = previews.clone();
    let display_db = Database::open(root.join("facts.db")).unwrap();
    let sink = move |notice: &fox_engine_protocol::KernelModelPreview| {
        assert!(display_db.save_kernel_model_display(notice).unwrap(), "live preview must own its durable cursor");
        observed.lock().unwrap().push(notice.clone());
    };
    let executions = AtomicUsize::new(0);
    super::super::kernel_host::drive_with_actions(
        &super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(), "local-test-only",
        &Allow,
        |_, _, _| {
            assert!(after_tool, "a preface cannot execute anything");
            executions.fetch_add(1, Ordering::SeqCst);
            Ok((true, json!({"content":[{"type":"text","text":"5 records"}]})))
        }, |_| Ok(()), |_| Ok(()), &sink,
    )
    .unwrap();
    let snapshot = db.kernel_build_full_snapshot(&run_id).unwrap();
    assert_eq!(snapshot.state, "completed");
    assert_eq!(snapshot.tool_calls.len(), usize::from(after_tool));
    assert_eq!(executions.load(Ordering::SeqCst), usize::from(after_tool));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), expected_rounds);
    // R8: the bounded review prompt appears only for the promised-but-unstarted
    // first round. A run that executed a tool and then delivered a complete
    // answer must not carry it.
    assert_eq!(
        requests
            .last()
            .unwrap()["messages"]
            .to_string()
            .contains("Fox 续答检查"),
        !after_tool,
        "the stop-review prompt must match whether a review round was warranted"
    );
    let final_preview = previews.lock().unwrap().last().cloned().expect("streamed final answer");
    let conversation_id = db.run_control_binding(&run_id).unwrap().unwrap().conversation_id;
    drop(sink);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "completed");
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let (count, last): (i64, String) = conn.query_row(
        "SELECT COUNT(*), COALESCE(MAX(content),'') FROM messages WHERE run_id=?1 AND role='assistant' AND status='completed' AND content<>''",
        [&run_id], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    assert_eq!(count, 1, "the continuation final answer replaces/owns the visible answer");
    assert_eq!(last, "统计完成，共 5 条记录。");
    let final_id: String = conn.query_row("SELECT id FROM messages WHERE run_id=?1 AND content=?2 AND status='completed'",
        rusqlite::params![run_id,last], |row| row.get(0)).unwrap();
    assert_eq!(final_id, format!("kernel-message:{run_id}:{}", final_preview.checkpoint_seq));
    assert_eq!(final_preview.conversation_id, conversation_id);
    let streaming: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE run_id=?1 AND status='streaming'",[&run_id],|row|row.get(0)).unwrap();
    assert_eq!(streaming, 0);
    let replaced: i64 = conn.query_row("SELECT COUNT(*) FROM messages WHERE run_id=?1 AND status='superseded'",[&run_id],|row|row.get(0)).unwrap();
    // R8 note: with the review no longer triggered by tool use, the after-tool
    // run reaches its final answer in one round, so nothing was superseded. The
    // preface run still replaces its streamed draft exactly once.
    assert_eq!(replaced, if after_tool { 0 } else { 1 });
    let usage_rows: i64 = conn.query_row("SELECT COUNT(*) FROM run_events WHERE run_id=?1 AND event_type='usage.updated'",[&run_id],|row|row.get(0)).unwrap();
    assert_eq!(usage_rows, expected_rounds as i64, "every model round contributes usage exactly once");
    let reasoning: i64 = conn.query_row("SELECT COUNT(*) FROM run_events WHERE run_id=?1 AND id=?2",
        rusqlite::params![run_id,format!("kernel-display:{run_id}:reasoning:{}",final_preview.checkpoint_seq)],|row|row.get(0)).unwrap();
    assert_eq!(reasoning, 1, "preview and final reasoning update one event");
}

#[test]
fn live_final_acknowledgement_survives_terminal_cancellation() {
    let config = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    freeze_host_scope(&db, &run_id);
    let sink = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap().with_preview(&sink);
    // Assert the transport result directly: the outer drive loop may already
    // see Completed even if a transport error interrupted the final exchange.
    coordinator.dispatch_initial_live("live-final",&Allow,&real_worker_command(),"local-test-only",
        &|_,_,_| panic!("no resources"), &|_|Ok(()), &|_|Ok(())).unwrap();
    assert!(cancellation.run_token(&run_id).unwrap().is_cancelled());
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
}

#[test]
fn live_readonly_complete_answer_completes_without_review_round() {
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","content":"1+1 等于 2。"}),
    ]);
    let mut config = worker_configuration(); // read-only `read` tool only
    config.system_prompt = "Answer directly.".into();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test","baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) =
        fixture_with_retry_opt(&clock, &config.hash().unwrap(), Some(&config), true, true, (0, 1));
    freeze_host_scope(&db, &run_id);
    super::super::kernel_host::drive(
        super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db, &clock, &CancellationRegistry::default(), &run_id, &real_worker_command(), "local-test-only",
        &Allow, |_, _, _| panic!("no execution"),
    )
    .unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "completed");
    assert_eq!(server.join().unwrap().len(), 1, "a complete read-only answer is not re-reviewed");
}

#[test]
fn continuous_host_keeps_finite_transport_windows_after_a_day_and_reopen() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let budgets = TimeBudgets::continuous();
    let (db, root, run_id) = fixture_with_budgets_opt(&clock, "test", None, false, false, (0,0), true, budgets.clone());
    {
        let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
        coordinator.apply(None, |controller, _| { controller.settle_model_request(); Ok(vec![]) }).unwrap();
        clock.advance(86_400_000);
        coordinator.tick().unwrap();
        assert_eq!(coordinator.snapshot().unwrap().state, "running");
        assert_eq!(coordinator.live_remaining_ms_for_test(), budgets.model_request_ms);
    }
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator.binding.budgets.run_execution_limited);
    assert_eq!(coordinator.live_remaining_ms_for_test(), budgets.model_request_ms);
    assert_eq!(coordinator.binding.budgets.limit_operation_ms(600_000, 86_400_000), 600_000);
}

#[test]
fn live_remaining_budget_excludes_approval_wait() {
    // A human approval legitimately parks the Run for longer than the
    // remaining execution budget. The running clock is suspended, so the live
    // transport deadline (recomputed from durable facts) must not expire while
    // waiting, and an independent approval deadline still governs the wait.
    let config = worker_configuration();
    let clock = TestClock::new(1_000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), false, false);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.propose_tools("batch", calls(), &Ask).unwrap();
    let facts = coordinator.snapshot().unwrap();
    assert_eq!(facts.state, "waiting_approval");
    let approval_deadline = facts.approval_deadline_wall_ms;
    assert!(approval_deadline.is_some());

    // Advance well beyond the model-request window and a chunk of run time;
    // while parked in approval the accumulated running time must not grow.
    let before = coordinator.live_remaining_ms_for_test();
    clock.advance(40_000);
    coordinator.tick().unwrap(); // approval wait is governed by its own deadline
    let parked = coordinator.snapshot().unwrap();
    assert_eq!(parked.state, "waiting_approval", "approval wait must not be charged to execution");
    assert_eq!(parked.running_elapsed_ms, 0, "running clock stays suspended during approval");
    let after = coordinator.live_remaining_ms_for_test();
    assert_eq!(before, after, "live execution deadline does not shrink across an approval wait");

    // Resolving the last pending approval resumes the run; the remaining
    // execution budget is still intact after the long wall wait.
    coordinator.resolve_approval("read-a", kernel::ApprovalDecision::AllowOnce).unwrap();
    coordinator.resolve_approval("read-b", kernel::ApprovalDecision::AllowOnce).unwrap();
    assert!(coordinator.live_remaining_ms_for_test() > 0);
}

/// #5 acceptance: one conversation approval covers the same exact operation a
/// second time, while a different path, a different tool, a frozen read-only
/// mode and a revoked grant all still ask (or refuse).
#[test]
fn approval_scope_is_reused_once_and_out_of_scope_still_asks() {
    use fox_engine_protocol::{ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding};
    use rusqlite::params;

    let root = std::env::temp_dir().join(format!("fox-grant-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("ask"))
        .unwrap();
    let run_id = db.create_run(&conversation.id, "test", None).unwrap().run.id;
    let permission = FrozenPermission {
        mode: PermissionMode::Ask,
        project_root: Some(root.to_string_lossy().into_owned()),
        grants: vec![],
    };
    let mut model = worker_configuration();
    // The frozen host scope must agree with the model tool catalog.
    model.proposal_tools = vec![
        json!({"name":"write_file","description":"Propose a Host-owned write",
            "parameters":{"type":"object","properties":{
                "path":{"type":"string"},"content":{"type":"string"}},
                "required":["path"]}}),
        json!({"name":"edit_file","description":"Propose a Host-owned edit",
            "parameters":{"type":"object","properties":{
                "path":{"type":"string"},"oldText":{"type":"string"},
                "newText":{"type":"string"}},
                "required":["path"]}}),
    ];
    let prompt_hash = model.hash().unwrap();
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id.clone(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets: TimeBudgets::default(),
    };
    db.freeze_run_control(&binding).unwrap();
    let config = kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "coordinator-test-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: prompt_hash.clone(),
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
    db.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        &prompt_hash,
        &serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
    // The frozen model config must agree with the run's frozen engine, and it
    // must exist before the host scope is frozen.
    db.freeze_kernel_model_config(&run_id, &model).unwrap();
    // The host scope row references the frozen initial input, so both frozen
    // facts must exist before the scope can be frozen (production freezes them
    // in this order too).
    db.freeze_kernel_initial_input(&initial_input(&run_id, &prompt_hash))
        .unwrap();
    let scope = crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["write_file".into(), "edit_file".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: Vec::new(),
    };
    db.freeze_kernel_host_scope(&run_id, &scope).unwrap();

    // The decided approval the grant is derived from, exactly as production
    // records it before the tool dispatches.
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_tool_batches(batch_id, run_id, ordered_tool_call_ids_json, barrier_emitted, created_at)
             VALUES ('gb', ?1, '[\"g-write-1\"]', 0, 0)",
            params![run_id],
        )?;
        Ok(())
    })
    .unwrap();
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_tool_calls
             (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json,
              state, result_json, created_at, settled_at, dispatch_idempotency_key)
             VALUES (?1, 'g-write-1', 'gb', 'write_file', 0, ?2, 'running', NULL, 0, 0, NULL)",
            params![run_id, json!({"path": root.join("a.txt").to_string_lossy()}).to_string()],
        )?;
        connection.execute(
            "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at, decided_at)
             VALUES (?1, 'g-write-1', 'allow_conversation', 0, 0)",
            params![run_id],
        )?;
        Ok(())
    })
    .unwrap();

    let asks = |policy: &super::super::kernel_gateway::GatewayPolicy, path: &std::path::Path| {
        let input = json!({"path": path.to_string_lossy(), "content": "x"}).to_string();
        policy.decide(&run_id, "perm", "write_file", &input)
    };
    let policy = || super::super::kernel_gateway::GatewayPolicy {
        binding: db.run_control_binding(&run_id).unwrap().unwrap(),
        scope: db.kernel_host_scope(&run_id).unwrap(),
        database: Some(db.clone()),
        sessions_dir: None, artifacts_dir: None,
    };

    let target = root.join("a.txt");
    let other = root.join("other.txt");
    // Nothing is authorized yet: the first real write asks.
    assert!(matches!(asks(&policy(), &target), PolicyDecision::RequireApproval));
    // The human answers "for this conversation"; the Host records the scope.
    let registered = db
        .kernel_register_authorization_grant(
            &run_id,
            "g-write-1",
            "allow_conversation",
            "write_file",
            super::super::shadow_reconcile::tool_operation_scope(
                "write_file",
                &json!({"path": target.to_string_lossy(), "content": "x"}),
                Some(root.to_str().unwrap()),
            )
            .as_deref(),
        )
        .unwrap();
    assert!(matches!(registered, crate::database::GrantRegistration::Registered { .. }));
    // A second write of the SAME operation is covered: no second question.
    assert_eq!(asks(&policy(), &target), PolicyDecision::Allow);
    // A different path is a different operation and still asks.
    assert!(matches!(asks(&policy(), &other), PolicyDecision::RequireApproval));
    // The reuse is auditable, not silent.
    let uses: i64 = db
        .with_connection(|connection| {
            connection.query_row(
                "SELECT use_count FROM kernel_authorization_grants WHERE run_id=?1",
                params![run_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert!(uses >= 1, "grant reuse must be recorded, got {uses}");
    // Withdrawing permission takes effect immediately.
    db.kernel_revoke_authorization_grants(&conversation.id, "user_withdrew_permission")
        .unwrap();
    assert!(matches!(asks(&policy(), &target), PolicyDecision::RequireApproval));
    // The frozen snapshot itself was never rewritten by any of this.
    assert!(db
        .run_control_binding(&run_id)
        .unwrap()
        .unwrap()
        .permission
        .grants
        .is_empty());
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

/// #4 acceptance: an unanswered approval must not destroy the work.
///
/// * the run ends as a *continuable* expiry, not a plain failure;
/// * the call that was waiting was never dispatched, so nothing executed;
/// * the expired approval cannot be executed afterwards (a late decision is
///   refused, and the durable row stays non-executable);
/// * the Host banked the pause, and a continuation re-verifies to build a NEW
///   attempt without rewriting the source run.
#[test]
fn expired_approval_is_never_executable_and_the_run_stays_continuable() {
    use fox_engine_protocol::{ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding};
    use rusqlite::params;

    let root = std::env::temp_dir().join(format!("fox-expiry-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("ask"))
        .unwrap();
    let run_id = db.create_run(&conversation.id, "test", None).unwrap().run.id;
    let permission = FrozenPermission {
        mode: PermissionMode::Ask,
        project_root: Some(root.to_string_lossy().into_owned()),
        grants: vec![],
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id.clone(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets: TimeBudgets::default(),
    };
    db.freeze_run_control(&binding).unwrap();
    let config = kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "coordinator-test-manifest".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: "expiry-hash".into(),
        model_request_timeout_ms: 120_000,
        model_first_response_ms: 60_000,
        model_idle_ms: 120_000,
        tool_execution_timeout_ms: 600_000,
        run_execution_budget_ms: 600_000,
        run_execution_limited: true,
        approval_wait_timeout_ms: 1_000,
        provider_max_retries: 2,
        turn_max_retries: 1,
    };
    db.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        "expiry-hash",
        &serde_json::to_string(&config).unwrap(),
    )
    .unwrap();

    let clock = TestClock::new(0);
    let (mut controller, start) =
        RunController::start(&run_id, "turn-1", config, &clock).unwrap();
    db.kernel_commit_decision(&run_id, 0, &controller.persist_command(&start))
        .unwrap();
    // One batch: a write that needs a human and is never answered.
    let proposed = controller
        .propose_tool_batch(
            "batch-expiry",
            vec![kernel::ToolCallRequest {
                tool_call_id: "never-answered".into(),
                tool: "write_file".into(),
                canonical_input_json: json!({
                    "path": root.join("a.txt").to_string_lossy(),
                    "content": "x"
                })
                .to_string(),
                source_order: 0,
            }],
            &Ask,
            0,
            0,
        )
        .unwrap();
    controller.set_approval_deadline(1_000);
    db.kernel_commit_decision(&run_id, 0, &controller.persist_command(&proposed))
        .unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&run_id).unwrap().state, "waiting_approval");

    // The human never answers.
    let expired = controller.tick(1_001, 1_001);
    assert!(expired.iter().any(|effect| matches!(
        effect,
        kernel::Effect::AppendEvent { event_type, .. } if event_type == "run.approval_expired"
    )));
    db.kernel_commit_decision(&run_id, 1_001, &controller.persist_command(&expired))
        .unwrap();
    let snapshot = db.kernel_build_full_snapshot(&run_id).unwrap();
    assert_eq!(
        snapshot.state, "approval_expired",
        "an unanswered approval must not be a plain failure"
    );
    assert_eq!(snapshot.tool_calls.len(), 1);
    assert_eq!(
        snapshot.tool_calls[0].state, "expired",
        "the un-decided call was never dispatched, so it is expired"
    );

    // A late decision cannot execute the dead approval.
    let late = db.kernel_resolve_approval(&run_id, "never-answered", "allow_once");
    assert!(
        matches!(late, Ok(false)) || late.is_err(),
        "a late approval must not be accepted: {late:?}"
    );
    let (approval_state, dispatched): (String, i64) = db
        .with_connection(|connection| {
            let state = connection.query_row(
                "SELECT state FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",
                params![run_id, "never-answered"],
                |row| row.get(0),
            )?;
            let dispatched = connection.query_row(
                "SELECT COUNT(*) FROM kernel_effect_outbox WHERE run_id=?1 AND effect_type='dispatch_tool' AND status='completed'",
                params![run_id],
                |row| row.get(0),
            )?;
            Ok((state, dispatched))
        })
        .unwrap();
    assert_eq!(approval_state, "expired");
    assert_eq!(dispatched, 0, "nothing may have executed");

    // The Host banked the pause, so the work is offerable again.
    let progress = db.kernel_continuable_run(&conversation.id, &run_id).unwrap();
    assert_eq!(progress.pause_reason, "approval_expired");
    assert!(!progress.summary.is_null());

    // A continuation creates a NEW attempt; the source run stays terminal.
    let prepared = crate::database::PreparedContinuation {
        artifacts: None,
        skill_activations: Vec::new(),
        budgets: db.run_time_budgets(&run_id).unwrap(),
        prompt_config_hash: "continuation-hash".into(),
        capability_manifest_hash: "continuation-manifest".into(),
        frozen_config_json: "{}".into(),
    };
    let continuation_request = || crate::database::ContinuationRequest {
        conversation_id: conversation.id.clone(),
        source_run_id: run_id.clone(),
        tier: crate::database::BudgetTier::Standard,
        custom_execution_ms: None,
    };
    let next = db
        .kernel_insert_continuation_attempt(&continuation_request(), "继续未完成的任务", &prepared)
        .unwrap();
    assert_ne!(next.run.id, run_id);
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "approval_expired",
        "a continuation must not rewrite the source run state"
    );
    let (child_link, source_link): (Option<String>, Option<String>) = db
        .with_connection(|connection| {
            Ok((
                connection.query_row(
                    "SELECT continued_from_run_id FROM kernel_runs WHERE run_id=?1",
                    params![next.run.id],
                    |row| row.get(0),
                )?,
                connection.query_row(
                    "SELECT continued_from_run_id FROM kernel_runs WHERE run_id=?1",
                    params![run_id],
                    |row| row.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(child_link.as_deref(), Some(run_id.as_str()));
    assert_eq!(
        source_link, None,
        "the source must not point forward at its own successor (that formed a cycle)"
    );
    // Repeating the continuation is refused rather than starting a third run.
    assert!(db
        .kernel_insert_continuation_attempt(&continuation_request(), "重复续做", &prepared)
        .is_err());
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

/// R3 acceptance: continuation chains are one-way, attempts increase, and a
/// source run gets exactly one direct successor even under concurrent asks.
#[test]
fn continuation_chain_is_one_way_and_one_successor_per_source() {
    use fox_engine_protocol::{ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding};
    use rusqlite::params;

    let root = std::env::temp_dir().join(format!("fox-continuation-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("facts.db");
    let db = Database::open(path).unwrap();
    let agent_id = db.default_agent_id().to_string();
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO conversations (id, agent_id, title, permission_mode, project_root, status, created_at, updated_at)
             VALUES ('conv-chain', ?1, 'chain', 'ask', ?2, 'active', 1, 1)",
            params![agent_id, root.to_string_lossy()],
        )?;
        Ok(())
    })
    .unwrap();

    // Create a paused attempt with everything the continuation path reads.
    let paused_attempt = |run_id: &str, pause: &str, attempt: i64| {
        let started = db.create_run("conv-chain", "原始请求：分析这批数据", None).unwrap();
        let legacy_run_id = started.run.id.clone();
        let permission = FrozenPermission {
            mode: PermissionMode::Ask,
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
        };
        let binding = RunControlBinding {
            schema_version: 1,
            run_id: legacy_run_id.clone(),
            conversation_id: "conv-chain".into(),
            engine_id: "pi".into(),
            execution_profile_id: "legacy".into(),
            authority: ExecutionAuthority::Authoritative,
            read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
            permission,
            budgets: TimeBudgets::default(),
        };
        db.freeze_run_control(&binding).unwrap();
        db.with_connection(|connection| {
            // A paused Run is not active in the legacy projection, otherwise a
            // continuation could not create its successor (the active-run guard
            // would refuse).
            connection.execute(
                "UPDATE runs SET status='failed', error_code='approval.wait_timeout' WHERE id=?1",
                params![legacy_run_id],
            )?;
            connection.execute(
                "INSERT INTO kernel_runs
                 (run_id, engine_id, kernel_mode, capability_manifest_version,
                  permission_snapshot_id, execution_profile_id, prompt_config_hash,
                  frozen_config_json, state, last_event_seq, created_at, updated_at,
                  terminal_at, capability_manifest_hash, terminal_written, attempt)
                 VALUES (?1, 'pi', 'authoritative', 2, ?2, 'legacy', 'h', '{}',
                         ?3, 3, 10, 20, 20, 'm', 1, ?4)",
                params![
                    legacy_run_id,
                    binding.permission_snapshot_id,
                    if pause == "budget_exhausted" { "budget_exhausted" } else { "approval_expired" },
                    attempt,
                ],
            )?;
            connection.execute(
                "INSERT INTO kernel_run_progress
                 (run_id, attempt, pause_reason, completed_tool_calls, pending_tool_calls,
                  terminal_written, running_elapsed_ms, continuable, summary_json, created_at)
                 VALUES (?1, ?2, ?3, 1, 1, 1, 500, 1, '{}', 20)",
                params![legacy_run_id, attempt, pause],
            )?;
            Ok(())
        })
        .unwrap();
        let _ = run_id;
        legacy_run_id
    };

    let first = paused_attempt("attempt-1", "approval_expired", 1);
    let prepared = || crate::database::PreparedContinuation {
        artifacts: None,
        skill_activations: Vec::new(),
        budgets: TimeBudgets::default(),
        prompt_config_hash: "chain-hash".into(),
        capability_manifest_hash: "chain-manifest".into(),
        frozen_config_json: "{}".into(),
    };
    let request = |source: &str| crate::database::ContinuationRequest {
        conversation_id: "conv-chain".into(),
        source_run_id: source.to_string(),
        tier: crate::database::BudgetTier::Standard,
        custom_execution_ms: None,
    };

    // Segment 1: source -> attempt 2.
    let second = db
        .kernel_insert_continuation_attempt(&request(&first), "继续第一段", &prepared())
        .unwrap()
        .run
        .id;
    // A second request for the SAME source is refused (one successor).
    assert!(db
        .kernel_insert_continuation_attempt(&request(&first), "重复", &prepared())
        .is_err());

    // Make attempt 2 paused so the chain can continue again.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE runs SET status='failed', error_code='approval.wait_timeout' WHERE id=?1",
            params![second],
        )?;
        connection.execute(
            "INSERT INTO kernel_run_progress
             (run_id, attempt, pause_reason, completed_tool_calls, pending_tool_calls,
              terminal_written, running_elapsed_ms, continuable, summary_json, created_at)
             VALUES (?1, 2, 'approval_expired', 2, 1, 1, 900, 1, '{}', 30)",
            params![second],
        )?;
        connection.execute(
            "UPDATE kernel_runs SET state='approval_expired', terminal_written=1 WHERE run_id=?1",
            params![second],
        )?;
        Ok(())
    })
    .unwrap();

    // Segment 2: attempt 2 -> attempt 3 (this is what a cycle used to break).
    let third = db
        .kernel_insert_continuation_attempt(&request(&second), "继续第二段", &prepared())
        .unwrap()
        .run
        .id;
    assert_ne!(third, second);

    // The chain is one-way and has no cycle; attempts increase per segment.
    let chain: Vec<(String, Option<String>, i64)> = db
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT run_id, continued_from_run_id, attempt FROM kernel_runs
                  WHERE run_id IN (?1, ?2, ?3) ORDER BY attempt",
            )?;
            let rows = statement.query_map(params![first, second, third], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })
        .unwrap();
    assert_eq!(chain.len(), 3, "all three attempts must exist");
    assert_eq!(chain[0].0, first);
    assert_eq!(chain[0].1, None, "the first attempt has no predecessor");
    assert_eq!(chain[1].0, second);
    assert_eq!(chain[1].1.as_deref(), Some(first.as_str()));
    assert_eq!(chain[2].0, third);
    assert_eq!(chain[2].1.as_deref(), Some(second.as_str()));
    assert_eq!(chain[0].2, 1);
    assert_eq!(chain[1].2, 2, "each segment increases the attempt number");
    assert_eq!(chain[2].2, 3);
    // No run points at itself or at a run that already has it as predecessor.
    for (run_id, predecessor, _) in &chain {
        assert_ne!(predecessor.as_deref(), Some(run_id.as_str()));
    }
    let cycle: i64 = db
        .with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_runs a JOIN kernel_runs b
                   ON b.run_id = a.continued_from_run_id
                  WHERE b.continued_from_run_id = a.run_id",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(cycle, 0, "no two runs may point at each other");
    // Concurrent asks for the same source still produce exactly one successor.
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO kernel_run_progress
             (run_id, attempt, pause_reason, completed_tool_calls, pending_tool_calls,
              terminal_written, running_elapsed_ms, continuable, summary_json, created_at)
             VALUES (?1, 3, 'approval_expired', 1, 1, 1, 10, 1, '{}', 40)",
            params![third],
        )?;
        connection.execute(
            "UPDATE runs SET status='failed', error_code='approval.wait_timeout' WHERE id=?1",
            params![third],
        )?;
        connection.execute(
            "UPDATE kernel_runs SET state='approval_expired', terminal_written=1 WHERE run_id=?1",
            params![third],
        )?;
        Ok(())
    })
    .unwrap();
    let concurrent = {
        let db = db.clone();
        let request = request(&third);
        let prepared = prepared();
        std::thread::spawn(move || {
            let mut ok = 0;
            let mut err = 0;
            for _ in 0..4 {
                match db.kernel_insert_continuation_attempt(&request, "并发", &prepared) {
                    Ok(_) => ok += 1,
                    Err(_) => err += 1,
                }
            }
            (ok, err)
        })
        .join()
        .unwrap()
    };
    assert_eq!(concurrent.0, 1, "exactly one concurrent ask may win: {concurrent:?}");
    assert_eq!(concurrent.1, 3, "the rest must be refused: {concurrent:?}");
    let successors: i64 = db
        .with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM kernel_runs WHERE continued_from_run_id = ?1",
                params![third],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(successors, 1, "one source has exactly one direct successor");
    drop(db);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn an_approved_effect_waits_for_other_approvals_instead_of_failing_admission() {
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = fixture(&clock);
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let requests = ["first", "second"].iter().enumerate().map(|(source_order, id)| ToolCallRequest {
        tool_call_id: (*id).into(), tool: "write_file".into(),
        canonical_input_json: json!({"path":format!("{id}.txt"),"content":id}).to_string(), source_order,
    }).collect();
    coordinator.propose_tools("two-approvals", requests, &Ask).unwrap();
    coordinator.resolve_approval("first", kernel::ApprovalDecision::AllowOnce).unwrap();
    let waiting = coordinator.snapshot().unwrap();
    assert_eq!(waiting.state, "waiting_approval");
    assert!(super::live::pending_dispatch_ids(&waiting).is_empty());
    let first = waiting.pending_effects.iter().find(|e| e.tool_call_id.as_deref() == Some("first")
        && e.kind == kernel::OutboxEffectKind::DispatchTool).unwrap();
    assert!(super::live::dispatch_waits_for_approval(&waiting.state, first));
    assert_eq!(first.status, kernel::OutboxStatus::Pending);
    coordinator.resolve_approval("second", kernel::ApprovalDecision::AllowOnce).unwrap();
    let ready = coordinator.snapshot().unwrap();
    assert_eq!(ready.state, "running");
    assert_eq!(super::live::pending_dispatch_ids(&ready), vec!["first", "second"]);
    let executions = AtomicUsize::new(0);
    for id in ["first", "second"] {
        coordinator.dispatch_tool(id, "approval-test", |_, _, _| {
            executions.fetch_add(1, Ordering::SeqCst); Ok((true, json!({"ok":true})))
        }).unwrap();
    }
    assert_eq!(executions.load(Ordering::SeqCst), 2);
    assert!(coordinator.snapshot().unwrap().tool_calls.iter().all(|call| call.state == "completed"));
}
