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
    coordinator.dispatch_batch("pi-batch", "pi-delivery", |binding, frame, token| {
        token.check()?;
        assert!(coordinator.prepare_stored_batch_resume("pi-batch").is_err());
        let facts = db.kernel_rehydrate(&run_id)?.unwrap();
        assert_eq!(facts.model_request_since_wall_ms, Some(clock.now_wall_ms()));
        let identity = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"replacement-pi","executionProfileId":binding.execution_profile_id});
        let request = json!({"runId":run_id,"conversationId":binding.conversation_id,"runtimeSessionId":"replacement-pi",
            "payload":{"controlBinding":binding,"batchResume":frame}});
        let resumed = pi_batch_probe("resume", json!({"request":request,"identity":identity,
            "expectedResults":["Rust persisted result A 中文","Rust persisted result B 😀"]}));
        assert_eq!(resumed["executions"], 0);
        assert_eq!(resumed["consumed"], json!(["read-a", "read-b"]));
        assert_eq!(resumed["answer"], "durable batch consumed");
        Ok(())
    }).unwrap();
    assert!(coordinator.prepare_stored_batch_resume("pi-batch").is_err());
    assert!(coordinator.snapshot().unwrap().pending_effects.is_empty());
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
    assert!(coordinator.dispatch_batch("batch", "model", |_, _, _| {
        executions.fetch_add(1, Ordering::SeqCst); Ok(())
    }).is_err());
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms, None);
    assert!(coordinator.prepare_stored_batch_resume("batch").is_ok());
    connection.execute_batch("DROP TRIGGER reject_model_dispatch;").unwrap();
    assert!(coordinator.dispatch_batch("batch", "model", |_, _, token| {
        token.check()?;
        executions.fetch_add(1, Ordering::SeqCst);
        Err("connection lost after sending request".into())
    }).is_err());
    let started_at = db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms;
    assert_eq!(started_at, Some(1_000));
    drop(coordinator);
    let reopened = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(reopened.dispatch_batch("batch", "new-owner", |_, _, _| {
        executions.fetch_add(1, Ordering::SeqCst); Ok(())
    }).is_err());
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms, started_at);
    clock.advance(120_001);
    reopened.tick().unwrap();
    assert!(db.kernel_rehydrate(&run_id).unwrap().unwrap().state.is_terminal());
    assert!(reopened.dispatch_batch("batch", "expired", |_, _, _| panic!("expired model dispatched")).is_err());
}
