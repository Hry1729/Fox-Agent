use super::*;
use crate::database::ReconciliationRequest;

#[test]
fn reconciliation_office_reads_committed_output_and_rejects_connector_drift() {
    use sha2::{Digest, Sha256};
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let mut model = worker_configuration();
    model.proposal_tools = vec![json!({"name":"call_mcp_tool","description":"Managed Office", "parameters":{"type":"object"}})];
    let (db, root, run) = fixture_with_start_opt(&clock, &model.hash().unwrap(), Some(&model), true, true);
    db.ensure_office_connector("isolated-office-query-fixture", None).unwrap();
    let server = db.get_mcp_server(crate::office::SERVER_ID).unwrap().unwrap();
    let digest = format!("sha256:{}", hex::encode(Sha256::digest(json!({"id":server.id,"transport":server.transport,
        "command":server.command,"args":server.args,"endpoint":server.endpoint_url,"definition":server.definition}).to_string().as_bytes())));
    let scope = crate::database::KernelHostScope { schema_version: 1, tool_names: ["call_mcp_tool".into()].into_iter().collect(),
        mcp_server_hashes: [(crate::office::SERVER_ID.into(), digest)].into_iter().collect(),
        knowledge_reference_hashes: Default::default(), knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks: vec![] };
    db.freeze_kernel_host_scope(&run, &scope).unwrap();
    let host = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    host.dispatch_initial("office-model", &Allow, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"office-write","name":"call_mcp_tool",
            "arguments":{"serverId":crate::office::SERVER_ID,"tool":"office_edit","arguments":{"file":"source.docx","output":"result.docx"}}}]}),
    })).unwrap();
    assert!(host.dispatch_tool("office-write", "lost-office-owner", |_,_,_| {
        // Fault injection models an existing Office output; this does not claim
        // that OfficeCLI itself executed or that these bytes form a valid DOCX.
        std::fs::write(root.join("result.docx"), b"independent output bytes").unwrap();
        std::fs::write(root.join("source.docx"), b"different source").unwrap();
        Err("injected lost Office result".into())
    }).is_err());
    host.fail("kernel.uncertain_execution", "isolated result loss").unwrap();
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    let view = db.kernel_reconciliation_view(&conversation, &run).unwrap();
    let request = ReconciliationRequest { conversation_id: conversation, run_id: run.clone(), expected_revision: 0,
        effect_key: view.items[0].effect_key.clone(), decision: "executed".into(), note: "isolated evidence".into(),
        query_tool: None, arguments: json!({"path":"source.docx"}), recovery_mode: Default::default() };
    assert_eq!(crate::kernel_reconciliation::options(&db, &request).unwrap()["kind"], "file");
    let queried = crate::kernel_reconciliation::query(&db, &request).unwrap();
    let evidence = queried.items[0].evidence.as_ref().unwrap();
    assert_eq!(evidence["path"], "result.docx");
    assert_eq!(evidence["sha256"], format!("sha256:{}", hex::encode(Sha256::digest(b"independent output bytes"))));
    assert!(queried.items[0].decision.is_none());
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    connection.execute("UPDATE mcp_servers SET command='changed-connector' WHERE id=?1", [crate::office::SERVER_ID]).unwrap();
    assert!(crate::kernel_reconciliation::options(&db, &request).is_err());
}

#[test]
fn reconciliation_recovery_drives_real_worker_and_rejects_mutation() {
    recovery_worker_flow(false);
}

#[test]
fn reconciliation_reapproved_new_write_executes_once_after_new_approval() {
    recovery_worker_flow(true);
}

fn recovery_worker_flow(reapprove: bool) {
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, mut request) = stopped(&clock);
    if reapprove { request.recovery_mode = serde_json::from_value(json!("reapprove")).unwrap(); }
    request.expected_revision = db.kernel_confirm_reconciliation(&request).unwrap().revision;
    let continued = db.kernel_create_recovery_run(&request).unwrap();
    let binding = db.run_control_binding(&continued.run.id).unwrap().unwrap();
    let mut model = worker_configuration();
    model.proposal_tools.push(json!({"name":"write_file","description":"Write an approved project file", "parameters":{
        "type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}}));
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
    let scope = crate::database::KernelHostScope { schema_version: 1,
        tool_names: ["read".into(), "write_file".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks: vec![] };
    db.freeze_kernel_host_scope(&continued.run.id, &scope).unwrap();
    let policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding: binding.clone(),
        scope: db.kernel_host_scope(&continued.run.id).unwrap(),

        database: None,
        sessions_dir: None,
        artifacts_dir: None,
    };
    let decision = policy.decide(
            &continued.run.id,
            "never-write",
            "write_file",
            r#"{"path":"must-not-create.txt","content":"never"}"#
        );
    assert!(if reapprove { matches!(decision, PolicyDecision::RequireApproval) }
        else { matches!(decision, PolicyDecision::Deny { .. }) });
    let cancellation = CancellationRegistry::default();
    let host =
        KernelCoordinator::start_prepared(&db, &clock, &continued.run.id, &cancellation).unwrap();
    if reapprove {
        host.dispatch_initial("new-write-proposal", &policy, |binding, frame, _| Ok(fox_engine_protocol::KernelInitialModelResponse {
            schema_version: 1, run_id: binding.run_id.clone(), turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
            assistant_message: json!({"role":"assistant","stopReason":"toolUse","content":[
                {"type":"toolCall","id":"new-write","name":"write_file","arguments":{"path":"new-after-confirmation.txt","content":"new approved proposal"}}]}),
        })).unwrap();
        assert_eq!(host.snapshot().unwrap().state, "waiting_approval");
        assert!(!root.join("new-after-confirmation.txt").exists());
        assert!(!host.dispatch_tool("new-write", "before-approval", |_,_,_| panic!("must wait for new approval")).unwrap());
        db.queue_kernel_host_command(&continued.run.id, Some(("new-write","allow_once"))).unwrap();
        drop(host);
        let count = AtomicUsize::new(0);
        super::super::super::kernel_host::drive(super::super::super::kernel_host::acquire(&root, &continued.run.id).unwrap(),
            &db, &clock, &cancellation, &continued.run.id, &real_worker_command(), "isolated-test", &policy,
            |_, effect, token| {
                count.fetch_add(1, Ordering::SeqCst);
                let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
                Ok((true, policy.execute(&db, "write_file", &payload["input"], token)?))
            }).unwrap();
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(std::fs::read_to_string(root.join("new-after-confirmation.txt")).unwrap(), "new approved proposal");
        assert_eq!(db.kernel_build_full_snapshot(&continued.run.id).unwrap().state, "completed");
    } else {
        host.dispatch_initial_with_worker(
        "recovery-worker",
        &policy,
        &real_worker_command(),
        "isolated-test",
    )
    .unwrap();
    assert_eq!(host.snapshot().unwrap().state, "completed");
    }
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
        recovery_mode: Default::default(),
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

#[test]
fn reconciliation_reapprove_requires_new_approval_and_preserves_old_terminal_run() {
    let clock = TestClock::new(1000);
    let (db, root, mut req) = stopped(&clock);
    let old = db.kernel_build_full_snapshot(&req.run_id).unwrap();
    req.recovery_mode = serde_json::from_value(json!("reapprove")).unwrap();
    assert!(db.kernel_create_recovery_run(&req).is_err());
    req.expected_revision = db.kernel_confirm_reconciliation(&req).unwrap().revision;
    let continued = db.kernel_create_recovery_run(&req).unwrap();
    let binding = db.run_control_binding(&continued.run.id).unwrap().unwrap();
    assert_eq!(binding.permission.mode, PermissionMode::Ask);
    assert!(binding.permission.grants.is_empty());
    let scope = crate::database::KernelHostScope { schema_version: 1,
        tool_names: ["write_file".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks: vec![] };
    let policy = super::super::super::kernel_gateway::GatewayPolicy { binding, scope , database: None, sessions_dir: None, artifacts_dir: None };
    assert!(matches!(policy.decide(&continued.run.id, "new-write", "write_file",
        r#"{"path":"new-after-confirmation.txt","content":"new proposal"}"#), PolicyDecision::RequireApproval));
    assert!(!root.join("new-after-confirmation.txt").exists());
    assert!(continued.user_message.content.contains("重新审批"));
    assert_eq!(db.kernel_build_full_snapshot(&req.run_id).unwrap(), old);
    drop(db);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(db.run_control_binding(&continued.run.id).unwrap().unwrap().permission.mode, PermissionMode::Ask);
    assert!(db.kernel_create_recovery_run(&req).is_err());
    assert!(serde_json::from_value::<ReconciliationRequest>(json!({"conversationId":"c","runId":"r","recoveryMode":"allow"})).is_err());
}
