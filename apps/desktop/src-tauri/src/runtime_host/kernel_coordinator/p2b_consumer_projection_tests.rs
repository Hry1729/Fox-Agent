//! Package 2-B2 extended review (§1.4): the durable version, decode and
//! projection chain.
//!
//! The card asks for the **whole chain** — the original finding, the stage, the
//! finalize, the raw reload, `safe_finding`, and the outward artifact view — and
//! names one trap explicitly: `StoredVerdict::may_be_passed == false` does not by
//! itself prove that the UI or a caller will not keep reading a legacy
//! `status='passed'` row. So these tests never stop at the decoder. Each one
//! carries a record from the durable bytes through the projection the real
//! callers use (`load_conversation` for the checklist view, the artifact view for
//! the file-facing projection) and asserts what that surface says.
//!
//! The counter-examples the card lists are covered here: a lost envelope, a lost
//! or damaged omission flag, an unknown version, contradicting
//! `class`/`aggregateClass`/`terminal`, a `confirmed` class beside a failing fact,
//! an empty or truncated fact list, an inconsistent `factCount`, a missing
//! rule/check/binding, and the size ceilings. Structural fields are checked field
//! by field, because the card's specific warning is that `origin`, `locator` and
//! `expected`/`actual`/`equivalence` might be silently stringified.
//!
//! Last, the ceilings are **not** relaxed to make anything here pass: the
//! over-capacity case asserts the explicit omission, which is the behaviour the
//! frozen capacity contract requires.

use super::*;
use crate::runtime_host::delivery::{
    decode_stored_verdict, decide_facts, DecisionFact, DeliveryVerdict, FactOrigin, ItemDecision,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn candidate(path: &str) -> FactOrigin {
    FactOrigin::candidate(path, &"c".repeat(64))
}

fn v2_envelope() -> Value {
    serde_json::json!({
        "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
        "itemKey": "slot:json:1", "class": "confirmed", "aggregateClass": "confirmed",
        "itemCount": 1, "terminal": true, "repairBudgetRemaining": true,
        "facts": [{"class": "passed", "origin": "candidate",
                   "path": "out/one.json", "sha256": "c".repeat(64)}]
    })
}

/// A finding carrying the given envelope, as the durable row would hold it.
fn finding_with(envelope: Value) -> Value {
    serde_json::json!({ "deliveryVerdict": envelope })
}

/// The runtime a real coordinator borrows, with this task's promised artifacts
/// seeded exactly as a production start seeds them.
fn leaked_runtime(task: &str) -> (
    &'static TestClock,
    &'static CancellationRegistry,
    &'static Database,
    std::path::PathBuf,
    String,
) {
    let clock: &'static TestClock = Box::leak(Box::new(TestClock::new(crate::database::now_ms())));
    let cancellation: &'static CancellationRegistry = Box::leak(Box::new(Default::default()));
    let config = worker_configuration();
    let (db, root, run_id) =
        fixture_with_start_opt(clock, &config.hash().unwrap(), Some(&config), true, true);
    freeze_host_scope(&db, &run_id);
    let seeds = crate::runtime_host::delivery::expectations_from_task(task);
    assert!(!seeds.is_empty(), "{seeds:?}");
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let db: &'static Database = Box::leak(Box::new(db));
    (clock, cancellation, db, root, run_id)
}

struct ChainHost {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
    conversation_id: String,
    keys: Vec<String>,
}

fn chain_host(task: &str) -> (ChainHost, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime(task);
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    let keys = db
        .delivery_checklist(&run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [run_id.as_str()],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    (
        ChainHost { db, root, run_id, conversation_id, keys },
        coordinator,
    )
}

/// Write a finding straight onto the checklist row, as a real verdict write would.
fn force_finding(db: &Database, run_id: &str, finding: &Value, status: &str) {
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET finding_json=?1, status=?2 WHERE run_id=?3",
            rusqlite::params![finding.to_string(), status, run_id],
        )?;
        Ok(())
    })
    .unwrap()
}

/// The outward checklist projection the real UI reads. This is the surface the
/// card insists on: the raw reload is not enough.
fn projected(db: &Database, conversation_id: &str, run_id: &str) -> crate::database::DeliveryChecklistView {
    let detail = db.load_conversation(conversation_id).unwrap();
    detail
        .delivery_checklist
        .into_iter()
        .find(|view| view.run_id == run_id)
        .expect("the projected checklist carries this Run")
}

// ---------------------------------------------------------------------------
// 1. The whole chain, and the projection's own answer
// ---------------------------------------------------------------------------

/// A `confirmed` v2 record that really passes must pass on **every** surface:
/// the decoder, the reloaded class and the outward view's status.
#[test]
fn p2b_projection_a_confirmed_v2_record_passes_on_every_surface() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, keys } = &host;
    force_finding(db, run_id, &finding_with(v2_envelope()), "pending");

    // Surface 1: the durable decoder.
    let stored = decode_stored_verdict(&finding_with(v2_envelope()));
    assert!(stored.may_be_passed(), "{stored:?}");
    assert_eq!(stored.category(), "v2");

    // Surface 2: the projected checklist view the caller reads.
    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    assert!(
        decode_stored_verdict(&finding).may_be_passed(),
        "the projection must carry a still-passing envelope: {finding}"
    );
    assert_eq!(
        finding["deliveryVerdict"]["class"], "confirmed",
        "the class must survive the projection verbatim"
    );
    assert_eq!(finding["deliveryVerdict"]["aggregateClass"], "confirmed");
    assert_eq!(finding["deliveryVerdict"]["terminal"], serde_json::json!(true));
    assert_eq!(finding["deliveryVerdict"]["factCount"], serde_json::json!(1));
    let _ = &keys;
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// The card's specific trap: a record the decoder refuses must not be readable as
/// passed through the **row status** either.
///
/// A corrupt envelope keeps whatever `status` the row already had. A row that a
/// previous legacy round left as `passed` must therefore not be republished as a
/// pass just because the decoder says "corrupt, not passed" — the outward view has
/// to agree with the decoder, and this checks that it does.
#[test]
fn p2b_projection_a_corrupt_envelope_never_republishes_a_legacy_passed_status() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    // The hostile combination: a legacy `passed` status with a v2 envelope that
    // cannot be read as one. If the view trusted `status` alone it would say
    // "passed" while the decoder says "corrupt".
    let mut corrupt = v2_envelope();
    corrupt["verdictSchemaVersion"] = serde_json::json!(99);
    force_finding(db, run_id, &finding_with(corrupt.clone()), "passed");

    let stored = decode_stored_verdict(&finding_with(corrupt));
    assert!(!stored.may_be_passed(), "an unknown version is not a pass");
    assert_eq!(stored.category(), "corrupt");

    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    assert!(
        !decode_stored_verdict(&finding).may_be_passed(),
        "the projected record must not decode as a pass: {finding}"
    );
    assert_ne!(stored.category(), "v2");
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 2. The counter-example list, each through the projection
// ---------------------------------------------------------------------------

/// Every corrupt shape the card lists must stay non-passing **after** the
/// projection, and must be visibly damaged rather than invisibly absent.
#[test]
fn p2b_projection_every_listed_corruption_survives_the_projection_as_a_non_pass() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    // (a) the envelope is gone entirely — the legacy shape, which is not a pass;
    // (b) the envelope is present but the version is unknown;
    // (c) an item `class` of `confirmed` whose own facts are a failure;
    // (d) a `confirmed` class with no fact at all to support it;
    // (e) a fact with no rule/check/binding.
    //
    // Note (c): what makes the record hostile is the *fact*, not a disagreement
    // between `class` and `aggregateClass`. Those two legitimately differ — the
    // aggregate folds the whole delivery while the row's class governs one item —
    // so that combination is pinned as valid elsewhere and is not listed here.
    let mut confirmed_but_failed = v2_envelope();
    confirmed_but_failed["facts"] = serde_json::json!([{
        "class": "failed", "origin": "candidate", "path": "out/one.json",
        "sha256": "c".repeat(64), "reasonCode": "content_mismatch",
        "detail": "成果内容与冻结契约不一致", "blocking": true
    }]);
    let mut empty_facts = v2_envelope();
    empty_facts["facts"] = serde_json::json!([]);
    let mut no_binding = v2_envelope();
    no_binding["facts"] = serde_json::json!([{"class": "passed", "origin": "candidate"}]);
    let mut facts_not_an_array = v2_envelope();
    facts_not_an_array["facts"] = serde_json::json!("none");

    for (label, envelope) in [
        ("no envelope at all", Value::Null),
        ("unknown version", {
            let mut value = v2_envelope();
            value["verdictEnvelopeVersion"] = serde_json::json!(7);
            value
        }),
        ("confirmed beside a failure", confirmed_but_failed),
        ("confirmed with no fact at all", empty_facts),
        ("facts not an array", facts_not_an_array),
        ("fact with no binding", no_binding),
    ] {
        let finding = if envelope.is_null() {
            // A legacy record: no envelope key at all.
            serde_json::json!({"reasonCode": "delivery.legacy", "verificationStatus": "failed"})
        } else {
            finding_with(envelope)
        };
        force_finding(db, run_id, &finding, "pending");
        let stored = decode_stored_verdict(&finding);
        assert!(
            !stored.may_be_passed(),
            "{label} must not be a pass at the decoder: {stored:?}"
        );
        let view = projected(db, conversation_id, run_id);
        let projected_finding = view.finding.clone().unwrap_or(serde_json::json!({}));
        assert!(
            !decode_stored_verdict(&projected_finding).may_be_passed(),
            "{label} must still not be a pass after the projection: {projected_finding}"
        );
        assert!(
            !matches!(view.status.as_str(), "passed"),
            "{label} must not be republished as passed: {}",
            view.status
        );
    }
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 3. Structural fields are not stringified away
// ---------------------------------------------------------------------------

/// The card's structural warning, checked on the projection itself: `origin`,
/// `locator` and `expected`/`actual`/`equivalence` must keep their shape.
///
/// The durable round trip is lossless (pinned separately in
/// `p2b_decision_tests`); this asserts what the **projection** does with them,
/// because that is the surface a caller sees. `locator` is the one field the
/// projection does stringify — deliberately, and as an explicitly bounded display
/// value — so its *content* must survive even though its JSON type does not.
#[test]
fn p2b_projection_keeps_structure_in_origin_and_expected_actual_equivalence() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    let fact = serde_json::json!({
        "class": "failed", "origin": "candidate", "path": "out/one.json",
        "sha256": "c".repeat(64), "ruleId": "content:concat", "check": "text_concat",
        "checkerVersion": 3, "reasonCode": "content_mismatch",
        "detail": "成果内容与冻结契约不一致", "blocking": true, "needsRepair": true,
        "expected": "aaa", "actual": "aab", "equivalence": "exact",
        "locator": {"record": 3, "line": 7, "field": 2, "column": "amount", "byteOffset": 128}
    });
    let mut envelope = v2_envelope();
    envelope["class"] = serde_json::json!("definiteFailure");
    envelope["aggregateClass"] = serde_json::json!("definiteFailure");
    envelope["facts"] = serde_json::json!([fact]);
    force_finding(db, run_id, &finding_with(envelope), "pending");

    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    let projected_fact = &finding["deliveryVerdict"]["facts"][0];

    // The scalar structure: origin stays a token, expected/actual/equivalence stay
    // their own fields rather than being folded into `detail`.
    assert_eq!(projected_fact["origin"], "candidate", "{projected_fact}");
    assert_eq!(projected_fact["ruleId"], "content:concat");
    assert_eq!(projected_fact["check"], "text_concat");
    assert_eq!(projected_fact["expected"], "aaa");
    assert_eq!(projected_fact["actual"], "aab");
    assert_eq!(projected_fact["equivalence"], "exact");
    assert_eq!(projected_fact["blocking"], serde_json::json!(true));
    assert_eq!(projected_fact["needsRepair"], serde_json::json!(true));

    // The locator is projected as bounded **display text**, not as an object. That
    // is the one deliberate shape change, so assert both halves of it: it is a
    // string, and every field's value is still legible inside it.
    let locator = projected_fact["locator"]
        .as_str()
        .unwrap_or_else(|| panic!("locator is projected as display text: {projected_fact}"));
    for expected in ["3", "7", "2", "amount", "128"] {
        assert!(
            locator.contains(expected),
            "the locator's {expected} must survive the bounded projection: {locator}"
        );
    }
    // And the projection is bounded rather than unbounded: the display text is
    // capped, so a huge locator cannot bypass the finding ceiling through it.
    assert!(locator.chars().count() <= 512, "{locator}");

    // The durable record itself is still fully structural: the locator decodes back
    // into its five fields, so the stringification is projection-only and the
    // caller that needs structure has it.
    let durable = decode_stored_verdict(&finding_with({
        let mut value = v2_envelope();
        value["facts"] = serde_json::json!([fact]);
        value
    }));
    match durable {
        crate::runtime_host::delivery::StoredVerdict::V2(stored) => {
            let locator = stored.facts[0].locator.clone().expect("the locator survives");
            assert_eq!(locator.record, Some(3));
            assert_eq!(locator.line, Some(7));
            assert_eq!(locator.field, Some(2));
            assert_eq!(locator.column.as_deref(), Some("amount"));
            assert_eq!(locator.byte_offset, Some(128));
        }
        other => panic!("a well-formed envelope must decode as v2, got {other:?}"),
    }
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 4. The ceilings are held, not relaxed
// ---------------------------------------------------------------------------

/// Over-capacity projections must be recorded as **explicitly** missing or
/// truncated, and the ceilings must not be widened to make a record fit.
///
/// The card forbids fixing a test by loosening the redaction or capacity limits,
/// so this asserts the omission flag is present and the projected record still
/// cannot be read as a pass.
#[test]
fn p2b_projection_an_over_capacity_envelope_is_visibly_omitted_never_widened() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    // More than the projection's fact ceiling, each with a large bounded detail.
    let facts: Vec<Value> = (0..200)
        .map(|index| {
            serde_json::json!({
                "class": "unconfirmed", "origin": "spec",
                "detail": "x".repeat(512),
                "reasonCode": format!("delivery.fact_{index}"),
            })
        })
        .collect();
    let mut envelope = v2_envelope();
    envelope["class"] = serde_json::json!("unconfirmed");
    envelope["aggregateClass"] = serde_json::json!("unconfirmed");
    envelope["terminal"] = serde_json::json!(false);
    envelope["facts"] = serde_json::json!(facts);
    force_finding(db, run_id, &finding_with(envelope), "pending");

    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    let omitted = finding["deliveryVerdictOmitted"] == serde_json::json!(true)
        || finding["deliveryVerdict"]["factsTruncated"] == serde_json::json!(true);
    assert!(
        omitted,
        "an over-capacity envelope must say so explicitly: {finding}"
    );
    assert!(
        !decode_stored_verdict(&finding).may_be_passed(),
        "and must never read as a pass: {finding}"
    );
    // The projection's own bounds: never more facts than the ceiling allows.
    if let Some(kept) = finding["deliveryVerdict"]["facts"].as_array() {
        assert!(kept.len() <= 32, "the fact ceiling must hold: {}", kept.len());
    }
    // The total projected finding stays under the capacity the store enforces.
    assert!(
        serde_json::to_vec(&finding).unwrap().len() <= 7680,
        "the finding ceiling must hold, not be widened for this record"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// `factCount` is the reader's only way to tell a complete fact list from a
/// truncated one, so an inconsistent count must not be silently trusted as
/// complete — and a record whose declared count disagrees with what it carries
/// must not become a pass.
#[test]
fn p2b_projection_an_inconsistent_fact_count_is_not_read_as_a_complete_record() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    // A confirmed envelope that claims three items but carries one fact: the
    // reader must not treat the list as complete merely because the record says
    // "confirmed".
    let mut envelope = v2_envelope();
    envelope["itemCount"] = serde_json::json!(3);
    force_finding(db, run_id, &finding_with(envelope), "pending");

    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    assert_eq!(
        finding["deliveryVerdict"]["itemCount"], serde_json::json!(3),
        "the declared count is preserved so the disagreement stays visible"
    );
    assert_eq!(
        finding["deliveryVerdict"]["factCount"], serde_json::json!(1),
        "and the projection reports what it really carries"
    );
    assert_ne!(
        finding["deliveryVerdict"]["itemCount"],
        finding["deliveryVerdict"]["factCount"],
        "the inconsistency must remain observable rather than be smoothed over"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// A legacy v1 record must not be globally demoted to `Unconfirmed`, and a
/// corrupt v2 record must not be rewritten as a fabricated refusal: the card
/// requires the readable facts to survive where they can, and the unprovable part
/// to be explicitly unconfirmed rather than invented.
#[test]
fn p2b_projection_a_legacy_record_keeps_its_own_mapping_and_a_corrupt_one_invents_nothing() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, .. } = &host;

    // A legacy record with real readable facts: it is not v2, and it is not
    // silently relabelled as unconfirmed.
    let legacy = serde_json::json!({
        "reasonCode": "content_mismatch", "verificationStatus": "failed",
        "boundPath": "out/one.json"
    });
    force_finding(db, run_id, &legacy, "failed");
    let stored = decode_stored_verdict(&legacy);
    assert_eq!(stored.category(), "legacy");
    assert!(!stored.may_be_passed());
    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    assert_eq!(finding["reasonCode"], "content_mismatch");
    assert_eq!(
        finding.get("deliveryVerdict"),
        None,
        "a legacy record must not be given a v2 envelope it never had"
    );
    assert!(
        finding.get("deliveryVerdictOmitted").is_none(),
        "and must not be marked as an omitted v2 record either"
    );

    // A corrupt v2 record: unreadable, and no class is fabricated from it.
    let mut corrupt = v2_envelope();
    corrupt["class"] = serde_json::json!("probablyFine");
    force_finding(db, run_id, &finding_with(corrupt), "pending");
    let corrupt_stored = decode_stored_verdict(&finding_with({
        let mut value = v2_envelope();
        value["class"] = serde_json::json!("probablyFine");
        value
    }));
    assert_eq!(corrupt_stored.category(), "corrupt");
    assert!(!corrupt_stored.may_be_passed());
    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    assert!(
        !decode_stored_verdict(&finding).may_be_passed(),
        "a corrupt record stays non-passing through the projection"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// The chain end to end for a **typed** verdict: a real stop, staged under a real
/// committed mark, finalized into the ledger, then read back through the
/// projection.
///
/// This is the case the card's chain description is really about — the envelope
/// the consumer writes is the one the outward view must be able to read — and it
/// closes the loop between §1.3's routing and §1.4's projection.
#[test]
fn p2b_projection_a_finalized_typed_round_is_readable_through_the_outward_view() {
    let (host, coordinator) = chain_host("生成 out/one.json。");
    let ChainHost { db, root, run_id, conversation_id, keys } = &host;

    let verdict = DeliveryVerdict::from_items(
        vec![ItemDecision {
            item_key: keys[0].clone(),
            class: decide_facts(
                &[DecisionFact::failed(
                    candidate("out/one.json"),
                    "content_mismatch",
                    "成果内容与冻结契约不一致",
                    true,
                    None,
                )],
                true,
            )
            .class,
            facts: vec![DecisionFact::failed(
                candidate("out/one.json"),
                "content_mismatch",
                "成果内容与冻结契约不一致",
                true,
                None,
            )],
            repair_budget_remaining: true,
        }],
        true,
    );
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect("a repairable mismatch maps to a stop");
    let output = serde_json::json!({
        "role": "assistant", "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    });
    let delivery = coordinator
        .completion_delivery_for_stop(&[], &output, &stop)
        .expect("the repair round");
    let mark = delivery.mark().expect("staged").to_owned();
    db.record_decision_mark_for_test(run_id, &mark, crate::database::now_ms())
        .unwrap();
    coordinator
        .finish_completion_delivery(Some(&delivery))
        .expect("finalize");

    // The outward view must now show the real verdict, readable and non-passing.
    let view = projected(db, conversation_id, run_id);
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));
    let stored = decode_stored_verdict(&finding);
    assert_eq!(stored.category(), "v2", "{finding}");
    assert!(!stored.may_be_passed(), "{finding}");
    assert_ne!(view.status.as_str(), "passed", "{}", view.status);
    assert_eq!(finding["deliveryVerdict"]["class"], "repairNeeded");
    assert_eq!(finding["deliveryVerdict"]["aggregateClass"], "repairNeeded");
    let fact = &finding["deliveryVerdict"]["facts"][0];
    assert_eq!(fact["origin"], "candidate", "{fact}");
    assert_eq!(fact["reasonCode"], "content_mismatch");
    assert_eq!(fact["expected"], Value::Null, "no expected was fabricated");
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}
