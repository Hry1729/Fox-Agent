use super::*;
use crate::database::{DeliveryChecklistSeed, DeliveryRequirement, RequirementKind};
use decision::{DeliveryClass, FactClass};

struct Fixture {
    root: PathBuf,
    database: Database,
    run: String,
    item: String,
    spec: Value,
    finding: Value,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "p2c2-{label}-{}-{}",
            std::process::id(),
            NEXT_EXECUTION.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("in")).unwrap();
        std::fs::write(root.join("in/a.txt"), b"alpha").unwrap();
        std::fs::write(root.join("in/b.txt"), b"beta").unwrap();
        std::fs::write(root.join("out.txt"), b"a.txt\nalpha\n\nb.txt\nbeta").unwrap();
        let database = Database::open(root.join("fox.db")).unwrap();
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some("read_only"),
            )
            .unwrap();
        let run = database
            .create_run(&conversation.id, "合并授权来源", None)
            .unwrap()
            .run
            .id;
        use fox_engine_protocol::{
            ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor,
            RunControlBinding, TimeBudgets,
        };
        let permission = FrozenPermission {
            mode: PermissionMode::Allow,
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
            approval_epoch: None,
        };
        database
            .freeze_run_control(&RunControlBinding {
                schema_version: 1,
                run_id: run.clone(),
                conversation_id: conversation.id,
                engine_id: "pi".into(),
                execution_profile_id: "legacy".into(),
                authority: ExecutionAuthority::Authoritative,
                read_only_executor: ResourceExecutor::Rust,
                permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
                permission,
                budgets: TimeBudgets::default(),
            })
            .unwrap();
        let source = |name: &str, bytes: &[u8]| json!({"path":name,"sha256":digest(bytes),"bytes":bytes.len(),"firstLine":1,"lastLine":1});
        let spec = json!({"schemaVersion":2,"envelopeVersion":2,"checkerVersion":2,"ruleId":"content:concat",
            "rule":{"type":"concat","sources":[source("in/a.txt",b"alpha"),source("in/b.txt",b"beta")],
                "selected":["in/a.txt","in/b.txt"],"selection_order":"bytes_ascending","tie_order":"filename_ascending","merge_order":"filename_ascending","read_only":true}});
        let finding = json!({"checkedHash":digest(b"a.txt\nalpha\n\nb.txt\nbeta"),"boundPath":"out.txt","sourceRunId":run});
        let mut fixture = Self {
            root,
            database,
            run,
            item: "file:out.txt".into(),
            spec,
            finding,
        };
        fixture.seed();
        fixture
    }
    fn seed(&mut self) {
        let seed = DeliveryChecklistSeed {
            item_key: self.item.clone(),
            target_path: Some("out.txt".into()),
            artifact_id: None,
            display_name: "合并产物".into(),
            checks: vec![],
            requirements: vec![DeliveryRequirement {
                item_key: Some(self.item.clone()),
                id: "content:concat".into(),
                kind: RequirementKind::ContentContract {
                    spec: self.spec.clone(),
                },
                source_text: "冻结合同".into(),
            }],
        };
        self.database
            .seed_delivery_checklist(&self.run, &[seed], crate::database::now_ms())
            .unwrap();
    }
    fn classify(&self, repair: bool) -> decision::DecisionResult {
        super::super::classify_v2_requirement(
            &self.database,
            &self.run,
            &self.item,
            &self.spec,
            Some(&self.root.join("out.txt")),
            &self.finding,
            repair,
        )
    }
    fn replace_candidate(&mut self, bytes: &[u8]) {
        std::fs::write(self.root.join("out.txt"), bytes).unwrap();
        self.finding["checkedHash"] = json!(digest(bytes));
    }
    fn set_rule(&mut self, rule: Value) {
        self.spec["rule"] = rule;
        let requirements = crate::canonical_json::to_string(&[DeliveryRequirement {
            item_key: Some(self.item.clone()),
            id: "content:concat".into(),
            kind: RequirementKind::ContentContract {
                spec: self.spec.clone(),
            },
            source_text: "修改冻结合同".into(),
        }])
        .unwrap();
        self.database.with_connection(|connection| {connection.execute("UPDATE delivery_checklist_items SET requirements_json=?3 WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![self.run,self.item,requirements])?;Ok(())}).unwrap();
    }
}

fn audit(result: &decision::DecisionResult) -> Value {
    serde_json::from_str(
        result
            .facts
            .iter()
            .find(|fact| fact.check.as_deref() == Some("delivery.execution"))
            .unwrap()
            .detail
            .as_deref()
            .unwrap(),
    )
    .unwrap()
}
fn has_reason(result: &decision::DecisionResult, code: &str) -> bool {
    result
        .facts
        .iter()
        .any(|fact| fact.reason_code.as_deref() == Some(code))
}
fn coverage_detail(result: &decision::DecisionResult) -> String {
    result
        .facts
        .iter()
        .find(|fact| fact.check.as_deref() == Some("delivery.coverage"))
        .unwrap()
        .detail
        .clone()
        .unwrap()
}

#[test]
fn c2_actual_crate_cfg_and_overflow_checks_are_recorded() {
    let expected = std::env::var("P2C2_EXPECT_DEBUG_ASSERTIONS").ok();
    println!(
        "P2C2_ACTUAL_CRATE_CFG debug_assertions={} expected={expected:?}",
        cfg!(debug_assertions)
    );
    if let Some(expected) = expected {
        assert_eq!(cfg!(debug_assertions).to_string(), expected);
        let overflow = std::panic::catch_unwind(|| {
            let value = std::hint::black_box(u8::MAX);
            value + 1
        });
        assert!(
            overflow.is_err(),
            "专项构建只能关闭debug assertions，overflow checks必须保持开启"
        );
    }
}

#[test]
fn c2_complete_concat_confirms_at_the_actual_entry_with_full_identity() {
    let fixture = Fixture::new("confirm");
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Confirmed, "{:?}", result.facts);
    assert!(result.facts.iter().any(|fact| fact.check.as_deref()
        == Some(leaf::check_name::TEXT_CONCAT)
        && fact.class == FactClass::Passed));
    let proof = audit(&result);
    assert_eq!(proof["runId"], fixture.run);
    assert_eq!(proof["itemKey"], fixture.item);
    assert_eq!(proof["candidate"]["sha256"], fixture.finding["checkedHash"]);
    assert_eq!(proof["sources"].as_array().unwrap().len(), 2);
    assert_eq!(
        proof["checks"].as_array().unwrap().len(),
        1,
        "one whole-set concat obligation"
    );
    assert_eq!(
        proof["specHash"],
        digest(&crate::canonical_json::to_vec(&fixture.spec).unwrap())
    );
    assert_eq!(proof["rowBound"], true);
    assert_eq!(proof["checks"][0]["checkerVersion"], 2);
    assert_eq!(proof["checks"][0]["ruleId"], "content:concat");
    let again = fixture.classify(true);
    assert_ne!(audit(&again)["executionId"], proof["executionId"]);
}

#[test]
fn c2_actual_mismatch_is_repair_needed_then_definite_failure() {
    let mut fixture = Fixture::new("mismatch");
    fixture.replace_candidate(b"wrong");
    let repair = fixture.classify(true);
    assert_eq!(
        repair.class,
        DeliveryClass::RepairNeeded,
        "{:?}",
        repair.facts
    );
    assert!(repair.has_class(FactClass::Failed));
    assert_eq!(
        fixture.classify(false).class,
        DeliveryClass::DefiniteFailure
    );
}

#[test]
fn c2_source_missing_is_unconfirmed_and_the_leaf_never_ran() {
    let fixture = Fixture::new("missing");
    std::fs::remove_file(fixture.root.join("in/b.txt")).unwrap();
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    assert!(has_reason(&result, leaf::reason_code::EVIDENCE_UNAVAILABLE));
    assert!(audit(&result)["checks"].as_array().unwrap().is_empty());
    assert!(coverage_detail(&result).contains("concat.all_sources"));
}

#[test]
fn c2_candidate_and_source_drift_are_distinct_refusals() {
    let fixture = Fixture::new("candidate-drift");
    std::fs::write(fixture.root.join("out.txt"), b"changed").unwrap();
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Refused);
    assert!(has_reason(&result, leaf::reason_code::CANDIDATE_HASH_DRIFT));
    let fixture = Fixture::new("source-drift");
    std::fs::write(fixture.root.join("in/a.txt"), b"changed").unwrap();
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Refused);
    assert!(has_reason(
        &result,
        leaf::reason_code::security::SOURCE_IDENTITY_MISMATCH
    ));
}

#[test]
fn c2_actual_read_budget_is_unconfirmed_without_a_fabricated_pass() {
    let fixture = Fixture::new("read-budget");
    let result = execute_with_limits(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        &fixture.spec,
        Some(&fixture.root.join("out.txt")),
        &fixture.finding,
        true,
        4,
        SOURCE_LIMIT,
        CheckLimits::default(),
    );
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    assert!(result.facts.iter().any(|fact| fact.limit == Some(4)));
    assert!(has_reason(&result, leaf::reason_code::BUDGET_EXCEEDED));
    assert!(audit(&result)["checks"].as_array().unwrap().is_empty());
}

#[test]
fn c2_actual_leaf_budget_is_preserved_as_resource_not_invalid() {
    let fixture = Fixture::new("leaf-budget");
    let mut limits = CheckLimits::default();
    limits.max_scan_bytes = 1;
    let result = execute_with_limits(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        &fixture.spec,
        Some(&fixture.root.join("out.txt")),
        &fixture.finding,
        true,
        MAX_VERIFY_BYTES,
        SOURCE_LIMIT,
        limits,
    );
    assert_eq!(
        result.class,
        DeliveryClass::Unconfirmed,
        "{:?}",
        result.facts
    );
    assert!(has_reason(&result, leaf::reason_code::BUDGET_EXCEEDED));
    assert_eq!(audit(&result)["checks"].as_array().unwrap().len(), 1);
}

#[test]
fn c2_after_check_candidate_and_source_changes_invalidate_this_execution() {
    for source in [false, true] {
        let fixture = Fixture::new(if source {
            "source-mid-check"
        } else {
            "candidate-mid-check"
        });
        let path = fixture
            .root
            .join(if source { "in/a.txt" } else { "out.txt" });
        test_after_check(move |_, _| std::fs::write(path, b"changed after real leaf").unwrap());
        let result = fixture.classify(true);
        assert_eq!(result.class, DeliveryClass::Refused);
        assert!(has_reason(
            &result,
            if source {
                leaf::reason_code::security::SOURCE_IDENTITY_MISMATCH
            } else {
                leaf::reason_code::CANDIDATE_HASH_DRIFT
            }
        ));
        assert_eq!(
            audit(&result)["checks"][0]["state"],
            "passed",
            "the real leaf ran before the drift"
        );
    }
}

#[test]
fn c2_after_check_revocation_is_a_real_authorization_refusal() {
    let fixture = Fixture::new("revoke");
    test_after_check(|database, run| {
        let binding = database.run_control_binding(run).unwrap().unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversations SET project_id=NULL, project_root=NULL WHERE id=?1",
                    [binding.conversation_id],
                )?;
                Ok(())
            })
            .unwrap();
    });
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Refused, "{:?}", result.facts);
    assert!(has_reason(
        &result,
        "authorization_changed_during_verification"
    ));
}

#[test]
fn c2_row_run_item_rule_and_candidate_cannot_borrow_a_previous_execution() {
    let mut fixture = Fixture::new("foreign-context");
    let mut other_binding = fixture
        .database
        .run_control_binding(&fixture.run)
        .unwrap()
        .unwrap();
    let active_run_error = rusqlite::Error::InvalidQuery.to_string();
    let before_classify = fixture
        .database
        .create_run(
            &other_binding.conversation_id,
            "active-run control before classify",
            None,
        )
        .err()
        .unwrap();
    assert_eq!(before_classify, active_run_error);
    assert_eq!(fixture.classify(true).class, DeliveryClass::Confirmed);
    for item in ["", "file:foreign", "file:out\n.txt"] {
        let result = super::super::classify_v2_requirement(
            &fixture.database,
            &fixture.run,
            item,
            &fixture.spec,
            Some(&fixture.root.join("out.txt")),
            &fixture.finding,
            true,
        );
        assert_eq!(result.class, DeliveryClass::Unconfirmed);
        assert_eq!(audit(&result)["rowBound"], false);
        assert!(audit(&result)["checks"].as_array().unwrap().is_empty());
    }
    let mut other = fixture.spec.clone();
    other["ruleId"] = json!("content:other");
    let result = super::super::classify_v2_requirement(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        &other,
        Some(&fixture.root.join("out.txt")),
        &fixture.finding,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    let after_classify = fixture
        .database
        .create_run(
            &other_binding.conversation_id,
            "active-run control after classify",
            None,
        )
        .err()
        .unwrap();
    assert_eq!(after_classify, active_run_error);
    eprintln!("P2C2 ACTIVE_RUN_CONTROL before={before_classify:?} after={after_classify:?}");
    let other_conversation = fixture
        .database
        .create_conversation(
            fixture.database.default_agent_id(),
            None,
            Some(fixture.root.to_str().unwrap()),
            Some("read_only"),
        )
        .unwrap();
    let other_run = fixture
        .database
        .create_run(&other_conversation.id, "另一个真实核验 Run", None)
        .unwrap()
        .run
        .id;
    other_binding.run_id = other_run.clone();
    other_binding.conversation_id = other_conversation.id;
    fixture.database.freeze_run_control(&other_binding).unwrap();
    assert_eq!(
        authorized_delivery_root_typed(&fixture.database, &other_run).unwrap(),
        authorized_delivery_root_typed(&fixture.database, &fixture.run).unwrap()
    );
    assert!(fixture
        .database
        .delivery_checklist(&other_run)
        .unwrap()
        .is_empty());
    let other_result = super::super::classify_v2_requirement(
        &fixture.database,
        &other_run,
        &fixture.item,
        &fixture.spec,
        Some(&fixture.root.join("out.txt")),
        &fixture.finding,
        true,
    );
    assert_eq!(other_result.class, DeliveryClass::Unconfirmed);
    let other_proof = audit(&other_result);
    assert_eq!(other_proof["runId"], other_run);
    assert_eq!(other_proof["rowBound"], false);
    assert!(
        other_proof["candidate"].is_object(),
        "the real candidate read succeeded before the row gate"
    );
    assert_eq!(other_proof["sources"].as_array().unwrap().len(), 2);
    assert!(other_proof["checks"].as_array().unwrap().is_empty());
    fixture.finding["sourceRunId"] = json!(other_run);
    let fresh = fixture.classify(true);
    assert_eq!(fresh.class, DeliveryClass::Confirmed);
    assert_eq!(audit(&fresh)["runId"], fixture.run);
    assert_eq!(audit(&fresh)["candidate"]["sourceRunId"], other_run);
    assert_eq!(
        audit(&fresh)["checks"][0]["state"],
        "passed",
        "fresh controlled leaf evidence, no borrowed outcome"
    );
    fixture.spec["checkerVersion"] = json!(1);
    assert_eq!(fixture.classify(true).class, DeliveryClass::Invalid);
}

#[test]
fn c2_no_receipt_hash_stays_unconfirmed_even_with_bound_inputs() {
    let mut fixture = Fixture::new("receipt-missing");
    fixture
        .finding
        .as_object_mut()
        .unwrap()
        .remove("checkedHash");
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    assert!(has_reason(&result, leaf::reason_code::EVIDENCE_UNAVAILABLE));
    assert!(audit(&result)["checks"].as_array().unwrap().is_empty());
}

#[test]
fn c2_empty_partial_unsupported_selection_and_readonly_are_not_vacuous() {
    for variant in 0..4 {
        let mut fixture = Fixture::new(&format!("concat-plan-{variant}"));
        let mut rule = fixture.spec["rule"].clone();
        match variant {
            0 => {
                rule["sources"] = json!([]);
                rule["selected"] = json!([]);
            }
            1 => rule["selected"] = json!(["in/a.txt", "in/missing.txt"]),
            2 => rule["selection_order"] = json!("unknown_order"),
            _ => {
                let actual = b"a.txt\nalpha\n\nb.txt\nbeta";
                rule["sources"] = json!([{"path":"out.txt","sha256":digest(actual),"bytes":actual.len(),"firstLine":1,"lastLine":5}]);
                rule["selected"] = json!(["out.txt"]);
            }
        }
        fixture.set_rule(rule);
        let result = fixture.classify(true);
        assert_eq!(
            result.class,
            if variant == 3 {
                DeliveryClass::Refused
            } else {
                DeliveryClass::Unconfirmed
            },
            "variant {variant}: {:?}",
            result.facts
        );
        assert!(coverage_detail(&result).contains("concat.all_sources"));
        if variant == 3 {
            assert!(has_reason(
                &result,
                leaf::reason_code::security::READONLY_INPUT_WRITE
            ));
        }
    }
}

#[test]
fn c2_datasource_real_table_structure_never_discharges_other_metrics_or_fields() {
    let mut fixture = Fixture::new("table");
    fixture.replace_candidate(b"date,value\n2026-01-01,2\n");
    std::fs::rename(fixture.root.join("out.txt"), fixture.root.join("out.csv")).unwrap();
    let source = fixture.spec["rule"]["sources"][0].clone();
    let rule = json!({"type":"dataSource","source":source,"sheet":"SheetA","headers":["date","value"],"date_required":true,
        "metrics":[{"column":"value","operation":"sum"},{"column":"other","operation":"mean"}]});
    fixture.set_rule(rule);
    // A second real row binds the CSV candidate, with its own frozen rule.
    fixture.item = "file:out.csv".into();
    fixture
        .database
        .seed_delivery_checklist(
            &fixture.run,
            &[DeliveryChecklistSeed {
                item_key: fixture.item.clone(),
                target_path: Some("out.csv".into()),
                artifact_id: None,
                display_name: "CSV".into(),
                checks: vec![],
                requirements: vec![DeliveryRequirement {
                    item_key: Some(fixture.item.clone()),
                    id: "content:concat".into(),
                    kind: RequirementKind::ContentContract {
                        spec: fixture.spec.clone(),
                    },
                    source_text: "表格合同".into(),
                }],
            }],
            crate::database::now_ms(),
        )
        .unwrap();
    let result = super::super::classify_v2_requirement(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        &fixture.spec,
        Some(&fixture.root.join("out.csv")),
        &fixture.finding,
        true,
    );
    assert_eq!(
        result.class,
        DeliveryClass::Unconfirmed,
        "{:?}",
        result.facts
    );
    assert_eq!(
        audit(&result)["checks"][0]["check"],
        leaf::check_name::TABLE_COLUMNS
    );
    assert_eq!(audit(&result)["checks"][0]["state"], "passed");
    let missing = coverage_detail(&result);
    for field in [
        "data.source_lineage",
        "data.expected_headers",
        "data.sheet",
        "data.date",
        "data.metric.0:value:sum",
        "data.metric.1:other:mean",
    ] {
        assert!(missing.contains(field), "{field}: {missing}");
    }
}

#[test]
fn c2_unimplemented_variants_keep_explicit_complete_obligations() {
    for (kind, expected) in [
        ("factQualifiers", "fact.qualifiers"),
        ("forbiddenMention", "forbidden.mention"),
        ("forbiddenAvailability", "forbidden.availability"),
        ("unbound", "contract.unbound"),
    ] {
        let mut fixture = Fixture::new(kind);
        let source = fixture.spec["rule"]["sources"][0].clone();
        let rule = match kind {
            "factQualifiers" => json!({"type":kind,"sources":[source]}),
            "unbound" => json!({"type":kind,"reason":"来源未绑定"}),
            _ => json!({"type":kind,"source":source,"literal":"已上线"}),
        };
        fixture.set_rule(rule);
        let result = fixture.classify(true);
        assert_eq!(
            result.class,
            DeliveryClass::Unconfirmed,
            "{:?}",
            result.facts
        );
        assert!(coverage_detail(&result).contains(expected));
        assert!(audit(&result)["checks"].as_array().unwrap().is_empty());
    }
}

#[test]
fn c2_contract_change_during_the_real_leaf_cannot_keep_the_receipt() {
    let fixture = Fixture::new("contract-drift");
    let item = fixture.item.clone();
    let mut other = fixture.spec.clone();
    other["ruleId"] = json!("replacement");
    test_after_check(move |database, run| {
        let requirements = crate::canonical_json::to_string(&[DeliveryRequirement {
            item_key: Some(item.clone()),
            id: "content:concat".into(),
            kind: RequirementKind::ContentContract { spec: other },
            source_text: "重写合同".into(),
        }])
        .unwrap();
        database.with_connection(|connection|{connection.execute("UPDATE delivery_checklist_items SET requirements_json=?3 WHERE run_id=?1 AND item_key=?2",rusqlite::params![run,item,requirements])?;Ok(())}).unwrap();
    });
    let result = fixture.classify(true);
    assert_eq!(
        result.class,
        DeliveryClass::Unconfirmed,
        "{:?}",
        result.facts
    );
    assert_eq!(audit(&result)["rowBound"], false);
    assert!(result.facts.iter().any(|fact| fact
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("合同身份改变"))));
}

#[test]
fn c2_target_path_rebinding_after_a_real_pass_invalidates_the_receipt() {
    let fixture = Fixture::new("target-rebind");
    test_after_check(|database, run| {
        database.with_connection(|connection| {
        connection.execute("UPDATE delivery_checklist_items SET target_path='different.txt' WHERE run_id=?1",[run])?;Ok(())}).unwrap()
    });
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    let proof = audit(&result);
    assert_eq!(proof["rowBound"], false);
    assert_eq!(proof["checks"][0]["state"], "passed");
    assert_eq!(proof["obligations"][0]["discharged"], false);
}

#[test]
fn c2_source_count_is_not_misreported_as_a_byte_budget() {
    let mut fixture = Fixture::new("source-count");
    let mut rule = fixture.spec["rule"].clone();
    rule["sources"]=Value::Array((0..MAX_SOURCES+1).map(|index|json!({"path":format!("in/{index}.txt"),"sha256":digest(b"x"),"bytes":1,"firstLine":1,"lastLine":1})).collect());
    fixture.set_rule(rule);
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    let fact = result
        .facts
        .iter()
        .find(|fact| fact.check.as_deref() == Some("delivery.source_count"))
        .unwrap();
    assert_eq!(fact.limit, None);
    assert_eq!(fact.expected.as_deref(), Some("sources <= 256"));
    assert_eq!(fact.actual.as_deref(), Some("257"));
    assert!(audit(&result)["sources"].as_array().unwrap().is_empty());
}

#[test]
fn c2_concat_source_line_fields_are_attribution_metadata_not_a_slice() {
    let mut fixture = Fixture::new("line-attribution");
    let mut rule = fixture.spec["rule"].clone();
    rule["sources"][0]["firstLine"] = json!(20);
    rule["sources"][0]["lastLine"] = json!(30);
    fixture.set_rule(rule);
    let result = fixture.classify(true);
    assert_eq!(result.class, DeliveryClass::Confirmed);
    assert_eq!(audit(&result)["sources"][0]["firstLine"], 20);
    assert_eq!(
        audit(&result)["sources"][0]["actualBytes"],
        5,
        "Concat always checks the complete frozen bytes"
    );
}
