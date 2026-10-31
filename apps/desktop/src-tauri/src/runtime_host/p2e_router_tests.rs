//! Package 2-E: the production version router and the producer gate it reads.
//!
//! These facilities cover the four things the card asks a unit test to hold down
//! before the native acceptance runs, because each is a place where a mistake
//! would stay invisible until a user's Run behaved wrongly:
//!
//! 1. **the producer gate** — off by default, off for an unrecognized value, and
//!    on only for an authorized rule, decided in one pure function;
//! 2. **the frozen record is the router's input** — a row already frozen as v2 is
//!    identified as v2 however the switching process is configured, so a restart,
//!    a continuation or a switch-off cannot silently re-read it as v1;
//! 3. **one fold over a mixed Run** — a v1 sibling's already-computed verdict is
//!    folded into the *same* verdict as the v2 row's, so neither version's
//!    priority can be applied in isolation;
//! 4. **no envelope on a v1 row** — the byte-level invariant that keeps a legacy
//!    record from being re-identified as a version it is not.
//!
//! The switch is driven through `delivery_v2::with_test_enablement`, a per-thread
//! override, rather than by mutating `FOX_DELIVERY_V2`: the real switch is
//! process-scoped, so a test that set the environment variable would be changing
//! state every concurrently running test shares — and any fixture that happened to
//! freeze an authorized rule inside that window would quietly write a v2 record.
//!
//! Rows are located by their **frozen identity** rather than by a hard-coded item
//! key. The producer decides a text target's key from the task, so naming one here
//! would make this file a second, silently-drifting opinion about what the producer
//! binds.

use super::*;
use crate::kernel::{self, CancellationRegistry, TestClock};
use fox_engine_protocol::{
    ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
    TimeBudgets,
};

/// The task whose frozen contract is a legal v2 `Concat` under this wiring.
///
/// Stated once so the shape under test is pinned. Two properties make it the
/// fixture the mixed cases need rather than any concat task:
///
/// * the merge clause is a byte-ascending selection from the initial directory's
///   `.txt` files merged by filename, which is the only rule the first release
///   authorizes; and
/// * it names a **second** deliverable whose extension is neither `txt` nor `md`,
///   so the merge output stays the *unique* text target and the second row is a
///   real v1 sibling. A second `.txt`/`.md` deliverable would make the target set
///   ambiguous and the producer would freeze `Unbound` instead — a v1 row, which
///   is exactly the case this fixture must not accidentally become.
const CONCAT_TASK: &str = "在初始目录的 .txt 文件中，按字节数从小到大选出 2 个最小文件，大小相同按文件名升序选取。再按文件名升序合并为 joined.txt。每段先写完整文件名，下一行保留全部内容；两段之间恰好一个空行。不要修改原文件。另外生成 summary.json，写明合并的文件名清单。";

/// The second deliverable the task above names, kept v1 by construction.
const SIBLING_ROW: &str = "summary.json";

/// The files the task above selects from, written before every freeze.
const INPUTS: [(&str, &str); 3] = [("alpha.txt", "aa"), ("zeta.txt", "z"), ("middle.txt", "mmm")];

fn write_inputs(root: &Path) {
    for (name, body) in INPUTS {
        fs::write(root.join(name), body).unwrap();
    }
}

fn concat_contract() -> content_requirements::Contract {
    content_requirements::Contract::Concat {
        sources: vec![],
        selected: vec!["alpha.txt".into(), "zeta.txt".into()],
        selection_order: "bytes_ascending".into(),
        tie_order: "filename_ascending".into(),
        merge_order: "filename_ascending".into(),
        read_only: false,
    }
}

/// A real run whose frozen rows the router can be pointed at.
///
/// Reuses the 2-D/r3 harness shape deliberately: `evaluate_stop` reads the same
/// tables those fixtures already exercise, so a fixture of another shape would be
/// testing a different call path.
#[allow(dead_code)]
struct Fixture {
    db: Database,
    root: PathBuf,
    run: String,
    clock: TestClock,
    cancellation: CancellationRegistry,
}

impl Fixture {
    fn new(task: &str) -> Self {
        let root = std::env::temp_dir().join(format!("fox-p2e-{}", uuid::Uuid::new_v4()));
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
            executor_route: None,
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
        let model = crate::kernel_model_config::KernelModelConfig {
            engine_id: "pi".into(),
            native_adapter: None,
            execution_profile_id: "legacy".into(),
            model_service: json!({"apiType":"faux","modelId":"p2e-fixture","baseUrl":"http://localhost","fauxResponses":["unused synthetic response"]}),
            system_prompt: "Synthetic 2-E fixture.".into(),
            proposal_tools: ["read", "write_file", "edit_file"]
                .into_iter()
                .map(|name| json!({"name":name,"description":"synthetic 2-E","parameters":{"type":"object","properties":{}}}))
                .collect(),
        };
        let prompt_hash = model.hash().unwrap();
        let config = kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "p2e-synthetic-manifest".into(),
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
            &crate::canonical_json::to_string(&config).unwrap(),
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
            tool_names: ["read", "write_file", "edit_file"]
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
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::create_dir_all(root.join("storage")).unwrap();
        Self { db, root, run, clock, cancellation }
    }

    fn context(&self) -> Value {
        json!({"deliverableRoot":"fox/session-results","deliverableFolder":"session-results"})
    }

    /// Freeze this Run's checklist from its task text, through the real producer.
    fn freeze(&self, task: &str) {
        freeze_task_delivery(&self.db, &self.run, task, &self.context(), self.root.to_str())
            .unwrap();
    }

    fn stop(&self) -> DeliveryStop {
        evaluate_stop(&self.db, self.root.to_str(), &self.run).unwrap()
    }

    /// The frozen requirement column of one row, exactly as the router reads it.
    fn requirements_json(&self, item_key: &str) -> Option<String> {
        self.db
            .delivery_checklist_requirements(&self.run)
            .unwrap()
            .into_iter()
            .find(|(key, _)| key == item_key)
            .and_then(|(_, raw)| raw)
    }

    /// Every frozen row's key and identity, in the router's own order.
    fn identities(&self) -> Vec<(String, p2b_contract_identity::ContractIdentity)> {
        self.db
            .delivery_checklist_requirements(&self.run)
            .unwrap()
            .into_iter()
            .map(|(key, raw)| {
                (
                    key,
                    p2b_contract_identity::classify_requirements(raw.as_deref()),
                )
            })
            .collect()
    }

    /// The one row the producer froze as v2, if any.
    ///
    /// Freezing more than one v2 row is not a defect, but the first release
    /// authorizes one rule and these cases reason about a single typed row — so a
    /// second one is reported rather than silently picked.
    fn v2_row_key(&self) -> Option<String> {
        let v2: Vec<String> = self
            .identities()
            .into_iter()
            .filter(|(_, identity)| {
                matches!(identity, p2b_contract_identity::ContractIdentity::HostV2)
            })
            .map(|(key, _)| key)
            .collect();
        assert!(
            v2.len() <= 1,
            "the first release authorizes one rule, so at most one row is v2: {v2:?}"
        );
        v2.into_iter().next()
    }

    /// The frozen v2 specs a row carries, decoded through the real strict reader.
    fn v2_specs(&self, item_key: &str) -> Vec<Value> {
        self.db
            .delivery_item_requirements(&self.run, item_key)
            .unwrap()
            .into_iter()
            .filter_map(|requirement| match requirement.kind {
                StoredRequirementKind::ContentContract { spec } => matches!(
                    content_requirements::decode_spec(&spec),
                    content_requirements::DecodedSpec::V2(_)
                )
                .then_some(spec),
                _ => None,
            })
            .collect()
    }
}

/// A fixture whose producer ran with the switch explicitly on.
///
/// Every promised deliverable is really written, so each row's leaf has an artifact
/// to verify rather than failing on a missing one: a missing v2 artifact would make
/// the row `Unconfirmed` for a reason unrelated to the wiring.
fn frozen_with_v2() -> Fixture {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = Fixture::new(CONCAT_TASK);
    write_inputs(&fixture.root);
    with_test_enablement(V2Enablement::On, || fixture.freeze(CONCAT_TASK));
    fs::write(fixture.root.join("joined.txt"), "alpha.txt\naa\n\nzeta.txt\nz").unwrap();
    fs::write(
        fixture.root.join(SIBLING_ROW),
        r#"{"files":["alpha.txt","zeta.txt"]}"#,
    )
    .unwrap();
    fixture
}

/// A fixture whose producer ran with no switch at all — the normal start.
fn frozen_without_v2() -> Fixture {
    let fixture = Fixture::new(CONCAT_TASK);
    write_inputs(&fixture.root);
    fixture.freeze(CONCAT_TASK);
    fixture
}

// ---------------------------------------------------------------------------
// 1. The producer gate
// ---------------------------------------------------------------------------

/// The generation decision is one pure function of the resolved switch state, and
/// each state produces exactly the record the card requires.
///
/// Pure because the alternative — consulting the process environment here — would
/// make this matrix order-dependent with every other test in the crate, and would
/// leave "which record was frozen" answerable only by running a whole process.
#[test]
fn the_generation_decision_is_a_pure_function_of_the_switch_state() {
    use content_requirements::delivery_v2::{V2Enablement, spec_for_contract};
    let concat = concat_contract();
    let unbound = content_requirements::Contract::Unbound { reason: "x".into() };
    let rule = serde_json::to_value(&concat).unwrap();

    // On + authorized: the v2 envelope, whose rule payload is the untouched v1 rule.
    let on = spec_for_contract(&concat, rule.clone(), "content:concat", &V2Enablement::On);
    assert_eq!(on["schemaVersion"], json!(2));
    assert_eq!(on["envelopeVersion"], json!(2));
    assert_eq!(on["checkerVersion"], json!(2));
    assert_eq!(
        on["rule"], rule,
        "the rule payload must be the untouched v1 rule"
    );
    assert!(matches!(
        content_requirements::decode_spec(&on),
        content_requirements::DecodedSpec::V2(_)
    ));

    // Off, whatever the rule: legal v1, and no v2 envelope key at all — not even a
    // null one, which a reader could mistake for a generation marker.
    for (label, contract, payload) in [
        ("concat", &concat, rule.clone()),
        ("unbound", &unbound, serde_json::to_value(&unbound).unwrap()),
    ] {
        let spec = spec_for_contract(contract, payload, "content:concat", &V2Enablement::Off);
        assert_eq!(spec["schemaVersion"], json!(1), "{label}: off writes v1");
        assert!(
            spec.get("envelopeVersion").is_none(),
            "{label}: an off record carries no v2 envelope key"
        );
        assert!(matches!(
            content_requirements::decode_spec(&spec),
            content_requirements::DecodedSpec::V1(..)
        ));
    }

    // On but **not** authorized: still v1. This is the allowlist, and it is the
    // difference between "v2 is enabled" and "v2 may be produced for this rule".
    let unauthorized =
        spec_for_contract(&unbound, rule.clone(), "content:unbound", &V2Enablement::On);
    assert_eq!(
        unauthorized["schemaVersion"],
        json!(1),
        "an unauthorized rule must not become v2 merely because the gate is on"
    );

    // A value the gate does not understand behaves exactly as off.
    let spec = spec_for_contract(
        &concat,
        rule,
        "content:concat",
        &V2Enablement::Invalid("enabled".into()),
    );
    assert_eq!(spec["schemaVersion"], json!(1));
}

/// The switch vocabulary itself: only an explicit affirmative enables, and a value
/// the gate does not understand must not be laundered into a deliberate "off".
#[test]
fn only_an_explicit_affirmative_enables_and_a_typo_stays_closed() {
    use content_requirements::delivery_v2::{V2Enablement, classify};
    for value in ["1", "true", "TRUE", "on", "yes", " on "] {
        assert_eq!(classify(value), V2Enablement::On, "{value:?}");
        assert!(classify(value).permits_v2(), "{value:?}");
    }
    for value in ["", "0", "false", "off", "no", "OFF"] {
        assert_eq!(classify(value), V2Enablement::Off, "{value:?}");
        assert!(!classify(value).permits_v2(), "{value:?}");
    }
    for value in ["2", "enabled", "tru", "y", "ope"] {
        let state = classify(value);
        assert!(
            matches!(state, V2Enablement::Invalid(_)),
            "{value:?} must be Invalid, not silently Off"
        );
        assert!(!state.permits_v2(), "{value:?} must not enable the gate");
    }
}

/// The real producer, with **no** switch set, freezes no v2 record for the very
/// task the allowlist would authorize.
///
/// This is the default-off evidence at the row level: it goes through
/// `freeze_task_delivery` and `add`, the production write path, not the pure helper.
#[test]
fn the_normal_producer_writes_no_v2_record_even_for_the_authorized_rule() {
    let fixture = frozen_without_v2();
    let rows = fixture.db.delivery_checklist(&fixture.run).unwrap();
    assert!(!rows.is_empty(), "the real producer froze a checklist");
    for row in &rows {
        let raw = fixture.requirements_json(&row.item_key);
        assert!(
            matches!(
                p2b_contract_identity::classify_requirements(raw.as_deref()),
                p2b_contract_identity::ContractIdentity::LegacyV1
            ),
            "row {} must be a legal v1 row while the switch is absent: {raw:?}",
            row.item_key
        );
        assert!(
            fixture.v2_specs(&row.item_key).is_empty(),
            "row {} must carry no v2 record when the process never enabled v2",
            row.item_key
        );
    }
    assert!(
        fixture.v2_row_key().is_none(),
        "no row may be v2 on a normal start"
    );

    // And the v1 lane still decides it, unchanged: an all-v1 Run is never typed.
    let stop = fixture.stop();
    assert!(
        !matches!(stop, DeliveryStop::Typed(_)),
        "an all-v1 Run must not be routed to the typed lane"
    );

    let _ = fs::remove_dir_all(&fixture.root);
}

/// With the switch explicitly on, the **same** task's authorized row is frozen as
/// v2 while every other row stays v1.
///
/// The contrast with the test above is the whole point: one task, one producer, and
/// only the allowlisted rule changes generation. It also pins the first-release
/// scope — the material and other rules this task freezes must not become v2.
#[test]
fn an_enabled_producer_writes_v2_for_the_authorized_rule_and_v1_for_everything_else() {
    let fixture = frozen_with_v2();
    let key = fixture
        .v2_row_key()
        .expect("the authorized concat rule must be frozen as v2 when the gate is on");

    let specs = fixture.v2_specs(&key);
    assert_eq!(
        specs.len(),
        1,
        "the authorized row must carry exactly one v2 record: {:?}",
        fixture.requirements_json(&key)
    );
    assert_eq!(specs[0]["schemaVersion"], json!(2));
    assert_eq!(specs[0]["envelopeVersion"], json!(2));
    assert_eq!(specs[0]["checkerVersion"], json!(2));
    assert!(
        specs[0]["ruleId"].as_str().is_some(),
        "a v2 record must name the requirement it came from"
    );
    // The authorized rule is the concat contract and nothing else.
    assert_eq!(
        specs[0]["rule"]["type"],
        json!("concat"),
        "the first release authorizes exactly the concat contract"
    );

    // Every other row keeps the v1 behaviour it has today.
    for (row_key, identity) in fixture.identities() {
        if row_key == key {
            continue;
        }
        assert!(
            matches!(identity, p2b_contract_identity::ContractIdentity::LegacyV1),
            "row {row_key} must stay v1: the first release authorizes Concat only"
        );
        assert!(
            fixture.v2_specs(&row_key).is_empty(),
            "row {row_key} must carry no v2 record"
        );
    }

    let _ = fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// 2. The frozen record drives the router
// ---------------------------------------------------------------------------

/// The lane a row is decided by comes from its **frozen record**, not from the
/// switch state of the process reading it.
///
/// This is the property the card states as its own bullet: closing the switch
/// stops new v2 records but must not convert an already-frozen one. The identity
/// function is asserted under every switch state to prove it never consults the
/// environment at all.
#[test]
fn the_frozen_record_decides_the_lane_and_not_the_switch() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};

    let on = frozen_with_v2();
    let key = on.v2_row_key().expect("a v2 row");
    let frozen_v2 = on.requirements_json(&key).expect("a frozen column");

    // Reading the same bytes with the switch explicitly **off** must still say v2.
    let read_back = with_test_enablement(V2Enablement::Off, || {
        p2b_contract_identity::classify_requirements(Some(&frozen_v2))
    });
    assert!(
        matches!(read_back, p2b_contract_identity::ContractIdentity::HostV2),
        "a frozen v2 row must stay v2 for a process whose switch is off"
    );

    // The v1 sibling: same Run, same producer, and its own column stays v1.
    let off = frozen_without_v2();
    let (off_key, off_identity) = off
        .identities()
        .into_iter()
        .find(|(_, identity)| {
            matches!(identity, p2b_contract_identity::ContractIdentity::LegacyV1)
        })
        .expect("an all-v1 Run has a v1 row");
    let frozen_v1 = off.requirements_json(&off_key).expect("a frozen column");
    assert!(matches!(
        off_identity,
        p2b_contract_identity::ContractIdentity::LegacyV1
    ));

    // No switch state changes either answer.
    for state in [
        V2Enablement::On,
        V2Enablement::Off,
        V2Enablement::Invalid("x".into()),
    ] {
        with_test_enablement(state, || {
            assert!(matches!(
                p2b_contract_identity::classify_requirements(Some(&frozen_v2)),
                p2b_contract_identity::ContractIdentity::HostV2
            ));
            assert!(matches!(
                p2b_contract_identity::classify_requirements(Some(&frozen_v1)),
                p2b_contract_identity::ContractIdentity::LegacyV1
            ));
        });
    }
    // A missing column is a legal legacy row; an unreadable one is neither version.
    assert!(matches!(
        p2b_contract_identity::classify_requirements(None),
        p2b_contract_identity::ContractIdentity::LegacyV1
    ));
    assert!(matches!(
        p2b_contract_identity::classify_requirements(Some("{not json")),
        p2b_contract_identity::ContractIdentity::Indeterminate(_)
    ));

    let _ = fs::remove_dir_all(&on.root);
    let _ = fs::remove_dir_all(&off.root);
}

/// Evaluating a Run does not rewrite its frozen requirement column.
///
/// The record is write-once, and the router reads it on every decision — so a
/// router that normalized or re-stamped the column would change a row's generation
/// under a reader that had already classified it.
#[test]
fn evaluation_does_not_rewrite_the_frozen_record() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let key = fixture.v2_row_key().expect("a v2 row");
    let before = fixture.requirements_json(&key).expect("a frozen column");
    let all_before: Vec<(String, Option<String>)> =
        fixture.db.delivery_checklist_requirements(&fixture.run).unwrap();

    let _ = with_test_enablement(V2Enablement::Off, || fixture.stop());

    let after = fixture.requirements_json(&key).expect("a frozen column");
    let all_after: Vec<(String, Option<String>)> =
        fixture.db.delivery_checklist_requirements(&fixture.run).unwrap();
    assert_eq!(before, after, "the v2 row's frozen column must not change");
    assert_eq!(
        all_before, all_after,
        "no row's frozen column may change across a decision"
    );

    let _ = fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// 3. One fold over a mixed Run
// ---------------------------------------------------------------------------

/// A Run carrying a v2 row is decided by the typed lane through `evaluate_stop`,
/// with the switch **off** — as a restart or a continuation would present it.
///
/// Read through the production entry, not by calling the router: the point is that
/// the wiring exists, and a test that invoked the router directly would prove only
/// that the router can be invoked.
#[test]
fn a_run_with_a_frozen_v2_row_is_decided_by_the_typed_lane() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let key = fixture.v2_row_key().expect("a v2 row");

    let stop = with_test_enablement(V2Enablement::Off, || fixture.stop());
    let DeliveryStop::Typed(typed) = &stop else {
        panic!(
            "a Run holding a frozen v2 row must be decided by the typed lane \
             even with the switch off, got {stop:?}"
        );
    };
    assert!(
        typed.rows.iter().any(|row| row.item_key == key),
        "the v2 row must be present: {:?}",
        typed.rows
            .iter()
            .map(|row| row.item_key.clone())
            .collect::<Vec<_>>()
    );
    // One aggregate class over every row, and each row carries its own rather than
    // the aggregate's.
    assert!(!typed.verdict.class.as_str().is_empty());
    for row in &typed.rows {
        assert!(
            !row.class.is_empty(),
            "{}: every row must carry its own class",
            row.item_key
        );
    }

    let _ = fs::remove_dir_all(&fixture.root);
}

/// Every row of a mixed Run lands in exactly one of the two groups the stop
/// carries.
///
/// This is the fold's durable consequence, and the place a v1 sibling could
/// otherwise be silently dropped: the terminal stage's delete is Run-wide, so a row
/// in neither group would be erased without a decision.
#[test]
fn a_mixed_run_partitions_every_row_into_exactly_one_group() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let stop = with_test_enablement(V2Enablement::Off, || fixture.stop());
    let DeliveryStop::Typed(typed) = &stop else {
        panic!("expected the typed lane for a Run holding a frozen v2 row: {stop:?}");
    };

    let staged: BTreeSet<&str> = typed
        .terminal_items
        .iter()
        .map(|item| item.item_key.as_str())
        .collect();
    let pending: BTreeSet<&str> = typed
        .pending_items
        .iter()
        .map(|(key, _)| key.as_str())
        .collect();
    assert!(
        staged.is_disjoint(&pending),
        "no row may be both staged and retained: staged={staged:?} pending={pending:?}"
    );
    for row in &typed.rows {
        assert!(
            staged.contains(row.item_key.as_str()) || pending.contains(row.item_key.as_str()),
            "row {} must be in exactly one group (staged={staged:?} pending={pending:?})",
            row.item_key
        );
    }
    // The groups are derived from the rows, so their union cannot exceed them...
    assert!(staged.len() + pending.len() <= typed.rows.len());
    // ...and a non-terminal row is never staged: `passed = 0` there is finalized as
    // `failed`, which would turn "not verified yet" into a content failure.
    for row in &typed.rows {
        if !row.stage {
            assert!(
                !staged.contains(row.item_key.as_str()),
                "non-staged row {} must not be in the terminal group",
                row.item_key
            );
        }
    }

    let _ = fs::remove_dir_all(&fixture.root);
}

/// Both generations of row are folded by the **same** call, so the aggregate class
/// is chosen by one priority order.
///
/// The property is asserted by construction: the aggregate must equal folding every
/// item's own facts back through the shared decision function. A router that
/// decided the versions separately and then combined two verdicts with a second
/// priority rule would disagree with this recomputation.
#[test]
fn the_mixed_aggregate_is_one_fold_of_every_item() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let stop = with_test_enablement(V2Enablement::Off, || fixture.stop());
    let DeliveryStop::Typed(typed) = &stop else {
        panic!("expected the typed lane: {stop:?}");
    };

    assert!(
        typed.verdict.items.len() >= 2,
        "the fixture must actually be mixed, otherwise this case proves nothing: {:?}",
        typed.verdict
            .items
            .iter()
            .map(|item| item.item_key.clone())
            .collect::<Vec<_>>()
    );
    let folded: Vec<DecisionFact> = typed
        .verdict
        .items
        .iter()
        .flat_map(|item| item.facts.clone())
        .collect();
    let recomputed = decide_facts(&folded, typed.verdict.repair_budget_remaining);
    assert_eq!(
        typed.verdict.class, recomputed.class,
        "the aggregate must be the one order's fold of every item's facts, not a second combination"
    );

    let _ = fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// 4. No envelope on a v1 row
// ---------------------------------------------------------------------------

/// A row the producer froze as v1 is finalized with its own bytes, never with a v2
/// envelope.
///
/// The invariant is byte-level on purpose: `qualify_pass` and `delivery_u1` decide
/// a record's version by what the finding *contains*, so an envelope added to a v1
/// row would re-identify a row the switch never touched — and U1 would then read it
/// as a v2 row whose class is not `Unconfirmed`.
#[test]
fn a_v1_row_is_never_stamped_with_a_v2_envelope() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let v2_key = fixture.v2_row_key().expect("a v2 row");
    let stop = with_test_enablement(V2Enablement::Off, || fixture.stop());
    let DeliveryStop::Typed(typed) = &stop else {
        panic!("expected the typed lane: {stop:?}");
    };

    let mut saw_v2 = false;
    let mut saw_v1 = false;
    for row in &typed.rows {
        let identity = p2b_contract_identity::classify_requirements(
            fixture.requirements_json(&row.item_key).as_deref(),
        );
        // `Some` means a decodable verdict envelope is present on these bytes.
        let carries_envelope = display_pass_of_finding(&row.finding_json).is_some();
        match identity {
            p2b_contract_identity::ContractIdentity::LegacyV1 => {
                saw_v1 = true;
                assert!(
                    !carries_envelope,
                    "v1 row {} must not carry a v2 envelope: {}",
                    row.item_key, row.finding_json
                );
            }
            p2b_contract_identity::ContractIdentity::HostV2 => {
                saw_v2 = true;
                assert_eq!(row.item_key, v2_key);
                assert!(
                    carries_envelope,
                    "v2 row {} must carry its decodable envelope: {}",
                    row.item_key, row.finding_json
                );
                let finding: Value = serde_json::from_str(&row.finding_json).unwrap();
                let envelope = &finding["deliveryVerdict"];
                assert_eq!(envelope["itemKey"], json!(row.item_key));
                assert_eq!(envelope["class"], json!(row.class));
            }
            p2b_contract_identity::ContractIdentity::Indeterminate(reason) => panic!(
                "row {} has an unreadable contract column: {reason}",
                row.item_key
            ),
        }
    }
    assert!(
        saw_v2,
        "the fixture must actually contain a v2 row, or this case proves nothing"
    );
    assert!(
        saw_v1,
        "the fixture must actually contain a v1 sibling, or the mixed case is untested"
    );

    let _ = fs::remove_dir_all(&fixture.root);
}

/// Every v2 row's envelope names the one fold it belongs to, so no row can be
/// decoded as belonging to a different fold than the stop it came from.
#[test]
fn every_v2_row_envelope_names_the_one_fold_it_belongs_to() {
    use content_requirements::delivery_v2::{V2Enablement, with_test_enablement};
    let fixture = frozen_with_v2();
    let stop = with_test_enablement(V2Enablement::Off, || fixture.stop());
    let DeliveryStop::Typed(typed) = &stop else {
        panic!("expected the typed lane: {stop:?}");
    };

    let item_count = typed.verdict.items.len();
    let aggregate = typed.verdict.class.as_str();
    let mut stamped = 0;
    for row in &typed.rows {
        let finding: Value = serde_json::from_str(&row.finding_json).unwrap();
        let Some(envelope) = finding.get("deliveryVerdict") else {
            continue;
        };
        stamped += 1;
        assert_eq!(
            envelope["aggregateClass"],
            json!(aggregate),
            "{}: the envelope must name the aggregate it was folded into",
            row.item_key
        );
        assert_eq!(
            envelope["itemCount"],
            json!(item_count),
            "{}: the envelope must state the size of the one fold",
            row.item_key
        );
    }
    assert_eq!(
        stamped, 1,
        "exactly the one v2 row carries an envelope, and it names the whole fold"
    );

    let _ = fs::remove_dir_all(&fixture.root);
}
