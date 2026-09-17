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
    let policy=super::super::super::kernel_gateway::GatewayPolicy {binding:binding.clone(),scope,database:Some(db.clone()),sessions_dir:None};
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
