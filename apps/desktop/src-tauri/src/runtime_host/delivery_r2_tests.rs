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
    assert!(matches!(state, DeliveryStop::Exhausted { .. }));
    let finding: Value = serde_json::from_str(&state.items()[0].finding_json).unwrap();
    assert_eq!(finding["verificationStatus"], "unverified");
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
    assert_eq!(
        f.db.delivery_item_requirements(&next.run.id, &slot.item_key)
            .unwrap(),
        source_requirements
    );
    assert_eq!(
        before,
        frozen_delivery_reference(&f.db, &next.run.id, &f.context()).unwrap()
    );
    assert!(f
        .db
        .bind_delivery_item(&next.run.id, &slot.item_key, "tmp/foreign.docx", None)
        .is_err());
    assert!(f
        .db
        .delivery_checklist(&next.run.id)
        .unwrap()
        .iter()
        .find(|i| i.item_key == slot.item_key)
        .unwrap()
        .target_path
        .is_none());
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
    assert!(matches!(stop, DeliveryStop::Exhausted { .. }));
    assert!(stop.items().iter().all(|item| !item.passed));
    assert!(stop.repair_findings().is_none());
    assert_eq!(
        f.db.delivery_item_requirements(&f.run, &slot.item_key)
            .unwrap(),
        source_requirements,
        "source rows stay intact"
    );
}
