use super::*;
use crate::kernel_compaction as context;

fn compaction_fixture(clock: &TestClock) -> (Database, PathBuf, String) {
    compaction_fixture_count(clock, 12)
}

fn compaction_fixture_count(clock: &TestClock, old_count: usize) -> (Database, PathBuf, String) {
    let mut config = worker_configuration();
    config.model_service["contextWindow"] = json!(8192);
    config.model_service["maxOutputTokens"] = json!(512);
    let (db, root, run_id) =
        fixture_with_start_opt(clock, &config.hash().unwrap(), Some(&config), false, true);
    let mut input = initial_input(&run_id, &config.hash().unwrap());
    input.messages.extend((0..old_count).map(|index| {
        json!({"role":if index%2==0 {"user"} else {"assistant"},
        "content":format!("old-{index}: {}", "bounded earlier discussion ".repeat(38))})
    }));
    input.messages.extend(
        (0..8).map(
            |index| json!({"role":"user","content":format!("recent-{index}: preserve exactly")}),
        ),
    );
    db.freeze_kernel_initial_input(&input).unwrap();
    freeze_host_scope(&db, &run_id);
    (db, root, run_id)
}

fn response(
    request: &fox_engine_protocol::KernelCompactionRequest,
) -> fox_engine_protocol::KernelCompactionResponse {
    fox_engine_protocol::KernelCompactionResponse {schema_version:1,run_id:request.run_id.clone(),turn_id:request.turn_id.clone(),
        compaction_id:request.compaction_id.clone(),input_hash:request.input_hash.clone(),
        summary:"Prior discussion is context only; preserve the current goal and do not infer approvals.".into(),
        usage:json!({"input":100,"output":20,"totalTokens":120})}
}

#[test]
fn kernel_compaction_prepared_and_committed_views_survive_reopen_without_repeating_model() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    assert_eq!(coordinator.snapshot().unwrap().state, "compacting");
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    let calls = AtomicUsize::new(0);
    coordinator
        .dispatch_pending_compaction("summary-owner", |_, request, _, remaining| {
            calls.fetch_add(1, Ordering::SeqCst);
            assert!(remaining > 0);
            Ok(response(request))
        })
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    let view = coordinator
        .context_view("initial", &original.messages)
        .unwrap();
    assert!(context::bytes(&view).unwrap() < context::bytes(&original.messages).unwrap());
    assert_eq!(view[0], original.messages[0]);
    assert_eq!(
        &view[view.len() - 8..],
        &original.messages[original.messages.len() - 8..]
    );
    assert_eq!(coordinator.snapshot().unwrap().compaction.compactions, 1);
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let visible: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(visible, 0, "summaries are not visible assistant answers");
    let usage: String = conn
        .query_row(
            "SELECT event_json FROM run_events WHERE run_id=?1 AND event_type='usage.updated'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&usage).unwrap()["totalTokens"],
        120
    );
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert!(!coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    coordinator.dispatch_initial("normal-model",&Allow,|binding,frame,_| {
        assert_eq!(frame.input.messages,view);
        Ok(fox_engine_protocol::KernelInitialModelResponse {schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),
            checkpoint_seq:frame.checkpoint_seq,assistant_message:json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"final"}]})})
    }).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn kernel_compaction_failed_result_commit_keeps_source_and_never_replays_after_restart() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_compaction BEFORE INSERT ON kernel_events WHEN NEW.event_type='context.compaction.result' BEGIN SELECT RAISE(ABORT,'injected compaction commit failure'); END;").unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| Ok(response(request)))
        .is_err());
    assert_eq!(coordinator.snapshot().unwrap().state, "compacting");
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    conn.execute_batch("DROP TRIGGER reject_compaction;")
        .unwrap();
    drop(conn);
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator
            .dispatch_pending_compaction("new-owner", |_, _, _, _| panic!(
                "unknown request must not repeat"
            ))
            .unwrap_err(),
        context::UNCERTAIN
    );
    drop(coordinator);
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("no resources"),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let error: String = conn
        .query_row(
            "SELECT error_code FROM runs WHERE id=?1",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(error, context::UNCERTAIN);
}

#[test]
fn kernel_compaction_queued_cancel_wins_late_summary_transaction() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            db.queue_kernel_host_command(&run_id, None).unwrap();
            Ok(response(request))
        })
        .is_err());
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
    drop(coordinator);
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("no resources"),
    )
    .unwrap();
    assert_eq!(
        db.kernel_build_full_snapshot(&run_id).unwrap().state,
        "cancelled"
    );
}

#[test]
fn kernel_compaction_single_owner_and_invalid_response_never_publish_a_view() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    let other = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            assert!(other
                .dispatch_pending_compaction("rival", |_, _, _, _| panic!("only one model request"))
                .is_err());
            let stale = coordinator.controller.lock().unwrap().persist_command(&[]);
            clock.advance(200);
            coordinator.tick().unwrap();
            assert!(
                db.kernel_commit_decision(&run_id, clock.now_wall_ms(), &stale)
                    .is_err(),
                "replayed ticks cannot refund compaction execution time"
            );
            Ok(response(request))
        })
        .unwrap();
    assert_eq!(
        db.kernel_compaction_results(&run_id, "initial")
            .unwrap()
            .len(),
        1
    );
    assert!(
        other.tick().is_err(),
        "stale tick cannot restore a completed compaction"
    );
    let (db, _root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap();
    assert!(coordinator
        .dispatch_pending_compaction("owner", |_, request, _, _| {
            let mut invalid = response(request);
            invalid.input_hash = "sha256:foreign".into();
            Ok(invalid)
        })
        .is_err());
    assert!(db
        .kernel_compaction_results(&run_id, "initial")
        .unwrap()
        .is_empty());
}

#[test]
fn kernel_compaction_host_drives_real_isolated_summarizer_then_normal_model() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root, &run_id).unwrap(),
        &db,
        &clock,
        &cancellation,
        &run_id,
        &real_worker_command(),
        "",
        &Allow,
        |_, _, _| panic!("summarizer cannot execute resources"),
    )
    .unwrap();
    let snapshot = db.kernel_build_full_snapshot(&run_id).unwrap();
    assert_eq!(snapshot.state, "completed");
    assert!(snapshot.compaction.compactions > 0);
    assert!(snapshot.tool_calls.is_empty());
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let visible: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND role='assistant'",
            [&run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(visible, 1, "only the normal model answer is visible");
}

#[test]
fn kernel_compaction_input_transaction_failure_leaves_no_intent_and_can_prepare_again() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    let before = coordinator.snapshot().unwrap();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_compaction_input BEFORE INSERT ON kernel_events WHEN NEW.event_type='context.compaction.prepared' BEGIN SELECT RAISE(ABORT,'injected input failure'); END;").unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .is_err());
    assert_eq!(coordinator.snapshot().unwrap(), before);
    conn.execute_batch("DROP TRIGGER reject_compaction_input;")
        .unwrap();
    assert!(coordinator
        .prepare_context_if_needed("initial", 4096)
        .unwrap());
    assert!(coordinator
        .snapshot()
        .unwrap()
        .compaction
        .pending
        .unwrap()
        .owner
        .is_none());
}

#[test]
fn kernel_compaction_after_settled_tool_batch_preserves_approval_and_result_facts() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture(&clock);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    coordinator.dispatch_initial("initial-owner",&Ask,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[
            {"type":"toolCall","id":"read-protected","name":"read","arguments":{"path":"proof.txt"}}]})
    })).unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    let batch = snapshot.tool_calls[0].batch_id.clone();
    assert!(
        coordinator.prepare_context_if_needed(&batch, 4096).is_err(),
        "approval boundary cannot compact"
    );
    coordinator
        .resolve_approval("read-protected", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator.dispatch_tool("read-protected","read-owner",|_,_,_|Ok((true,json!({"content":[{"type":"text","text":std::fs::read_to_string(root.join("proof.txt")).unwrap()}]})))).unwrap();
    let before = coordinator.prepare_stored_batch_resume(&batch).unwrap();
    let facts = coordinator.snapshot().unwrap().tool_calls;
    assert!(coordinator.prepare_context_if_needed(&batch, 4096).unwrap());
    coordinator
        .dispatch_pending_compaction("summary-owner", |_, request, _, _| Ok(response(request)))
        .unwrap();
    let after = coordinator.prepare_stored_batch_resume(&batch).unwrap();
    assert_eq!(after.tools, before.tools);
    assert_eq!(after.assistant_message, before.assistant_message);
    assert_eq!(coordinator.snapshot().unwrap().tool_calls, facts);
    assert!(after.tools[0].result["content"]
        .as_array()
        .unwrap()
        .iter()
        .any(|block| block["text"]
            .as_str()
            .unwrap_or("")
            .contains("FOX_EXECUTION_RECEIPT_V1")));
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
    coordinator.dispatch_batch(&batch,"final-owner",&Allow,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"done"}]})
    })).unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
}

#[test]
fn kernel_compaction_has_a_persisted_pass_limit_instead_of_unbounded_summary_calls() {
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run_id) = compaction_fixture_count(&clock, 90);
    let original = db.kernel_initial_input(&run_id).unwrap();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    for _ in 0..context::MAX_PASSES {
        assert!(coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap());
        coordinator
            .dispatch_pending_compaction("owner", |_, request, _, _| Ok(response(request)))
            .unwrap();
    }
    drop(coordinator);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db, &clock, &run_id, &cancellation).unwrap();
    assert_eq!(
        coordinator
            .prepare_context_if_needed("initial", 4096)
            .unwrap_err(),
        context::INSUFFICIENT
    );
    assert_eq!(
        coordinator.snapshot().unwrap().compaction.compactions,
        context::MAX_PASSES as u32
    );
    assert_eq!(db.kernel_initial_input(&run_id).unwrap(), original);
}
