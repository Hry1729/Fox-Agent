//! Package 2-C1 §1: version-layer interop between the frozen **leaf** rule
//! payload / envelope (`delivery_checks_v2`) and the Host-side strict contract
//! decoder (`delivery_content_requirements::decode_spec`).
//!
//! The five version layers are independent numbers, and a difference between
//! them is not by itself a defect:
//!
//! | layer | marker | value |
//! |---|---|---|
//! | Host contract selector | `spec.schemaVersion` | 1 (production gate) |
//! | frozen rule payload | `delivery_checks_v2::RULE_PAYLOAD_SCHEMA_VERSION` | 1 |
//! | leaf rule envelope | `envelopeVersion` | 1 / 2 |
//! | leaf checker | `checkerVersion` | 1 / 2 |
//! | persistent verdict record | `verdictSchemaVersion`/`verdictEnvelopeVersion` | 2 / 2 |
//!
//! The leaf module's own doc and round-trip test already pin the shape
//! `{"schemaVersion": 1, "rule": …, "envelopeVersion": 2, "checkerVersion": 2}`:
//! v1 readers ignore the extra keys. The Host selector stays `schemaVersion`, so
//! this crate does **not** bump `RULE_PAYLOAD_SCHEMA_VERSION` to make the markers
//! agree, and it does not rewrite the frozen round-trip test. These cases prove
//! the two readings coexist and that the adaptation point is explicit.
//!
//! Nothing here changes production reachability: `apply` still evaluates only
//! `schemaVersion == 1`, so a decoded V2 remains unreachable from a delivery
//! outcome until package 2-D opens that gate.

use super::content_requirements::{DecodedSpec, decode_spec};
use super::super::delivery_checks_v2::{
    CHECKER_VERSION_V2, ENVELOPE_VERSION_V2, RULE_PAYLOAD_SCHEMA_VERSION, RuleEnvelope, RulePolicy,
};
use serde_json::{Value, json};

/// The frozen `Contract` payload tag is `type` (see `Contract`'s serde
/// attribute) — a different key from `RequirementKind`'s `kind`.
fn unbound_rule() -> Value {
    json!({"type": "unbound", "reason": "未绑定"})
}

/// A Host-acceptable v2 spec: `schemaVersion 2` plus the strict envelope, with the
/// executable rule under the **same** `rule` key the v1 payload uses.
///
/// This is the shape a future Host writes once v2 is activated. The adaptation
/// point is exactly one: the rule body is decoded by the *same* frozen leaf reader
/// v1 uses, so activating v2 does not fork the rule language.
fn host_acceptable_v2_spec() -> Value {
    json!({
        "schemaVersion": 2,
        "envelopeVersion": ENVELOPE_VERSION_V2,
        "checkerVersion": CHECKER_VERSION_V2,
        "ruleId": "content:concat",
        "policy": {"comparison": "textSegments", "eof": "allowSingleTrailingLf"},
        "rule": unbound_rule(),
    })
}

#[test]
fn c1_a_legal_host_v2_spec_decodes_as_v2_with_the_frozen_rule_payload_untouched() {
    assert_eq!(
        RULE_PAYLOAD_SCHEMA_VERSION, 1,
        "the leaf rule payload version is frozen; the Host selector is a different layer"
    );

    let spec = host_acceptable_v2_spec();
    match decode_spec(&spec) {
        DecodedSpec::V2(rule) => {
            assert_eq!(rule.envelope.schema_version, 2);
            assert_eq!(rule.envelope.envelope_version, ENVELOPE_VERSION_V2);
            assert_eq!(rule.envelope.checker_version, CHECKER_VERSION_V2);
            assert_eq!(rule.envelope.rule_id.as_deref(), Some("content:concat"));
            // The rule body decodes through the frozen leaf reader unchanged:
            // `Contract` has no `PartialEq`, so compare its serialized form.
            assert_eq!(serde_json::to_value(&rule.contract).unwrap(), unbound_rule());
        }
        other => panic!("a legal Host v2 spec must decode as V2, got {other:?}"),
    }
}

/// The disagreement the card asks to be *documented and pinned* rather than
/// "fixed" by bumping a version: for one and the same record, the Host selector
/// and the leaf envelope marker read two different fields and can therefore
/// legitimately reach two different answers.
#[test]
fn c1_the_host_selector_and_the_leaf_envelope_marker_read_different_fields() {
    let v1_beside_v2_markers = json!({
        "schemaVersion": RULE_PAYLOAD_SCHEMA_VERSION,
        "rule": unbound_rule(),
        "envelopeVersion": ENVELOPE_VERSION_V2,
        "ruleId": "content:concat",
        "checkerVersion": CHECKER_VERSION_V2,
        "policy": {"comparison": "textSegments"},
    });

    // The Host gate reads `schemaVersion` and selects the frozen v1 contract; the
    // v2-only keys beside it are ignored rather than rejected.
    match decode_spec(&v1_beside_v2_markers) {
        DecodedSpec::V1(contract) => {
            assert_eq!(serde_json::to_value(&contract).unwrap(), unbound_rule())
        }
        other => panic!("schemaVersion 1 stays v1 beside extra keys, got {other:?}"),
    }

    // The leaf reader reads `envelopeVersion` for the *leaf* layer, so the same
    // record is a v2 leaf envelope. Two layers, two questions, one record.
    let leaf = RuleEnvelope::from_spec(&v1_beside_v2_markers).expect("the leaf envelope must read");
    assert_eq!(leaf.envelope_version, ENVELOPE_VERSION_V2);
    assert!(
        !leaf.is_v1(),
        "the leaf layer resolves this record by envelopeVersion, not schemaVersion"
    );
    assert_eq!(leaf.rule_id.as_deref(), Some("content:concat"));
}

/// The Host decoder accepts what the leaf envelope writer itself produces, so the
/// two are interoperable without changing either version constant.
#[test]
fn c1_the_host_decoder_accepts_what_the_leaf_envelope_writer_produces() {
    let v2_spec = merge_leaf_envelope(
        json!({"schemaVersion": 2, "rule": unbound_rule()}),
        &RuleEnvelope::v2("content:concat", RulePolicy::default()),
    );
    match decode_spec(&v2_spec) {
        DecodedSpec::V2(rule) => {
            assert_eq!(rule.envelope.envelope_version, ENVELOPE_VERSION_V2);
            assert_eq!(rule.envelope.checker_version, CHECKER_VERSION_V2);
        }
        other => panic!("the leaf writer's v2 output must be Host-decodable, got {other:?}"),
    }

    let v1_spec = merge_leaf_envelope(
        json!({"schemaVersion": 1, "rule": unbound_rule()}),
        &RuleEnvelope::v1(),
    );
    match decode_spec(&v1_spec) {
        DecodedSpec::V1(contract) => {
            assert_eq!(serde_json::to_value(&contract).unwrap(), unbound_rule())
        }
        other => panic!("a v1 leaf envelope must stay on the v1 branch, got {other:?}"),
    }
}

/// A missing, unknown or malformed selector must never be read as the permissive
/// version, and a v2 selector with mismatched extra markers is refused outright.
#[test]
fn c1_an_unusable_selector_never_falls_back_to_v1() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "missing schemaVersion",
            json!({"rule": unbound_rule()}),
        ),
        (
            "null schemaVersion",
            json!({"schemaVersion": null, "rule": unbound_rule()}),
        ),
        (
            "non-numeric schemaVersion",
            json!({"schemaVersion": "2", "rule": unbound_rule()}),
        ),
        (
            "unsupported schemaVersion",
            json!({"schemaVersion": 3, "rule": unbound_rule()}),
        ),
        (
            "v2 with envelopeVersion 1",
            json!({
                "schemaVersion": 2, "envelopeVersion": 1, "checkerVersion": 2,
                "rule": unbound_rule(),
            }),
        ),
        (
            "v2 with checkerVersion 1",
            json!({
                "schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 1,
                "rule": unbound_rule(),
            }),
        ),
        (
            "v2 without an executable rule",
            json!({"schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 2}),
        ),
        (
            "v2 with an unknown envelope key",
            json!({
                "schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 2,
                "rule": unbound_rule(), "unexpectedKey": true,
            }),
        ),
        (
            "v2 with an explicit null policy",
            json!({
                "schemaVersion": 2, "envelopeVersion": 2, "checkerVersion": 2,
                "policy": null, "rule": unbound_rule(),
            }),
        ),
        (
            "v1 whose rule payload is absent",
            json!({"schemaVersion": 1}),
        ),
    ];
    for (label, spec) in cases {
        match decode_spec(&spec) {
            DecodedSpec::Invalid(reason) => {
                assert!(!reason.is_empty(), "{label}: reason must be stable")
            }
            other => panic!("{label} must be Invalid, not {other:?}"),
        }
    }
}

/// Merge a leaf envelope's own keys into a spec, the way a Host writer would.
fn merge_leaf_envelope(mut spec: Value, envelope: &RuleEnvelope) -> Value {
    let keys = envelope.to_spec_keys();
    let object = keys
        .as_object()
        .expect("the leaf envelope serializes to an object");
    for (key, value) in object {
        spec[key.as_str()] = value.clone();
    }
    spec
}
