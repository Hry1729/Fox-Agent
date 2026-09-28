use super::*;

#[test]
fn kernel_reads_project_root_and_office_attachments_through_frozen_gateway() {
    let mut config = worker_configuration();
    config.proposal_tools = ["ls", "find", "read", "read_attachment"].iter()
        .map(|name| json!({"name":name,"description":"reading test","parameters":{"type":"object","properties":{}}})).collect();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let scope = crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: config
            .proposal_tools
            .iter()
            .map(|tool| tool["name"].as_str().unwrap().to_owned())
            .collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: Vec::new(),
    };
    db.freeze_kernel_host_scope(&run, &scope).unwrap();
    let policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding: db.run_control_binding(&run).unwrap().unwrap(),
        scope,

        database: None,
        sessions_dir: None,
        artifacts_dir: None,
    };
    let sheet_path = root.join("AGV长时间任务汇总统计表.xlsx");
    let slides_path = root.join("slides.pptx");
    std::fs::write(
        &sheet_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/office-reading.xlsx"
        )),
    )
    .unwrap();
    std::fs::write(
        &slides_path,
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/office-reading.pptx"
        )),
    )
    .unwrap();
    let foreign = db
        .create_conversation(db.default_agent_id(), None, None, None)
        .unwrap();
    for (id, conversation, path, name) in [
        (
            "sheet",
            policy.binding.conversation_id.as_str(),
            &sheet_path,
            "AGV.xlsx",
        ),
        (
            "slides",
            policy.binding.conversation_id.as_str(),
            &slides_path,
            "slides.pptx",
        ),
        ("foreign", foreign.id.as_str(), &sheet_path, "AGV.xlsx"),
    ] {
        db.add_attachments(&[crate::database::AttachmentRecord {
            id: id.into(),
            conversation_id: conversation.into(),
            message_id: None,
            display_name: name.into(),
            storage_path: path.to_string_lossy().into_owned(),
            media_type: None,
            byte_size: std::fs::metadata(path).unwrap().len() as i64,
            sha256: None,
            status: "ready".into(),
            created_at: 0,
        }])
        .unwrap();
    }
    let calls = [
        ("list", "ls", json!({}), "AGV长时间任务汇总统计表.xlsx"),
        (
            "find",
            "find",
            json!({"pattern":"AGV"}),
            "AGV长时间任务汇总统计表.xlsx",
        ),
        (
            "read",
            "read",
            json!({"path":"AGV长时间任务汇总统计表.xlsx"}),
            "B2: 123.5",
        ),
        (
            "sheet",
            "read_attachment",
            json!({"attachmentId":"sheet"}),
            "A2: AGV长时间任务",
        ),
        (
            "slides",
            "read_attachment",
            json!({"attachmentId":"slides"}),
            "AGV任务汇总 & 分析",
        ),
    ];
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    coordinator.dispatch_initial("read-model", &policy, |binding,frame,_| Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":calls.iter().map(|(id,tool,input,_)|
            json!({"type":"toolCall","id":id,"name":tool,"arguments":input})).collect::<Vec<_>>()})
    })).unwrap();
    for (id, tool, _, expected) in &calls {
        coordinator
            .dispatch_tool(id, "read-owner", |_, effect, token| {
                let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
                let result = if *tool == "read_attachment" {
                    assert!(policy
                        .execute_context_resource(
                            &db,
                            &root,
                            &root,
                            &root,
                            tool,
                            &json!({"attachmentId":"foreign"}),
                            token
                        )
                        .is_err());
                    policy.execute_context_resource(&db, &root, &root, &root, tool, &payload["input"], token)?
                } else {
                    policy.execute(&db, tool, &payload["input"], token)?
                };
                assert!(result.to_string().contains(expected));
                Ok((true, result))
            })
            .unwrap();
    }
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.tool_calls.len(), 5);
    assert!(snapshot
        .tool_calls
        .iter()
        .all(|tool| tool.state == "completed"));
    let connection = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let saved: String = connection
        .query_row(
            "SELECT result_json FROM tool_calls WHERE run_id=?1 AND tool_name='read'",
            [&run],
            |row| row.get(0),
        )
        .unwrap();
    assert!(saved.contains("B2: 123.5"));
}

#[test]
fn approved_html_write_and_streamed_answer_remain_available_after_cancel() {
    let config = worker_configuration();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    let conversation = db
        .run_control_binding(&run)
        .unwrap()
        .unwrap()
        .conversation_id;
    let html = "<!doctype html><h1>真实写入的本地页面</h1>";
    coordinator.dispatch_initial("owner",&Ask,|_,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {
        schema_version:1,run_id:run.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"write-html","name":"write_file","arguments":{"path":"result.html","content":html}}]})
    })).unwrap();
    assert!(!root.join("result.html").exists());
    assert!(db
        .load_conversation(&conversation)
        .unwrap()
        .artifacts
        .is_empty());
    coordinator
        .resolve_approval("write-html", kernel::ApprovalDecision::AllowOnce)
        .unwrap();
    coordinator
        .dispatch_tool("write-html", "writer", |_, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let action =
                crate::tool_host::prepare("write_file", &payload["input"], root.to_str().unwrap())?;
            Ok((
                true,
                crate::tool_host::execute_with_cancellation(action, Some(token))?,
            ))
        })
        .unwrap();
    let first = db.load_conversation(&conversation).unwrap();
    assert_eq!(first.artifacts.len(), 1);
    assert_eq!(first.artifacts[0].media_type.as_deref(), Some("text/html"));
    let batch = coordinator
        .snapshot()
        .unwrap()
        .pending_effects
        .iter()
        .find(|e| e.kind == kernel::OutboxEffectKind::DeliverToolBatch)
        .unwrap()
        .batch_id
        .clone()
        .unwrap();
    assert!(coordinator
        .dispatch_batch(&batch, "reply", &Allow, |_, frame, _| {
            assert!(db
                .save_kernel_model_display(&fox_engine_protocol::KernelModelPreview {
                    schema_version: 1,
                    run_id: run.clone(),
                    conversation_id: conversation.clone(),
                    turn_id: frame.turn_id.clone(),
                    checkpoint_seq: frame.checkpoint_seq,
                    revision: 1,
                    text: "页面已生成，接下来说明使用方法…".into(),
                    reasoning: String::new(),
                    progress_bytes: None,
                })
                .unwrap());
            Err("cancel during streamed explanation".into())
        })
        .is_err());
    coordinator.cancel().unwrap();
    coordinator.settle_cancellation().unwrap();
    drop(coordinator);
    drop(db);
    let reopened = Database::open(root.join("facts.db"))
        .unwrap()
        .load_conversation(&conversation)
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("result.html")).unwrap(),
        html
    );
    assert_eq!(reopened.artifacts.len(), 1);
    assert_eq!(reopened.artifacts[0].id, first.artifacts[0].id);
    assert!(reopened
        .messages
        .iter()
        .any(|m| m.content == "页面已生成，接下来说明使用方法…" && m.status == "interrupted"));
}

#[test]
fn received_display_survives_cancel_failure_and_database_reopen_without_settling_execution() {
    for cancel in [true, false] {
        let config = worker_configuration();
        let clock = TestClock::new(1000);
        let cancellation = CancellationRegistry::default();
        let (db, root, run) =
            fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
        let coordinator =
            KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
        let conversation = db
            .run_control_binding(&run)
            .unwrap()
            .unwrap()
            .conversation_id;
        let mut received = None;
        assert!(coordinator
            .dispatch_initial("display-owner", &Allow, |_, frame, _| {
                let notice = fox_engine_protocol::KernelModelPreview {
                    schema_version: 1,
                    run_id: run.clone(),
                    conversation_id: conversation.clone(),
                    turn_id: frame.input.turn_id.clone(),
                    checkpoint_seq: frame.checkpoint_seq,
                    revision: 1,
                    text: "已经输出的中文内容 😀".into(),
                    reasoning: "供应商返回的思考说明".into(),
                    progress_bytes: Some(128),
                };
                let before = coordinator.snapshot().unwrap();
                assert!(db.save_kernel_model_display(&notice).unwrap());
                assert!(
                    !db.save_kernel_model_display(&notice).unwrap(),
                    "duplicate revision"
                );
                let mut wrong = notice.clone();
                wrong.turn_id = "foreign".into();
                wrong.revision = 2;
                assert!(!db.save_kernel_model_display(&wrong).unwrap());
                assert_eq!(
                    coordinator.snapshot().unwrap().last_event_seq,
                    before.last_event_seq
                );
                let detail = db.load_conversation(&conversation).unwrap();
                assert_eq!(
                    detail
                        .messages
                        .iter()
                        .filter(|m| m.role == "assistant")
                        .count(),
                    1
                );
                assert_eq!(detail.messages.last().unwrap().status, "streaming");
                received = Some(notice);
                Err("injected model delivery interruption".into())
            })
            .is_err());
        if cancel {
            coordinator.cancel().unwrap();
            coordinator.settle_cancellation().unwrap();
        } else {
            coordinator.fail("test.failed", "injected failure").unwrap();
        }
        let mut late = received.unwrap();
        late.revision = 2;
        late.text = "late replacement".into();
        assert!(!db.save_kernel_model_display(&late).unwrap());
        drop(coordinator);
        drop(db);
        let db = Database::open(root.join("facts.db")).unwrap();
        let detail = db.load_conversation(&conversation).unwrap();
        let message = detail
            .messages
            .iter()
            .find(|m| m.role == "assistant")
            .unwrap();
        assert_eq!(message.content, "已经输出的中文内容 😀");
        assert_eq!(message.status, "interrupted");
        assert!(detail.runtime_events.iter().any(
            |e| e.event_type == "reasoning.delta" && e.event["delta"] == "供应商返回的思考说明"
        ));
    }
}

#[test]
fn final_response_replaces_partial_and_projects_tool_process_without_duplicate_usage() {
    let config = worker_configuration();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, _, run) =
        fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    let conversation = db
        .run_control_binding(&run)
        .unwrap()
        .unwrap()
        .conversation_id;
    coordinator.dispatch_initial("owner",&Allow,|_,frame,_| {
        db.save_kernel_model_display(&fox_engine_protocol::KernelModelPreview {schema_version:1,run_id:run.clone(),conversation_id:conversation.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,revision:1,text:"long provisional answer".into(),reasoning:"partial reasoning".into(),progress_bytes:None}).unwrap();
        Ok(fox_engine_protocol::KernelInitialModelResponse {schema_version:1,run_id:run.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,
            assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"thinking","thinking":"final reasoning"},{"type":"text","text":"short"},{"type":"toolCall","id":"read-a","name":"read","arguments":{"path":"proof.txt"}}],"usage":{"input":10,"output":2,"totalTokens":12}})})
    }).unwrap();
    coordinator
        .dispatch_tool("read-a", "reader", |_, _, _| {
            Ok((true, json!({"content":"read result"})))
        })
        .unwrap();
    let detail = db.load_conversation(&conversation).unwrap();
    let messages = detail
        .messages
        .iter()
        .filter(|m| m.role == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "short");
    assert_eq!(messages[0].status, "completed");
    let checkpoint = messages[0].id.rsplit(':').next().unwrap();
    let projected_tools: Vec<_> = detail.runtime_events.iter().filter(|e| e.event_type.starts_with("tool.")).collect();
    assert_eq!(projected_tools.len(), 2);
    assert!(projected_tools.iter().all(|e| e.event["kernelCheckpointSeq"] == checkpoint));
    assert_eq!(db.load_conversation(&conversation).unwrap().runtime_events.iter()
        .filter(|e| e.event_type.starts_with("tool.") && e.event["kernelCheckpointSeq"] == checkpoint).count(), 2);

    assert_eq!(
        detail
            .runtime_events
            .iter()
            .filter(|e| e.event_type == "reasoning.delta")
            .count(),
        1
    );
    assert!(detail
        .runtime_events
        .iter()
        .any(|e| e.event["delta"] == "final reasoning"));
    assert_eq!(
        detail
            .runtime_events
            .iter()
            .filter(|e| e.event_type == "usage.updated")
            .count(),
        1
    );
    assert!(detail
        .runtime_events
        .iter()
        .any(|e| e.event_type == "tool.started" && e.event["tool"] == "read"));
    assert!(
        detail
            .runtime_events
            .iter()
            .any(|e| e.event_type == "tool.completed"
                && e.event["result"]["content"] == "read result")
    );
}

fn exercise_projectless_compute(source: Option<&std::path::Path>, code: &str) -> Value {
    use sha2::{Digest,Sha256};
    let mut config=worker_configuration();
    config.proposal_tools=vec![json!({"name":"attachment_compute","description":"compute","parameters":{"type":"object","properties":{}}})];
    let clock=TestClock::new(1000);
    let cancellation=CancellationRegistry::default();
    let (db,root,run)=fixture_with_project_opt(&clock,&config.hash().unwrap(),Some(&config),true,true,(0,0),false);
    let binding=db.run_control_binding(&run).unwrap().unwrap();
    assert!(binding.permission.project_root.is_none());
    let scope=crate::database::KernelHostScope {schema_version:1,tool_names:["attachment_compute".to_owned()].into_iter().collect(),mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:vec![]};
    db.freeze_kernel_host_scope(&run,&scope).unwrap();
    let policy=super::super::super::kernel_gateway::GatewayPolicy {binding,scope, database: None, sessions_dir: None, artifacts_dir: None };
    let pure=super::super::super::attachment_compute::execute(&db,&root,&root,&policy.binding.conversation_id,&run,&json!({"code":"return {sum: [1, 2, 3].reduce((a,b)=>a+b,0)};"}),||false).unwrap();
    assert_eq!(pure["result"]["sum"],6);
    let file=root.join(if source.is_some(){"input.xlsx"}else{"input.csv"});
    if let Some(source)=source { std::fs::copy(source,&file).unwrap(); } else {std::fs::write(&file,"id,wait\nA,1.5\nA,3.5\nB,-1\n").unwrap();}
    let original=std::fs::read(&file).unwrap();
    let foreign=db.create_conversation(db.default_agent_id(),None,None,None).unwrap();
    for (id,conversation) in [("source",policy.binding.conversation_id.as_str()),("foreign",foreign.id.as_str())] {
        db.add_attachments(&[crate::database::AttachmentRecord {id:id.into(),conversation_id:conversation.into(),message_id:None,display_name:file.file_name().unwrap().to_string_lossy().into_owned(),storage_path:file.to_string_lossy().into_owned(),media_type:None,byte_size:original.len() as i64,sha256:Some(hex::encode(Sha256::digest(&original))),status:"ready".into(),created_at:0}]).unwrap();
    }
    let coordinator=KernelCoordinator::start_prepared(&db,&clock,&run,&cancellation).unwrap();
    let input=json!({"attachmentIds":["source"],"code":code});
    coordinator.dispatch_initial("compute-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse {schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"compute","name":"attachment_compute","arguments":input}]})})).unwrap();
    let mut actual=Value::Null;
    coordinator.dispatch_tool("compute","compute-owner",|_,effect,token|{
        assert!(policy.execute_context_resource(&db,&root,&root,&root,"attachment_compute",&json!({"attachmentIds":["foreign"],"code":"return 1;"}),token).is_err());
        let payload:Value=serde_json::from_str(&effect.payload_json).unwrap();
        let result=policy.execute_context_resource(&db,&root,&root,&root,"attachment_compute",&payload["input"],token)?;
        assert!(serde_json::to_vec(&result).unwrap().len()<256*1024);
        actual=result["details"]["result"].clone();
        Ok((true,result))
    }).unwrap();
    let conversation=db.load_conversation(&policy.binding.conversation_id).unwrap();
    assert!(!conversation.artifacts.is_empty(),"generated files must be projected into the conversation");
    let own=super::super::super::attachment_compute::authorized_artifact_root(&root,&policy.binding.conversation_id).unwrap().unwrap();
    let foreign_root=super::super::super::attachment_compute::safe_workspace(&root,&foreign.id,"foreign-run").unwrap();
    for artifact in &conversation.artifacts {
        let resolved=crate::artifact_gateway::resolve_artifact_path(artifact,&[own.clone()]).unwrap();
        assert!(crate::artifact_gateway::resolve_artifact_path(artifact,&[foreign_root.clone()]).is_err());
        let bytes=std::fs::read(resolved).unwrap();
        assert_eq!(artifact.byte_size,bytes.len() as i64);
        assert_eq!(artifact.sha256.as_deref(),Some(hex::encode(Sha256::digest(&bytes)).as_str()));
        if artifact.display_name.ends_with(".json") { assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(),actual); }
    }
    let saved=conversation.artifacts.iter().find(|a|a.display_name.ends_with(".json")).unwrap();
    let readback=json!({"artifactIds":[saved.id],"code":"return attachments[0].data;"});
    let batch=coordinator.snapshot().unwrap().tool_calls.iter().find(|tool|tool.tool_call_id=="compute").unwrap().batch_id.clone();
    coordinator.dispatch_batch(&batch,"readback-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse {
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"readback","name":"attachment_compute","arguments":readback}]})
    })).unwrap();
    coordinator.dispatch_tool("readback","readback-owner",|_,effect,token|{
        let input:Value=serde_json::from_str(&effect.payload_json).unwrap();
        let reread=policy.execute_context_resource(&db,&root,&root,&root,"attachment_compute",&input["input"],token)?;
        assert_eq!(reread["details"]["result"],actual,"a later tool call must reread the saved file");
        assert!(policy.execute_context_resource(&db,&root,&root,&root,"attachment_compute",&json!({"artifactIds":["foreign-artifact"],"code":"return 1;"}),token).is_err());
        Ok((true,reread))
    }).unwrap();
    assert_eq!(std::fs::read(file).unwrap(),original,"source attachment must remain unchanged");
    if let Some(source)=source {
        std::fs::write(source.parent().unwrap().join("product-artifacts.json"),serde_json::to_vec_pretty(&conversation.artifacts).unwrap()).unwrap();
    }
    actual
}

#[test]
fn projectless_attachment_code_computes_and_persists_scoped_artifacts() {
    let actual=exercise_projectless_compute(None,r#"const rows=attachments[0].sheets[0].rows.slice(1); const unique=[...new Map(rows.map(r=>[r[0],r])).values()]; const result={count:unique.length,total:unique.reduce((s,r)=>s+Number(r[1]),0)}; saveFile('统计.json',JSON.stringify(result)); return result;"#);
    assert_eq!(actual,json!({"count":2,"total":2.5}));
}

#[test]
#[ignore = "independent external XLSX acceptance fixture"]
fn projectless_real_xlsx_code_acceptance() {
    let root=std::path::PathBuf::from(std::env::var("FOX_COMPUTE_ACCEPTANCE_DIR").expect("external fixture directory"));
    let code=std::fs::read_to_string(root.join("analyze.js")).unwrap();
    let actual=exercise_projectless_compute(Some(&root.join("AGV-927-task-acceptance.xlsx")),&code);
    std::fs::write(root.join("product-actual.json"),serde_json::to_vec_pretty(&actual).unwrap()).unwrap();
}


#[test]
fn kernel_empty_final_reasoning_clears_streaming_display() {
    let config = worker_configuration();
    let clock = TestClock::new(1000);
    let cancellation = CancellationRegistry::default();
    let (db, root, run) = fixture_with_start_opt(&clock, &config.hash().unwrap(), Some(&config), true, true);
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    let conversation = db.run_control_binding(&run).unwrap().unwrap().conversation_id;
    coordinator.dispatch_initial("owner", &Allow, |_, frame, _| {
        db.save_kernel_model_display(&fox_engine_protocol::KernelModelPreview {
            schema_version: 1, run_id: run.clone(), conversation_id: conversation.clone(), turn_id: frame.input.turn_id.clone(),
            checkpoint_seq: frame.checkpoint_seq, revision: 1, text: "draft".into(), reasoning: "obsolete".into(), progress_bytes: None,
        }).unwrap();
        Ok(fox_engine_protocol::KernelInitialModelResponse { schema_version: 1, run_id: run.clone(),
            turn_id: frame.input.turn_id.clone(), checkpoint_seq: frame.checkpoint_seq,
            assistant_message: json!({"role":"assistant","stopReason":"stop","content":[{"type":"text","text":"final"}]}),
        })
    }).unwrap();
    drop(coordinator);
    drop(db);
    let detail = Database::open(root.join("facts.db")).unwrap().load_conversation(&conversation).unwrap();
    assert!(detail.runtime_events.iter().filter(|e| e.event_type == "reasoning.delta").all(|e| e.event["delta"] == ""));
    assert_eq!(detail.messages.last().unwrap().content, "final");
}
