//! Independent engineering material through the production freeze and consumers.
use super::*;
use crate::kernel::{self, CancellationRegistry, TestClock};
use crate::runtime_host::{kernel_coordinator::KernelCoordinator, kernel_gateway::GatewayPolicy};
use fox_engine_protocol::{
    ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
    TimeBudgets,
};

fn paths(text: &str) -> Vec<String> {
    expectations_from_task(text)
        .into_iter()
        .filter_map(|seed| seed.target_path)
        .collect()
}

#[test]
fn r2_parser_scopes_named_paths_counts_and_non_outputs() {
    for (text, expected) in [
        (
            "读取 raw.csv，合并为 joined_notes.txt；在 rationale.txt 给出判断及依据。",
            vec!["joined_notes.txt", "rationale.txt"],
        ),
        (
            "生成根目录 ledger.csv、notes.md。",
            vec!["ledger.csv", "notes.md"],
        ),
        (
            "在 alpha/ 生成 first.csv；在 beta/ 生成 second.csv。",
            vec!["alpha/first.csv", "beta/second.csv"],
        ),
        (
            "在根目录生成 first.csv，在 beta/ 生成 second.csv。",
            vec!["first.csv", "beta/second.csv"],
        ),
        (
            "输出到 out/：scores.csv、review.md；再在根目录生成 final.txt。",
            vec!["out/scores.csv", "out/review.md", "final.txt"],
        ),
        (
            "例如输出目录为 demo/。请生成 result.txt。",
            vec!["result.txt"],
        ),
        (
            "不要使用输出目录 demo/。请生成 result.txt。",
            vec!["result.txt"],
        ),
        (
            "输出目录为 out/。读取项目根目录的 source.csv，生成 result.txt。",
            vec!["out/result.txt"],
        ),
        (
            "读取 source.xlsx，在 out/ 中生成同名 source.xlsx。",
            vec!["out/source.xlsx"],
        ),
        (
            "Generate \"mixed report.md\" and \"中文 摘要.txt\".",
            vec!["mixed report.md", "中文 摘要.txt"],
        ),
        (
            "生成一份 Excel 并保存为 renamed.xlsx。",
            vec!["renamed.xlsx"],
        ),
    ] {
        let mut actual = paths(text);
        let mut expected = expected;
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected, "{text}");
    }
    let counted = expectations_from_task("在 alpha/ 生成 table.xlsx；在 beta/ 生成一份 Excel。");
    assert_eq!(counted.len(), 2);
    // Added after the user stopped execution; this counterexample is unrun.
    let chained=expectations_from_task("请在 alpha/ 生成一份 Excel 并在 beta/ 产出一份 Word。");
    for (extension,directory) in [("xlsx","alpha"),("docx","beta")] {
        let item=chained.iter().find(|item|item.item_key.starts_with(&format!("slot:{extension}:"))).unwrap();
        assert!(item.requirements.iter().any(|r|matches!(&r.kind,StoredRequirementKind::TargetDirectory{directory:actual} if actual==directory)));
    }
    let anonymous = counted.iter().find(|s| s.target_path.is_none()).unwrap();
    assert!(anonymous.requirements.iter().any(
        |r| matches!(&r.kind,StoredRequirementKind::TargetDirectory{directory} if directory=="beta")
    ));
    assert_eq!(
        expectations_from_task("生成一份 Excel 并保存为 renamed.xlsx。").len(),
        1
    );
    assert_eq!(
        expectations_from_task("读取一份 Excel，生成 out/report.md。").len(),
        1
    );
    for text in [
        "请解释如何生成报告。",
        "请解释如何生成 report.txt。",
        "读取 source.csv 并解释其内容。",
        "不要生成 result.txt。",
        "例如生成 demo.txt。请解释这一示例。",
        "Explain how to generate a report.",
    ] {
        assert!(expectations_from_task(text).is_empty(), "{text}");
        assert!(!unresolved_delivery_intent(text, &[]), "{text}");
    }
    assert!(unresolved_delivery_intent("请生成报告。", &[]));
    let partial = "生成 known.md；请生成报告。";
    assert!(unresolved_delivery_intent(
        partial,
        &expectations_from_task(partial)
    ));
    let root_input =
        read_only_path_roles("读取 source.xlsx，在 out/ 中生成同名 source.xlsx。", None);
    assert!(root_input.contains_relative("source.xlsx"));
    assert!(!root_input.contains_relative("out/source.xlsx"));
    assert!(
        !read_only_path_roles("读取 source.xlsx，然后修改并保存 source.xlsx。", None)
            .contains_relative("source.xlsx")
    );
}

struct Fixture {
    db: Database,
    root: PathBuf,
    run: String,
    clock: TestClock,
    cancellation: CancellationRegistry,
    policy: GatewayPolicy,
}
impl Fixture {
    fn new(task: &str) -> Self {
        let root = std::env::temp_dir().join(format!("fox-r2-{}", uuid::Uuid::new_v4()));
        Self::in_project(task, root)
    }
    fn in_project(task: &str, root: PathBuf) -> Self {
        fs::create_dir_all(&root).unwrap();
        let db = Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(
                db.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some("allow"),
            )
            .unwrap();
        let run = db.create_run(&conversation.id, task, None).unwrap().run.id;
        let clock = TestClock::new(crate::database::now_ms());
        let budgets = TimeBudgets::default();
        let permission = FrozenPermission {
            mode: PermissionMode::Allow,
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
            approval_epoch: None,
        };
        let binding = RunControlBinding {
            schema_version: 1,
            run_id: run.clone(),
            conversation_id: conversation.id,
            engine_id: "pi".into(),
            execution_profile_id: "legacy".into(),
            authority: ExecutionAuthority::Authoritative,
            read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
            permission,
            budgets: budgets.clone(),
        };
        db.freeze_run_control(&binding).unwrap();
        let model=crate::kernel_model_config::KernelModelConfig{engine_id:"pi".into(),native_adapter:None,execution_profile_id:"legacy".into(),
            model_service:json!({"apiType":"faux","modelId":"r2-fixture","baseUrl":"http://localhost","fauxResponses":["unused synthetic response"]}),
            system_prompt:"Synthetic R2 fixture.".into(),proposal_tools:["read","write_file","edit_file","attachment_compute"].into_iter()
                .map(|name|json!({"name":name,"description":"synthetic R2","parameters":{"type":"object","properties":{}}})).collect()};
        let prompt_hash = model.hash().unwrap();
        let config = kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "r2-synthetic-manifest".into(),
            permission_snapshot_id: binding.permission_snapshot_id.clone(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: prompt_hash.clone(),
            model_request_timeout_ms: budgets.model_request_ms,
            model_first_response_ms: budgets.model_first_response_ms,
            model_idle_ms: budgets.model_idle_ms,
            tool_execution_timeout_ms: budgets.tool_execution_ms,
            run_execution_budget_ms: budgets.run_execution_ms,
            run_execution_limited: budgets.run_execution_limited,
            approval_wait_timeout_ms: budgets.approval_wait_ms,
            provider_max_retries: 0,
            turn_max_retries: 0,
            experimental_compute_job_notice: false,
        };
        db.kernel_create_run(
            &run,
            "pi",
            "authoritative",
            2,
            &binding.permission_snapshot_id,
            "legacy",
            &prompt_hash,
            &serde_json::to_string(&config).unwrap(),
        )
        .unwrap();
        db.freeze_kernel_model_config(&run, &model).unwrap();
        db.freeze_kernel_initial_input(&fox_engine_protocol::KernelInitialModelInput {
            schema_version: 1,
            run_id: run.clone(),
            turn_id: format!("kernel-turn:{run}"),
            prompt_config_hash: prompt_hash,
            messages: vec![json!({"role":"user","content":task})],
        })
        .unwrap();
        let scope = crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: ["read", "write_file", "edit_file", "attachment_compute"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: Default::default(),
            lifecycle_hooks: vec![],
        };
        db.freeze_kernel_host_scope(&run, &scope).unwrap();
        let cancellation = CancellationRegistry::default();
        KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(root.join("storage")).unwrap();
        let policy = GatewayPolicy {
            binding,
            scope,
            database: Some(db.clone()),
            sessions_dir: Some(sessions),
            artifacts_dir: None,
        };
        Self {
            db,
            root,
            run,
            clock,
            cancellation,
            policy,
        }
    }
    fn context(&self) -> Value {
        json!({"deliverableRoot":"fox/session-results","deliverableFolder":"session-results"})
    }
    fn freeze(&self, task: &str) {
        freeze_task_delivery(
            &self.db,
            &self.run,
            task,
            &self.context(),
            self.root.to_str(),
        )
        .unwrap();
    }
    fn call(&self, id: &str, tool: &str, input: Value) -> Result<Value, String> {
        let coordinator =
            KernelCoordinator::reopen(&self.db, &self.clock, &self.run, &self.cancellation)?;
        if !coordinator
            .snapshot()?
            .tool_calls
            .iter()
            .any(|call| call.tool_call_id == id)
        {
            coordinator.propose_engine_batch(fox_engine_protocol::KernelEngineBatchCheckpoint{schema_version:1,batch_id:format!("batch-{id}"),
                history:self.db.kernel_initial_input(&self.run)?.messages,
                assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":id,"name":tool,"arguments":input}]})},&self.policy)?;
        }
        let mut output = None;
        coordinator.dispatch_tool(id, "r2-owned-executor", |binding, effect, token| {
            let payload: Value = serde_json::from_str(&effect.payload_json).unwrap();
            let result = crate::runtime_host::kernel_host::execute_claimed_dispatch(
                &self.db,
                binding,
                effect,
                tool,
                &payload,
                |durable, claim| {
                    let input: Value = serde_json::from_str(durable).unwrap();
                    let (result, evidence) = if matches!(tool, "write_file" | "edit_file") {
                        crate::runtime_host::managed_files::execute_admitted_file(
                            crate::runtime_host::managed_files::ManagedExecutionContext {
                                database: &self.db,
                                backups_dir: &self.root.join("versions"),
                                conversation_id: &binding.conversation_id,
                                run_id: &self.run,
                                project_root: binding.permission.project_root.as_deref(),
                                permission_mode: binding.permission.mode.as_str(),
                                scope: &self.policy.scope,
                                sessions_dir: self.policy.sessions_dir.as_deref(),
                            },
                            tool,
                            &input,
                            id,
                            claim.credential(),
                            Some(token),
                            &mut || {
                                self.db
                                    .revalidate_execution_credential(claim.credential())?;
                                self.policy.validate(tool, &input)
                            },
                        )
                    } else if tool == "read" {
                        (
                            crate::runtime_host::managed_files::execute_observed_reader(
                                &self.db,
                                binding,
                                tool,
                                &input,
                                id,
                                token,
                                std::time::Duration::from_secs(10),
                            ),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    } else {
                        (
                            self.policy.execute_context_resource(
                                &self.db,
                                &self.root.join("storage"),
                                self.policy.sessions_dir.as_ref().unwrap(),
                                &self.root,
                                tool,
                                &input,
                                token,
                            ),
                            fox_engine_protocol::ExecutionEvidence::NotStarted,
                        )
                    };
                    let outcome = crate::runtime_host::kernel_host::call_outcome(&result);
                    (result, evidence, outcome)
                },
            );
            match result {
                Ok(value) => {
                    let succeeded = matches!(
                        crate::runtime_host::kernel_host::call_outcome(&Ok(value.clone())),
                        fox_engine_protocol::CallOutcome::Completed
                    );
                    output = Some(value.clone());
                    Ok((succeeded, value))
                }
                Err(error) => {
                    output = Some(json!({"error":error}));
                    Ok((false, json!({"error":error})))
                }
            }
        })?;
        let result = output.ok_or_else(|| "proposal was denied before execution".to_string())?;
        if result["isError"] == true || result.get("error").is_some() {
            return Err(format!("{id}: {result}"));
        }
        Ok(result)
    }
    fn stop(&self) -> DeliveryStop {
        evaluate_stop(&self.db, self.root.to_str(), &self.run).unwrap()
    }
    fn assert_committed_write(&self, id: &str, path: &str) {
        let dispatch = fox_engine_protocol::encode_dispatch_id(&self.run, id).unwrap();
        let execution = self
            .db
            .execution_receipt(&self.run, &dispatch)
            .unwrap()
            .unwrap();
        assert_eq!(
            execution.stage,
            fox_engine_protocol::ExecutionStage::FileCommitted
        );
        assert_eq!(
            execution.external_effect,
            fox_engine_protocol::SideEffectState::Committed
        );
        let receipts = self.db.run_write_receipts(&self.run).unwrap();
        let receipt = receipts
            .iter()
            .find(|r| r.tool_call_id.as_deref() == Some(id))
            .unwrap();
        assert!(same_managed_path(
            &receipt.storage_path,
            &self.root.join(path)
        ));
        assert!(compare_receipt_hash(receipt, &self.root.join(path)).unwrap());
        let versions = self
            .db
            .managed_file_versions(&self.policy.binding.conversation_id, None)
            .unwrap();
        let version = versions
            .iter()
            .find(|v| v.id == receipt.version_id)
            .unwrap();
        assert_eq!(
            hex_digest(receipt.after_hash.as_deref().unwrap()),
            version.after_hash.as_deref().unwrap()
        );
    }
}

#[test]
fn r2_freeze_prompt_reference_and_atomic_reentry() {
    let task = "合并为 consolidated.txt；在 explanation.md 给出依据；在 out/ 生成一份 PDF。";
    let fixture = Fixture::new(task);
    fixture.db.with_connection(|c|c.execute_batch("CREATE TRIGGER r2_abort_seed BEFORE INSERT ON delivery_checklist_items WHEN NEW.item_key='file:explanation.md' BEGIN SELECT RAISE(ABORT,'synthetic mid-seed failure'); END;")).unwrap();
    assert!(freeze_task_delivery(
        &fixture.db,
        &fixture.run,
        task,
        &fixture.context(),
        fixture.root.to_str()
    )
    .is_err());
    assert!(fixture
        .db
        .delivery_checklist(&fixture.run)
        .unwrap()
        .is_empty());
    fixture
        .db
        .with_connection(|c| c.execute_batch("DROP TRIGGER r2_abort_seed;"))
        .unwrap();
    fixture.freeze(task);
    let reference =
        frozen_delivery_reference(&fixture.db, &fixture.run, &fixture.context()).unwrap();
    assert_eq!(reference["recognition"], "recognized");
    assert_eq!(reference["targets"].as_array().unwrap().len(), 3);
    assert_eq!(reference["targets"][0]["path"], "consolidated.txt");
    assert!(reference["targets"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["path"].is_null() && item["directory"] == "out"));
    fixture.freeze("生成 wrong.txt。");
    assert_eq!(
        reference,
        frozen_delivery_reference(&fixture.db, &fixture.run, &fixture.context()).unwrap()
    );
    let root = fixture.root.clone();
    let run = fixture.run.clone();
    drop(fixture);
    let db = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(
        reference,
        frozen_delivery_reference(&db, &run, &json!({"deliverableRoot":"fox/session-results"}))
            .unwrap()
    );
}

#[test]
fn r2_named_exact_targets_real_receipts_and_repair() {
    let task =
        "读取 source.csv；合并为 combined.txt；在 analysis.md 给出判断依据；生成 out/records.csv。";
    let f = Fixture::new(task);
    fs::write(f.root.join("source.csv"), "item,value\nsource,7\n").unwrap();
    f.freeze(task);
    f.call(
        "wrong",
        "write_file",
        json!({"path":"fox/session-results/combined.txt","content":"wrong placement"}),
    )
    .unwrap();
    f.call(
        "analysis",
        "write_file",
        json!({"path":"analysis.md","content":"独立合成依据。"}),
    )
    .unwrap();
    let DeliveryStop::Repair { prompt, items, .. } = f.stop() else {
        panic!("missing targets must arm repair")
    };
    assert!(prompt.contains("目标路径：combined.txt"));
    assert!(prompt.contains("fox/session-results/combined.txt"));
    assert!(prompt.contains("继续覆盖同一错误路径不会完成"));
    assert_eq!(items.len(), 3);
    assert_eq!(items.iter().filter(|v| v.passed).count(), 1);
    f.call(
        "correct",
        "write_file",
        json!({"path":"combined.txt","content":"right placement"}),
    )
    .unwrap();
    f.call(
        "csv",
        "write_file",
        json!({"path":"out/records.csv","content":"item,value\nresult,7\n"}),
    )
    .unwrap();
    assert!(matches!(f.stop(), DeliveryStop::Passed { .. }));
    let version = crate::tool_host::file_version(b"right placement");
    f.call("read-output", "read", json!({"path":"combined.txt"}))
        .unwrap();
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    coordinator.dispatch_batch("batch-read-output","r2-synthetic-model",&f.policy,|binding,frame,_|Ok(fox_engine_protocol::KernelModelResponse{
        schema_version:1,run_id:binding.run_id.clone(),turn_id:frame.turn_id.clone(),batch_id:frame.batch_id.clone(),checkpoint_seq:frame.checkpoint_seq,
        assistant_message:json!({"role":"assistant","stopReason":"toolUse","content":[{"type":"toolCall","id":"versioned","name":"write_file","arguments":{"path":"combined.txt","content":"revised placement","expectedVersion":version}}]})
    })).unwrap();
    f.call(
        "versioned",
        "write_file",
        json!({"path":"combined.txt","content":"revised placement","expectedVersion":version}),
    )
    .unwrap();
    f.assert_committed_write("versioned", "combined.txt");
    assert!(matches!(f.stop(), DeliveryStop::Passed { .. }));
    assert_eq!(
        fs::read_to_string(f.root.join("source.csv")).unwrap(),
        "item,value\nsource,7\n"
    );
    let receipts = f.db.run_write_receipts(&f.run).unwrap();
    assert_eq!(receipts.len(), 5);
    assert_eq!(
        receipts
            .iter()
            .filter(|r| same_managed_path(&r.storage_path, &f.root.join("combined.txt")))
            .count(),
        2
    );
    assert_eq!(
        f.db.managed_file_versions(&f.policy.binding.conversation_id, None)
            .unwrap()
            .len(),
        5
    );
}

#[test]
fn r2_anonymous_directory_and_authorship_cannot_be_borrowed() {
    let task = "在 out/ 生成一份 PDF。";
    let f = Fixture::new(task);
    f.freeze(task);
    let key = f.db.delivery_checklist(&f.run).unwrap()[0].item_key.clone();
    assert!(f
        .db
        .bind_delivery_item(&f.run, &key, "tmp/anonymous.pdf", None)
        .is_err());
    assert!(f.db.delivery_checklist(&f.run).unwrap()[0]
        .target_path
        .is_none());
    f.call(
        "wrong-directory",
        "write_file",
        json!({"path":"tmp/anonymous.pdf","content":"%PDF-1.4\n/Type /Page\n%%EOF"}),
    )
    .unwrap();
    fs::create_dir_all(f.root.join("out")).unwrap();
    fs::write(
        f.root.join("out/external.pdf"),
        "%PDF-1.4\n/Type /Page\n%%EOF",
    )
    .unwrap();
    let other = Fixture::in_project("生成 out/foreign.pdf。", f.root.clone());
    other
        .call(
            "foreign-output",
            "write_file",
            json!({"path":"out/foreign.pdf","content":"%PDF-1.4\n/Type /Page\n%%EOF"}),
        )
        .unwrap();
    assert_eq!(other.db.run_write_receipts(&other.run).unwrap().len(), 1);
    let refused = f.stop();
    assert!(!matches!(refused, DeliveryStop::Passed { .. }));
    let finding: Value = serde_json::from_str(&refused.items()[0].finding_json).unwrap();
    assert_eq!(finding["reasonCode"], "delivery.missing_artifact");
    assert!(f.db.delivery_checklist(&f.run).unwrap()[0]
        .target_path
        .is_none());
    f.call(
        "own-output",
        "write_file",
        json!({"path":"out/produced.pdf","content":"%PDF-1.4\n/Type /Page\n%%EOF"}),
    )
    .unwrap();
    f.assert_committed_write("own-output", "out/produced.pdf");
    assert!(matches!(f.stop(), DeliveryStop::Passed { .. }));
    assert_eq!(
        f.db.delivery_checklist(&f.run).unwrap()[0]
            .target_path
            .as_deref(),
        Some("out/produced.pdf")
    );
    assert!(f
        .db
        .bind_delivery_item(&f.run, &key, "out/external.pdf", None)
        .is_err());
}

#[test]
fn r2_default_slots_input_protection_and_unknown_without_empty_repair() {
    let task = "读取 original.csv；生成一份 PDF。";
    let f = Fixture::new(task);
    fs::write(f.root.join("original.csv"), "key,value\na,1\n").unwrap();
    f.freeze(task);
    let reference = frozen_delivery_reference(&f.db, &f.run, &f.context()).unwrap();
    assert_eq!(reference["targets"][0]["directory"], "fox/session-results");
    let denied = json!({"path":"original.csv","content":"must not write"});
    let validation = f.policy.validate("write_file", &denied).unwrap_err();
    assert!(validation.contains("tool.read_only_input"), "{validation}");
    assert!(f.call("input-denied", "write_file", denied).is_err());
    assert_eq!(
        fs::read_to_string(f.root.join("original.csv")).unwrap(),
        "key,value\na,1\n"
    );
    assert!(f.db.run_write_receipts(&f.run).unwrap().is_empty());
    f.call(
        "default-output",
        "write_file",
        json!({"path":"fox/session-results/new.pdf","content":"%PDF-1.4\n/Type /Page\n%%EOF"}),
    )
    .unwrap();
    f.assert_committed_write("default-output", "fox/session-results/new.pdf");
    assert!(matches!(f.stop(), DeliveryStop::Passed { .. }));
    let unknown = Fixture::new("请生成报告。");
    unknown.freeze("请生成报告。");
    let state = unknown.stop();
    // An unresolved requirement is a question, not a content failure: it routes to
    // the parking lane, never to a repair round and never to a failed terminal.
    assert!(matches!(state, DeliveryStop::Typed(_)));
    assert!(matches!(state.route(), DeliveryRoute::RequiresU1 { .. }));
    // The parked reason must be the row's own answerable question, not a
    // placeholder: that is the only thing the user can act on.
    let DeliveryRoute::RequiresU1 { reason } = state.route() else {
        unreachable!()
    };
    assert!(
        reason.contains("请明确要交付的文件名"),
        "the deferred reason must carry the recognition question: {reason}"
    );
    assert!(!reason.contains("无具体检查原因码"), "{reason}");
    assert!(state.repair_findings().is_none());
    assert!(state.items().is_empty());
    let typed = state.typed().unwrap();
    assert_eq!(p2b_consumer::delivery_class_str(typed.verdict.class), "unconfirmed");
    assert!(typed.repair_prompt.is_none());
    let row = typed
        .rows
        .iter()
        .find(|row| row.item_key == RECOGNITION_ITEM)
        .unwrap();
    assert!(!row.passed);
    assert!(!row.stage, "an unverified requirement may not enter the stage");
    let finding: Value = serde_json::from_str(&row.finding_json).unwrap();
    assert_eq!(finding["verificationStatus"], "unverified");
    assert_eq!(finding["reasonCode"], "delivery.requirements_unresolved");
    assert!(finding["reason"].as_str().is_some_and(|reason| !reason.is_empty()));
    assert!(unknown.db.delivery_checklist(&unknown.run).unwrap()[0]
        .target_path
        .is_none());
    assert_eq!(
        unknown
            .db
            .delivery_repair_round_count(&unknown.run)
            .unwrap(),
        0
    );
    let explain = Fixture::new("请解释如何生成报告。");
    explain.freeze("请解释如何生成报告。");
    assert!(matches!(explain.stop(), DeliveryStop::NoChecklist));
    let named_explain = Fixture::new("请解释如何生成 example-report.txt。");
    named_explain.freeze("请解释如何生成 example-report.txt。");
    assert!(matches!(named_explain.stop(), DeliveryStop::NoChecklist));
    assert!(freeze_task_delivery(
        &explain.db,
        &explain.run,
        "在 ../outside/ 生成一份 PDF。",
        &explain.context(),
        explain.root.to_str()
    )
    .is_err());
}

#[test]
fn r2_artifact_publish_preserves_real_versions_and_semantic_requirements() {
    let task="读取 material.csv；生成 publication.txt；在 out/ 生成一份 Word，报告需包含：过程、结论 两个章节。";
    let f = Fixture::new(task);
    fs::write(f.root.join("material.csv"), "k,v\none,1\n").unwrap();
    f.freeze(task);
    let rows = f.db.delivery_checklist(&f.run).unwrap();
    let slot = rows
        .iter()
        .find(|i| i.item_key.starts_with("slot:"))
        .unwrap();
    let stored =
        f.db.delivery_item_requirements(&f.run, &slot.item_key)
            .unwrap();
    assert!(stored
        .iter()
        .any(|r| matches!(r.kind, StoredRequirementKind::TargetDirectory { .. })));
    // Metadata filtering must preserve every old typed semantic requirement.
    let semantic = f.db.delivery_requirements(&f.run).unwrap();
    assert!(
        semantic
            .iter()
            .any(|r| matches!(r.kind, StoredRequirementKind::Sections { .. })),
        "a nonempty old semantic requirement must survive"
    );
    assert_eq!(stored_requirements(&semantic).len(), semantic.len());
    f.call("compute","attachment_compute",json!({"projectPaths":["material.csv"],"code":"saveFile('publication.txt','合成独立发布样例。');return 1;"})).unwrap();
    let artifact =
        f.db.load_conversation(&f.policy.binding.conversation_id)
            .unwrap()
            .artifacts
            .into_iter()
            .find(|a| a.display_name == "publication.txt")
            .unwrap();
    f.call(
        "publish",
        "write_file",
        json!({"path":"publication.txt","artifactId":artifact.id}),
    )
    .unwrap();
    f.assert_committed_write("publish", "publication.txt");
    assert_eq!(
        fs::read_to_string(f.root.join("publication.txt")).unwrap(),
        "合成独立发布样例。"
    );
    assert_eq!(f.db.run_write_receipts(&f.run).unwrap().len(), 1);
    assert_eq!(
        f.db.managed_file_versions(&f.policy.binding.conversation_id, None)
            .unwrap()
            .len(),
        1
    );
    assert!(
        !matches!(f.stop(), DeliveryStop::Passed { .. }),
        "missing Word cannot be hidden by a published text artifact"
    );
}

#[test]
fn r2_real_continuation_copies_directory_recognition_and_semantics() {
    let task = "在 out/ 生成一份 Word，报告需包含：过程、结论 两个章节；请生成报告。";
    let f = Fixture::new(task);
    f.freeze(task);
    let before = frozen_delivery_reference(&f.db, &f.run, &f.context()).unwrap();
    assert_eq!(before["recognition"], "unresolved");
    let slot =
        f.db.delivery_checklist(&f.run)
            .unwrap()
            .into_iter()
            .find(|i| i.item_key.starts_with("slot:"))
            .unwrap();
    let source_requirements =
        f.db.delivery_item_requirements(&f.run, &slot.item_key)
            .unwrap();
    assert!(source_requirements
        .iter()
        .any(|r| matches!(r.kind, StoredRequirementKind::Sections { .. })));
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    coordinator
        .fail(
            crate::kernel_compaction::INSUFFICIENT,
            "synthetic bounded stop",
        )
        .unwrap();
    let request = crate::database::ContinuationRequest {
        conversation_id: f.policy.binding.conversation_id.clone(),
        source_run_id: f.run.clone(),
        tier: crate::database::BudgetTier::Standard,
        custom_execution_ms: None,
    };
    let prompt =
        f.db.kernel_continuation_prompt(&request.conversation_id, &f.run)
            .unwrap();
    let frozen =
        f.db.with_connection(|c| {
            c.query_row(
                "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1",
                [&f.run],
                |r| r.get::<_, String>(0),
            )
        })
        .unwrap();
    let model = f.db.kernel_model_config(&f.run).unwrap();
    let frozen_value: Value = serde_json::from_str(&frozen).unwrap();
    let prepared = crate::database::PreparedContinuation {
        budgets: f.db.run_time_budgets(&f.run).unwrap(),
        prompt_config_hash: model.hash().unwrap(),
        capability_manifest_hash: frozen_value["capability_manifest_hash"]
            .as_str()
            .unwrap()
            .into(),
        frozen_config_json: frozen,
        artifacts: Some((
            model,
            f.db.kernel_initial_input(&f.run).unwrap(),
            f.policy.scope.clone(),
        )),
        skill_activations: vec![],
    };
    let next =
        f.db.kernel_insert_continuation_attempt(&request, &prompt, &prepared)
            .unwrap();
    // A successor is an ordinary Run under the simplified hand-over. It inherits
    // no content obligations at all — neither the concrete slots nor the source's
    // unresolved recognition marker — because the checklist is what puts a Run
    // back on content verification, and that contract no longer governs a new
    // Run. The source's rows stay exactly as they were.
    assert!(
        f.db.delivery_checklist(&next.run.id).unwrap().is_empty(),
        "the successor starts with no checklist: {:?}",
        f.db.delivery_checklist(&next.run.id)
    );
    assert!(
        f.db.delivery_item_requirements(&next.run.id, &slot.item_key)
            .is_err(),
        "the successor has no inherited requirement"
    );
    assert_eq!(
        f.db.delivery_item_requirements(&f.run, &slot.item_key)
            .unwrap(),
        source_requirements,
        "source rows stay intact"
    );
    assert!(f
        .db
        .delivery_checklist(&f.run)
        .unwrap()
        .iter()
        .any(|item| item.item_key == RECOGNITION_ITEM));
    assert!(f
        .db
        .bind_delivery_item(&next.run.id, &slot.item_key, "tmp/foreign.docx", None)
        .is_err());
    fs::create_dir_all(f.root.join("out")).unwrap();
    fs::write(
        f.root.join("out/external.docx"),
        stored_zip(&[(
            "word/document.xml".into(),
            b"<w:document><w:body><w:p><w:r><w:t>external</w:t></w:r></w:p></w:body></w:document>"
                .to_vec(),
        )]),
    )
    .unwrap();
    let stop = evaluate_stop(&f.db, f.root.to_str(), &next.run.id).unwrap();
    // With no checklist at all, the gate reports the ordinary `NoChecklist` path:
    // nothing in the successor is waiting on content verification.
    assert!(
        matches!(stop, DeliveryStop::NoChecklist),
        "an ordinary successor goes through normal stop handling: {stop:?}"
    );
    assert!(
        f.db
            .delivery_checklist(&next.run.id)
            .unwrap()
            .iter()
            .all(|item| item.item_key != RECOGNITION_ITEM)
    );
    assert_eq!(
        f.db.delivery_item_requirements(&f.run, &slot.item_key)
            .unwrap(),
        source_requirements,
        "source rows stay intact"
    );
}

// ---------------------------------------------------------------------------
// Recognition:delivery_unresolved — the constrained transition matrix
//
// Every case below drives the production entries (`freeze_task_delivery`,
// `evaluate_stop`, `KernelCoordinator::completion_tail_for_test`,
// `kernel_insert_continuation_attempt`) rather than a test-only copy of the
// rule, so what is asserted is the transition the Host really performs.
// ---------------------------------------------------------------------------

fn state_of(db: &Database, run: &str) -> String {
    db.kernel_build_full_snapshot(run).unwrap().state
}

/// The stored `kernel_runs.state`, for a Run that has not started yet.
///
/// A fresh successor has no retry state until its first decision, so the full
/// snapshot cannot be built for it; its stored state is still the durable fact.
fn stored_kernel_state(db: &Database, run: &str) -> String {
    db.with_connection(|connection| {
        connection
            .query_row("SELECT state FROM kernel_runs WHERE run_id=?1", [run], |row| {
                row.get::<_, String>(0)
            })
            .map_err(Into::into)
    })
    .unwrap()
}

fn event_types(db: &Database, run: &str) -> Vec<String> {
    db.with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT event_type FROM kernel_events WHERE run_id=?1 ORDER BY seq")?;
        let rows = statement
            .query_map([run], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn legacy_status(db: &Database, run: &str) -> String {
    db.with_connection(|connection| {
        connection
            .query_row("SELECT status FROM runs WHERE id=?1", [run], |row| {
                row.get::<_, String>(0)
            })
            .map_err(Into::into)
    })
    .unwrap()
}

fn decision_marks(db: &Database, run: &str) -> Vec<String> {
    db.with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT mark FROM kernel_decision_marks WHERE run_id=?1")?;
        let rows = statement
            .query_map([run], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

/// Every durable column of the checklist, so "nothing was rewritten" is checked
/// against the stored bytes rather than against a re-derived view.
fn stored_rows(db: &Database, run: &str) -> Vec<(String, String, String, String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status, COALESCE(target_path,''), COALESCE(finding_json,''),
                    COALESCE(requirements_json,'')
               FROM delivery_checklist_items WHERE run_id=?1 ORDER BY item_key",
        )?;
        let rows = statement
            .query_map([run], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn typed_row<'a>(stop: &'a DeliveryStop, item_key: &str) -> &'a TypedDeliveryRow {
    stop.typed()
        .expect("the stop is typed")
        .rows
        .iter()
        .find(|row| row.item_key == item_key)
        .unwrap_or_else(|| panic!("the typed stop carries {item_key}"))
}

fn row_finding(row: &TypedDeliveryRow) -> Value {
    serde_json::from_str(&row.finding_json).unwrap()
}

fn item_of<'a>(db: &'a Database, run: &str, item_key: &str) -> DeliveryChecklistItem {
    let items = db.delivery_checklist(run).unwrap();
    items
        .into_iter()
        .find(|item| item.item_key == item_key)
        .unwrap_or_else(|| panic!("{run} carries {item_key}"))
}

/// The frozen artifacts a continuation needs, read from the source Run.
fn prepared_of(f: &Fixture) -> crate::database::PreparedContinuation {
    let frozen: String = f
        .db
        .with_connection(|connection| {
            connection
                .query_row(
                    "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1",
                    [&f.run],
                    |row| row.get(0),
                )
                .map_err(Into::into)
        })
        .unwrap();
    let frozen_value: Value = serde_json::from_str(&frozen).unwrap();
    let model = f.db.kernel_model_config(&f.run).unwrap();
    crate::database::PreparedContinuation {
        budgets: f.db.run_time_budgets(&f.run).unwrap(),
        prompt_config_hash: model.hash().unwrap(),
        capability_manifest_hash: frozen_value["capability_manifest_hash"]
            .as_str()
            .unwrap()
            .into(),
        frozen_config_json: frozen,
        artifacts: Some((
            model,
            f.db.kernel_initial_input(&f.run).unwrap(),
            f.policy.scope.clone(),
        )),
        skill_activations: vec![],
    }
}

fn continuation_request(f: &Fixture) -> crate::database::ContinuationRequest {
    crate::database::ContinuationRequest {
        conversation_id: f.policy.binding.conversation_id.clone(),
        source_run_id: f.run.clone(),
        tier: crate::database::BudgetTier::Standard,
        custom_execution_ms: None,
    }
}

/// Insert the real successor attempt for a source that already has banked
/// progress — a source that ended (`failed`) or one that is parked.
///
/// Mirrors what `mod.rs` prepares for a continuation (frozen config, model,
/// initial input, host scope) and then inserts it through the one repository
/// entry, so the inheritance rules under test are the production ones.
fn insert_successor(f: &Fixture) -> String {
    let request = continuation_request(f);
    let prompt = f
        .db
        .kernel_continuation_prompt(&request.conversation_id, &f.run)
        .unwrap();
    f.db.kernel_insert_continuation_attempt(&request, &prompt, &prepared_of(f))
        .unwrap()
        .run
        .id
}

/// Fail the source in a bounded way, then insert the real successor attempt.
fn successor_of(f: &Fixture) -> String {
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    coordinator
        .fail(crate::kernel_compaction::INSUFFICIENT, "synthetic bounded stop")
        .unwrap();
    insert_successor(f)
}

/// ① The unresolved requirement parks the Run through the real shared tail: no
/// repair round, no terminal, no decision mark that could finalize anything.
#[test]
fn recognition_unresolved_parks_instead_of_repairing() {
    let task = "请生成报告。";
    let f = Fixture::new(task);
    f.freeze(task);
    let stop = f.stop();
    let DeliveryRoute::RequiresU1 { reason } = stop.route() else {
        panic!("an unresolved requirement routes to the parking lane")
    };
    assert!(reason.contains("请明确要交付的文件名"), "{reason}");
    assert!(stop.repair_prompt().is_none());
    assert!(f.db.staged_delivery_rounds().unwrap().is_empty());
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    assert_eq!(state_of(&f.db, &f.run), "running");
    let mark = coordinator.completion_tail_for_test(&stop).unwrap();
    assert!(
        mark.is_none(),
        "a deferred delivery carries no decision mark: {mark:?}"
    );
    assert_eq!(state_of(&f.db, &f.run), "awaiting_delivery_review");
    assert_eq!(legacy_status(&f.db, &f.run), "interrupted");
    let events = event_types(&f.db, &f.run);
    assert!(
        events.iter().any(|event| event == "run.awaiting_delivery_review"),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| event == "run.completed" || event == "run.failed"),
        "{events:?}"
    );
    assert!(decision_marks(&f.db, &f.run).is_empty());
    assert!(f.db.staged_delivery_rounds().unwrap().is_empty());
    assert_eq!(f.db.delivery_repair_round_count(&f.run).unwrap(), 0);
    // The queue the parked Run exposes still names the unresolved row, so the
    // decision channel sees the obligation rather than an empty review.
    let outstanding = f.db.kernel_build_full_snapshot(&f.run).unwrap();
    assert_eq!(outstanding.state, "awaiting_delivery_review");
}

/// ② A genuine refusal, unreadable contract or content failure keeps its own
/// class: the unresolved row is the weakest fact and never wins the fold.
#[test]
fn genuine_refused_invalid_and_definite_failure_are_not_downgraded() {
    let recognition = DecisionFact::unconfirmed(
        FactOrigin::CheckUnbound,
        Some(RECOGNITION_UNRESOLVED_CODE),
        "请明确要交付的文件名、类型或目录；当前没有可靠目标可自动修复",
        None,
    );
    let candidate = FactOrigin::candidate("out/notes.md", &"a".repeat(64));
    let refused = DecisionFact::refused(FactOrigin::Spec, "delivery.trust_refused", "拒绝");
    let invalid = DecisionFact::invalid_with_code("delivery.check.contract_unreadable", "契约不可读");
    let failed = DecisionFact::failed(
        candidate,
        "delivery.content_mismatch",
        "内容不符",
        true,
        None,
    );
    for (facts, budget, expected) in [
        (vec![recognition.clone(), refused], true, "refused"),
        (vec![recognition.clone(), invalid], true, "invalid"),
        (vec![recognition.clone(), failed.clone()], true, "repairNeeded"),
        (vec![recognition.clone(), failed], false, "definiteFailure"),
        (vec![recognition.clone()], true, "unconfirmed"),
        (vec![recognition], false, "unconfirmed"),
    ] {
        let decided = p2b_decision::decide(&facts, budget);
        assert_eq!(
            p2b_consumer::delivery_class_str(decided.class),
            expected,
            "budget={budget}"
        );
    }
    // Integration: a real missing file beside the unresolved requirement, with the
    // repair budget spent, is a definite failure — not a downgrade to "unconfirmed"
    // and not a silent pass.
    let task = "生成 out/notes.md，并整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    for _ in 0..2 {
        f.db.record_delivery_repair_round(&f.run, "{}", crate::database::now_ms())
            .unwrap();
    }
    assert_eq!(f.db.delivery_repair_round_count(&f.run).unwrap(), 2);
    let stop = f.stop();
    let DeliveryRoute::Fail { code, message } = stop.route() else {
        panic!("a spent budget on a real failure is a failed terminal: {:?}", stop.route())
    };
    assert_eq!(code, "delivery.definite_failure");
    assert!(!message.is_empty(), "{message}");
    // The unresolved row is still carried, unresolved, in the same verdict.
    let recognition = typed_row(&stop, RECOGNITION_ITEM);
    assert_eq!(recognition.class, "unconfirmed");
    assert!(!recognition.stage);
}

/// ③ Ordinary rows keep their own verdicts while the unresolved row stays
/// unverified: an unbound slot repairs, the recognition row never does.
#[test]
fn ordinary_slot_and_file_rows_keep_their_own_verdicts() {
    let task = "在 out/ 生成一份 Word，报告需包含：过程、结论 两个章节；请生成报告。";
    let f = Fixture::new(task);
    f.freeze(task);
    let slot_key = f
        .db
        .delivery_checklist(&f.run)
        .unwrap()
        .into_iter()
        .find(|item| item.item_key.starts_with("slot:"))
        .expect("the Word slot is a row")
        .item_key;
    let stop = f.stop();
    assert_eq!(
        p2b_consumer::delivery_class_str(stop.typed().unwrap().verdict.class),
        "repairNeeded"
    );
    let slot = typed_row(&stop, &slot_key);
    assert_eq!(slot.class, "repairNeeded");
    assert!(!slot.passed);
    assert!(slot.stage, "a repairNeeded row is the terminal-stage row here");
    assert!(row_finding(slot).get("deliveryVerdict").is_none());
    let recognition = typed_row(&stop, RECOGNITION_ITEM);
    assert_eq!(recognition.class, "unconfirmed");
    assert!(!recognition.passed);
    assert!(!recognition.stage, "an unresolved requirement is never staged");
    assert_eq!(
        row_finding(recognition)["reasonCode"],
        RECOGNITION_UNRESOLVED_CODE
    );
    // Aggregate routing and the per-row verdicts agree: the slot repairs.
    assert!(matches!(stop.route(), DeliveryRoute::Repair));
    let typed = stop.typed().unwrap();
    assert!(typed
        .terminal_items
        .iter()
        .any(|item| item.item_key == slot_key));
    assert!(!typed
        .terminal_items
        .iter()
        .any(|item| item.item_key == RECOGNITION_ITEM));
    assert!(typed
        .pending_items
        .iter()
        .any(|(key, _)| key == RECOGNITION_ITEM));
}

/// ④ A verified sibling cannot complete the Run while the requirement set is
/// unresolved, and the parking branch arms no repair.
#[test]
fn recognition_blocks_completed_without_arming_repair() {
    let task = "生成 out/notes.md，并整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    f.call(
        "notes",
        "write_file",
        json!({"path":"out/notes.md","content":"# notes\n\nsynthetic\n"}),
    )
    .unwrap();
    let stop = f.stop();
    let file_row = typed_row(&stop, "file:out%2fnotes.md");
    assert!(file_row.passed, "the verified sibling really passed: {file_row:?}");
    let recognition = typed_row(&stop, RECOGNITION_ITEM);
    assert!(!recognition.passed);
    assert!(matches!(stop.route(), DeliveryRoute::RequiresU1 { .. }));
    assert!(stop.repair_prompt().is_none());
    assert!(stop.repair_findings().is_none());
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    // The staging decision carries no mark, so nothing can finalize a terminal.
    assert!(coordinator.completion_decision_for_test(&stop).unwrap().is_none());
    assert!(coordinator.completion_tail_for_test(&stop).unwrap().is_none());
    assert_eq!(state_of(&f.db, &f.run), "awaiting_delivery_review");
    let events = event_types(&f.db, &f.run);
    assert!(
        !events.iter().any(|event| event == "run.completed"),
        "a passing sibling must not complete an unresolved delivery: {events:?}"
    );
    // The obligation-bearing row is durable: the parked Run still answers "what is
    // unresolved" from the ledger, not from memory.
    let retained = item_of(&f.db, &f.run, RECOGNITION_ITEM);
    let finding: Value = serde_json::from_str(retained.finding.as_deref().unwrap()).unwrap();
    assert_eq!(finding["reasonCode"], RECOGNITION_UNRESOLVED_CODE);
    assert_eq!(finding["verificationStatus"], "unverified");
    // Neither row reached a terminal status, and no decision mark was written, so
    // the passing sibling cannot have completed anything either.
    for row in stored_rows(&f.db, &f.run) {
        assert_eq!(row.1, "pending", "{} must stay pending", row.0);
    }
    assert!(decision_marks(&f.db, &f.run).is_empty());
}

/// ⑤ A spent repair budget cannot turn the unresolved requirement into a
/// content failure: the Run stays parked on the same question.
#[test]
fn recognition_stays_parked_when_repair_budget_is_exhausted() {
    let task = "整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    for _ in 0..2 {
        f.db.record_delivery_repair_round(&f.run, "{}", crate::database::now_ms())
            .unwrap();
    }
    let stop = f.stop();
    assert_eq!(p2b_consumer::delivery_class_str(stop.typed().unwrap().verdict.class), "unconfirmed");
    let DeliveryRoute::RequiresU1 { reason } = stop.route() else {
        panic!("an unresolved requirement stays parked whatever the budget: {:?}", stop.route())
    };
    assert!(reason.contains("请明确要交付的文件名"), "{reason}");
    let recognition = typed_row(&stop, RECOGNITION_ITEM);
    assert_eq!(recognition.class, "unconfirmed");
    assert!(!recognition.stage);
    assert!(!f.stop().typed().unwrap().verdict.repair_budget_remaining);
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    assert!(coordinator.completion_tail_for_test(&stop).unwrap().is_none());
    assert_eq!(state_of(&f.db, &f.run), "awaiting_delivery_review");
    assert_eq!(f.db.delivery_repair_round_count(&f.run).unwrap(), 2);
}

/// ⑥ Both rows of one mixed-version Run reach the router, and only the typed row
/// is stamped: the recognition row is decided in the legacy lane, with its own
/// question and no v2 envelope.
#[test]
fn typed_and_legacy_paths_both_route_the_recognition_row() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let task = "在初始目录的 .txt 文件中，按字节数从小到大选出 2 个最小文件，大小相同按文件名升序选取。再按文件名升序合并为 joined.txt。每段先写完整文件名，下一行保留全部内容；两段之间恰好一个空行。不要修改原文件。另外生成 summary.json，写明合并的文件名清单。另外生成一份摘要报告。";
    let f = Fixture::new(task);
    fs::write(f.root.join("alpha.txt"), "aa").unwrap();
    fs::write(f.root.join("zeta.txt"), "z").unwrap();
    fs::write(f.root.join("middle.txt"), "mmm").unwrap();
    with_test_enablement(V2Enablement::On, || f.freeze(task));
    fs::write(f.root.join("joined.txt"), "alpha.txt\naa\n\nzeta.txt\nz").unwrap();
    fs::write(
        f.root.join("summary.json"),
        r#"{"files":["alpha.txt","zeta.txt"]}"#,
    )
    .unwrap();
    let identities = f.db.delivery_checklist_requirements(&f.run).unwrap();
    let typed_keys: Vec<String> = identities
        .iter()
        .filter(|(_, raw)| {
            matches!(
                p2b_contract_identity::classify_requirements(raw.as_deref()),
                p2b_contract_identity::ContractIdentity::HostV2
            )
        })
        .map(|(key, _)| key.clone())
        .collect();
    assert_eq!(typed_keys, vec!["file:joined.txt".to_string()]);
    let stop = f.stop();
    assert!(matches!(stop, DeliveryStop::Typed(_)));
    // The v2 row is stamped with its envelope; the recognition row is not.
    let joined = row_finding(typed_row(&stop, "file:joined.txt"));
    assert!(
        joined.get("deliveryVerdict").is_some(),
        "the typed row owes a v2 record: {joined}"
    );
    let sibling = row_finding(typed_row(&stop, "file:summary.json"));
    assert!(sibling.get("deliveryVerdict").is_none());
    let recognition = typed_row(&stop, RECOGNITION_ITEM);
    assert_eq!(recognition.class, "unconfirmed");
    assert!(!recognition.stage);
    let recognition_finding = row_finding(recognition);
    assert!(
        recognition_finding.get("deliveryVerdict").is_none(),
        "a recognition row is a legacy row and must not be re-identified as v2"
    );
    assert_eq!(
        recognition_finding["reasonCode"],
        RECOGNITION_UNRESOLVED_CODE
    );
    assert!(recognition_finding["reason"]
        .as_str()
        .is_some_and(|reason| reason.contains("请明确要交付的文件名")));
}

/// ⑦ Beside a real file failure the Run still repairs, and the recognition fact
/// stays visible in the repair findings as an unresolved (non-repairable) item.
#[test]
fn recognition_beside_a_real_file_failure_still_repairs() {
    let task = "生成 out/notes.md，并整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    let stop = f.stop();
    assert!(matches!(stop.route(), DeliveryRoute::Repair));
    let prompt = stop.repair_prompt().expect("repair is armed for the missing file");
    assert!(prompt.contains("out/notes.md"), "{prompt}");
    let findings: Value =
        serde_json::from_str(stop.repair_findings().expect("repair findings are recorded"))
            .unwrap();
    assert_eq!(findings["source"], "deliveryVerdict");
    let items = findings["items"].as_array().unwrap();
    let file_item = items
        .iter()
        .find(|item| item["itemKey"] == "file:out%2fnotes.md")
        .expect("the failing file is in the findings");
    assert_eq!(file_item["class"], "repairNeeded");
    let recognition_item = items
        .iter()
        .find(|item| item["itemKey"] == RECOGNITION_ITEM)
        .expect("the unresolved requirement is retained in the same findings");
    assert_eq!(recognition_item["class"], "unconfirmed");
    assert!(recognition_item["reasonCodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|code| code == RECOGNITION_UNRESOLVED_CODE));
    // Arming repair is not the same as having spent a round, and nothing is
    // staged before a decision owns it.
    assert_eq!(f.db.delivery_repair_round_count(&f.run).unwrap(), 0);
    assert!(f.db.staged_delivery_rounds().unwrap().is_empty());
}

/// ⑧ The successor Run does not inherit the stale marker: it starts with no
/// recognition row and re-decides the question from its own text.
#[test]
fn successor_run_skips_the_stale_recognition_row_and_re_recognizes() {
    let task = "整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    let parent_rows = stored_rows(&f.db, &f.run);
    assert_eq!(parent_rows.len(), 1);
    assert_eq!(parent_rows[0].0, RECOGNITION_ITEM);
    let child = successor_of(&f);
    assert!(f.db.delivery_checklist(&child).unwrap().is_empty(),
        "the source's unresolved marker is a statement about the source text only");
    assert_eq!(f.db.continued_from_run_id(&child).unwrap().as_deref(), Some(f.run.as_str()));
    assert_eq!(
        frozen_delivery_reference(&f.db, &child, &f.context()).unwrap()["recognition"],
        "no_file_deliverables",
        "an empty successor checklist is not an unresolved one"
    );
    // The parent keeps its row and its bytes.
    assert_eq!(stored_rows(&f.db, &f.run), parent_rows);
}

/// ⑨ A valid clarification freezes the new obligations on the successor and
/// leaves the parent's frozen contract untouched.
#[test]
fn valid_clarification_freezes_new_obligations_without_touching_the_old_contract() {
    let task = "整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    let parent_rows = stored_rows(&f.db, &f.run);
    let child = successor_of(&f);
    // A clarification that names one concrete artifact. Nothing generic ("一份文档")
    // is left unresolved here, so this is the resolved case by construction.
    freeze_task_delivery(
        &f.db,
        &child,
        "把结果整理成 out/report.docx。",
        &f.context(),
        f.root.to_str(),
    )
    .unwrap();
    let reference = frozen_delivery_reference(&f.db, &child, &f.context()).unwrap();
    assert_eq!(reference["recognition"], "recognized");
    assert_eq!(reference["targets"][0]["path"], "out/report.docx");
    let rows = f.db.delivery_checklist(&child).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].target_path.as_deref(), Some("out/report.docx"));
    assert_eq!(rows[0].item_key, "file:out%2freport.docx");
    assert_eq!(rows[0].status, "pending");
    assert!(rows[0].finding.is_none());
    // The parent's frozen contract — rows, bytes and the reference it hands the
    // model — is exactly what it was; a clarification does not rewrite history.
    assert_eq!(stored_rows(&f.db, &f.run), parent_rows);
    let parent_reference = frozen_delivery_reference(&f.db, &f.run, &f.context()).unwrap();
    assert_eq!(parent_reference["recognition"], "unresolved");
    assert!(parent_reference["targets"].as_array().unwrap().is_empty());
    assert_eq!(
        f.db.delivery_item_requirements(&f.run, RECOGNITION_ITEM)
            .unwrap()
            .iter()
            .filter(|requirement| matches!(
                requirement.kind,
                StoredRequirementKind::RecognitionUnresolved { .. }
            ))
            .count(),
        1,
        "the source still states, once, that its own requirement set was unresolved"
    );
}

/// ⑩ An unusable or repeated clarification lands on the same unresolved state
/// instead of a fabricated obligation, and the source cannot be continued twice.
#[test]
fn invalid_or_repeated_clarification_yields_unresolved_again() {
    let task = "整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    let child = successor_of(&f);
    // An unsafe directory is refused outright and freezes nothing.
    assert!(freeze_task_delivery(
        &f.db,
        &child,
        "在 ../outside/ 生成一份 PDF。",
        &f.context(),
        f.root.to_str(),
    )
    .is_err());
    assert!(f.db.delivery_checklist(&child).unwrap().is_empty());
    // The same unresolved phrasing resolves to the same unresolved state — it is
    // a question again, not a guessed obligation and not a forged pass.
    freeze_task_delivery(&f.db, &child, "把结果整理成文件。", &f.context(), f.root.to_str()).unwrap();
    let rows = f.db.delivery_checklist(&child).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].item_key, RECOGNITION_ITEM);
    assert!(rows[0].target_path.is_none());
    assert_eq!(
        frozen_delivery_reference(&f.db, &child, &f.context()).unwrap()["recognition"],
        "unresolved"
    );
    assert!(matches!(
        evaluate_stop(&f.db, f.root.to_str(), &child).unwrap().route(),
        DeliveryRoute::RequiresU1 { .. }
    ));
    // One source has exactly one successor.
    let request = continuation_request(&f);
    let prompt = f
        .db
        .kernel_continuation_prompt(&request.conversation_id, &f.run)
        .unwrap();
    let repeated = f
        .db
        .kernel_insert_continuation_attempt(&request, &prompt, &prepared_of(&f))
        .unwrap_err();
    assert!(
        repeated.to_string().contains("已经续做过"),
        "{repeated}"
    );
}

/// ⑪ Everything that *is* an obligation travels; only the unresolved marker is
/// left behind.
#[test]
fn other_obligations_are_still_inherited() {
    let task = "生成 summary.txt，并整理成文件。";
    let f = Fixture::new(task);
    f.freeze(task);
    let parent_keys: Vec<String> = f
        .db
        .delivery_checklist(&f.run)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    assert!(parent_keys.contains(&"file:summary.txt".to_string()));
    assert!(parent_keys.contains(&RECOGNITION_ITEM.to_string()));
    let requirement = f
        .db
        .delivery_item_requirements(&f.run, "file:summary.txt")
        .unwrap();
    let child = successor_of(&f);
    // A successor is an ordinary Run under the simplified hand-over: it gets no
    // checklist, so nothing can put it back on content verification. The source
    // keeps its own obligations untouched.
    let child_items = f.db.delivery_checklist(&child).unwrap();
    assert!(
        child_items.is_empty(),
        "the successor must not inherit a checklist: {child_items:?}"
    );
    assert!(
        f.db.delivery_item_requirements(&child, "file:summary.txt")
            .is_err(),
        "the successor has no inherited requirement to read"
    );
    let parent_items = f.db.delivery_checklist(&f.run).unwrap();
    assert!(
        parent_items.iter().any(|item| item.item_key == "file:summary.txt"),
        "the source keeps its own obligation"
    );
    assert_eq!(
        f.db.delivery_item_requirements(&f.run, "file:summary.txt")
            .unwrap(),
        requirement,
        "the source's requirement is unchanged"
    );
}

/// ⑫ The parked state, its event and the successor link survive a persisted
/// read, so nothing about the stop depends on in-memory state.
#[test]
fn parked_state_and_successor_link_survive_a_persisted_read() {
    let task = "请生成报告。";
    let f = Fixture::new(task);
    f.freeze(task);
    let stop = f.stop();
    let coordinator = KernelCoordinator::reopen(&f.db, &f.clock, &f.run, &f.cancellation).unwrap();
    coordinator.completion_tail_for_test(&stop).unwrap();
    let parent_rows = stored_rows(&f.db, &f.run);
    // The real recovery path: a parked source is continued, not failed. The park
    // decision already banked the progress rows the continuation reads.
    let child = insert_successor(&f);
    let root = f.root.clone();
    let parent = f.run.clone();
    drop(f);
    let db = Database::open(root.join("facts.db")).unwrap();
    // The parent is still parked after a fresh process-level read, with the same
    // event and the same rows.
    assert_eq!(state_of(&db, &parent), "awaiting_delivery_review");
    assert_eq!(legacy_status(&db, &parent), "interrupted");
    assert_eq!(stored_rows(&db, &parent), parent_rows);
    assert!(event_types(&db, &parent)
        .iter()
        .any(|event| event == "run.awaiting_delivery_review"));
    // The successor link is one-way and durable: the child points at the parent,
    // the parent points at nothing, so a chain cannot cycle.
    assert_eq!(
        db.continued_from_run_id(&child).unwrap().as_deref(),
        Some(parent.as_str())
    );
    assert!(db.continued_from_run_id(&parent).unwrap().is_none());
    assert_eq!(stored_kernel_state(&db, &child), "created");
    assert!(db.delivery_checklist(&child).unwrap().is_empty());
}
