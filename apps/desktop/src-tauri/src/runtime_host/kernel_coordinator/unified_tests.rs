use super::*;

#[test]
fn continuation_artifacts_commit_atomically_rebind_identity_and_finish_on_real_worker() {
    let config=worker_configuration();
    let clock=TestClock::new(crate::database::now_ms());
    let (db,root,run)=fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let scope=freeze_host_scope(&db,&run);
    let cancellation=CancellationRegistry::default();
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    coordinator.fail(crate::kernel_compaction::INSUFFICIENT,"bounded context stop").unwrap();
    let conversation=db.run_conversation(&run).unwrap().unwrap();
    assert_eq!(db.kernel_continuable_run(&conversation,&run).unwrap().pause_reason,"context_limit");
    let frozen=db.with_connection(|c|c.query_row("SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1",[&run],|r|r.get::<_,String>(0))).unwrap();
    let prepared=crate::database::PreparedContinuation {
        budgets:db.run_time_budgets(&run).unwrap(),prompt_config_hash:config.hash().unwrap(),
        capability_manifest_hash:"coordinator-test-manifest".into(),frozen_config_json:frozen,
        artifacts:Some((config,db.kernel_initial_input(&run).unwrap(),scope)),skill_activations:vec![],
    };
    let request=crate::database::ContinuationRequest {conversation_id:conversation.clone(),source_run_id:run.clone(),tier:crate::database::BudgetTier::Standard,custom_execution_ms:None};
    db.with_connection(|c|c.execute_batch("CREATE TRIGGER unified_reject_scope BEFORE INSERT ON kernel_host_runs BEGIN SELECT RAISE(ABORT,'scope publication failed'); END")).unwrap();
    assert!(db.kernel_insert_continuation_attempt(&request,"continue",&prepared).is_err());
    let count=db.with_connection(|c|c.query_row("SELECT COUNT(*) FROM runs WHERE conversation_id=?1",[&conversation],|r|r.get::<_,i64>(0))).unwrap();
    assert_eq!(count,1,"failed freezing leaves no orphan attempt or user message");
    db.with_connection(|c|c.execute_batch("DROP TRIGGER unified_reject_scope")).unwrap();
    let next=db.kernel_insert_continuation_attempt(&request,"continue",&prepared).unwrap();
    assert_ne!(next.run.id,run);
    let input=db.kernel_initial_input(&next.run.id).unwrap();
    assert_eq!(input.run_id,next.run.id);
    assert_eq!(input.turn_id,format!("kernel-turn:{}",next.run.id));
    super::super::super::kernel_host::drive(
        super::super::super::kernel_host::acquire(&root,&next.run.id).unwrap(),
        &db,&clock,&CancellationRegistry::default(),&next.run.id,&real_worker_command(),"",
        &Allow,|_,_,_|panic!("completed tools must not replay"),
    ).unwrap();
    assert_eq!(db.kernel_build_full_snapshot(&next.run.id).unwrap().state,"completed");
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state,"failed");
    assert!(db.kernel_insert_continuation_attempt(&request,"duplicate",&prepared).is_err());
}

#[test]
fn job_model_tools_use_real_gateway_worker_storage_and_scope() {
    let mut config=worker_configuration();
    for name in super::super::super::background_jobs::TOOLS {
        config.proposal_tools.push(json!({"name":name,"description":"bounded attachment jobs","parameters":{"type":"object"}}));
    }
    let clock=TestClock::new(crate::database::now_ms());
    let (db,root,run)=fixture_with_start_opt(&clock,&config.hash().unwrap(),Some(&config),true,true);
    let mut scope=crate::database::KernelHostScope {schema_version:1,tool_names:Default::default(),mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:vec![]};
    // freeze_host_scope mirrors config's catalog in this fixture.
    scope.tool_names=config.proposal_tools.iter().filter_map(|t|t["name"].as_str().map(str::to_owned)).collect();
    db.freeze_kernel_host_scope(&run,&scope).unwrap();
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    let policy=super::super::super::kernel_gateway::GatewayPolicy {binding:binding.clone(),scope,database:Some(db.clone()),sessions_dir:None,artifacts_dir:None};
    let cancellation=CancellationRegistry::default();
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let token=cancellation.run_token(&run).unwrap();
    let execute=|tool,input:&Value|policy.execute_context_resource(&db,&root,&root,&root,tool,input,&token);
    let request=json!({"idempotencyKey":"calculate-once","params":{"code":"function onChunk(chunk) {} function onFinish() {return {total: 40+2, text: '中😀'};}"}});
    let unpack=|v:Value|serde_json::from_str::<Value>(v["content"][0]["text"].as_str().unwrap()).unwrap();
    let started=unpack(execute("compute_job_start",&request).unwrap());
    let id=started["jobId"].as_str().unwrap();
    let repeated=unpack(execute("compute_job_start",&request).unwrap());assert_eq!(repeated["jobId"],id);
    let status=unpack(execute("compute_job_status",&json!({"jobId":id,"waitMs":5000})).unwrap());
    assert_eq!(status["state"],"completed","{status}");assert_eq!(status["attempts"],1);
    let mut offset=0;let mut text=String::new();
    loop {
        let page=unpack(execute("compute_job_result",&json!({"jobId":id,"offset":offset,"limit":64})).unwrap());
        text.push_str(page["content"].as_str().unwrap());
        if page["complete"]==true {break;}let next=page["nextOffset"].as_u64().unwrap();assert!(next>offset);offset=next;
    }
    let actual=db.kernel_job_result_value(&binding.conversation_id,id).unwrap();assert_eq!(text,actual.to_string());
    assert!(text.contains("42"));
    assert!(execute("compute_job_status",&json!({"jobId":id,"conversationId":"fake"})).is_err());
    assert!(db.kernel_job_result_value("foreign",id).is_err());
    assert!(execute("compute_job_start",&json!({"idempotencyKey":"calculate-once","params":{"code":"return 99;"}})).is_err());
    assert_eq!(db.kernel_jobs_for_run(&run).unwrap().len(),1);
    drop(coordinator);
}

// Drive the same production job-operation consumer with two Run identities in
// one conversation. The older Run owns both rows; the newer Run has its own
// frozen Authoritative identity and may only observe its own Jobs when the
// experimental notice contract is enabled.
fn job_scope_fixture(experimental: bool, legacy_caller: bool)
    -> (Database, PathBuf, String, String, String) {
    use crate::database::JobStartRequest;
    let model = worker_configuration();
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, owner) = fixture_with_start_opt(
        &clock, &model.hash().unwrap(), Some(&model), true, true);
    let first = db.run_control_binding(&owner).unwrap().unwrap();
    let conversation = first.conversation_id.clone();
    let completed = db.kernel_job_start(&JobStartRequest {
        run_id: owner.clone(), kind: "attachment_compute".into(),
        idempotency_key: "older-result".into(), params: json!({"input":"fixture"}),
        deadline_ms: Some(crate::database::now_ms() + 60_000), progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&completed, 1).unwrap();
    db.kernel_job_complete_attempt(&completed, 1, &conversation,
        &json!({"privateToOwner":"older-result"})).unwrap();
    let queued = db.kernel_job_start(&JobStartRequest {
        run_id: owner.clone(), kind: "attachment_compute".into(),
        idempotency_key: "older-cancel".into(), params: json!({"input":"fixture"}),
        deadline_ms: Some(crate::database::now_ms() + 60_000), progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    let cancellation = CancellationRegistry::default();
    KernelCoordinator::start_prepared(&db, &clock, &owner, &cancellation)
        .unwrap().fail("test.scope_finished", "first Run ended").unwrap();
    let caller = db.create_run(&conversation, "later Run", None).unwrap().run.id;
    let mut binding = first;
    binding.run_id = caller.clone();
    if legacy_caller { binding.authority = fox_engine_protocol::ExecutionAuthority::Legacy; }
    db.freeze_run_control(&binding).unwrap();
    if !legacy_caller {
        let frozen: String = db.with_connection(|conn| conn.query_row(
            "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1", [&owner],
            |row| row.get(0))).unwrap();
        db.kernel_create_run(&caller, "pi", "authoritative", 2,
            &binding.permission_snapshot_id, &binding.execution_profile_id,
            &model.hash().unwrap(), &frozen).unwrap();
        if experimental {
            db.with_connection(|conn| conn.execute(
                "UPDATE kernel_runs SET frozen_config_json=json_set(frozen_config_json,
                 '$.experimentalComputeJobNotice',json('true')) WHERE run_id=?1", [&caller],
            )).unwrap();
            assert!(db.compute_job_notice_enabled(&caller).unwrap());
        }
    }
    (db, root, caller, completed, queued)
}

#[test]
fn experimental_compute_job_operations_reject_another_run_before_read_wait_or_cancel() {
    let (db, root, run, completed, queued) = job_scope_fixture(true, false);
    let call = |tool, input: Value| super::super::super::background_jobs::execute(
        &db, &root, &root, &run, tool, &input, None,
        std::time::Duration::from_secs(1), None);
    let status = call("compute_job_status", json!({"jobId":completed}));
    let waiting = call("compute_job_status", json!({"jobId":queued,"waitMs":100}));
    let result = call("compute_job_result", json!({"jobId":completed}));
    let cancel = call("compute_job_cancel", json!({"jobId":queued}));
    assert!(status.is_err() && waiting.is_err() && result.is_err() && cancel.is_err(),
        "cross-Run operation reached a sibling Job: status={status:?}, wait={waiting:?}, result={result:?}, cancel={cancel:?}");
    let untouched = db.kernel_job_snapshot(&queued).unwrap();
    assert_eq!(untouched.state.as_str(), "queued");
    assert_eq!(untouched.cancel_requested_at, None,
        "cross-Run cancel must not change the target Job");
}

#[test]
fn default_off_and_legacy_compute_job_operations_keep_conversation_compatibility() {
    for legacy in [false, true] {
        let (db, root, run, completed, queued) = job_scope_fixture(false, legacy);
        let call = |tool, input: Value| super::super::super::background_jobs::execute(
            &db, &root, &root, &run, tool, &input, None,
            std::time::Duration::from_secs(1), None);
        let status = call("compute_job_status", json!({"jobId":completed})).unwrap();
        assert!(status.to_string().contains("completed"));
        let result = call("compute_job_result", json!({"jobId":completed})).unwrap();
        assert!(result.to_string().contains("older-result"));
        call("compute_job_cancel", json!({"jobId":queued})).unwrap();
        assert!(db.kernel_job_snapshot(&queued).unwrap().cancel_requested_at.is_some());
    }
}
