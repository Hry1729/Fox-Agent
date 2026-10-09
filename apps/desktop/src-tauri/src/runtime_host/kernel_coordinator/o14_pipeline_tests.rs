//! Synthetic engineering tests. No O14 materials or answer are used.
use super::*;
use std::fs;

fn project_fixture(
    tools: &[&str],
) -> (
    Database,
    PathBuf,
    String,
    crate::kernel_model_config::KernelModelConfig,
    TestClock,
    CancellationRegistry,
) {
    let mut config = worker_configuration();
    config.proposal_tools=tools.iter().map(|name|json!({"name":name,"description":"synthetic engineering","parameters":{"type":"object","properties":{}}})).collect();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let (db, root, run) = fixture_with_budgets_notice_permission_opt(
        &clock,
        &config.hash().unwrap(),
        Some(&config),
        true,
        true,
        (0, 0),
        true,
        TimeBudgets::default(),
        false,
        PermissionMode::Allow,
    );
    (db, root, run, config, clock, cancellation)
}

#[test]
fn project_snapshot_rejects_escape_devices_junction_and_live_root_change() {
    let (db, root, run, _config, _clock, _cancellation) = project_fixture(&["attachment_compute"]);
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    fs::write(root.join("input.csv"), "id,value\na,10\n").unwrap();
    let storage = root.join("storage");
    let sessions = root.join("sessions");
    fs::create_dir(&storage).unwrap();
    fs::create_dir(&sessions).unwrap();
    for path in [
        "../outside.csv",
        "C:\\outside.csv",
        "C:outside.csv",
        "\\\\server\\share\\file.csv",
        "\\\\?\\C:\\file.csv",
        "\\\\.\\NUL",
        "CON.csv",
        "con.txt",
        "LPT1.csv",
        "COM9.json",
        "input.csv:secret",
        "input.csv.",
    ] {
        let input = json!({"projectPaths":[path],"code":"return 1;"});
        assert!(
            super::super::super::attachment_compute::execute_for_run(
                &db,
                &storage,
                &sessions,
                &binding.conversation_id,
                &run,
                &input,
                || false
            )
            .is_err(),
            "must reject {path}"
        );
    }
    let outside = std::env::temp_dir().join(format!("fox-outside-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("secret.csv"), "s,v\nx,42").unwrap();
    #[cfg(windows)]
    {
        let junction = root.join("junction");
        let status = std::process::Command::new("cmd.exe")
            .args(["/D", "/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(status.status.success(), "junction fixture must be created");
        let input = json!({"projectPaths":["junction/secret.csv"],"code":"return 1;"});
        assert!(super::super::super::attachment_compute::execute_for_run(
            &db,
            &storage,
            &sessions,
            &binding.conversation_id,
            &run,
            &input,
            || false
        )
        .is_err());
        fs::remove_dir(junction).unwrap();
    }
    let db_hook = db.clone();
    let conversation = binding.conversation_id.clone();
    let outside_hook = outside.clone();
    super::super::super::attachment_compute::set_post_copy_hook(Some(Box::new(move || {
        db_hook
            .with_connection(|c| {
                c.execute(
                    "UPDATE conversations SET project_root=?2 WHERE id=?1",
                    rusqlite::params![conversation, outside_hook.to_string_lossy()],
                )?;
                c.execute(
                    "UPDATE projects SET root_path=?2 WHERE id=(SELECT project_id FROM conversations WHERE id=?1)",
                    rusqlite::params![conversation, outside_hook.to_string_lossy()],
                )?;
                Ok(())
            })
            .unwrap();
    })));
    let result = super::super::super::attachment_compute::execute_for_run(
        &db,
        &storage,
        &sessions,
        &binding.conversation_id,
        &run,
        &json!({"projectPaths":["input.csv"],"code":"return 1;"}),
        || false,
    );
    super::super::super::attachment_compute::set_post_copy_hook(None);
    assert!(result.err().unwrap().contains("authorization changed"));
    fs::remove_dir_all(outside).unwrap();
}

#[test]
#[cfg(windows)]
fn synthetic_project_compute_pdf_csv_method_publish_receipts_versions_and_delivery() {
    use sha2::{Digest, Sha256};
    let (db, root, run, config, clock, cancellation) =
        project_fixture(&["attachment_compute", "write_file", "read"]);
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let source = root.join("input.csv");
    let original = b"label,value\nfirst,10\nsecond,20\nthird,30\n";
    fs::write(&source, original).unwrap();
    let storage = root.join("storage");
    let sessions = root.join("sessions");
    let backups = root.join("versions");
    fs::create_dir(&storage).unwrap();
    fs::create_dir(&sessions).unwrap();
    fs::create_dir(&backups).unwrap();
    let scope = crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: config
            .proposal_tools
            .iter()
            .map(|t| t["name"].as_str().unwrap().into())
            .collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    };
    db.freeze_kernel_host_scope(&run, &scope).unwrap();
    let policy = super::super::super::kernel_gateway::GatewayPolicy {
        binding: binding.clone(),
        scope: scope.clone(),
        database: Some(db.clone()),
        sessions_dir: Some(sessions.clone()),
        artifacts_dir: None,
    };
    let task =
        "读取 input.csv，不修改输入；生成 synthetic_report.pdf、metrics.csv、analysis_method.md。";
    db.seed_delivery_checklist(
        &run,
        &super::super::super::delivery::expectations_from_task(task),
        crate::database::now_ms(),
    )
    .unwrap();
    let code = r#"const rows=attachments[0].sheets[0].rows.slice(1);const values=rows.map(r=>Number(r[1]));const total=values.reduce((a,b)=>a+b,0);const labels=rows.map(r=>String(r[0]));
saveFile('metrics.csv','metric,value\ntotal,'+total+'\n');saveFile('analysis_method.md','合成技术样例。Total=sum(value)。数据缺口保持未知。');
const charts=['line','bar','pie'].map(type=>({type,title:'合成技术图 '+type,labels,series:[{name:'数值',values}]}));charts.forEach((c,i)=>saveChart('chart'+i+'.svg',c));
savePdf('synthetic_report.pdf',{title:'合成技术报告',blocks:[{type:'paragraph',text:'总额 '+total+'。不是 O14 业务交付。'},{type:'table',columns:['指标','数值'],rows:[['total',total]]},...charts.map(chart=>({type:'chart',chart}))]});return {total};"#;
    let input = json!({"projectPaths":["input.csv"],"code":code});
    let coordinator = KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
    coordinator.dispatch_initial("synthetic-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelInitialModelResponse{schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.input.turn_id.clone(),checkpoint_seq:frame.checkpoint_seq,assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"compute","name":"attachment_compute","arguments":input}]})})).unwrap();
    coordinator
        .dispatch_tool("compute", "compute-owner", |binding, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let result = super::super::super::kernel_host::execute_claimed_dispatch(
                &db,
                binding,
                effect,
                "attachment_compute",
                &payload,
                |durable, _| {
                    let input: Value = serde_json::from_str(durable).unwrap();
                    let result = policy.execute_context_resource(
                        &db,
                        &storage,
                        &sessions,
                        &root,
                        "attachment_compute",
                        &input,
                        token,
                    );
                    let outcome = super::super::super::kernel_host::call_outcome(&result);
                    (
                        result,
                        fox_engine_protocol::ExecutionEvidence::NotStarted,
                        outcome,
                    )
                },
            )?;
            assert_eq!(result["details"]["result"]["total"], 60);
            assert_eq!(
                result["details"]["inputSources"][0]["sha256"],
                hex::encode(Sha256::digest(original))
            );
            Ok((true, result))
        })
        .unwrap();
    let artifacts = db
        .load_conversation(&binding.conversation_id)
        .unwrap()
        .artifacts;
    assert_eq!(artifacts.len(), 6);
    let outputs = ["synthetic_report.pdf", "metrics.csv", "analysis_method.md"];
    let calls=outputs.iter().enumerate().map(|(i,name)|{
        let artifact=artifacts.iter().find(|a|a.display_name==*name).unwrap();
        json!({"type":"toolCall","id":format!("publish-{i}"),"name":"write_file","arguments":{"path":name,"artifactId":artifact.id}})
    }).collect::<Vec<_>>();
    let batch = coordinator
        .snapshot()
        .unwrap()
        .tool_calls
        .iter()
        .find(|t| t.tool_call_id == "compute")
        .unwrap()
        .batch_id
        .clone();
    coordinator.dispatch_batch(&batch,"publish-model",&policy,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse{schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":calls})})).unwrap();
    for (i, name) in outputs.iter().enumerate() {
        let id = format!("publish-{i}");
        coordinator
            .dispatch_tool(&id, "publish-owner", |binding, effect, token| {
                let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
                let result = super::super::super::kernel_host::execute_claimed_dispatch(
                    &db,
                    binding,
                    effect,
                    "write_file",
                    &payload,
                    |durable, claim| {
                        let input: Value = serde_json::from_str(durable).unwrap();
                        let (result, evidence) =
                            super::super::super::managed_files::execute_admitted_file(
                                super::super::super::managed_files::ManagedExecutionContext {
                                    database: &db,
                                    backups_dir: &backups,
                                    conversation_id: &binding.conversation_id,
                                    run_id: &run,
                                    project_root: binding.permission.project_root.as_deref(),
                                    permission_mode: binding.permission.mode.as_str(),
                                    scope: &scope,
                                    sessions_dir: Some(&sessions),
                                },
                                "write_file",
                                &input,
                                &id,
                                claim.credential(),
                                Some(token),
                                &mut || {
                                    db.revalidate_execution_credential(claim.credential())?;
                                    policy.validate("write_file", &input)
                                },
                            );
                        let outcome = super::super::super::kernel_host::call_outcome(&result);
                        (result, evidence, outcome)
                    },
                )?;
                Ok((true, result))
            })
            .unwrap();
        let bytes = fs::read(root.join(name)).unwrap();
        let artifact = artifacts.iter().find(|a| a.display_name == *name).unwrap();
        assert_eq!(
            hex::encode(Sha256::digest(&bytes)),
            artifact.sha256.clone().unwrap()
        );
    }
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(db.run_write_receipts(&run).unwrap().len(), 3);
    let versions=db.managed_file_versions(&binding.conversation_id,None).unwrap();
    assert_eq!(versions.len(),3);
    for name in outputs {let identity=crate::tool_host::canonical_file_identity(&root,name).unwrap();assert!(versions.iter().any(|v|v.storage_path==identity));}
    let verdict = super::super::super::delivery::evaluate_stop(
        &db,
        binding.permission.project_root.as_deref(),
        &run,
    )
    .unwrap();
    assert!(
        matches!(
            verdict,
            super::super::super::delivery::DeliveryStop::Passed { .. }
        ),
        "{verdict:?}"
    );
    // Foreign ids, ambiguous input and read-only input never bypass the gate.
    let bad = json!({"path":"copy.pdf","artifactId":"foreign"});
    assert!(policy.validate("write_file", &bad).is_err());
    let input_guard =
        crate::tool_host::ensure_writable_target(Some(task), root.to_str().unwrap(), &source);
    assert!(input_guard.is_err());
    if let Some(dir) = std::env::var_os("FOX_PDF_TECH_SAMPLE_DIR") {
        let dir = PathBuf::from(dir).join("pipeline");
        fs::create_dir_all(&dir).unwrap();
        for name in outputs {
            fs::copy(root.join(name), dir.join(name)).unwrap();
        }
        fs::write(
            dir.join("receipts.txt"),
            format!("{:#?}", db.run_write_receipts(&run).unwrap()),
        )
        .unwrap();
    }
}
