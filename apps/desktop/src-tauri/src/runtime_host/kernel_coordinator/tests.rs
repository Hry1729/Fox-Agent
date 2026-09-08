use super::*;
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
    let root = std::env::temp_dir().join(format!("fox-coordinator-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("proof.txt"), "durable coordinator 中文 😀").unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
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
        project_root: Some(root.to_string_lossy().into_owned()),
        grants: vec![],
    };
    let binding = RunControlBinding {
        schema_version: 1,
        run_id: run_id.clone(),
        conversation_id: conversation.id,
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
        prompt_config_hash: "test-prompt".into(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0,
        turn_max_retries: 0,
    };
    db.kernel_create_run(
        &run_id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        "legacy",
        "test-prompt",
        &serde_json::to_string(&config).unwrap(),
    )
    .unwrap();
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
