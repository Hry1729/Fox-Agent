//! Package 2-A tests: Host strict spec decoding, typed delivery reads, and
//! the legacy production behaviour they must not change.
//!
//! Registered from `delivery.rs` with `#[cfg(test)]` only; nothing here is
//! compiled into production builds. The three groups map to the card's three
//! verification groups:
//!
//! 1. `decode_*` — the pure decode matrix (no I/O, no activation).
//! 2. `typed_*` — the typed read twins' classification at their real
//!    production sites (temp dirs; OS errors that are impractical to trigger
//!    are covered as documented unit tests).
//! 3. `legacy_*` — characterization of the *unchanged* legacy path, including
//!    the fact that a schemaVersion-2 artifact still records `unverified` with
//!    `passed` untouched (the completion gate is a later package's job).
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use crate::database::{
    Database, DeliveryChecklistItem, DeliveryChecklistSeed, DeliveryRequirement, RequirementKind,
};
use serde_json::{json, Value};

use crate::runtime_host::attachment_compute::{
    authorized_delivery_root_typed, read_authorized_delivery_bytes_typed, DeliveryReadFailure,
};
use super::content_requirements::{decode_spec, model_reference, DecodedSpec};
use super::ItemVerdict;

// ---------------------------------------------------------------------------
// Group 1: decode matrix
// ---------------------------------------------------------------------------

/// A real stored v1 `rule` payload. Note the field spelling: `Contract`'s
/// `rename_all = "camelCase"` renames only the *variant* names, so its variant
/// fields stay snake_case (`selection_order`), while `Source` carries its own
/// camelCase rename (`firstLine`). Both spellings are what the production
/// writer (`serde_json::to_value(contract)`) actually emits.
fn v1_concat_spec() -> Value {
    json!({
        "schemaVersion": 1,
        "rule": {
            "type": "concat",
            "sources": [{
                "path": "in/a.txt",
                "sha256": "a".repeat(64),
                "bytes": 5,
                "firstLine": 1,
                "lastLine": 1
            }],
            "selected": ["in/a.txt"],
            "selection_order": "bytes_ascending",
            "tie_order": "filename_ascending",
            "merge_order": "filename_ascending",
            "read_only": true
        }
    })
}

fn v2_spec(policy: Value) -> Value {
    json!({
        "schemaVersion": 2,
        "envelopeVersion": 2,
        "checkerVersion": 2,
        "ruleId": "content:concat",
        "policy": policy,
        "rule": v1_concat_spec()["rule"].clone()
    })
}

fn default_policy() -> Value {
    json!({
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
    })
}

fn invalid_reason(spec: &Value) -> String {
    match decode_spec(spec) {
        DecodedSpec::Invalid(reason) => reason,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn p2a_decode_v1_ignores_extra_top_level_keys_but_rejects_inside_rule() {
    // Extra top-level keys keep the historical behaviour: ignored. This is the
    // documented §60 compatibility rule (the production writer never emits them).
    let mut spec = v1_concat_spec();
    spec["envelopeVersion"] = json!(2);
    spec["checkerVersion"] = json!(2);
    spec["policy"] = default_policy();
    spec["ruleId"] = json!("content:concat");
    let decoded = decode_spec(&spec);
    assert!(
        matches!(decoded, DecodedSpec::V1(..)),
        "extra top-level keys must stay ignored, got {decoded:?}"
    );
    // The legacy reader still reads only `rule`.
    assert_eq!(model_reference(&spec)["type"], json!("concat"));

    // An unknown key *inside* rule is rejected by the existing strict Contract.
    let mut broken = v1_concat_spec();
    broken["rule"]["surprise"] = json!(1);
    let reason = invalid_reason(&broken);
    assert!(reason.contains("内容契约字段不完整"), "{reason}");
}

#[test]
fn p2a_decode_missing_or_malformed_schema_never_falls_back_to_v1() {
    for spec in [
        json!({"rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": null, "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": "1", "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": 1.5, "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": true, "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": 3, "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": [1], "rule": v1_concat_spec()["rule"].clone()}),
        json!({"schemaVersion": {"n": 1}, "rule": v1_concat_spec()["rule"].clone()}),
    ] {
        let reason = invalid_reason(&spec);
        assert!(
            reason.contains("schemaVersion"),
            "reason must name the schema problem: {reason}"
        );
    }
}

#[test]
fn p2a_decode_v2_complete_spec_is_accepted() {
    let spec = v2_spec(default_policy());
    match decode_spec(&spec) {
        DecodedSpec::V2(decoded) => {
            assert_eq!(decoded.envelope.schema_version, 2);
            assert_eq!(decoded.envelope.envelope_version, 2);
            assert_eq!(decoded.envelope.checker_version, 2);
            assert_eq!(
                decoded.envelope.rule_id.as_deref(),
                Some("content:concat")
            );
            // The executable rule is decoded through the same strict Contract
            // reader v1 uses, and is carried for package 2-D.
            assert!(
                matches!(decoded.contract, super::content_requirements::Contract::Concat { .. }),
                "a legal concat rule must decode as the Concat contract"
            );
        }
        other => panic!("expected V2, got {other:?}"),
    }
}

#[test]
fn p2a_decode_v2_requires_envelope_and_checker_two() {
    let rule = v1_concat_spec()["rule"].clone();
    for spec in [
        json!({"schemaVersion": 2, "checkerVersion": 2, "policy": default_policy(), "rule": rule}),
        json!({"schemaVersion": 2, "envelopeVersion": 2, "policy": default_policy(), "rule": rule}),
        json!({"schemaVersion": 2, "envelopeVersion": 1, "checkerVersion": 2, "rule": rule}),
        json!({"schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 1, "rule": rule}),
        json!({"schemaVersion": 2, "envelopeVersion": 3, "checkerVersion": 2, "rule": rule}),
        json!({"schemaVersion": 2, "envelopeVersion": "2", "checkerVersion": 2, "rule": rule}),
    ] {
        let reason = invalid_reason(&spec);
        assert!(
            reason.contains("v2"),
            "reason must name the v2 requirement: {reason}"
        );
    }
}

#[test]
fn p2a_decode_v2_rejects_unknown_fields_and_bad_enums() {
    // Unknown top-level key.
    let mut spec = v2_spec(default_policy());
    spec["surprise"] = json!(1);
    assert!(invalid_reason(&spec).contains("strict") || invalid_reason(&spec).contains("unknown"));
    let reason = invalid_reason(&spec);
    assert!(reason.contains("v2"), "{reason}");

    // Unknown policy key.
    let mut policy = default_policy();
    policy["surprise"] = json!(1);
    assert!(invalid_reason(&v2_spec(policy)).contains("v2"));

    // Unknown table key.
    let mut policy = default_policy();
    policy["table"]["surprise"] = json!(1);
    assert!(invalid_reason(&v2_spec(policy)).contains("v2"));

    // Bad enums at every level.
    for bad in [
        json!({"comparison": "nope"}),
        json!({"eof": "nope"}),
        json!({"table": {"lineEnding": "nope"}}),
        json!({"table": {"delimiter": "nope"}}),
        json!({"table": {"bom": "nope"}}),
        json!({"table": {"blankLine": "nope"}}),
        json!({"table": {"header": "nope"}}),
        json!({"table": {"quote": "nope"}}),
    ] {
        let rendered = bad.canonical_string();
        assert!(
            invalid_reason(&v2_spec(bad)).contains("v2"),
            "bad policy must be rejected: {rendered}"
        );
    }
}

#[test]
fn p2a_decode_v2_policy_absent_means_defaults_but_null_is_rejected() {
    // Absent policy: the strict defaults (byte-exact) apply.
    let mut spec = v2_spec(default_policy());
    spec.as_object_mut().unwrap().remove("policy");
    match decode_spec(&spec) {
        DecodedSpec::V2(decoded) => {
            assert_eq!(decoded.envelope.policy.comparison, super::content_requirements::V2Comparison::BytesExact);
            assert_eq!(decoded.envelope.policy.eof, super::content_requirements::V2Eof::ByteExact);
            assert_eq!(decoded.envelope.policy.table.line_ending, super::content_requirements::V2LineEnding::Any);
        }
        other => panic!("expected V2, got {other:?}"),
    }

    // Explicit null is not silently "default".
    let mut spec = v2_spec(default_policy());
    spec["policy"] = Value::Null;
    let reason = invalid_reason(&spec);
    assert!(reason.contains("null"), "{reason}");
}

#[test]
fn p2a_decode_v2_all_declared_line_endings_are_legal() {
    for line_ending in ["any", "lf", "crlf"] {
        let mut policy = default_policy();
        policy["table"]["lineEnding"] = json!(line_ending);
        assert!(
            matches!(decode_spec(&v2_spec(policy)), DecodedSpec::V2(_)),
            "lineEnding {line_ending} must be accepted"
        );
    }
}

/// A v2 envelope must carry a rule the existing strict `Contract` reader can
/// decode. Every non-executable shape is `Invalid` — a v2 envelope with no
/// decodable rule would leave the checker with nothing to run. (Deferring v2
/// concerns *new* rule variants/policy semantics, never "rule may be absent".)
#[test]
fn p2a_decode_v2_without_an_executable_rule_is_invalid() {
    // Missing key, explicit null, wrong JSON type, unknown tag, incomplete
    // concat, and a legal concat carrying an unknown key (which v1 also
    // rejects via `deny_unknown_fields`).
    let mut unknown_key_rule = v1_concat_spec()["rule"].clone();
    unknown_key_rule["surprise"] = json!(1);
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("absent", None),
        ("null", Some(Value::Null)),
        ("string", Some(json!("not an object"))),
        ("array", Some(json!([1, 2]))),
        ("number", Some(json!(7))),
        ("empty object", Some(json!({}))),
        ("unknown type", Some(json!({"type": "nope"}))),
        ("incomplete concat", Some(json!({"type": "concat"}))),
        ("legal concat + unknown key", Some(unknown_key_rule)),
    ];
    for (label, rule) in cases {
        let mut spec = v2_spec(default_policy());
        match rule {
            None => {
                spec.as_object_mut().unwrap().remove("rule");
            }
            Some(rule) => spec["rule"] = rule,
        }
        let decoded = decode_spec(&spec);
        assert!(
            matches!(decoded, DecodedSpec::Invalid(_)),
            "v2 rule `{label}` must be Invalid, got {decoded:?}"
        );
    }
}

/// ...while every legal existing rule family stays a valid V2 envelope, so the
/// stricter rule gate does not reject real contracts.
#[test]
fn p2a_decode_v2_with_a_legal_contract_rule_is_accepted() {
    let mut unbound = v2_spec(default_policy());
    unbound["rule"] = json!({"type": "unbound", "reason": "无法确认"});
    assert!(
        matches!(decode_spec(&unbound), DecodedSpec::V2(_)),
        "an Unbound rule is a legal Contract variant"
    );

    let concat = v2_spec(default_policy());
    match decode_spec(&concat) {
        DecodedSpec::V2(decoded) => assert!(matches!(
            decoded.contract,
            super::content_requirements::Contract::Concat { .. }
        )),
        other => panic!("expected V2 for a legal concat rule, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Group 2: typed reads
// ---------------------------------------------------------------------------

/// A writable scratch directory. Derived from the test executable's own
/// target directory (`<target>/debug/deps/<exe>` → `<target>/debug`), which is
/// always writable for a test run and needs no fixed absolute path; falls back
/// to the system temp dir only if the executable path is unavailable.
fn temp_root(label: &str) -> std::path::PathBuf {
    let base = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
        .and_then(|deps| deps.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir);
    let root = base
        .join("p2a-tmp")
        .join(format!("{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("temp root");
    root
}

#[test]
fn p2a_typed_read_succeeds_for_a_regular_bounded_file() {
    let root = temp_root("read-ok");
    std::fs::write(root.join("a.txt"), b"alpha").expect("write");
    let bytes = read_authorized_delivery_bytes_typed(&root, "a.txt", 64 * 1024 * 1024)
        .expect("regular file inside the budget");
    assert_eq!(bytes, b"alpha");
    let _ = std::fs::remove_dir_all(&root);
}

/// On Windows the open of a directory fails before any metadata is read, so an
/// OS access denial is what the classifier sees. An OS denial proves only that
/// the OS would not serve this read — it is **not** a project-authorization
/// fact, so it must stay `Io` and must not become `AuthorizationRefused`
/// (which the later acceptance gate excludes). The independent `non_regular`
/// branch is defensive for objects that *do* open but are not regular files;
/// directory opens fail first on this platform, and this test does not claim
/// to cover that branch dynamically.
#[test]
fn p2a_typed_read_directory_is_not_an_authorization_refusal() {
    let root = temp_root("dir");
    std::fs::create_dir_all(root.join("adir")).expect("mkdir");
    let failure = read_authorized_delivery_bytes_typed(&root, "adir", 64 * 1024 * 1024)
        .expect_err("a directory must never read as bytes");
    assert!(
        !matches!(failure, DeliveryReadFailure::AuthorizationRefused { .. }),
        "a directory / OS denial is not project-authorization refusal, got {failure:?}"
    );
    match &failure {
        DeliveryReadFailure::Io(detail) => assert!(
            !detail.is_empty(),
            "the OS reason must be preserved for audit"
        ),
        other => panic!("an OS access denial must classify as Io, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The classifier keeps an OS `PermissionDenied` as `Io` with its original
/// message: no project-authorization conclusion is drawn from an OS error.
/// Unit-level (an OS denial cannot be raised on demand), and it is the direct
/// classifier contract rather than a directory-driven path.
#[test]
fn p2a_classify_os_permission_denied_is_io_with_its_original_reason() {
    let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "拒绝访问。");
    match crate::runtime_host::attachment_compute::classify_io_for_test(error) {
        DeliveryReadFailure::Io(message) => {
            assert!(message.contains("拒绝访问"), "reason preserved: {message}");
        }
        other => panic!("an OS denial must be Io, got {other:?}"),
    }
}

#[test]
fn p2a_typed_read_too_large_is_a_resource_with_its_limit() {
    let root = temp_root("too-large");
    std::fs::write(root.join("big.txt"), b"0123456789").expect("write");
    match read_authorized_delivery_bytes_typed(&root, "big.txt", 4) {
        Err(DeliveryReadFailure::Resource { limit }) => assert_eq!(limit, 4),
        other => panic!("expected Resource, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p2a_typed_read_identity_change_is_io_not_a_security_conclusion() {
    // A changed-during-read condition must never be dressed up as tampering.
    // This asserts the classifier's contract directly: an unknown io error
    // stays `Io` with its message intact.
    let error = std::io::Error::new(std::io::ErrorKind::Other, "changed during read");
    match crate::runtime_host::attachment_compute::classify_io_for_test(error) {
        DeliveryReadFailure::Io(message) => assert!(message.contains("changed during read")),
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn p2a_typed_root_without_a_frozen_binding_is_evidence_unavailable() {
    let root = temp_root("root-binding");
    let database = Database::open(root.join("fox.db")).expect("open database");
    match authorized_delivery_root_typed(&database, "no-such-run") {
        Err(DeliveryReadFailure::EvidenceUnavailable { detail }) => {
            assert!(detail.contains("Run binding"), "{detail}");
        }
        other => panic!("expected EvidenceUnavailable, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

// ---------------------------------------------------------------------------
// Group 2b: real-DB attribution of the root lookup
// ---------------------------------------------------------------------------

/// Freeze a real `RunControlBinding` for a conversation. Mirrors the fixture
/// the independent probes used; no real project authorization is touched.
fn p2a_freeze_binding(
    database: &Database,
    run: &str,
    conversation: &str,
    project_root: Option<&str>,
) {
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

/// A frozen binding with **no** project root is genuinely missing evidence.
#[test]
fn p2a_typed_root_missing_project_root_is_evidence_unavailable() {
    let root = temp_root("no-root");
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
    p2a_freeze_binding(&database, &run, &conversation.id, None);

    match authorized_delivery_root_typed(&database, &run) {
        Err(DeliveryReadFailure::EvidenceUnavailable { detail }) => {
            assert!(detail.contains("project root"), "{detail}");
        }
        other => panic!("expected EvidenceUnavailable, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// The conversation no longer grants a project root at all, while the frozen
/// binding names one: the authorization that existed at freeze time has been
/// **explicitly removed**, which is a refusal — not missing evidence.
#[test]
fn p2a_typed_root_removed_project_access_is_authorization_refused() {
    let root = temp_root("access-removed");
    let database = Database::open(root.join("fox.db")).expect("open database");
    let conversation = database
        .create_conversation(database.default_agent_id(), None, None, None)
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    p2a_freeze_binding(
        &database,
        &run,
        &conversation.id,
        Some(root.to_str().expect("root path")),
    );

    match authorized_delivery_root_typed(&database, &run) {
        Err(DeliveryReadFailure::AuthorizationRefused { detail }) => {
            assert!(
                detail.contains("Project access was removed"),
                "the explicit withdrawal must be named: {detail}"
            );
        }
        other => panic!("expected AuthorizationRefused, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A stored binding that fails its own structural validation is unusable
/// *evidence*, never a withdrawn authorization. `run_control_binding()`'s own
/// `encode()`/`validate()` rejects it first (the restart/version-skew path),
/// so the twin must report `Io` — hard-asserted here, not printed.
#[test]
fn p2a_typed_root_structurally_invalid_binding_is_io() {
    use fox_engine_protocol::{
        ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding,
        TimeBudgets,
    };
    let root = temp_root("invalid-binding");
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
    // Freeze a valid binding first, then replace the stored JSON with one that
    // `validate()` rejects — the same seam the existing corruption tests use.
    // `Database`'s own encode() validates before storing, so this is the only
    // reachable way to persist such a record.
    p2a_freeze_binding(
        &database,
        &run,
        &conversation.id,
        Some(root.to_str().expect("root path")),
    );
    let mut budgets = TimeBudgets::default();
    budgets.model_request_ms = 0; // validate() must reject this
    let permission = FrozenPermission {
        mode: PermissionMode::Allow,
        project_root: Some(root.to_string_lossy().into_owned()),
        grants: vec![],
        approval_epoch: None,
    };
    let invalid = RunControlBinding {
        schema_version: 1,
        run_id: run.clone(),
        conversation_id: conversation.id.clone(),
        engine_id: "pi".into(),
        execution_profile_id: "legacy".into(),
        authority: ExecutionAuthority::Authoritative,
        read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
        permission,
        budgets,
    };
    let stored = crate::canonical_json::to_string(&invalid).expect("serialize binding");
    // Keep the stored hash consistent so the hash gate passes and the binding's
    // own validation is the gate that rejects it.
    use sha2::{Digest, Sha256};
    let stored_hash = format!("sha256:{}", hex::encode(Sha256::digest(stored.as_bytes())));
    database
        .with_connection(|connection| {
            connection.execute(
                "UPDATE run_control_bindings SET binding_json=?2, binding_hash=?3 WHERE run_id=?1",
                rusqlite::params![run.as_str(), stored.as_str(), stored_hash.as_str()],
            )
        })
        .expect("inject a stored binding that fails validate()");

    match authorized_delivery_root_typed(&database, &run) {
        Err(DeliveryReadFailure::Io(detail)) => {
            assert!(
                !detail.is_empty(),
                "the structural reason must be preserved: {detail}"
            );
        }
        other => panic!(
            "a structurally invalid frozen binding is unusable evidence (Io), got {other:?}"
        ),
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// A binding whose frozen root no longer matches the live authorization root:
/// an explicit mismatch, which is a refusal.
#[test]
fn p2a_typed_root_authorization_change_is_authorization_refused() {
    let frozen = temp_root("auth-frozen");
    let live = temp_root("auth-live");
    let database = Database::open(live.join("fox.db")).expect("open database");
    let conversation = database
        .create_conversation(
            database.default_agent_id(),
            None,
            Some(live.to_str().expect("live path")),
            Some("read_only"),
        )
        .expect("conversation");
    let run = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    // The frozen binding names a *different* (but still task-owned) directory
    // than the one the conversation currently authorizes.
    p2a_freeze_binding(
        &database,
        &run,
        &conversation.id,
        Some(frozen.to_str().expect("frozen path")),
    );

    match authorized_delivery_root_typed(&database, &run) {
        Err(DeliveryReadFailure::AuthorizationRefused { detail }) => {
            assert!(
                detail.contains("Project authorization changed"),
                "the mismatch must be named: {detail}"
            );
        }
        other => panic!("expected AuthorizationRefused, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&frozen);
    let _ = std::fs::remove_dir_all(&live);
}

#[test]
fn p2a_legacy_schema1_with_extra_top_level_keys_still_reads_as_v1() {
    // The historical reader ignores extra top-level keys, and the decoder
    // agrees, so a schema1 record carrying a stray `policy` still runs v1.
    let mut spec = v1_concat_spec();
    spec["policy"] = default_policy();
    spec["envelopeVersion"] = json!(2);
    let decoded = decode_spec(&spec);
    assert!(
        matches!(decoded, DecodedSpec::V1(..)),
        "a schema1 record with a stray policy must stay v1, got {decoded:?}"
    );
    assert_eq!(model_reference(&spec)["type"], json!("concat"));
    assert_eq!(model_reference(&spec)["schemaVersion"], json!(1));
}

#[test]
fn p2a_legacy_schema2_records_unverified_but_keeps_passed() {
    // Characterization of the *current* chain, exactly as the card demands: a
    // bound artifact carrying schemaVersion 2 is recorded `unverified` by the
    // version gate, `verdict.passed` stays true, and the completion gate can
    // therefore still pass. This is a known gap the later package must close;
    // the assertion pins the current behaviour so the change is deliberate.
    let root = temp_root("legacy-schema2");
    let database = Database::open(root.join("fox.db")).expect("open database");
    // `delivery_checklist_items.run_id` references `runs(id)`, so a real
    // conversation + Run row is needed before the checklist can be seeded.
    let conversation = database
        .create_conversation(
            database.default_agent_id(),
            None,
            Some(root.to_str().expect("root path")),
            Some("read_only"),
        )
        .expect("conversation");
    let run_id = database
        .create_run(&conversation.id, "生成一份报告", None)
        .expect("run")
        .run
        .id;
    let spec = v2_spec(default_policy());
    let seeds = vec![DeliveryChecklistSeed {
        item_key: "out/report.txt".to_owned(),
        target_path: Some("out/report.txt".to_owned()),
        artifact_id: None,
        display_name: "report".to_owned(),
        checks: vec![],
        requirements: vec![DeliveryRequirement {
            item_key: Some("out/report.txt".to_owned()),
            id: "content:concat".to_owned(),
            kind: RequirementKind::ContentContract { spec },
            source_text: "合并输入".to_owned(),
        }],
    }];
    database
        .seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .expect("seed");
    let checklist = vec![DeliveryChecklistItem {
        item_key: "out/report.txt".to_owned(),
        target_path: Some("out/report.txt".to_owned()),
        artifact_id: None,
        display_name: "report".to_owned(),
        checks: vec![],
        status: "pending".to_owned(),
        finding: None,
        checked_at: None,
        updated_at: 0,
        inherited_from_run_id: None,
    }];
    let mut verdicts = vec![ItemVerdict {
        item_key: "out/report.txt".to_owned(),
        passed: true,
        finding_json: json!({"checks": {}}).canonical_string(),
        bound_path: None,
    }];

    super::content_requirements::apply(&database, &run_id, &checklist, &mut verdicts)
        .expect("apply keeps its legacy signature");

    let verdict = &verdicts[0];
    assert!(
        verdict.passed,
        "legacy behaviour: an unverified contract does not clear the artifact"
    );
    let finding: Value = serde_json::from_str(&verdict.finding_json).expect("finding json");
    let entry = &finding["checks"]["content:concat"];
    assert_eq!(entry["state"], json!("unverified"));
    assert_eq!(finding["verificationStatus"], json!("unverified"));
    let reason = entry["reason"].as_str().unwrap_or_default();
    // Package 2-E R-1: a `schemaVersion == 2` record is decided by the typed
    // chain, so this lane records *that* reason — the accurate routing note —
    // not the generic unsupported-version one. The assertion is the exact string
    // on purpose: a broad `contains("版本")` (or an OR over the old and the new
    // wording) would also be satisfied by a record that was wrongly routed here,
    // which is precisely the regression this pins. The state and `passed`
    // assertions above are the behaviour contract and are unchanged.
    assert_eq!(
        reason,
        "内容契约 v2 由托管分类链核验，本链路不解码",
        "a schemaVersion-2 record must record the typed-chain routing reason, \
         not the generic unsupported-version reason: {reason}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn p2a_legacy_other_unsupported_schema_keeps_the_fail_closed_reason() {
    // Package 2-E R-1, companion: only `schemaVersion == 2` gets the typed-chain
    // routing reason. Every other non-v1 schema is a genuinely unknown version on
    // this lane and must keep the original fail-closed reason, so a future route
    // change cannot quietly widen the v2 carve-out.
    for schema in [0u64, 3, 4, 99] {
        let root = temp_root("legacy-other-schema");
        let database = Database::open(root.join("fox.db")).expect("open database");
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                None,
                Some(root.to_str().expect("root path")),
                Some("read_only"),
            )
            .expect("conversation");
        let run_id = database
            .create_run(&conversation.id, "生成一份报告", None)
            .expect("run")
            .run
            .id;
        let mut spec = v2_spec(default_policy());
        spec["schemaVersion"] = json!(schema);
        let seeds = vec![DeliveryChecklistSeed {
            item_key: "out/report.txt".to_owned(),
            target_path: Some("out/report.txt".to_owned()),
            artifact_id: None,
            display_name: "report".to_owned(),
            checks: vec![],
            requirements: vec![DeliveryRequirement {
                item_key: Some("out/report.txt".to_owned()),
                id: "content:concat".to_owned(),
                kind: RequirementKind::ContentContract { spec },
                source_text: "合并输入".to_owned(),
            }],
        }];
        database
            .seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
            .expect("seed");
        let checklist = vec![DeliveryChecklistItem {
            item_key: "out/report.txt".to_owned(),
            target_path: Some("out/report.txt".to_owned()),
            artifact_id: None,
            display_name: "report".to_owned(),
            checks: vec![],
            status: "pending".to_owned(),
            finding: None,
            checked_at: None,
            updated_at: 0,
            inherited_from_run_id: None,
        }];
        let mut verdicts = vec![ItemVerdict {
            item_key: "out/report.txt".to_owned(),
            passed: true,
            finding_json: json!({"checks": {}}).canonical_string(),
            bound_path: None,
        }];

        super::content_requirements::apply(&database, &run_id, &checklist, &mut verdicts)
            .expect("apply keeps its legacy signature");

        let finding: Value =
            serde_json::from_str(&verdicts[0].finding_json).expect("finding json");
        let entry = &finding["checks"]["content:concat"];
        assert_eq!(entry["state"], json!("unverified"), "schema {schema}");
        assert_eq!(
            finding["verificationStatus"],
            json!("unverified"),
            "schema {schema}"
        );
        let reason = entry["reason"].as_str().unwrap_or_default();
        assert_eq!(
            reason, "内容契约版本不支持",
            "schema {schema} is not v2, so it must keep the fail-closed reason: {reason}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
