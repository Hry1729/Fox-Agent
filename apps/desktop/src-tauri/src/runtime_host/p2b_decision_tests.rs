//! Package 2-B1 tests: internal v2 fact classification and pure aggregation.
//!
//! Registered from `p2b_decision.rs` with `#[cfg(test)]` only; nothing here is
//! compiled into a production build.
//!
//! Two clearly separated layers, as the card requires:
//!
//! * **Pure aggregation units** — `decide` / `classify_v2_facts` over
//!   *constructed* observations. These prove the decision order and fact
//!   preservation; they do **not** claim a production constructor produced the
//!   input. Every such test is marked "构造观察".
//! * **Real-context classification** — `classify_v2_requirement` against a real
//!   `Database` and a real typed read on the filesystem, so `TrustRefusal` /
//!   `AuthorizationRefused` / `Resource` / `Io` classification is exercised at
//!   its real producing site.
//!
//! A real leaf `CheckOutcome` is fed in explicitly (via the public leaf
//! constructors `failed` / `passed` / `unverified` / `budget_exceeded`), which is
//! what distinguishes a real parser result from a hand-built observation.

use serde_json::{json, Value};

use crate::database::{
    Database, DeliveryChecklistItem, DeliveryChecklistSeed, DeliveryRequirement, RequirementKind,
};
use crate::runtime_host::attachment_compute::DeliveryReadFailure;
use crate::runtime_host::delivery_checks_v2::{
    CandidateBinding, CheckOutcome, CheckState, SecurityVeto, budget_exceeded, equivalence, failed,
    not_implemented, passed, passed_with_equivalence, reason_code, unverified,
};

use super::super::content_requirements::{DecodedSpec, classify_v2_requirement, decode_spec};
use super::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The same hex SHA-256 spelling the production writer records.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// A writable scratch directory derived from the test executable's own target
/// directory, exactly like the 2-A fixture (no fixed absolute path, and the
/// system temp dir is only a fallback).
fn temp_root(label: &str) -> std::path::PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .and_then(|deps| deps.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let root = base
        .join("p2b-tmp")
        .join(format!("{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

fn v1_concat_rule(sources: Vec<Value>) -> Value {
    json!({
        "type": "concat",
        "sources": sources,
        "selected": ["in/a.txt"],
        "selection_order": "bytes_ascending",
        "tie_order": "filename_ascending",
        "merge_order": "filename_ascending",
        "read_only": true
    })
}

fn v2_spec(rule: Value) -> Value {
    json!({
        "schemaVersion": 2,
        "envelopeVersion": 2,
        "checkerVersion": 2,
        "ruleId": "content:concat",
        "policy": {
            "comparison": "bytesExact",
            "eof": "byteExact",
            "table": {
                "delimiter": "comma",
                "quote": "doubleQuote",
                "bom": "auto",
                "lineEnding": "any",
                "blankLine": "skip",
                "header": "required"
            }
        },
        "rule": rule
    })
}

fn source_ref(path: &str, sha256: &str, bytes: u64) -> Value {
    json!({"path": path, "sha256": sha256, "bytes": bytes, "firstLine": 1, "lastLine": 1})
}

/// A real DB with a conversation + Run whose frozen binding names `root`.
/// Mirrors the 2-A fixture; touches no real project authorization.
fn database_with_run(_label: &str, root: &std::path::Path) -> (Database, String) {
    let database = Database::open(root.join("fox.db")).expect("open database");
    let conversation = database
        .create_conversation(
            database.default_agent_id(),
            None,
            Some(root.to_str().expect("root path")),
            Some("read_only"),
        )
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    freeze_binding(&database, &run, &conversation.id, Some(root.to_str().expect("root")));
    (database, run)
}

fn freeze_binding(database: &Database, run: &str, conversation: &str, project_root: Option<&str>) {
    use fox_engine_protocol::{
        ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
        TimeBudgets,
    };
    let permission = FrozenPermission {
        mode: PermissionMode::Allow,
        project_root: project_root.map(str::to_owned),
        grants: vec![],
        approval_epoch: None,
    };
    let binding = RunControlBinding {
        executor_route: None,
        schema_version: 1,
        run_id: run.to_owned(),
        conversation_id: conversation.to_owned(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets: TimeBudgets::default(),
    };
    database.freeze_run_control(&binding).expect("freeze binding");
}

/// A finding shaped like the ones `apply` persists, with a real `checkedHash`.
fn finding_with_hash(hash: &str, path: &str) -> Value {
    json!({"checkedHash": hash, "boundPath": path, "checks": {}})
}

// ===========================================================================
// Group 1: pure aggregation truth table (构造观察)
//
// Every input below is a *constructed* fact/observation. These units pin the
// decision order and fact preservation; they deliberately do not claim that a
// production constructor emits these inputs.
// ===========================================================================

/// 构造观察: the six-step order, one row per documented rule.
#[test]
fn p2b_decision_order_truth_table() {
    let cand = FactOrigin::candidate("out/report.txt", "");
    let src = FactOrigin::source("in/a.txt", "");
    let refused = DecisionFact::refused(cand.clone(), "path_denied", "escaped");
    // A refusal against a *source* is attributed to that source, and it refuses
    // just as hard as a candidate refusal.
    let source_refused = DecisionFact::refused(src.clone(), reason_code::security::SOURCE_IDENTITY_MISMATCH, "source changed");
    let invalid = DecisionFact::invalid("内容契约 schemaVersion 不受支持");
    let blocking_failed = DecisionFact::failed(cand.clone(), reason_code::TEXT_CONTENT_MISMATCH, "mismatch", true, None);
    let nonblocking_failed = DecisionFact::failed(cand.clone(), reason_code::BUDGET_EXCEEDED, "soft", false, None);
    let unconfirmed = DecisionFact::unconfirmed(
        cand.clone(),
        Some(reason_code::EVIDENCE_UNAVAILABLE),
        "missing",
        None,
    );
    let resource = DecisionFact::unconfirmed(
        cand.clone(),
        Some(reason_code::BUDGET_EXCEEDED),
        "limit",
        Some(4096),
    );
    let pass = DecisionFact::passed(cand.clone());

    // 1. Refused wins over everything, including a spec error and a repair.
    for facts in [
        vec![refused.clone()],
        vec![refused.clone(), pass.clone()],
        vec![refused.clone(), invalid.clone(), blocking_failed.clone()],
        vec![refused.clone(), unconfirmed.clone()],
    ] {
        let result = decide(&facts, true);
        assert_eq!(result.class, DeliveryClass::Refused, "{facts:?}");
        // Rule 1 forbids repair: the budget flag must not change the class.
        assert_eq!(decide(&facts, false).class, DeliveryClass::Refused);
    }

    // 2. Spec Invalid wins over repair and over every content fact.
    let with_invalid = vec![invalid.clone(), blocking_failed.clone(), unconfirmed.clone()];
    assert_eq!(decide(&with_invalid, true).class, DeliveryClass::Invalid);
    assert_eq!(decide(&with_invalid, false).class, DeliveryClass::Invalid);

    // 3. Blocking Failed with budget left is repairable, even beside a
    //    non-blocking failure or an unconfirmed fact — and every fact survives.
    let mixed = vec![
        blocking_failed.clone(),
        nonblocking_failed.clone(),
        unconfirmed.clone(),
    ];
    let repair = decide(&mixed, true);
    assert_eq!(repair.class, DeliveryClass::RepairNeeded);
    assert_eq!(repair.facts.len(), 3, "no fact may be dropped by the aggregate");
    assert!(repair.has_class(FactClass::Failed));
    assert!(repair.has_class(FactClass::Unconfirmed));

    // 4a. Budget exhausted on a blocking failure: a definite non-pass, and NOT
    //     renamed "the spec is illegal".
    let exhausted = decide(&[blocking_failed.clone()], false);
    assert_eq!(exhausted.class, DeliveryClass::DefiniteFailure);
    assert_ne!(exhausted.class, DeliveryClass::Invalid);
    assert!(exhausted.has_class(FactClass::Failed));

    // 4b. A non-blocking failure is a definite non-pass even with budget left:
    //     it was never repairable, and it must not be silently dropped.
    let soft = decide(&[nonblocking_failed.clone()], true);
    assert_eq!(soft.class, DeliveryClass::DefiniteFailure);
    assert!(soft.has_class(FactClass::Failed));

    // 5. Unverified / resource / IO / missing evidence -> Unconfirmed, and the
    //    byte limit is retained.
    assert_eq!(decide(&[unconfirmed.clone()], true).class, DeliveryClass::Unconfirmed);
    let limited = decide(&[resource.clone()], true);
    assert_eq!(limited.class, DeliveryClass::Unconfirmed);
    assert_eq!(limited.facts[0].limit, Some(4096));
    assert_eq!(limited.facts[0].reason_code.as_deref(), Some(reason_code::BUDGET_EXCEEDED));

    // 6. Only exclusively-Passed facts confirm.
    assert_eq!(decide(&[pass.clone()], false).class, DeliveryClass::Confirmed);
    assert_eq!(decide(&[pass.clone(), pass], false).class, DeliveryClass::Confirmed);

    // Source-role refusal: same class, and the retained fact keeps its role.
    let source_case = decide(&[source_refused], true);
    assert_eq!(source_case.class, DeliveryClass::Refused);
    assert_eq!(source_case.facts[0].origin, src);
    assert_eq!(
        source_case.facts[0].reason_code.as_deref(),
        Some(reason_code::security::SOURCE_IDENTITY_MISMATCH)
    );
}

/// 构造观察: the exact counterexample the review found — an all-passed check
/// must never swallow an `Unconfirmed` item.
#[test]
fn p2b_unconfirmed_is_never_swallowed_by_all_passed() {
    let cand = FactOrigin::candidate("out/report.txt", "");
    let pass = DecisionFact::passed(cand.clone());
    let unconfirmed = DecisionFact::unconfirmed(
        cand.clone(),
        Some(reason_code::EVIDENCE_UNAVAILABLE),
        "尚未核验",
        None,
    );

    // Single unconfirmed item.
    assert_eq!(decide(&[unconfirmed.clone()], false).class, DeliveryClass::Unconfirmed);
    // Passed + Unconfirmed must not pass.
    let mixed = decide(&[pass.clone(), unconfirmed.clone()], true);
    assert_eq!(mixed.class, DeliveryClass::Unconfirmed);
    assert_ne!(mixed.class, DeliveryClass::Confirmed);
    assert!(mixed.has_class(FactClass::Passed), "the pass fact is retained");
    assert!(mixed.has_class(FactClass::Unconfirmed));
}

/// 构造观察: empty observation. Both an empty fact set and a non-empty contract
/// with zero observations must land Unconfirmed, never Confirmed — and this
/// function never impersonates the legacy `NoChecklist` state as a pass.
#[test]
fn p2b_empty_observations_are_unconfirmed_not_confirmed() {
    assert_eq!(decide(&[], true).class, DeliveryClass::Unconfirmed);
    assert_eq!(decide(&[], false).class, DeliveryClass::Unconfirmed);

    // A decodable v2 spec whose checkout produced no observation at all.
    let spec = v2_spec(v1_concat_rule(vec![]));
    let outcome = CheckOutcome::default();
    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&outcome),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let result = classify_and_decide(&input, true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed);
    // Zero observations still produce no *leaf* fact. The only fact is the
    // coverage gate's own, which is exactly why this cannot confirm: no leaf
    // proved anything, so nothing proves the contract was covered either.
    assert_eq!(result.facts.len(), 1, "{:?}", result.facts);
    assert_eq!(result.facts[0].check.as_deref(), Some(super::COVERAGE_CHECK));
    assert!(result.facts[0].rule_id.is_none(), "no rule id is invented");
}

/// 构造观察: the type order, not string order. `Refused` and `Invalid` are
/// distinct from `DefiniteFailure`, and none of them is a pass.
#[test]
fn p2b_class_vocabulary_stays_distinct() {
    let classes = [
        DeliveryClass::Confirmed,
        DeliveryClass::Refused,
        DeliveryClass::Invalid,
        DeliveryClass::RepairNeeded,
        DeliveryClass::Unconfirmed,
        DeliveryClass::DefiniteFailure,
    ];
    let names = classes.iter().map(|class| class.as_str()).collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "confirmed",
            "refused",
            "invalid",
            "repairNeeded",
            "unconfirmed",
            "definiteFailure"
        ]
    );
    // No shortcut to the legacy `ItemVerdict.passed` exists on the output.
    // (`repair_budget_remaining` is a budget flag only.)
    assert_ne!(DeliveryClass::Unconfirmed, DeliveryClass::Confirmed);
    assert_ne!(DeliveryClass::RepairNeeded, DeliveryClass::Confirmed);
    assert_ne!(DeliveryClass::DefiniteFailure, DeliveryClass::Invalid);
}

// ===========================================================================
// Group 2: real leaf CheckOutcome inputs (real parser constructors)
//
// These use the leaf's own public constructors, so the input is what the
// checker layer really emits, not a hand-written observation.
// ===========================================================================

fn binding() -> CandidateBinding {
    CandidateBinding::new("out/report.txt", "b".repeat(64), 5)
}

/// A real `failed()` observation is blocking and repairable; a real `passed()`
/// observation is neither.
#[test]
fn p2b_real_leaf_outcomes_classify_and_preserve_repair_semantics() {
    let spec = v2_spec(v1_concat_rule(vec![]));

    let failing = CheckOutcome::single(failed(
        "content:concat",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "顺序不符",
    ));
    assert!(failing.needs_repair(), "the leaf's own repair rule");
    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&failing),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let result = classify_and_decide(&input, true);
    assert_eq!(result.class, DeliveryClass::RepairNeeded);
    assert_eq!(result.facts[0].reason_code.as_deref(), Some(reason_code::TEXT_CONTENT_MISMATCH));
    // The candidate binding's identity travels with the fact, not by inference.
    assert_eq!(
        result.facts[0].origin,
        FactOrigin::candidate("out/report.txt", &"b".repeat(64))
    );
    // Budget exhausted: same facts, definite non-pass.
    assert_eq!(classify_and_decide(&input, false).class, DeliveryClass::DefiniteFailure);

    let passing = CheckOutcome::single(passed("content:concat", "text_concat", &binding()));
    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&passing),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    // With no declared obligation set the entry stays Unconfirmed even though the
    // leaf passed: an all-Passed leaf is a local reading, not contract coverage.
    assert_eq!(classify_and_decide(&input, false).class, DeliveryClass::Unconfirmed);
    // Declaring the obligation discharged is what lets the pass confirm.
    let input = FactInput {
        coverage: Coverage::single_check("text_concat", true),
        ..input
    };
    assert_eq!(classify_and_decide(&input, false).class, DeliveryClass::Confirmed);
}

/// A real `unverified()` / `budget_exceeded()` / `not_implemented()` observation
/// is never blocking and never repairable: a resource ceiling must not arm a
/// model repair round, and an unimplemented checker is not a pass.
#[test]
fn p2b_real_unverified_and_budget_and_not_implemented_are_unconfirmed() {
    let spec = v2_spec(v1_concat_rule(vec![]));
    let cases = [
        unverified("content:concat", "text_concat", reason_code::EVIDENCE_UNAVAILABLE, &binding(), "证据不足"),
        budget_exceeded("content:concat", "text_concat", &binding(), crate::runtime_host::delivery_checks_v2::LimitKind::Records, 100, 101),
        not_implemented("content:concat", "text_concat", &binding()),
    ];
    for observation in cases {
        assert_eq!(observation.state, CheckState::Unverified);
        assert!(!observation.blocking, "unverified never blocks");
        let outcome = CheckOutcome::single(observation);
        assert!(!outcome.needs_repair(), "a resource/unverified result must not repair");
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: Some(&outcome),
            authorization_changed: false,
            coverage: Coverage::Unstated,
            context: None,
        };
        // Even with a fresh repair budget, this stays Unconfirmed.
        assert_eq!(classify_and_decide(&input, true).class, DeliveryClass::Unconfirmed);
    }
}

/// A real security veto is a refusal, and it is never repairable — even when the
/// same outcome also carries a repairable failure.
#[test]
fn p2b_real_security_veto_is_a_refusal_and_forbids_repair() {
    let spec = v2_spec(v1_concat_rule(vec![]));
    let mut outcome = CheckOutcome::single(failed(
        "content:concat",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "mismatch",
    ));
    outcome.security_veto = Some(SecurityVeto::new(
        reason_code::security::SOURCE_IDENTITY_MISMATCH,
        "text_concat",
        "来源身份不符",
    ));
    assert!(outcome.has_failure() && !outcome.needs_repair());

    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&outcome),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let result = classify_and_decide(&input, true);
    assert_eq!(result.class, DeliveryClass::Refused);
    assert_eq!(result.refusals().count(), 1);
    // Both facts survive: the veto AND the real content failure behind it.
    assert!(result.has_class(FactClass::Refused));
    assert!(result.has_class(FactClass::Failed));
}

// ===========================================================================
// Group 3: spec decoding facts
// ===========================================================================

/// An unusable spec is `Invalid` with its concrete reason preserved, and it
/// never becomes a pass or a repair.
#[test]
fn p2b_invalid_spec_is_invalid_with_its_concrete_reason() {
    // A missing / unsupported version is a version problem.
    for spec in [
        json!({"rule": v1_concat_rule(vec![])}),
        json!({"schemaVersion": 3, "rule": v1_concat_rule(vec![])}),
        json!({"schemaVersion": "2", "rule": v1_concat_rule(vec![])}),
    ] {
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: None,
            authorization_changed: false,
            coverage: Coverage::Unstated,
            context: None,
        };
        let result = classify_and_decide(&input, true);
        assert_eq!(result.class, DeliveryClass::Invalid, "{spec}");
        let detail = result.facts[0].detail.clone().unwrap_or_default();
        assert!(!detail.is_empty(), "the concrete decode reason is preserved");
        assert_eq!(result.facts[0].origin, FactOrigin::Spec);
        assert_eq!(
            result.facts[0].reason_code.as_deref(),
            Some(reason_code::CONTRACT_VERSION_UNSUPPORTED),
            "a version-shaped failure keeps the version code: {spec}"
        );
    }

    // A version the reader accepts but whose fields are unusable is a *different*
    // problem, and must not be reported as an unsupported version.
    let incomplete = json!({"schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 2});
    let input = FactInput {
        spec: &incomplete,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: None,
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let result = classify_and_decide(&input, true);
    assert_eq!(result.class, DeliveryClass::Invalid);
    assert_eq!(
        result.facts[0].reason_code.as_deref(),
        Some(reason_code::CONTRACT_UNREADABLE),
        "an unusable body is not a version problem: {:?}",
        result.facts[0].reason_code
    );
}

/// A v1 record is not this function's to classify: it yields no v2 fact, so it
/// can never be confirmed here (the v1 path owns it).
#[test]
fn p2b_v1_spec_is_not_classified_as_v2() {
    let spec = json!({"schemaVersion": 1, "rule": v1_concat_rule(vec![])});
    assert!(matches!(decode_spec(&spec), DecodedSpec::V1(..)));
    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: None,
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    // No facts at all: unconfirmed, never confirmed.
    assert_eq!(classify_and_decide(&input, true).class, DeliveryClass::Unconfirmed);
}

// ===========================================================================
// Group 4: hash drift, by role
// ===========================================================================

/// Candidate-receipt drift and source-identity drift are both refusals, both
/// reuse an existing frozen reason code, and they are attributed to the right
/// identity.
#[test]
fn p2b_hash_drift_by_role_reuses_frozen_codes_and_keeps_attribution() {
    let candidate = candidate_hash_drift_fact("sha256:aaaa", "bbbb", "out/report.txt");
    assert_eq!(candidate.class, FactClass::Refused);
    assert_eq!(candidate.reason_code.as_deref(), Some(reason_code::CANDIDATE_HASH_DRIFT));
    assert_eq!(
        candidate.origin,
        FactOrigin::candidate("out/report.txt", "bbbb"),
        "a candidate drift belongs to the candidate identity"
    );

    let source = source_hash_drift_fact(&"a".repeat(64), &"b".repeat(64), "in/a.txt");
    assert_eq!(source.reason_code.as_deref(), Some(reason_code::security::SOURCE_IDENTITY_MISMATCH));
    assert_eq!(
        source.origin,
        FactOrigin::source("in/a.txt", &"b".repeat(64)),
        "a source drift belongs to the source identity"
    );

    // The two roles must not collapse into one code.
    assert_ne!(candidate.reason_code, source.reason_code);

    // A drift refuses even when everything else passed.
    let spec = v2_spec(v1_concat_rule(vec![]));
    let pass = DecisionFact::passed(FactOrigin::candidate("out/report.txt", "bbbb"));
    let input = FactInput {
        spec: &spec,
        read_facts: vec![pass],
        drift_facts: vec![candidate],
        outcome: None,
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    assert_eq!(classify_and_decide(&input, true).class, DeliveryClass::Refused);
}

// ===========================================================================
// Group 5: read-failure classification (typed variants, no string parsing)
// ===========================================================================

/// Each typed failure variant maps to its documented class, at the classifier
/// itself — no message is ever parsed.
#[test]
fn p2b_typed_read_failures_classify_by_variant() {
    let origin = FactOrigin::candidate("out/report.txt", "");

    let trust = classify_read_failure(
        origin.clone(),
        &DeliveryReadFailure::TrustRefusal {
            reason: "path_denied",
            detail: "Opened delivery source escaped its project".to_owned(),
        },
    );
    assert_eq!(trust.class, FactClass::Refused);
    assert_eq!(trust.reason_code.as_deref(), Some("path_denied"));

    let auth = classify_read_failure(
        origin.clone(),
        &DeliveryReadFailure::AuthorizationRefused {
            detail: "Project access was removed".to_owned(),
        },
    );
    assert_eq!(auth.class, FactClass::Refused);
    assert!(auth.detail.unwrap().contains("Project access was removed"));

    let unavailable = classify_read_failure(
        origin.clone(),
        &DeliveryReadFailure::EvidenceUnavailable {
            detail: "No frozen project root".to_owned(),
        },
    );
    assert_eq!(unavailable.class, FactClass::Unconfirmed);
    assert_eq!(
        unavailable.reason_code.as_deref(),
        Some(reason_code::EVIDENCE_UNAVAILABLE)
    );

    let resource = classify_read_failure(origin.clone(), &DeliveryReadFailure::Resource { limit: 64 });
    assert_eq!(resource.class, FactClass::Unconfirmed);
    assert_eq!(resource.limit, Some(64), "the byte ceiling is preserved");

    // An OS denial is `Io`: never upgraded to an authorization conclusion, and
    // never acceptable.
    let io = classify_read_failure(
        origin,
        &DeliveryReadFailure::Io("拒绝访问。 (os error 5)".to_owned()),
    );
    assert_eq!(io.class, FactClass::Unconfirmed);
    assert!(io.reason_code.is_none(), "no invented code for an OS error");
    assert_eq!(io.limit, None);
}

/// A root/authorization lookup failure is attributed to the Run binding, not to
/// an artifact path it never reached.
#[test]
fn p2b_root_failure_is_attributed_to_the_binding() {
    let auth = classify_root_failure(&DeliveryReadFailure::AuthorizationRefused {
        detail: "Project authorization changed".to_owned(),
    });
    assert_eq!(auth.class, FactClass::Refused);
    assert_eq!(auth.origin, FactOrigin::ProjectBinding);

    let io = classify_root_failure(&DeliveryReadFailure::Io("db locked".to_owned()));
    assert_eq!(io.class, FactClass::Unconfirmed);
    assert_eq!(io.origin, FactOrigin::ProjectBinding);
    assert_ne!(io.class, FactClass::Refused, "an OS/DB error is not a refusal");
}

/// An empty-but-readable candidate is missing evidence, not a silent agreement.
#[test]
fn p2b_empty_candidate_read_is_unconfirmed() {
    let facts = classify_read_result("out/report.txt", &Ok(Vec::new()));
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].class, FactClass::Unconfirmed);

    // A non-empty read produces no fact of its own (a checker decides content).
    assert!(classify_read_result("out/report.txt", &Ok(b"body".to_vec())).is_empty());
}

// ===========================================================================
// Group 6: real-context classification (real Database + real filesystem)
// ===========================================================================

/// The success path: a real v2 spec, a real bound candidate whose bytes match
/// the recorded `checkedHash`, and a frozen source that still matches. Neither
/// read produces a refusal or a drift — but no v2 checker is wired yet, so the
/// result is **Unconfirmed**, never Confirmed. A format that merely decodes is
/// not a content pass.
#[test]
fn p2b_real_context_matching_reads_yield_no_refusal_but_stay_unconfirmed() {
    let root = temp_root("confirm");
    std::fs::create_dir_all(root.join("in")).expect("mkdir in");
    std::fs::write(root.join("in/a.txt"), b"alpha").expect("write source");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let (database, run) = database_with_run("confirm", &root);

    let source_bytes = std::fs::read(root.join("in/a.txt")).expect("read source");
    let candidate_bytes = std::fs::read(root.join("out.txt")).expect("read candidate");
    let source_sha = sha256_hex(&source_bytes);
    let candidate_sha = sha256_hex(&candidate_bytes);

    let spec = v2_spec(v1_concat_rule(vec![source_ref(
        "in/a.txt",
        &source_sha,
        source_bytes.len() as u64,
    )]));
    let finding = finding_with_hash(&candidate_sha, "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );
    // Every read agreed, so no refusal and no drift fact exists...
    assert_eq!(result.refusals().count(), 0, "{:?}", result.facts);
    assert!(!result.has_class(FactClass::Failed));
    // ...and because no content checker has run, this is still not a pass.
    assert_eq!(result.class, DeliveryClass::Unconfirmed, "{:?}", result.facts);
    assert_ne!(result.class, DeliveryClass::Confirmed);
    let _ = std::fs::remove_dir_all(&root);
}

/// A candidate whose bytes no longer match the recorded `checkedHash` is a
/// **candidate** refusal carrying the existing `CANDIDATE_HASH_DRIFT` code.
#[test]
fn p2b_real_context_candidate_hash_drift_is_refused() {
    let root = temp_root("cand-drift");
    std::fs::create_dir_all(root.join("in")).expect("mkdir in");
    std::fs::write(root.join("in/a.txt"), b"alpha").expect("write source");
    std::fs::write(root.join("out.txt"), b"real bytes").expect("write candidate");
    let (database, run) = database_with_run("cand-drift", &root);

    let source_bytes = std::fs::read(root.join("in/a.txt")).expect("read source");
    let source_sha = sha256_hex(&source_bytes);
    let spec = v2_spec(v1_concat_rule(vec![source_ref(
        "in/a.txt",
        &source_sha,
        source_bytes.len() as u64,
    )]));
    // A receipt hash that cannot match the real bytes.
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Refused, "{:?}", result.facts);
    let refusal = result.refusals().next().expect("a retained refusal");
    assert_eq!(refusal.reason_code.as_deref(), Some(reason_code::CANDIDATE_HASH_DRIFT));
    assert!(matches!(refusal.origin, FactOrigin::Candidate { .. }));
    let _ = std::fs::remove_dir_all(&root);
}

/// A frozen **source** that no longer matches its recorded hash is a **source**
/// refusal carrying the existing security code — and a real checker failure on
/// the same requirement is *also* retained.
#[test]
fn p2b_real_context_source_hash_drift_is_refused_and_keeps_other_facts() {
    let root = temp_root("source-drift");
    std::fs::create_dir_all(root.join("in")).expect("mkdir in");
    std::fs::write(root.join("in/a.txt"), b"changed contents").expect("write source");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let (database, run) = database_with_run("source-drift", &root);

    let candidate_bytes = std::fs::read(root.join("out.txt")).expect("read candidate");
    let candidate_sha = sha256_hex(&candidate_bytes);
    // The contract remembers a *different* source identity than what is on disk.
    let spec = v2_spec(v1_concat_rule(vec![source_ref(
        "in/a.txt",
        &"a".repeat(64),
        7,
    )]));
    let finding = finding_with_hash(&candidate_sha, "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Refused, "{:?}", result.facts);
    let refusal = result
        .refusals()
        .find(|fact| fact.reason_code.as_deref() == Some(reason_code::security::SOURCE_IDENTITY_MISMATCH))
        .expect("the source drift is retained as a refusal");
    assert!(matches!(refusal.origin, FactOrigin::Source { .. }));
    let _ = std::fs::remove_dir_all(&root);
}

/// The read resource gate: a real candidate larger than the declared ceiling
/// yields `Resource` with the byte limit preserved, and it does not become a
/// content failure or a repair round.
#[test]
fn p2b_real_context_read_resource_gate_is_unconfirmed_with_its_limit() {
    let root = temp_root("resource");
    std::fs::write(root.join("out.txt"), b"0123456789").expect("write candidate");
    let (database, run) = database_with_run("resource", &root);

    // A ceiling the real reader enforces, exercised through the classifier by
    // calling the typed twin at the classifier's own call site shape.
    let failure = crate::runtime_host::attachment_compute::read_authorized_delivery_bytes_typed(
        &root,
        "out.txt",
        4,
    )
    .expect_err("the ceiling must be enforced");
    let fact = classify_read_failure(FactOrigin::candidate("out.txt", ""), &failure);
    assert_eq!(fact.class, FactClass::Unconfirmed);
    assert_eq!(fact.limit, Some(4), "the real byte limit is preserved");
    assert_eq!(fact.reason_code.as_deref(), Some(reason_code::BUDGET_EXCEEDED));

    // And through the real classifier: the candidate's recorded hash cannot
    // match, so the drift refusal outranks any resource fact.
    let spec = v2_spec(v1_concat_rule(vec![]));
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        false,
    );
    assert_eq!(result.class, DeliveryClass::Refused, "drift outranks the resource fact");
    let _ = std::fs::remove_dir_all(&root);
}

/// An OS-level read denial stays `Io` at the real producing site: it is never
/// dressed up as an authorization refusal, and it is never a pass.
#[test]
fn p2b_real_context_os_denial_stays_io_not_authorization() {
    let root = temp_root("os-denial");
    std::fs::create_dir_all(root.join("adir")).expect("mkdir adir");
    // On Windows a directory open fails first, so the typed classifier sees an
    // OS error. That proves only that the OS would not serve this read.
    let failure = crate::runtime_host::attachment_compute::read_authorized_delivery_bytes_typed(
        &root,
        "adir",
        64 * 1024 * 1024,
    )
    .expect_err("a directory must never read as bytes");
    assert!(!matches!(failure, DeliveryReadFailure::AuthorizationRefused { .. }));
    let fact = classify_read_failure(FactOrigin::candidate("adir", ""), &failure);
    assert_eq!(fact.class, FactClass::Unconfirmed);
    assert_ne!(fact.class, FactClass::Refused, "an OS denial is not a project refusal");
    let _ = std::fs::remove_dir_all(&root);
}

/// An isolated-DB authorization withdrawal: the conversation's project access is
/// removed after freeze, so the real classifier reports `AuthorizationRefused`
/// attributed to the Run binding — never to the artifact, and never a pass.
#[test]
fn p2b_real_context_authorization_withdrawal_is_refused() {
    let root = temp_root("auth-withdrawn");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let database = Database::open(root.join("fox.db")).expect("open database");
    // A conversation with **no** project root, while the frozen binding names one.
    let conversation = database
        .create_conversation(database.default_agent_id(), None, None, None)
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    freeze_binding(
        &database,
        &run,
        &conversation.id,
        Some(root.to_str().expect("root")),
    );

    let spec = v2_spec(v1_concat_rule(vec![]));
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Refused, "{:?}", result.facts);
    let refusal = result.refusals().next().expect("a retained refusal");
    assert_eq!(refusal.origin, FactOrigin::ProjectBinding);
    assert!(
        refusal.detail.as_deref().unwrap_or_default().contains("Project access was removed"),
        "the explicit withdrawal must be named"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// An isolated-DB root change: the frozen root and the live project root differ,
/// which `authorized_delivery_root_typed` reports as an explicit refusal.
#[test]
fn p2b_real_context_root_change_is_refused() {
    let root = temp_root("root-changed");
    let other = root.join("other");
    std::fs::create_dir_all(&other).expect("mkdir other");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let database = Database::open(root.join("fox.db")).expect("open database");
    let conversation = database
        .create_conversation(
            database.default_agent_id(),
            None,
            Some(other.to_str().expect("other root")),
            Some("read_only"),
        )
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    // Freeze against `root`, while the live conversation grants `other`.
    freeze_binding(&database, &run, &conversation.id, Some(root.to_str().expect("root")));

    let spec = v2_spec(v1_concat_rule(vec![]));
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Refused, "{:?}", result.facts);
    let refusal = result.refusals().next().expect("a retained refusal");
    assert!(
        refusal.detail.as_deref().unwrap_or_default().contains("Project authorization changed"),
        "{:?}",
        refusal.detail
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// A missing candidate binding is missing evidence, not an agreement.
#[test]
fn p2b_real_context_unbound_candidate_is_unconfirmed() {
    let root = temp_root("unbound");
    let (database, run) = database_with_run("unbound", &root);
    let spec = v2_spec(v1_concat_rule(vec![]));
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(&database, &run, "", &spec, None, &finding, true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed, "{:?}", result.facts);
    assert!(result.has_class(FactClass::Unconfirmed));
    let _ = std::fs::remove_dir_all(&root);
}

/// A missing Run binding is genuinely absent evidence.
#[test]
fn p2b_real_context_missing_binding_is_unconfirmed_evidence() {
    let root = temp_root("no-binding");
    let database = Database::open(root.join("fox.db")).expect("open database");
    let spec = v2_spec(v1_concat_rule(vec![]));
    let finding = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(&database, "no-such-run", "", &spec, None, &finding, true);
    assert_eq!(result.class, DeliveryClass::Unconfirmed, "{:?}", result.facts);
    let fact = &result.facts[0];
    assert_eq!(fact.origin, FactOrigin::ProjectBinding);
    assert_eq!(fact.reason_code.as_deref(), Some(reason_code::EVIDENCE_UNAVAILABLE));
    let _ = std::fs::remove_dir_all(&root);
}

/// The isolated-DB fixture's own sanity: the checklist types still line up with
/// what this package consumes, so a future consumer cannot silently retype them.
#[test]
fn p2b_checklist_seed_types_still_accept_a_v2_requirement() {
    let root = temp_root("seed-shape");
    let database = Database::open(root.join("fox.db")).expect("open database");
    let conversation = database
        .create_conversation(
            database.default_agent_id(),
            None,
            Some(root.to_str().expect("root")),
            Some("read_only"),
        )
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    let seeds = vec![DeliveryChecklistSeed {
        item_key: "out/report.txt".to_owned(),
        target_path: Some("out/report.txt".to_owned()),
        artifact_id: None,
        display_name: "report".to_owned(),
        checks: vec![],
        requirements: vec![DeliveryRequirement {
            item_key: Some("out/report.txt".to_owned()),
            id: "content:concat".to_owned(),
            kind: RequirementKind::ContentContract {
                spec: v2_spec(v1_concat_rule(vec![])),
            },
            source_text: "合并输入".to_owned(),
        }],
    }];
    database
        .seed_delivery_checklist(&run, &seeds, crate::database::now_ms())
        .expect("seed");
    let checklist: Vec<DeliveryChecklistItem> = database
        .delivery_checklist(&run)
        .expect("checklist");
    assert_eq!(checklist.len(), 1);
    let requirements = database
        .delivery_item_requirements(&run, "out/report.txt")
        .expect("requirements");
    assert_eq!(requirements.len(), 1);
    let _ = std::fs::remove_dir_all(&root);
}

// ===========================================================================
// Group 7: F1–F6 repair-contract regressions
//
// Each of these is the *hard assertion* the repair card demands. The earlier
// defect-characterization probes are deliberately not reused as-is: a probe that
// only proves "fields are still lost" is not a fix, and a green test is only
// evidence when it asserts the retained field.
// ===========================================================================

/// F1: two different rules must not classify to equal facts. The identity is
/// `rule_id` + `check` + `checker_version`, taken verbatim from the observation.
#[test]
fn p2b_f1_rule_and_check_identity_is_retained_and_distinguishes_facts() {
    let spec = v2_spec(v1_concat_rule(vec![]));

    let r01 = CheckOutcome::single(failed(
        "content:R01",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "R01 不符",
    ));
    let r02 = CheckOutcome::single(failed(
        "content:R02",
        "text_csv",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "R02 不符",
    ));

    let classify = |outcome: &CheckOutcome| {
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: Some(outcome),
            authorization_changed: false,
            coverage: Coverage::Unstated,
            context: None,
        };
        classify_v2_facts(&input)
    };

    let first = classify(&r01);
    let second = classify(&r02);

    // The identity is carried, not dropped.
    assert_eq!(first[0].rule_id.as_deref(), Some("content:R01"));
    assert_eq!(first[0].check.as_deref(), Some("text_concat"));
    assert_eq!(first[0].checker_version, Some(2));
    assert_eq!(second[0].rule_id.as_deref(), Some("content:R02"));
    assert_eq!(second[0].check.as_deref(), Some("text_csv"));

    // The defect: these used to be fully equal. Different rule ids must differ.
    assert_ne!(
        first[0], second[0],
        "two different rule ids must not classify to equal facts"
    );
    assert_ne!(first[0].rule_id, second[0].rule_id);
    assert_ne!(first[0].check, second[0].check);
}

/// F2: the leaf's `LimitKind` and the typed byte ceiling are kept in **separate**
/// fields. A record ceiling is not a byte count, and a byte ceiling keeps its
/// number.
#[test]
fn p2b_f2_limit_kind_is_separate_from_the_byte_ceiling() {
    let spec = v2_spec(v1_concat_rule(vec![]));

    let records = budget_exceeded(
        "content:concat",
        "text_concat",
        &binding(),
        LimitKind::Records,
        100,
        101,
    );
    let outcome = CheckOutcome::single(records);
    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&outcome),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let facts = classify_v2_facts(&input);
    let fact = &facts[0];

    // The kind survives as a typed field, and the byte ceiling stays empty: a
    // record count must never be reported as a byte count.
    assert_eq!(fact.limit_kind, Some(LimitKind::Records));
    assert_eq!(fact.limit, None, "a Records ceiling is not a byte ceiling");
    // The structured ceiling and observation survive verbatim.
    assert_eq!(fact.expected.as_deref(), Some("records <= 100"));
    assert_eq!(fact.actual.as_deref(), Some("101"));

    // A genuine byte ceiling keeps its number in `limit` and its kind in
    // `limit_kind`; the two never collide.
    let byte_fact = classify_read_failure(
        FactOrigin::candidate("out/report.txt", ""),
        &DeliveryReadFailure::Resource { limit: 64 },
    );
    assert_eq!(byte_fact.limit, Some(64));
    assert_eq!(byte_fact.limit_kind, None, "the typed unit is Bytes, not a kind");
}

/// F3: an equivalence pass and a byte-exact pass must not classify to equal
/// facts, and `expected`/`actual` survive when the leaf produced them.
#[test]
fn p2b_f3_equivalence_pass_is_distinguishable_from_a_byte_pass() {
    let spec = v2_spec(v1_concat_rule(vec![]));

    let byte_exact = CheckOutcome::single(passed("content:concat", "text_concat", &binding()));
    let by_equivalence = CheckOutcome::single(passed_with_equivalence(
        "content:concat",
        "text_concat",
        &binding(),
        equivalence::OPTIONAL_FINAL_LF,
        "仅末行换行等价",
    ));

    let classify = |outcome: &CheckOutcome| {
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: Some(outcome),
            authorization_changed: false,
            coverage: Coverage::Unstated,
            context: None,
        };
        classify_v2_facts(&input)
    };

    let plain = classify(&byte_exact);
    let equivalent = classify(&by_equivalence);

    assert_eq!(plain[0].equivalence, None, "a byte match records no equivalence");
    assert_eq!(
        equivalent[0].equivalence.as_deref(),
        Some(equivalence::OPTIONAL_FINAL_LF)
    );
    assert_ne!(
        plain[0], equivalent[0],
        "an equivalence pass must not look like a byte-exact pass"
    );

    // The structured ceiling/observation survive on an unverified observation.
    let limited = CheckOutcome::single(budget_exceeded(
        "content:concat",
        "text_concat",
        &binding(),
        crate::runtime_host::delivery_checks_v2::LimitKind::Cells,
        8,
        9,
    ));
    let fact = classify(&limited).remove(0);
    assert_eq!(fact.expected.as_deref(), Some("cells <= 8"));
    assert_eq!(fact.actual.as_deref(), Some("9"));
}

/// F4: a veto's origin is never an empty identity. With a provable sole
/// candidate it is that candidate; with two candidates it stays explicitly
/// check-level; with an explicit caller context that context wins. The veto's
/// own `check` is kept in every case.
#[test]
fn p2b_f4_veto_origin_is_never_empty_and_its_check_is_kept() {
    let spec = v2_spec(v1_concat_rule(vec![]));
    let veto = |outcome: &mut CheckOutcome| {
        outcome.security_veto = Some(SecurityVeto::new(
            reason_code::security::SOURCE_IDENTITY_MISMATCH,
            "text_concat",
            "来源身份不符",
        ));
    };
    let classify = |outcome: &CheckOutcome, context: Option<&FactContext>| {
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: Some(outcome),
            authorization_changed: false,
            coverage: Coverage::Unstated,
            context: context.cloned(),
        };
        classify_v2_facts(&input)
            .into_iter()
            .find(|fact| fact.class == FactClass::Refused)
            .expect("a retained veto refusal")
    };

    // (a) A sole candidate is provable: the veto belongs to it. No empty identity.
    let mut single = CheckOutcome::single(failed(
        "content:concat",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "mismatch",
    ));
    veto(&mut single);
    let fact = classify(&single, None);
    assert_eq!(
        fact.origin,
        FactOrigin::candidate("out/report.txt", &"b".repeat(64)),
        "a sole candidate is a provable attribution"
    );
    assert_ne!(fact.origin, FactOrigin::candidate("", ""));
    assert_eq!(fact.check.as_deref(), Some("text_concat"));

    // (b) Two different candidates: no basis exists, so no candidate is claimed.
    let mut two = CheckOutcome::single(failed(
        "content:R01",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "first",
    ));
    two.observations.push(failed(
        "content:R02",
        "text_csv",
        reason_code::TEXT_CONTENT_MISMATCH,
        &CandidateBinding::new("out/other.txt", "c".repeat(64), 7),
        "second",
    ));
    veto(&mut two);
    let fact = classify(&two, None);
    assert_eq!(
        fact.origin,
        FactOrigin::CheckUnbound,
        "with two candidates the fact must not borrow either identity"
    );
    assert_ne!(fact.origin, FactOrigin::candidate("", ""));
    assert_eq!(fact.check.as_deref(), Some("text_concat"), "the check survives");

    // (c) An explicit caller context wins over the sole-candidate inference.
    let explicit = FactContext::Source {
        path: "in/a.txt".to_owned(),
        sha256: "d".repeat(64),
    };
    let fact = classify(&single, Some(&explicit));
    assert_eq!(
        fact.origin,
        FactOrigin::source("in/a.txt", &"d".repeat(64)),
        "an explicit binding is not overridden by an accompanying observation"
    );
    assert_eq!(fact.check.as_deref(), Some("text_concat"));
}

/// F4 + §5: veto, `Failed` and `Unverified` coexist; every one is retained, the
/// class is `Refused`, and the veto never forges a project-authorization fact.
#[test]
fn p2b_f4_veto_and_failed_and_unverified_all_survive_together() {
    let spec = v2_spec(v1_concat_rule(vec![]));
    let mut outcome = CheckOutcome::single(failed(
        "content:R01",
        "text_concat",
        reason_code::TEXT_CONTENT_MISMATCH,
        &binding(),
        "真实不符",
    ));
    outcome.observations.push(unverified(
        "content:R02",
        "text_csv",
        reason_code::EVIDENCE_UNAVAILABLE,
        &binding(),
        "证据不足",
    ));
    outcome.security_veto = Some(SecurityVeto::new(
        reason_code::security::SOURCE_IDENTITY_MISMATCH,
        "lineage",
        "身份不符",
    ));

    let input = FactInput {
        spec: &spec,
        read_facts: vec![],
        drift_facts: vec![],
        outcome: Some(&outcome),
        authorization_changed: false,
        coverage: Coverage::Unstated,
        context: None,
    };
    let result = classify_and_decide(&input, true);

    assert_eq!(result.class, DeliveryClass::Refused);
    assert!(result.has_class(FactClass::Refused));
    assert!(result.has_class(FactClass::Failed));
    assert!(result.has_class(FactClass::Unconfirmed));
    // The refusal is a check-level security fact, not a project-authority claim.
    let refusal = result.refusals().next().expect("the veto is retained");
    assert_ne!(refusal.origin, FactOrigin::ProjectBinding);
    assert_eq!(refusal.check.as_deref(), Some("lineage"));
    assert_eq!(
        refusal.reason_code.as_deref(),
        Some(reason_code::security::SOURCE_IDENTITY_MISMATCH)
    );
}

/// F5: the verification-time authorization change is a **project binding** fact,
/// and the two authorization-refusal adapters produce the same attribution and
/// the same single code.
#[test]
fn p2b_f5_authorization_facts_are_project_bound_and_share_one_constructor() {
    let changed = authorization_changed_fact();
    assert!(matches!(changed.origin, FactOrigin::ProjectBinding));
    assert_eq!(changed.class, FactClass::Refused);
    assert_eq!(
        changed.reason_code.as_deref(),
        Some("authorization_changed_during_verification")
    );

    // The root adapter and the artifact-read adapter must agree: same code, same
    // class, and the project attribution survives on the root path.
    let detail = "Project authorization changed";
    let from_root = classify_root_failure(&DeliveryReadFailure::AuthorizationRefused {
        detail: detail.to_owned(),
    });
    let from_read = classify_read_failure(
        FactOrigin::candidate("out/report.txt", ""),
        &DeliveryReadFailure::AuthorizationRefused {
            detail: detail.to_owned(),
        },
    );
    assert_eq!(from_root.origin, FactOrigin::ProjectBinding);
    assert_eq!(from_read.origin, FactOrigin::candidate("out/report.txt", ""));
    assert_eq!(from_root.reason_code, from_read.reason_code);
    assert_eq!(from_root.reason_code.as_deref(), Some("authorization_refused"));
    assert_eq!(from_root.class, FactClass::Refused);
    assert_eq!(from_read.class, FactClass::Refused);

    // A known path is retained on the read side, but the project fact type is
    // never replaced by a path.
    assert_ne!(from_root.origin, from_read.origin);
    assert!(matches!(from_read.origin, FactOrigin::Candidate { .. }));
}

/// F6: a missing / null / non-string `checkedHash` is explicit missing evidence
/// — never a match, never a fabricated drift — and an empty or malformed value
/// stays on the existing non-match path while recording its input category.
#[test]
fn p2b_f6_checked_hash_input_categories_are_explicit() {
    let good = "a".repeat(64);

    // Absent / null / non-string: explicit categories, never a match.
    for (finding, expected_category) in [
        (json!({}), "字段缺失"),
        (json!({"checkedHash": null}), "字段为 null"),
        (json!({"checkedHash": 42}), "字段不是字符串"),
        (json!({"checkedHash": ["a"]}), "字段不是字符串"),
    ] {
        match classify_checked_hash(&finding) {
            CheckedHash::Absent(category) => assert_eq!(category, expected_category),
            other => panic!("expected Absent for {finding}, got {other:?}"),
        }
    }

    // Empty and malformed are present-but-unusable, a distinct category.
    for finding in [
        json!({"checkedHash": ""}),
        json!({"checkedHash": "sha256:"}),
        json!({"checkedHash": "not-a-hash"}),
        json!({"checkedHash": &"a".repeat(63)}),
        json!({"checkedHash": &"z".repeat(64)}),
    ] {
        assert!(
            matches!(classify_checked_hash(&finding), CheckedHash::Malformed { .. }),
            "{finding} must not be treated as a match"
        );
    }

    // A valid hash is the only matching category, with the prefix stripped.
    assert_eq!(
        classify_checked_hash(&json!({"checkedHash": format!("sha256:{good}")})),
        CheckedHash::Recorded(good.clone())
    );
    assert_eq!(
        classify_checked_hash(&json!({"checkedHash": good})),
        CheckedHash::Recorded(good)
    );
}

/// F6 at the real entry: a candidate whose finding carries **no** `checkedHash`
/// yields an explicit `EVIDENCE_UNAVAILABLE` Unconfirmed fact, and it blocks
/// `Confirmed` even when every leaf observation passed.
#[test]
fn p2b_f6_missing_hash_blocks_confirmation_even_with_all_passed_leaves() {
    let root = temp_root("missing-hash");
    std::fs::create_dir_all(root.join("in")).expect("mkdir in");
    std::fs::write(root.join("in/a.txt"), b"alpha").expect("write source");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let (database, run) = database_with_run("missing-hash", &root);

    let source_bytes = std::fs::read(root.join("in/a.txt")).expect("read source");
    let spec = v2_spec(v1_concat_rule(vec![source_ref(
        "in/a.txt",
        &sha256_hex(&source_bytes),
        source_bytes.len() as u64,
    )]));

    // No `checkedHash` key at all — the real production hole.
    let finding = json!({"boundPath": "out.txt", "checks": {}});
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &finding,
        true,
    );

    let missing = result
        .facts
        .iter()
        .find(|fact| fact.reason_code.as_deref() == Some(reason_code::EVIDENCE_UNAVAILABLE))
        .expect("a missing receipt must produce an explicit fact");
    assert_eq!(missing.class, FactClass::Unconfirmed);
    assert!(
        missing.detail.as_deref().unwrap_or_default().contains("回执哈希"),
        "{:?}",
        missing.detail
    );

    // The critical combination: the missing-hash fact folded in with a fully
    // Passed leaf outcome must still not confirm. Coverage is declared fully
    // discharged, so only the missing receipt can block — the merge is real, not
    // assumed, and the old `outcome: None` is not what protects it.
    let passed_leaf = CheckOutcome::single(passed("content:concat", "text_concat", &CandidateBinding::new(
        "out.txt",
        &sha256_hex(b"body"),
        b"body".len() as u64,
    )));
    let input = FactInput {
        spec: &spec,
        read_facts: vec![missing.clone()],
        drift_facts: vec![],
        outcome: Some(&passed_leaf),
        authorization_changed: false,
        coverage: Coverage::single_check("text_concat", true),
        context: None,
    };
    let merged = classify_and_decide(&input, true);
    assert!(merged.has_class(FactClass::Unconfirmed));
    assert!(merged.has_class(FactClass::Passed), "the pass is not dropped");
    assert_eq!(
        merged.class,
        DeliveryClass::Unconfirmed,
        "a missing receipt must survive an all-Passed leaf merge: {:?}",
        merged.facts
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// F6/§5: a real matching hash produces no drift fact, and a real mismatch is
/// refused with the frozen code — the read failure path never fabricates a hash
/// deviation.
#[test]
fn p2b_f6_valid_match_and_valid_mismatch_at_the_real_entry() {
    let root = temp_root("hash-branch");
    std::fs::create_dir_all(root.join("in")).expect("mkdir in");
    std::fs::write(root.join("in/a.txt"), b"alpha").expect("write source");
    std::fs::write(root.join("out.txt"), b"body").expect("write candidate");
    let (database, run) = database_with_run("hash-branch", &root);

    let source_bytes = std::fs::read(root.join("in/a.txt")).expect("read source");
    let candidate_sha = sha256_hex(b"body");
    let spec = v2_spec(v1_concat_rule(vec![source_ref(
        "in/a.txt",
        &sha256_hex(&source_bytes),
        source_bytes.len() as u64,
    )]));

    // A genuine match: no drift, no missing-hash fact.
    let matching = finding_with_hash(&candidate_sha, "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &matching,
        true,
    );
    assert!(
        !result
            .facts
            .iter()
            .any(|fact| fact.reason_code.as_deref() == Some(reason_code::CANDIDATE_HASH_DRIFT)),
        "a real match must not produce a drift: {:?}",
        result.facts
    );

    // A genuine mismatch: refused, with the frozen candidate-drift code.
    let drifted = finding_with_hash(&"0".repeat(64), "out.txt");
    let result = classify_v2_requirement(
        &database,
        &run,
        "",
        &spec,
        Some(root.join("out.txt").as_path()),
        &drifted,
        true,
    );
    assert_eq!(result.class, DeliveryClass::Refused);
    assert!(result.facts.iter().any(|fact| {
        fact.reason_code.as_deref() == Some(reason_code::CANDIDATE_HASH_DRIFT)
    }));

    let _ = std::fs::remove_dir_all(&root);
}

/// F7: unknown or partial coverage must not reach `Confirmed` at the entry, and
/// this holds in release mode because the block is a retained fact rather than a
/// `debug_assert!`.
#[test]
fn p2b_f7_unknown_or_partial_coverage_forbids_confirmed() {
    let spec = v2_spec(v1_concat_rule(vec![]));
    let leaf = CheckOutcome::single(passed("content:concat", "text_concat", &binding()));

    let classify = |coverage: Coverage| {
        let input = FactInput {
            spec: &spec,
            read_facts: vec![],
            drift_facts: vec![],
            outcome: Some(&leaf),
            authorization_changed: false,
            coverage,
            context: None,
        };
        classify_and_decide(&input, true)
    };

    // Unstated: no trustworthy completeness information.
    let unstated = classify(Coverage::Unstated);
    assert_ne!(unstated.class, DeliveryClass::Confirmed);
    assert_eq!(unstated.class, DeliveryClass::Unconfirmed);
    assert!(unstated.has_class(FactClass::Unconfirmed));

    // A declared-but-undischarged obligation: partial coverage.
    let partial = classify(Coverage::single_check("text_concat", false));
    assert_ne!(partial.class, DeliveryClass::Confirmed);
    assert_eq!(partial.class, DeliveryClass::Unconfirmed);

    // An empty declaration is not vacuously complete.
    let empty = classify(Coverage::test_declared(vec![]));
    assert_ne!(empty.class, DeliveryClass::Confirmed);
    assert_eq!(empty.class, DeliveryClass::Unconfirmed);

    // Only an explicitly discharged obligation set confirms. This is the
    // positive case, and it is reachable only by declaring the discharge.
    let complete = classify(Coverage::single_check("text_concat", true));
    assert_eq!(complete.class, DeliveryClass::Confirmed, "{:?}", complete.facts);

    // The pure local fold keeps its capability: `decide` over an all-Passed set
    // still reports the local reading, and the docs/entry carry the distinction.
    let local = decide(&[DecisionFact::passed(FactOrigin::candidate("out/report.txt", "b"))], true);
    assert_eq!(local.class, DeliveryClass::Confirmed);
}

/// §5: two sources with different bindings and distinct five-field locators are
/// retained separately, and a refusal plus a resource shortfall coexist without
/// either being dropped or merged.
#[test]
fn p2b_two_sources_refusal_and_resource_shortfall_are_all_retained() {
    use crate::runtime_host::delivery_checks_v2::{LimitKind, RecordLocator};

    let source_a = "in/a.txt";
    let source_b = "in/b.txt";
    let hash_a = "a".repeat(64);
    let hash_b = "b".repeat(64);

    // A refusal against source A, with a full five-field locator.
    let mut refusal = DecisionFact::refused(
        FactOrigin::source(source_a, &hash_a),
        reason_code::security::SOURCE_IDENTITY_MISMATCH,
        "来源 A 身份不符",
    );
    refusal.locator = Some(RecordLocator {
        record: Some(3),
        line: Some(7),
        field: Some(2),
        column: Some("amount".to_owned()),
        byte_offset: Some(128),
    });

    // A resource shortfall against source B, with its own visible limit kind.
    let mut shortfall = classify_read_failure(
        FactOrigin::source(source_b, &hash_b),
        &DeliveryReadFailure::Resource { limit: 4096 },
    );
    shortfall.limit_kind = Some(LimitKind::ScanBytes);

    let facts = vec![refusal.clone(), shortfall.clone()];
    let result = decide(&facts, true);

    assert_eq!(result.class, DeliveryClass::Refused);
    assert_eq!(result.facts.len(), 2, "neither fact may be dropped");
    assert_ne!(
        result.facts[0].origin, result.facts[1].origin,
        "the two sources stay distinguishable"
    );
    assert_eq!(result.facts[0].origin, FactOrigin::source(source_a, &hash_a));
    assert_eq!(result.facts[1].origin, FactOrigin::source(source_b, &hash_b));

    // The locator survives whole, and the byte ceiling is not confused with the
    // visible ceiling kind.
    let locator = result.facts[0].locator.clone().expect("the locator survives");
    assert_eq!(locator.record, Some(3));
    assert_eq!(locator.line, Some(7));
    assert_eq!(locator.field, Some(2));
    assert_eq!(locator.column.as_deref(), Some("amount"));
    assert_eq!(locator.byte_offset, Some(128));
    assert_eq!(result.facts[1].limit, Some(4096));
    assert_eq!(result.facts[1].limit_kind, Some(LimitKind::ScanBytes));
    assert_eq!(result.facts[1].class, FactClass::Unconfirmed);
}

