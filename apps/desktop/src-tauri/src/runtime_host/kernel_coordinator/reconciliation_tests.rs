use super::*;
use crate::database::ReconciliationRequest;

#[test]
fn reconciliation_recovery_drives_real_worker_and_rejects_mutation() {
    let clock = TestClock::new(1000);
    let (db, root, mut request) = stopped(&clock);
    request.expected_revision = db.kernel_confirm_reconciliation(&request).unwrap().revision;
    let continued = db.kernel_create_recovery_run(&request).unwrap();
    let binding = db.run_control_binding(&continued.run.id).unwrap().unwrap();
    let model = worker_configuration();
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let old: String = connection
        .query_row(
            "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1",
            [&request.run_id],
            |r| r.get(0),
        )
        .unwrap();
    let mut frozen: kernel::RunFrozenConfig = serde_json::from_str(&old).unwrap();
    frozen.permission_snapshot_id = binding.permission_snapshot_id.clone();
    frozen.prompt_config_hash = model.hash().unwrap();
    db.kernel_create_run(
        &continued.run.id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        &binding.execution_profile_id,
        &frozen.prompt_config_hash,
        &serde_json::to_string(&frozen).unwrap(),
    )
    .unwrap();
    db.freeze_kernel_model_config(&continued.run.id, &model)
        .unwrap();
    let mut input = initial_input(&continued.run.id, &frozen.prompt_config_hash);
    input.messages = vec![json!({"role":"user","content":continued.user_message.content})];
    db.freeze_kernel_initial_input(&input).unwrap();
    freeze_host_scope(&db, &continued.run.id);
    let policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding: binding.clone(),
        scope: db.kernel_host_scope(&continued.run.id).unwrap(),
    };
    assert!(matches!(
        policy.decide(
            &continued.run.id,
            "never-write",
            "write_file",
            r#"{"path":"must-not-create.txt","content":"never"}"#
        ),
        PolicyDecision::Deny { .. }
    ));
    let cancellation = CancellationRegistry::default();
    let host =
        KernelCoordinator::start_prepared(&db, &clock, &continued.run.id, &cancellation).unwrap();
    host.dispatch_initial_with_worker(
        "recovery-worker",
        &policy,
        &real_worker_command(),
        "isolated-test",
    )
    .unwrap();
    assert_eq!(host.snapshot().unwrap().state, "completed");
    assert!(!root.join("must-not-create.txt").exists());
    assert_eq!(
        db.kernel_build_full_snapshot(&request.run_id)
            .unwrap()
            .state,
        "failed"
    );
}

fn stopped(clock: &TestClock) -> (Database, PathBuf, ReconciliationRequest) {
    let cancellation = CancellationRegistry::default();
    let (db, root, run) = fixture(clock);
    let host = KernelCoordinator::reopen(&db, clock, &run, &cancellation).unwrap();
    host.propose_tools("interrupted", calls(), &Allow).unwrap();
    assert!(host
        .dispatch_tool("read-a", "lost-owner", |_, _, _| Err(
            "injected result loss".into()
        ))
        .is_err());
    host.fail("kernel.uncertain_execution", "injected lost response")
        .unwrap();
    let conversation = db
        .run_control_binding(&run)
        .unwrap()
        .unwrap()
        .conversation_id;
    let view = db.kernel_reconciliation_view(&conversation, &run).unwrap();
    assert_eq!(view.items.len(), 1);
    let req = ReconciliationRequest {
        conversation_id: conversation,
        run_id: run,
        expected_revision: 0,
        effect_key: view.items[0].effect_key.clone(),
        decision: "executed".into(),
        note: "Read back the isolated test resource.".into(),
        query_tool: None,
        arguments: json!({}),
    };
    (db, root, req)
}

#[test]
fn reconciliation_query_confirmation_and_new_readonly_run_preserve_original_facts() {
    let clock = TestClock::new(1000);
    let (db, root, mut req) = stopped(&clock);
    let old = db.kernel_build_full_snapshot(&req.run_id).unwrap();
    // The proposed path is proof.txt in the standard fixture.
    let target = db.kernel_reconciliation_target(&req).unwrap();
    assert!(target.input["path"].is_string());
    let queried = crate::kernel_reconciliation::query(&db, &req).unwrap();
    assert_eq!(
        queried.items[0].evidence.as_ref().unwrap()["source"],
        "independent_file_read"
    );
    assert!(queried.items[0].decision.is_none());
    assert!(!queried.can_resume);
    assert!(
        db.kernel_confirm_reconciliation(&req).is_err(),
        "stale confirmation is rejected"
    );
    req.expected_revision = queried.revision;
    let confirmed = db.kernel_confirm_reconciliation(&req).unwrap();
    assert!(confirmed.can_resume);
    req.expected_revision = confirmed.revision;
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    let continued = db.kernel_create_recovery_run(&req).unwrap();
    let binding = db.run_control_binding(&continued.run.id).unwrap().unwrap();
    assert_eq!(binding.authority, ExecutionAuthority::Authoritative);
    assert_eq!(binding.permission.mode, PermissionMode::ReadOnly);
    assert!(binding.permission.grants.is_empty());
    assert!(continued
        .user_message
        .content
        .contains("人工确认不等同于系统执行证明"));
    assert_eq!(db.kernel_build_full_snapshot(&req.run_id).unwrap(), old);
    assert!(
        db.kernel_create_recovery_run(&req).is_err(),
        "a second recovery run cannot be created"
    );
    let reread = db
        .kernel_reconciliation_view(&req.conversation_id, &req.run_id)
        .unwrap();
    assert_eq!(reread.recovery_run_id, Some(continued.run.id));
}

#[test]
fn reconciliation_unresolved_foreign_stale_and_query_after_confirm_fail_closed() {
    let clock = TestClock::new(1000);
    let (db, _, mut req) = stopped(&clock);
    let mut foreign = req.clone();
    foreign.conversation_id = "foreign".into();
    assert!(db.kernel_confirm_reconciliation(&foreign).is_err());
    foreign = req.clone();
    foreign.effect_key = "foreign-effect".into();
    assert!(db.kernel_confirm_reconciliation(&foreign).is_err());
    req.decision = "unresolved".into();
    let v = db.kernel_confirm_reconciliation(&req).unwrap();
    req.expected_revision = v.revision;
    assert!(!v.can_resume);
    assert!(db.kernel_create_recovery_run(&req).is_err());
    req.decision = "not_executed".into();
    let v = db.kernel_confirm_reconciliation(&req).unwrap();
    req.expected_revision = v.revision;
    assert!(v.can_resume);
    let v = crate::kernel_reconciliation::query(&db, &req).unwrap();
    req.expected_revision = v.revision;
    assert!(
        !v.can_resume,
        "new observation requires another explicit confirmation"
    );
    assert!(db.kernel_create_recovery_run(&req).is_err());
}

#[test]
fn reconciliation_resume_transaction_rolls_back_and_rival_requests_create_one_run() {
    let clock = TestClock::new(1000);
    let (db, root, mut req) = stopped(&clock);
    req.expected_revision = db.kernel_confirm_reconciliation(&req).unwrap().revision;
    let c = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let before: i64 = c
        .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
        .unwrap();
    c.execute_batch("CREATE TRIGGER reject_recovery BEFORE INSERT ON kernel_recovery_runs BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(db.kernel_create_recovery_run(&req).is_err());
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        before
    );
    c.execute_batch("DROP TRIGGER reject_recovery").unwrap();
    let left = db.clone();
    let right = db.clone();
    let a = req.clone();
    let b = req;
    let one = std::thread::spawn(move || left.kernel_create_recovery_run(&a));
    let two = std::thread::spawn(move || right.kernel_create_recovery_run(&b));
    assert_eq!(
        usize::from(one.join().unwrap().is_ok()) + usize::from(two.join().unwrap().is_ok()),
        1
    );
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        before + 1
    );
}

#[test]
fn reconciliation_cannot_modify_active_or_superseded_runs() {
    let clock = TestClock::new(1000);
    let (db, _, mut req) = stopped(&clock);
    let newer = db
        .create_run(&req.conversation_id, "A new independent request", None)
        .unwrap();
    assert!(db.kernel_confirm_reconciliation(&req).is_err());
    req.run_id = newer.run.id;
    assert!(db
        .kernel_reconciliation_view(&req.conversation_id, &req.run_id)
        .is_err());
}
