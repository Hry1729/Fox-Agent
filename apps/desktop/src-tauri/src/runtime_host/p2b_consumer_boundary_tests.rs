//! Package 2-B2 consumer-boundary tests.
//!
//! These cover the typed consumer itself: the six-class disposition matrix, the
//! aggregation that must not be a second boolean fold, and the version-isolated
//! durable encoding/decoding round trip — including every way a v2 record can be
//! missing, unknown, corrupt or oversized, none of which may become a pass.
//!
//! The **real Host** paths (stage → terminate through the coordinator and the
//! live loop, with actual events and post-finalize database state) are driven
//! from the adjacent module `p2b_consumer_host_tests`, registered beside the
//! coordinator, because they need the coordinator's private scope and its real
//! HTTP provider harness.

use super::*;
use super::super::p2b_decision::{
    DecisionFact, FactClass, FactOrigin, decide,
};
use crate::runtime_host::delivery_checks_v2::{LimitKind, RecordLocator};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn candidate() -> FactOrigin {
    FactOrigin::candidate("out/one.json", &"a".repeat(64))
}

fn source() -> FactOrigin {
    FactOrigin::source("in/source.csv", &"b".repeat(64))
}

/// A real, decodable v1 content contract. The consumer tests need a spec the
/// frozen reader accepts, so that a `Confirmed` verdict is reached through the
/// genuine decode path rather than a stub.
fn v1_spec() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 1,
        "rule": {
            "type": "concat",
            "sources": [],
            "selected": ["in/a.txt"],
            "selection_order": "bytes_ascending",
            "tie_order": "filename_ascending",
            "merge_order": "filename_ascending",
            "read_only": true
        }
    })
}

/// An all-passed leaf outcome, as the real leaf produces it.
fn passed_leaf() -> DecisionFact {
    DecisionFact::passed(candidate())
}

/// A **caller declaration** that exactly one check covers every source it merged.
///
/// This proves the consumer mapping only. It is never proof that the frozen
/// contract was completely verified, and production still declares `Unstated`.
fn declared_coverage() -> super::super::p2b_decision::Coverage {
    super::super::p2b_decision::Coverage::single_check("text_concat", true)
}

/// A `Confirmed` item, reached through the real classifier with a declared
/// coverage. Test evidence for the mapping, not for the contract.
fn confirmed_item(item_key: &str) -> ItemDecision {
    ItemDecision::from_results(
        item_key,
        &[super::super::p2b_decision::classify_and_decide(
            &super::super::p2b_decision::FactInput {
                spec: &v1_spec(),
                read_facts: vec![passed_leaf()],
                drift_facts: Vec::new(),
                outcome: None,
                authorization_changed: false,
                coverage: declared_coverage(),
                context: None,
            },
            true,
        )],
        true,
    )
}

/// A checklist row as the ledger would hand it back.
fn ledger_row(item_key: &str, target: &str) -> crate::database::DeliveryChecklistItem {
    crate::database::DeliveryChecklistItem {
        item_key: item_key.to_owned(),
        target_path: Some(target.to_owned()),
        artifact_id: None,
        display_name: target.to_owned(),
        checks: Vec::new(),
        status: "pending".to_owned(),
        finding: None,
        checked_at: None,
        updated_at: 0,
        inherited_from_run_id: None,
    }
}

fn verdict_for(facts: Vec<DecisionFact>, budget: bool) -> DeliveryVerdict {
    DeliveryVerdict::from_items(vec![item_for("slot:json:1", facts, budget)], budget)
}

/// One item folded from an explicit fact set by 2-B1's own `decide`.
fn item_for(item_key: &str, facts: Vec<DecisionFact>, budget: bool) -> ItemDecision {
    ItemDecision {
        item_key: item_key.to_owned(),
        class: decide(&facts, budget).class,
        facts,
        repair_budget_remaining: budget,
    }
}

/// The standard unconfirmed fact: nothing was bound to a real artifact.
fn unbound_fact() -> DecisionFact {
    DecisionFact::unconfirmed(
        FactOrigin::CheckUnbound,
        None,
        "内容核验没有绑定真实产物",
        None,
    )
}

/// The standard blocking content mismatch.
fn mismatch_fact(path: &str) -> DecisionFact {
    DecisionFact::failed(
        FactOrigin::candidate(path, &"e".repeat(64)),
        "text_content_mismatch",
        "body differs",
        true,
        None,
    )
}

// ---------------------------------------------------------------------------
// 1. Six-class consumer matrix
// ---------------------------------------------------------------------------

/// The durable class vocabulary is one definition, not two.
///
/// `from_str` was added to `p2b_decision` by this package so the record can be
/// decoded; the risk it introduces is a second spelling of the six tokens that
/// drifts from `as_str`. This pins the inverse for every class, and pins that an
/// unknown token is an error rather than a default — a defaulted class is how a
/// corrupt record would become `Confirmed`.
#[test]
fn p2b_consumer_the_class_vocabulary_round_trips_and_never_defaults() {
    for class in [
        DeliveryClass::Confirmed,
        DeliveryClass::Refused,
        DeliveryClass::Invalid,
        DeliveryClass::RepairNeeded,
        DeliveryClass::Unconfirmed,
        DeliveryClass::DefiniteFailure,
    ] {
        let token = class.as_str();
        assert_eq!(
            super::super::p2b_decision::DeliveryClass::from_str(token),
            Some(class),
            "{token} must decode back to the class it was written from"
        );
        // The consumer's two outward decoders must agree with the class's own,
        // so a caller cannot reach a different answer through a different path.
        assert_eq!(parse_delivery_class_token(token), Some(class));
        assert_eq!(super::delivery_class_str(class), token);
    }
    for unknown in ["", "Confirmed", "confirmed ", "probablyFine", "null"] {
        assert_eq!(
            super::super::p2b_decision::DeliveryClass::from_str(unknown),
            None,
            "{unknown:?} is not a class and must not decode to one"
        );
    }
}

#[test]
fn p2b_consumer_maps_all_six_classes_to_their_dispositions() {
    // Refused: no repair, not passed, terminal through the existing failed outcome.
    let refused = verdict_for(
        vec![DecisionFact::refused(candidate(), "non_regular", "not a regular file")],
        true,
    );
    assert_eq!(refused.class, DeliveryClass::Refused);
    match refused.disposition() {
        DeliveryDisposition::Terminal { class, code, .. } => {
            assert_eq!(class, DeliveryClass::Refused);
            assert_eq!(code, DELIVERY_REFUSED);
        }
        other => panic!("refused must be terminal, got {other:?}"),
    }
    assert!(!refused.requires_u1());
    assert!(refused.items.iter().all(|item| !item.is_passed()));

    // Invalid: the spec is unusable. Distinct class, distinct code.
    let invalid = verdict_for(vec![DecisionFact::invalid("内容契约版本不支持")], true);
    assert_eq!(invalid.class, DeliveryClass::Invalid);
    match invalid.disposition() {
        DeliveryDisposition::Terminal { class, code, .. } => {
            assert_eq!(class, DeliveryClass::Invalid);
            assert_eq!(code, DELIVERY_INVALID);
        }
        other => panic!("invalid must be terminal, got {other:?}"),
    }

    // RepairNeeded: a real blocking mismatch with budget left.
    let repairable = verdict_for(
        vec![DecisionFact::failed(
            candidate(),
            "text_content_mismatch",
            "body differs",
            true,
            None,
        )],
        true,
    );
    assert_eq!(repairable.class, DeliveryClass::RepairNeeded);
    assert_eq!(repairable.disposition(), DeliveryDisposition::Repair);
    assert!(!repairable.requires_u1());

    // DefiniteFailure: the same fact with the budget gone is NOT renamed Invalid.
    let exhausted = verdict_for(
        vec![DecisionFact::failed(
            candidate(),
            "text_content_mismatch",
            "body differs",
            true,
            None,
        )],
        false,
    );
    assert_eq!(exhausted.class, DeliveryClass::DefiniteFailure);
    match exhausted.disposition() {
        DeliveryDisposition::Terminal { class, code, .. } => {
            assert_eq!(class, DeliveryClass::DefiniteFailure);
            assert_eq!(code, DELIVERY_DEFINITE_FAILURE);
        }
        other => panic!("definite failure must be terminal, got {other:?}"),
    }

    // Unconfirmed: not passed, not repaired, and emphatically not terminal.
    let unconfirmed = verdict_for(
        vec![DecisionFact::unconfirmed(
            FactOrigin::CheckUnbound,
            None,
            "内容核验没有绑定真实产物",
            None,
        )],
        true,
    );
    assert_eq!(unconfirmed.class, DeliveryClass::Unconfirmed);
    assert!(unconfirmed.requires_u1());
    assert!(!matches!(
        unconfirmed.disposition(),
        DeliveryDisposition::Terminal { .. } | DeliveryDisposition::Accept
    ));
    assert!(unconfirmed.items.iter().all(|item| !item.is_final_class()));
    assert!(
        unconfirmed
            .items
            .iter()
            .all(|item| !item.may_enter_terminal_stage()),
        "an unconfirmed item has no terminal decision to stage"
    );
    assert!(unconfirmed.items.iter().all(|item| !item.is_passed()));

    // Confirmed: acceptance, and only through an explicit sufficient verdict.
    let confirmed = ItemDecision::from_results(
        "slot:json:1",
        &[super::super::p2b_decision::classify_and_decide(
            &super::super::p2b_decision::FactInput {
                spec: &v1_spec(),
                read_facts: vec![passed_leaf()],
                drift_facts: Vec::new(),
                outcome: None,
                authorization_changed: false,
                coverage: declared_coverage(),
                context: None,
            },
            true,
        )],
        true,
    );
    assert_eq!(confirmed.class, DeliveryClass::Confirmed, "facts: {:?}", confirmed.facts);
    assert!(confirmed.is_passed());
    assert!(confirmed.is_final_class());
    let confirmed_verdict = DeliveryVerdict::from_items(vec![confirmed], true);
    assert_eq!(confirmed_verdict.disposition(), DeliveryDisposition::Accept);
}

#[test]
fn p2b_consumer_repair_needed_never_overrides_a_higher_priority_class() {
    // A repairable failure standing beside a refusal: the refusal wins, and the
    // repair branch must not be taken. This is the aggregation-priority contract,
    // not a second boolean fold.
    let mixed = verdict_for(
        vec![
            DecisionFact::failed(candidate(), "text_content_mismatch", "differs", true, None),
            DecisionFact::refused(candidate(), "path_denied", "outside the authorized root"),
        ],
        true,
    );
    assert_eq!(mixed.class, DeliveryClass::Refused);
    assert!(matches!(
        mixed.disposition(),
        DeliveryDisposition::Terminal { .. }
    ));

    // The same for Invalid over a repairable failure.
    let mixed = verdict_for(
        vec![
            DecisionFact::failed(candidate(), "text_content_mismatch", "differs", true, None),
            DecisionFact::invalid("内容契约版本不支持"),
        ],
        true,
    );
    assert_eq!(mixed.class, DeliveryClass::Invalid);
}

// ---------------------------------------------------------------------------
// 2. One item cannot cover another
// ---------------------------------------------------------------------------

#[test]
fn p2b_consumer_passing_item_cannot_cover_another_items_failure() {
    let passed = confirmed_item("slot:json:1");

    assert_eq!(passed.class, DeliveryClass::Confirmed);

    let failed = ItemDecision::from_results(
        "slot:json:2",
        &[decide(
            &[DecisionFact::failed(
                FactOrigin::candidate("out/two.json", &"c".repeat(64)),
                "text_content_mismatch",
                "differs",
                true,
                None,
            )],
            true,
        )],
        true,
    );
    assert_eq!(failed.class, DeliveryClass::RepairNeeded);

    // Two items, different classes: the aggregate must not be `Confirmed` just
    // because one item passed.
    let verdict = DeliveryVerdict::from_items(vec![passed, failed], true);
    assert_eq!(verdict.class, DeliveryClass::RepairNeeded);
    assert_eq!(verdict.items.len(), 2);
    assert_ne!(verdict.items[0].class, verdict.items[1].class);

    // Refused + Passed: the pass survives as a fact and is not lost, but the
    // delivery is refused.
    let refused = ItemDecision::from_results(
        "slot:json:3",
        &[decide(
            &[DecisionFact::refused(candidate(), "path_denied", "outside root")],
            true,
        )],
        true,
    );
    let second_passed = confirmed_item("slot:json:4");
    let verdict = DeliveryVerdict::from_items(vec![refused, second_passed], true);
    assert_eq!(verdict.class, DeliveryClass::Refused);
    assert!(
        verdict.facts().any(|fact| fact.class == FactClass::Passed),
        "the passing item's fact must survive beside the refusal"
    );
    assert_eq!(
        verdict.final_items().count(),
        2,
        "both classes are final outcomes"
    );
}

#[test]
fn p2b_consumer_definite_failure_and_unconfirmed_both_survive_one_verdict() {
    let failed = item_for("slot:json:1", vec![mismatch_fact("out/one.json")], false);

    let unconfirmed = item_for("slot:json:2", vec![unbound_fact()], false);


    let verdict = DeliveryVerdict::from_items(vec![failed, unconfirmed], false);
    assert_eq!(verdict.class, DeliveryClass::DefiniteFailure);
    // The unconfirmed item is retained and stays non-terminal: it must not be
    // written through the terminal stage, where `passed = 0` becomes `failed`.
    assert_eq!(verdict.final_items().count(), 1);
    assert_eq!(verdict.pending_items().count(), 1);
    assert_eq!(verdict.pending_items().next().unwrap().item_key, "slot:json:2");

    // The reverse order folds to the same class: priority is not order-dependent.
    let failed_again = ItemDecision::from_results(
        "slot:json:1",
        &[decide(
            &[DecisionFact::failed(
                candidate(),
                "text_content_mismatch",
                "differs",
                true,
                None,
            )],
            false,
        )],
        false,
    );
    let unconfirmed_again = ItemDecision::from_results(
        "slot:json:2",
        &[decide(
            &[DecisionFact::unconfirmed(
                FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            )],
            false,
        )],
        false,
    );
    let reversed = DeliveryVerdict::from_items(vec![unconfirmed_again, failed_again], false);
    assert_eq!(reversed.class, DeliveryClass::DefiniteFailure);
}

#[test]
fn p2b_consumer_mixed_facts_and_an_exhausted_budget_lose_nothing() {
    // Four facts of four classes, budget exhausted: every fact is retained and
    // the aggregate is the highest-priority class present.
    let mut locator = RecordLocator::default();
    locator.record = Some(7);
    let facts = vec![
        DecisionFact::passed(candidate()),
        DecisionFact::unconfirmed(source(), Some("evidence_unavailable"), "unread", Some(64)),
        DecisionFact::failed(
            candidate(),
            "table_width_mismatch",
            "row 7 has 3 fields",
            true,
            Some(locator),
        ),
        DecisionFact::refused(candidate(), "reparse_point", "reparse point"),
    ];
    let verdict = verdict_for(facts, false);
    assert_eq!(verdict.class, DeliveryClass::Refused);
    let retained: Vec<FactClass> = verdict.facts().map(|fact| fact.class).collect();
    assert_eq!(
        retained,
        vec![
            FactClass::Passed,
            FactClass::Unconfirmed,
            FactClass::Failed,
            FactClass::Refused
        ],
        "every fact must survive the fold in order"
    );
}

// ---------------------------------------------------------------------------
// 3. Durable encoding round trip
// ---------------------------------------------------------------------------

/// Every retained field must survive an encode → decode round trip, so nothing
/// has to be re-derived from `detail` and no fact silently loses its class.
#[test]
fn p2b_consumer_durable_encoding_round_trips_every_retained_field() {
    let mut locator = RecordLocator::default();
    locator.record = Some(3);
    locator.line = Some(9);
    locator.field = Some(2);
    locator.column = Some("amount".to_owned());
    locator.byte_offset = Some(128);
    let fact = DecisionFact::failed(
        FactOrigin::candidate("out/one.json", &"a".repeat(64)),
        "table_width_mismatch",
        "row 3 has 2 fields",
        true,
        Some(locator),
    )
    .with_check_identity(Some("content:table"), "table_width", 2)
    .with_structured(
        Some("3 fields"),
        Some("2 fields"),
        Some("optionalFinalLf"),
    );
    let item = ItemDecision {
        item_key: "slot:table:1".to_owned(),
        class: DeliveryClass::DefiniteFailure,
        facts: vec![fact.clone()],
        repair_budget_remaining: false,
    };
    let verdict = DeliveryVerdict::from_items(vec![item.clone()], false);
    let finding = verdict.item_finding(&item, &serde_json::json!({"reasonCode": "legacy.key"}));

    // The legacy key is preserved beside the new envelope.
    assert_eq!(finding["reasonCode"], "legacy.key");

    match decode_verdict(&finding) {
        StoredVerdict::V2(stored) => {
            assert_eq!(stored.item_key, "slot:table:1");
            assert_eq!(stored.class, DeliveryClass::DefiniteFailure);
            assert_eq!(stored.aggregate_class, DeliveryClass::DefiniteFailure);
            assert_eq!(stored.item_count, 1);
            assert!(stored.terminal);
            assert!(!stored.repair_budget_remaining);
            assert_eq!(stored.facts.len(), 1);
            let decoded = &stored.facts[0];
            assert_eq!(decoded.class, FactClass::Failed);
            assert_eq!(decoded.rule_id.as_deref(), Some("content:table"));
            assert_eq!(decoded.check.as_deref(), Some("table_width"));
            assert_eq!(decoded.checker_version, Some(2));
            assert_eq!(decoded.reason_code.as_deref(), Some("table_width_mismatch"));
            assert!(decoded.blocking);
            assert!(decoded.needs_repair);
            assert_eq!(decoded.expected.as_deref(), Some("3 fields"));
            assert_eq!(decoded.actual.as_deref(), Some("2 fields"));
            assert_eq!(decoded.equivalence.as_deref(), Some("optionalFinalLf"));
            let locator = decoded.locator.as_ref().expect("locator must survive");
            assert_eq!(locator.record, Some(3));
            assert_eq!(locator.line, Some(9));
            assert_eq!(locator.field, Some(2));
            assert_eq!(locator.column.as_deref(), Some("amount"));
            assert_eq!(locator.byte_offset, Some(128));
            match &decoded.origin {
                FactOrigin::Candidate { path, sha256 } => {
                    assert_eq!(path, "out/one.json");
                    assert_eq!(sha256, &"a".repeat(64));
                }
                other => panic!("candidate binding lost: {other:?}"),
            }
        }
        other => panic!("expected a v2 record, got {other:?}"),
    }
}

#[test]
fn p2b_consumer_durable_encoding_keeps_limit_and_limit_kind_separate() {
    let mut fact = DecisionFact::unconfirmed(
        source(),
        Some("budget_exceeded"),
        "row ceiling reached",
        Some(50_000),
    );
    fact.limit_kind = Some(LimitKind::Records);
    let item = ItemDecision {
        item_key: "slot:csv:1".to_owned(),
        class: DeliveryClass::Unconfirmed,
        facts: vec![fact],
        repair_budget_remaining: true,
    };
    let verdict = DeliveryVerdict::from_items(vec![item.clone()], true);
    let finding = verdict.item_finding(&item, &serde_json::json!({}));

    let StoredVerdict::V2(stored) = decode_verdict(&finding) else {
        panic!("expected a v2 record");
    };
    let decoded = &stored.facts[0];
    // The byte/row ceiling and *which* ceiling it was must both survive: a byte
    // ceiling reported as a record ceiling is a different fact.
    assert_eq!(decoded.limit, Some(50_000));
    assert_eq!(decoded.limit_kind, Some(LimitKind::Records));
    assert_eq!(stored.class, DeliveryClass::Unconfirmed);
    assert!(!decode_verdict(&finding).may_be_passed());
    assert!(!stored.terminal);
}
#[test]
fn p2b_consumer_multi_source_bindings_are_not_merged() {    // Two sources checked by one concat rule: the encoding must keep both
    // bindings distinct rather than collapsing them into the first one.
    let item = ItemDecision {
        item_key: "slot:csv:1".to_owned(),
        class: DeliveryClass::DefiniteFailure,
        facts: vec![
            DecisionFact::failed(
                FactOrigin::source("in/a.csv", &"1".repeat(64)),
                "text_order_mismatch",
                "order differs",
                true,
                None,
            ),
            DecisionFact::failed(
                FactOrigin::source("in/b.csv", &"2".repeat(64)),
                "text_content_mismatch",
                "body differs",
                true,
                None,
            ),
        ],
        repair_budget_remaining: false,
    };
    let verdict = DeliveryVerdict::from_items(vec![item.clone()], false);
    let finding = verdict.item_finding(&item, &serde_json::json!({}));

    let StoredVerdict::V2(stored) = decode_verdict(&finding) else {
        panic!("expected a v2 record");
    };
    let bindings: Vec<(String, String)> = stored
        .facts
        .iter()
        .filter_map(|fact| match &fact.origin {
            FactOrigin::Source { path, sha256 } => Some((path.clone(), sha256.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        bindings,
        vec![
            ("in/a.csv".to_owned(), "1".repeat(64)),
            ("in/b.csv".to_owned(), "2".repeat(64)),
        ],
        "each source keeps its own binding"
    );
}

// ---------------------------------------------------------------------------
// 4. Version isolation: nothing unreadable may become a pass
// ---------------------------------------------------------------------------

#[test]
fn p2b_consumer_legacy_record_is_not_read_as_a_verdict() {
    // A v1 / historical record has no verdict envelope at all. It must be
    // reported as `Legacy` — the legacy mapping applies — and never as passed
    // through the v2 reader.
    let legacy = serde_json::json!({
        "reasonCode": "delivery.missing_artifact",
        "verificationStatus": "unverified",
        "checks": {},
    });
    match decode_verdict(&legacy) {
        StoredVerdict::Legacy => {}
        other => panic!("a record without an envelope must stay legacy, got {other:?}"),
    }
    assert!(!decode_verdict(&legacy).may_be_passed());
    assert_eq!(decode_verdict(&legacy).category(), "legacy");
}

#[test]
fn p2b_consumer_v2_records_that_are_missing_unknown_corrupt_or_oversized_never_pass() {
    // (a) The envelope key is present but is not an object.
    let not_an_object = serde_json::json!({"deliveryVerdict": "confirmed"});
    assert!(matches!(
        decode_verdict(&not_an_object),
        StoredVerdict::Corrupt { .. }
    ));

    // (b) An unknown version pair is corrupt, not legacy and not v2.
    let unknown_version = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 3,
            "verdictEnvelopeVersion": 3,
            "itemKey": "slot:json:1",
            "class": "confirmed",
            "aggregateClass": "confirmed",
            "itemCount": 1,
            "terminal": true,
            "facts": [],
        }
    });
    match decode_verdict(&unknown_version) {
        StoredVerdict::Corrupt { detail } => {
            assert!(detail.contains("unsupported verdict version"), "{detail}");
        }
        other => panic!("an unknown version must be corrupt, got {other:?}"),
    }

    // (c) The version is missing entirely.
    let missing_version = serde_json::json!({
        "deliveryVerdict": {"itemKey": "slot:json:1", "class": "confirmed"}
    });
    assert!(matches!(
        decode_verdict(&missing_version),
        StoredVerdict::Corrupt { .. }
    ));

    // (d) An unknown class string is corrupt: a class vocabulary the reader does
    // not know must never be coerced to the nearest known one.
    let unknown_class = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": "slot:json:1",
            "class": "probablyFine",
            "aggregateClass": "confirmed",
            "itemCount": 1,
            "terminal": true,
            "facts": [],
        }
    });
    match decode_verdict(&unknown_class) {
        StoredVerdict::Corrupt { detail } => {
            assert!(detail.contains("class"), "{detail}");
        }
        other => panic!("an unknown class must be corrupt, got {other:?}"),
    }

    // (e) A truncated fact list: one fact is unreadable, so the whole record is
    // corrupt rather than the fact being silently dropped.
    let truncated_fact = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": "slot:json:1",
            "class": "confirmed",
            "aggregateClass": "confirmed",
            "itemCount": 1,
            "terminal": true,
            "facts": [{"class": "passed", "origin": "source"}],
        }
    });
    match decode_verdict(&truncated_fact) {
        StoredVerdict::Corrupt { detail } => {
            assert!(detail.contains("path"), "{detail}");
        }
        other => panic!("a fact missing its binding must be corrupt, got {other:?}"),
    }

    // (f) A fact whose class is not in the vocabulary.
    let unknown_fact_class = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": "slot:json:1",
            "class": "unconfirmed",
            "aggregateClass": "unconfirmed",
            "itemCount": 1,
            "terminal": false,
            "facts": [{"class": "maybe", "origin": "spec"}],
        }
    });
    assert!(matches!(
        decode_verdict(&unknown_fact_class),
        StoredVerdict::Corrupt { .. }
    ));

    // (g) An unknown limit kind: the ceiling kind must not be guessed.
    let unknown_limit_kind = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": "slot:json:1",
            "class": "unconfirmed",
            "aggregateClass": "unconfirmed",
            "itemCount": 1,
            "terminal": false,
            "facts": [{"class": "unconfirmed", "origin": "spec", "limitKind": "gigabytes"}],
        }
    });
    assert!(matches!(
        decode_verdict(&unknown_limit_kind),
        StoredVerdict::Corrupt { .. }
    ));

    // Every corrupt shape above is emphatically not a pass.
    for finding in [
        not_an_object,
        unknown_version,
        missing_version,
        unknown_class,
        truncated_fact,
        unknown_fact_class,
        unknown_limit_kind,
    ] {
        let stored = decode_verdict(&finding);
        assert!(
            !stored.may_be_passed(),
            "{finding} must never read as passed"
        );
        assert_ne!(stored.category(), "v2");
    }
}

#[test]
fn p2b_consumer_non_object_finding_is_legacy_and_a_confirmed_v2_record_is_the_only_pass() {
    // A non-object finding is not a v2 record.
    for finding in [
        serde_json::json!("not an object"),
        serde_json::json!(7),
        serde_json::json!([]),
        serde_json::json!(null),
    ] {
        assert!(matches!(decode_verdict(&finding), StoredVerdict::Legacy));
        assert!(!decode_verdict(&finding).may_be_passed());
    }

    // The only passing shape: an explicit, terminal, confirmed v2 envelope.
    let item = ItemDecision {
        item_key: "slot:json:1".to_owned(),
        class: DeliveryClass::Confirmed,
        facts: vec![DecisionFact::passed(candidate())],
        repair_budget_remaining: true,
    };
    let verdict = DeliveryVerdict::from_items(vec![item.clone()], true);
    let stored = decode_verdict(&verdict.item_finding(&item, &serde_json::json!({})));
    assert!(stored.may_be_passed());
    assert_eq!(stored.category(), "v2");

    // The same envelope with `terminal: false` is not a pass, even though its
    // class is `confirmed`: a non-terminal item must never be written as passed.
    let mut non_terminal = verdict.item_finding(&item, &serde_json::json!({}));
    non_terminal["deliveryVerdict"]["terminal"] = serde_json::json!(false);
    assert!(!decode_verdict(&non_terminal).may_be_passed());

    // And a confirmed aggregate whose item says `unconfirmed` is not a pass
    // either — the item's own class is what governs the row.
    let mismatch_item = ItemDecision {
        item_key: "slot:json:1".to_owned(),
        class: DeliveryClass::Unconfirmed,
        facts: vec![DecisionFact::unconfirmed(
            FactOrigin::CheckUnbound,
            None,
            "no binding",
            None,
        )],
        repair_budget_remaining: true,
    };
    let mismatch = DeliveryVerdict::from_items(vec![mismatch_item.clone()], true);
    let finding = mismatch.item_finding(&mismatch_item, &serde_json::json!({}));
    assert!(!decode_verdict(&finding).may_be_passed());
}

// ---------------------------------------------------------------------------
// 5. Pending (non-terminal) retention encoding
// ---------------------------------------------------------------------------

#[test]
fn p2b_consumer_unconfirmed_item_is_encoded_for_the_pending_path_not_the_terminal_one() {
    let unconfirmed = ItemDecision::from_results(
        "slot:json:2",
        &[decide(
            &[DecisionFact::unconfirmed(
                FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            )],
            true,
        )],
        true,
    );
    let confirmed = confirmed_item("slot:json:1");
    assert_eq!(confirmed.class, DeliveryClass::Confirmed);
    assert_eq!(unconfirmed.class, DeliveryClass::Unconfirmed);

    let verdict = DeliveryVerdict::from_items(vec![confirmed, unconfirmed], true);
    // The confirmed item is terminal; the unconfirmed one must be kept on the
    // non-terminal path so it can never be finalized as `failed`.
    assert_eq!(verdict.final_items().count(), 1);
    assert_eq!(
        verdict.final_items().next().unwrap().item_key,
        "slot:json:1"
    );
    assert_eq!(verdict.pending_items().count(), 1);
    assert_eq!(verdict.pending_items().next().unwrap().item_key, "slot:json:2");

    // The unconfirmed row's encoded finding is still a well-formed v2 record that
    // reloads to the same non-passing class — the facts are retentive, not dropped.
    let item = verdict.pending_items().next().unwrap();
    let encoded = verdict.item_finding(item, &serde_json::json!({}));
    let StoredVerdict::V2(stored) = decode_verdict(&encoded) else {
        panic!("the pending row must still encode a v2 verdict");
    };
    assert_eq!(stored.class, DeliveryClass::Unconfirmed);
    assert!(!stored.terminal);
    assert!(!decode_verdict(&encoded).may_be_passed());
    assert_eq!(stored.facts.len(), 1);
    assert_eq!(stored.facts[0].class, FactClass::Unconfirmed);
}

// ---------------------------------------------------------------------------
// 6. Stop mapping
// ---------------------------------------------------------------------------

#[test]
fn p2b_consumer_stop_mapping_refuses_a_requires_u1_verdict_and_accepts_the_rest() {
    use crate::database::DeliveryChecklistItem;

    fn ledger(item_key: &str) -> Vec<DeliveryChecklistItem> {
        vec![DeliveryChecklistItem {
            item_key: item_key.to_owned(),
            target_path: Some("out/one.json".to_owned()),
            artifact_id: None,
            display_name: "out/one.json".to_owned(),
            checks: Vec::new(),
            status: "pending".to_owned(),
            finding: Some(serde_json::json!({"reasonCode": "legacy"}).to_string()),
            checked_at: None,
            updated_at: 0,
            inherited_from_run_id: None,
        }]
    }

    // A requires-U1 verdict is handed back rather than being forced into a stop.
    let unconfirmed = verdict_for(
        vec![DecisionFact::unconfirmed(
            FactOrigin::CheckUnbound,
            None,
            "no binding",
            None,
        )],
        true,
    );
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &ledger("slot:json:1"),
        unconfirmed,
    )
    .expect_err("a requires-U1 verdict must not produce a stop");
    assert!(returned.is_requires_u1());
    assert!(
        returned.ambiguity().is_none(),
        "a RequiresU1 refusal is a valid decision, not an ambiguous input"
    );

    // A confirmed verdict becomes a typed stop whose row is passed.
    let item = ItemDecision {
        item_key: "slot:json:1".to_owned(),
        class: DeliveryClass::Confirmed,
        facts: vec![DecisionFact::passed(candidate())],
        repair_budget_remaining: true,
    };
    let verdict = DeliveryVerdict::from_items(vec![item], true);
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &ledger("slot:json:1"),
        verdict,
    )
    .expect("a confirmed verdict must map to a stop");
    let typed = stop.typed().expect("typed stop");
    assert_eq!(typed.rows.len(), 1);
    assert!(typed.rows[0].passed);
    assert!(typed.rows[0].terminal);
    assert_eq!(typed.rows[0].class, "confirmed");
    assert_eq!(typed.terminal_items.len(), 1);
    assert!(typed.pending_items.is_empty());
    // The legacy key from the ledger's own finding is preserved.
    let encoded: serde_json::Value =
        serde_json::from_str(&typed.rows[0].finding_json).expect("valid json");
    assert_eq!(encoded["reasonCode"], "legacy");

    // A definite failure becomes a typed stop whose row is terminal and NOT
    // passed, and whose class is retained rather than flattened.
    let failed = verdict_for(
        vec![DecisionFact::failed(
            candidate(),
            "text_content_mismatch",
            "differs",
            true,
            None,
        )],
        false,
    );
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &ledger("slot:json:1"),
        failed,
    )
    .expect("a definite failure must map to a stop");
    let typed = stop.typed().expect("typed stop");
    assert!(!typed.rows[0].passed);
    assert!(typed.rows[0].terminal);
    assert_eq!(typed.rows[0].class, "definiteFailure");
    assert_eq!(typed.terminal_items.len(), 1);
    assert!(!typed.terminal_items[0].passed);
}

#[test]
fn p2b_consumer_typed_stop_items_never_expose_a_non_terminal_row_to_the_terminal_stage() {
    use crate::database::DeliveryChecklistItem;

    // An aggregate that is **not** `RequiresU1` can still contain an unconfirmed
    // item: here the refusal outranks it. Only the refusal's row may be staged;
    // the unconfirmed row must be retained, because `passed = 0` in the terminal
    // stage is finalized as `failed`.
    let refused = item_for(
        "slot:json:1",
        vec![DecisionFact::refused(candidate(), "path_denied", "outside root")],
        true,
    );
    let unconfirmed = item_for("slot:json:2", vec![unbound_fact()], true);
    let verdict = DeliveryVerdict::from_items(vec![refused, unconfirmed], true);
    assert_eq!(verdict.class, DeliveryClass::Refused);

    let ledger: Vec<DeliveryChecklistItem> = ["slot:json:1", "slot:json:2"]
        .iter()
        .map(|key| ledger_row(key, &format!("out/{key}.json")))
        .collect();
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, verdict)
        .expect("a refused aggregate maps to a stop");
    let typed = stop.typed().expect("typed stop");
    assert_eq!(typed.rows.len(), 2);
    // Exactly one row may be staged, and it is the refused one.
    assert_eq!(typed.terminal_items.len(), 1);
    assert_eq!(typed.terminal_items[0].item_key, "slot:json:1");
    assert!(!typed.terminal_items[0].passed);
    assert_eq!(typed.pending_items.len(), 1);
    assert_eq!(typed.pending_items[0].0, "slot:json:2");
    // The pending row's finding is still the full v2 envelope, so the facts
    // survive for a later U1 rather than being dropped.
    let retained: serde_json::Value =
        serde_json::from_str(&typed.pending_items[0].1).expect("valid json");
    assert_eq!(retained["deliveryVerdict"]["class"], "unconfirmed");
    let unconfirmed_row = typed
        .rows
        .iter()
        .find(|row| row.item_key == "slot:json:2")
        .expect("row");
    assert!(!unconfirmed_row.passed);
    assert!(!unconfirmed_row.terminal, "unconfirmed is not a final class");
    assert!(!unconfirmed_row.stage, "and it may not enter the stage");
    // No `ItemVerdict` leaks out of the typed variant: the legacy shape cannot
    // express a class, so reporting it would lose the classification.
    assert!(stop.items().is_empty());
    assert!(stop.repair_findings().is_none());
}

#[test]
fn p2b_consumer_an_unconfirmed_aggregate_has_no_stop_at_all() {
    use crate::database::DeliveryChecklistItem;

    // The aggregate itself is `Unconfirmed`: there is no terminal decision for
    // this round, so there must be no stop. Not even a passing item buys one —
    // a pass beside an unverified item is not a verified delivery.
    let confirmed = confirmed_item("slot:json:1");
    let unconfirmed = item_for("slot:json:2", vec![unbound_fact()], true);
    let verdict = DeliveryVerdict::from_items(vec![confirmed, unconfirmed], true);
    assert_eq!(verdict.class, DeliveryClass::Unconfirmed);
    assert!(verdict.requires_u1());

    let ledger: Vec<DeliveryChecklistItem> = ["slot:json:1", "slot:json:2"]
        .iter()
        .map(|key| ledger_row(key, &format!("out/{key}.json")))
        .collect();
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, verdict)
        .expect_err("an unconfirmed aggregate must not produce a stop");
    let returned = returned.expect_requires_u1();
    // The verdict comes back verbatim rather than being reconstructed, so no
    // fact can be lost between the refusal and the retention write.
    assert_eq!(returned.class, DeliveryClass::Unconfirmed);
    assert_eq!(returned.items.len(), 2);
    assert!(
        returned.facts().any(|fact| fact.class == FactClass::Passed),
        "the passing item's fact survives"
    );
    assert_eq!(returned.pending_items().count(), 1);
}

#[test]
fn p2b_consumer_a_repair_round_stages_its_rows_though_they_are_not_final() {
    use crate::database::DeliveryChecklistItem;

    // `RepairNeeded` is staged and finalized like the legacy repair round, but it
    // is not a final outcome. Both predicates are asserted separately so neither
    // can silently absorb the other.
    let confirmed = confirmed_item("slot:json:1");
    let repairable = item_for("slot:json:2", vec![mismatch_fact("out/two.json")], true);
    let verdict = DeliveryVerdict::from_items(vec![confirmed, repairable], true);
    assert_eq!(verdict.class, DeliveryClass::RepairNeeded);

    let ledger: Vec<DeliveryChecklistItem> = ["slot:json:1", "slot:json:2"]
        .iter()
        .map(|key| ledger_row(key, &format!("out/{key}.json")))
        .collect();
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, verdict)
        .expect("a repairable verdict maps to a stop");
    let typed = stop.typed().expect("typed stop");
    assert_eq!(typed.terminal_items.len(), 2, "both rows are stageable");
    assert!(typed.pending_items.is_empty());

    let confirmed_row = typed
        .rows
        .iter()
        .find(|row| row.item_key == "slot:json:1")
        .expect("row");
    assert!(confirmed_row.terminal, "confirmed is a final class");
    assert!(confirmed_row.stage);
    assert!(confirmed_row.passed);

    let repairable_row = typed
        .rows
        .iter()
        .find(|row| row.item_key == "slot:json:2")
        .expect("row");
    assert!(
        !repairable_row.terminal,
        "a repairable item is not a final class"
    );
    assert!(repairable_row.stage, "but it is still staged and finalized");
    assert!(!repairable_row.passed);
}

// ---------------------------------------------------------------------------
// D2: the relabelled text diagnosis survives the durable encoding
// ---------------------------------------------------------------------------
//
// `expected` is a free-form string on the wire, so the text-concat check may
// describe *what* is expected instead of pasting the expected bytes. That is
// only useful if the description survives the receipt/finding round trip
// unchanged and is not reparsed into something else.

#[test]
fn d2_a_described_expected_survives_the_durable_encoding() {
    let described = "来源标题行顺序（只有标题名，不是文件内容）：01_a.txt → 02_b.txt → 03_c.txt";
    let fact = DecisionFact::failed(
        FactOrigin::candidate("out/joined.txt", &"c".repeat(64)),
        "delivery.check.text.separator_mismatch",
        "各段正文与冻结来源逐字节一致，但段间空行不符合冻结合并格式",
        true,
        Some(RecordLocator::default().at_byte(0)),
    )
    .with_check_identity(Some("content:concat"), "text.concat", 2)
    .with_structured(Some(described), Some("段间空行数量不符"), None);
    let item = ItemDecision {
        item_key: "file:joined.txt".to_owned(),
        class: DeliveryClass::DefiniteFailure,
        facts: vec![fact],
        repair_budget_remaining: true,
    };
    let verdict = DeliveryVerdict::from_items(vec![item.clone()], false);
    let finding = verdict.item_finding(&item, &serde_json::json!({}));

    // The encoded fact carries the description verbatim, not a re-joined list.
    assert_eq!(
        finding["deliveryVerdict"]["facts"][0]["expected"], described,
        "the encoded expected field must be the description itself"
    );

    match decode_verdict(&finding) {
        StoredVerdict::V2(stored) => {
            let decoded = &stored.facts[0];
            assert_eq!(decoded.expected.as_deref(), Some(described));
            assert_eq!(
                decoded.reason_code.as_deref(),
                Some("delivery.check.text.separator_mismatch")
            );
            // A description is still a blocking, repairable failure: relabelling
            // does not soften the consequence.
            assert!(decoded.blocking);
            assert!(decoded.needs_repair);
        }
        other => panic!("expected a v2 stored verdict, got {other:?}"),
    }
}
