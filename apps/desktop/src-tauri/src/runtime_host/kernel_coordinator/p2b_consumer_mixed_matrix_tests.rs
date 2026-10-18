//! Package 2-B2 extended review (§1.3): the mixed-item matrix.
//!
//! The card asks one question in several shapes: when one delivery verdict holds
//! items of **different** classes, does every item keep its own facts, and does
//! each item end up in the state its class requires rather than the state its
//! neighbour's class implies?
//!
//! The matrix below pairs each stoppable class with an `Unconfirmed` sibling, and
//! adds the two structural cases the card names: a verdict whose items repeat an
//! `item_key`, and a verdict item whose key the checklist does not carry.
//!
//! Every case reads two things back from the database: the row status (which must
//! never turn "not verified" into "failed") and the retained finding's own class
//! envelope (which must survive for the item the terminal write does not own).

use super::*;
use crate::runtime_host::delivery::{
    decide_facts, DecisionFact, DeliveryVerdict, FactOrigin, ItemDecision,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn candidate(path: &str) -> FactOrigin {
    FactOrigin::candidate(path, &"c".repeat(64))
}

fn item(item_key: &str, facts: Vec<DecisionFact>, budget: bool) -> ItemDecision {
    ItemDecision {
        item_key: item_key.to_owned(),
        class: decide_facts(&facts, budget).class,
        facts,
        repair_budget_remaining: budget,
    }
}

fn unconfirmed_fact() -> DecisionFact {
    DecisionFact::unconfirmed(
        FactOrigin::CheckUnbound,
        None,
        "内容核验没有绑定真实产物",
        None,
    )
}

fn unconfirmed_item(item_key: &str) -> ItemDecision {
    item(item_key, vec![unconfirmed_fact()], true)
}

fn refused_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::refused(
            candidate("out/one.json"),
            "path_denied",
            "outside the authorized root",
        )],
        true,
    )
}

fn invalid_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::invalid_with_code(
            "delivery.unreadable_contract",
            "内容契约版本不支持",
        )],
        true,
    )
}

fn definite_failure_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::failed(
            candidate("out/one.json"),
            "content_mismatch",
            "成果内容与冻结契约不一致",
            true,
            None,
        )],
        false,
    )
}

fn repairable_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::failed(
            candidate("out/one.json"),
            "content_mismatch",
            "成果内容与冻结契约不一致",
            true,
            None,
        )],
        true,
    )
}

fn confirmed_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::passed(candidate("out/one.json"))],
        true,
    )
}

/// The clock, cancellation and database a real coordinator borrows. Leaked for
/// the reason the neighbouring consumer tests leak them: `KernelCoordinator<'a>`
/// holds `&'a` references.
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

struct MixedHost {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
    keys: Vec<String>,
}

fn mixed_host(task: &str) -> (MixedHost, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime(task);
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    let keys = db
        .delivery_checklist(&run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    (
        MixedHost { db, root, run_id, keys },
        coordinator,
    )
}

fn pair_host() -> (MixedHost, KernelCoordinator<'static>) {
    mixed_host("生成 out/one.json 与 out/two.json。")
}

fn triple_host() -> (MixedHost, KernelCoordinator<'static>) {
    mixed_host("生成 out/one.json、out/two.json 与 out/three.json。")
}

/// The durable rows, keyed, as `(status, finding)`.
fn rows_by_key(db: &Database, run_id: &str) -> std::collections::BTreeMap<String, (String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status, COALESCE(finding_json,'') FROM delivery_checklist_items
             WHERE run_id=?1",
        )?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                ))
            })?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn event_types(db: &Database, run_id: &str) -> Vec<String> {
    db.with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT event_type FROM kernel_events WHERE run_id=?1 ORDER BY seq")?;
        let rows = statement
            .query_map([run_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn class_of(finding: &str) -> Option<String> {
    let value: Value = serde_json::from_str(finding).ok()?;
    value["deliveryVerdict"]["class"].as_str().map(str::to_owned)
}

/// The durable class envelope of one row, or `None` when the row carries no
/// verdict at all.
fn retained_class(db: &Database, run_id: &str, key: &str) -> Option<String> {
    let rows = rows_by_key(db, run_id);
    rows.get(key).and_then(|(_, finding)| class_of(finding))
}

// ---------------------------------------------------------------------------
// 1. The four mixed pairs
// ---------------------------------------------------------------------------

/// `Refused` beside `Unconfirmed`: the refusal is a determined non-pass and owns
/// the terminal write; the unknown sibling must stay unknown, with its facts.
#[test]
fn p2b_mixed_refused_beside_unconfirmed_keeps_both_facts() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let verdict = DeliveryVerdict::from_items(
        vec![refused_item(&keys[0]), unconfirmed_item(&keys[1])],
        true,
    );
    assert_eq!(verdict.class.as_str(), "refused");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    let typed = stop.typed().unwrap();
    assert_eq!(typed.terminal_items.len(), 1, "only the refusal is staged");
    assert_eq!(typed.pending_items.len(), 1, "the unknown sibling is retained");
    coordinator
        .terminate_with_typed_verdict(&stop)
        .expect("a refused aggregate terminates the Run");

    let rows = rows_by_key(db, run_id);
    assert_eq!(rows[&keys[0]].0, "failed", "the refusal is the failed row");
    assert_eq!(
        class_of(&rows[&keys[0]].1).as_deref(),
        Some("refused"),
        "and it keeps its own class"
    );
    assert_eq!(
        rows[&keys[1]].0, "pending",
        "an unknown sibling must not be written as failed by the refusal beside it"
    );
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed"),
        "nor may the refusal erase the sibling's facts"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// `DefiniteFailure` beside `Unconfirmed`: same terminal ownership, and the
/// sibling's unknown state must survive it.
#[test]
fn p2b_mixed_definite_failure_beside_unconfirmed_keeps_both_facts() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let verdict = DeliveryVerdict::from_items(
        vec![definite_failure_item(&keys[0]), unconfirmed_item(&keys[1])],
        false,
    );
    assert_eq!(verdict.class.as_str(), "definiteFailure");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    assert_eq!(stop.typed().unwrap().pending_items.len(), 1);
    coordinator
        .terminate_with_typed_verdict(&stop)
        .expect("a definite failure terminates the Run");

    let rows = rows_by_key(db, run_id);
    assert_eq!(rows[&keys[0]].0, "failed");
    assert_eq!(class_of(&rows[&keys[0]].1).as_deref(), Some("definiteFailure"));
    assert_eq!(rows[&keys[1]].0, "pending");
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed"),
        "a definite failure must not absorb its unknown sibling"
    );
    // The Run really did fail, and the terminal carries the class's own code.
    assert!(event_types(db, run_id).iter().any(|event| event == "run.failed"));
    assert_eq!(db.kernel_build_full_snapshot(run_id).unwrap().state, "failed");
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// `Invalid` beside `Unconfirmed`: an unusable contract is a spec problem, and it
/// must neither be reported as a content failure nor lose the sibling's facts.
#[test]
fn p2b_mixed_invalid_beside_unconfirmed_keeps_both_facts() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let verdict = DeliveryVerdict::from_items(
        vec![invalid_item(&keys[0]), unconfirmed_item(&keys[1])],
        true,
    );
    assert_eq!(verdict.class.as_str(), "invalid");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    coordinator.terminate_with_typed_verdict(&stop).expect("invalid terminates");
    let rows = rows_by_key(db, run_id);
    assert_eq!(class_of(&rows[&keys[0]].1).as_deref(), Some("invalid"));
    assert_ne!(
        class_of(&rows[&keys[0]].1).as_deref(),
        Some("definiteFailure"),
        "an unreadable contract is not a content mismatch"
    );
    assert_eq!(rows[&keys[1]].0, "pending");
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed")
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// `RepairNeeded` beside `Unconfirmed`: the repairable item is staged **under the
/// repair round's own decision**, not finalized as a failure — and the unknown
/// sibling is retained rather than dragged into the repair's terminal write.
#[test]
fn p2b_mixed_repair_needed_beside_unconfirmed_keeps_both_facts() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let verdict =
        DeliveryVerdict::from_items(vec![repairable_item(&keys[0]), unconfirmed_item(&keys[1])], true);
    assert_eq!(
        verdict.class.as_str(),
        "repairNeeded",
        "a repairable mismatch beside an unknown item is still repairable"
    );
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    let typed = stop.typed().unwrap();
    assert_eq!(
        typed.terminal_items.len(),
        1,
        "the repairable row rides the repair round's decision"
    );
    assert_eq!(typed.pending_items.len(), 1);
    assert!(
        stop.repair_prompt().is_some(),
        "the repair round carries its instruction"
    );

    // The repair lane is the completion lane's `Repair` route; drive it as the
    // live loop does rather than calling the terminal admission, which must
    // refuse a non-terminal class.
    let history: Vec<Value> = Vec::new();
    let output = serde_json::json!({
        "role": "assistant", "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    });
    let delivery = coordinator
        .completion_delivery_for_stop(&history, &output, &stop)
        .expect("a typed repair stop must produce a repair round");
    assert!(matches!(
        delivery,
        super::super::CompletionDelivery::Repair { .. }
    ));
    // The sibling's facts are durable before the round commits, so a crash mid
    // round cannot lose them either.
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed"),
        "the unknown sibling keeps its own class through the repair round"
    );
    assert_eq!(rows_by_key(db, run_id)[&keys[1]].0, "pending");
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// `Confirmed` beside `Unconfirmed`: the aggregate is `Unconfirmed`, so **no**
/// item may be written as passed or failed — and the confirmed item's own facts
/// must still be durably retained rather than dropped for being "not pending".
///
/// This is the case the card singles out: filtering to the pending rows alone
/// keeps the unknown item and silently discards the known one.
#[test]
fn p2b_mixed_confirmed_beside_unconfirmed_keeps_the_known_items_facts() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let verdict =
        DeliveryVerdict::from_items(vec![confirmed_item(&keys[0]), unconfirmed_item(&keys[1])], true);
    assert_eq!(
        verdict.class.as_str(),
        "unconfirmed",
        "one unknown item holds the whole aggregate back"
    );
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        verdict,
    )
    .expect_err("an unconfirmed aggregate has no stop");
    let returned = returned.expect_requires_u1();
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(
        written, 2,
        "both rows carry facts the ledger must keep — the withheld aggregate is not a licence to drop the known item"
    );

    let rows = rows_by_key(db, run_id);
    for key in keys.iter() {
        assert_eq!(
            rows[key].0, "pending",
            "an unconfirmed aggregate writes no terminal for any item"
        );
    }
    assert_eq!(
        retained_class(db, run_id, &keys[0]).as_deref(),
        Some("confirmed"),
        "the confirmed item's own class must survive the aggregate refusal to decide"
    );
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed")
    );
    assert!(
        !event_types(db, run_id).iter().any(|event| event == "run.failed"
            || event == "run.completed"),
        "no terminal may be written for a delivery that was never decided"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// The three-item form of the same rule: a `Refused` non-pass, a `Confirmed`
/// item and an `Unconfirmed` one. The refusal owns the terminal; both siblings
/// keep their own class and status.
#[test]
fn p2b_mixed_three_items_keep_every_class_apart() {
    let (host, coordinator) = triple_host();
    let MixedHost { db, root, run_id, keys } = &host;
    assert_eq!(keys.len(), 3, "{keys:?}");
    let verdict = DeliveryVerdict::from_items(
        vec![
            refused_item(&keys[0]),
            confirmed_item(&keys[1]),
            unconfirmed_item(&keys[2]),
        ],
        true,
    );
    assert_eq!(verdict.class.as_str(), "refused");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    coordinator.terminate_with_typed_verdict(&stop).expect("refused terminates");

    let rows = rows_by_key(db, run_id);
    assert_eq!(rows[&keys[0]].0, "failed");
    assert_eq!(class_of(&rows[&keys[0]].1).as_deref(), Some("refused"));
    // The Confirmed sibling is part of the terminal decision's own staging (its
    // class may enter the terminal stage), so it is written, and written passed.
    assert_eq!(
        rows[&keys[1]].0, "passed",
        "a confirmed item is the one thing the terminal stage may record as passed"
    );
    assert_eq!(class_of(&rows[&keys[1]].1).as_deref(), Some("confirmed"));
    assert_eq!(rows[&keys[2]].0, "pending");
    assert_eq!(
        retained_class(db, run_id, &keys[2]).as_deref(),
        Some("unconfirmed")
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 2. Structural cases the card names
// ---------------------------------------------------------------------------

/// Two verdict items that name the same `item_key` must not collapse one item's
/// facts onto the other silently.
///
/// **Expectation changed by the `G-P2B2-H-return-repair` card (H3 RETURN).**
///
/// Before: this test asserted the encoder resolved the repetition itself by
/// letting the row whose own class equalled the aggregate class win, and then
/// asserted that one row was written. G's own comment above old line 490 already
/// claimed the surviving row was "never a merge", but the assertion could not see
/// the *other* item: `refused_item(&repeated)` and `confirmed_item(&repeated)`
/// both bound `out/one.json`, so the overwritten row's own facts were gone with no
/// visible record. H's probe proved that with two distinct bindings, where the
/// dropped fact is observable.
///
/// After: the card chose the explicit-refusal route (§3: "本卡选 H 推荐的显式拒绝
/// 路线，不把两个不同 item 偷合成一个新裁决"), so an input that names one ledger key
/// twice is refused **before any write** rather than resolved by picking a winner.
/// This test therefore asserts the refusal, and the durable consequence: nothing
/// staged, nothing retained, no terminal, and the ledger rows unchanged.
///
/// The replacement assertion is strictly stronger than the old one — it still
/// forbids a silent merge, and it additionally forbids silently discarding the
/// losing item — so no coverage is lost by the change.
#[test]
fn p2b_mixed_a_repeated_item_key_never_merges_two_items_into_a_new_verdict() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let repeated = keys[0].clone();
    let before = rows_by_key(db, run_id);
    let verdict = DeliveryVerdict::from_items(
        vec![refused_item(&repeated), confirmed_item(&repeated)],
        true,
    );
    assert_eq!(
        verdict.class.as_str(),
        "refused",
        "the aggregate still folds both items' facts"
    );
    // The refusal, not a winner. H3's contract: one ledger row cannot carry two
    // verdicts, so two items claiming it is malformed input, and neither item may
    // be dropped to make the input look well-formed.
    let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect_err("a repeated item_key is refused, not resolved by picking a winner");
    let ambiguity = refusal
        .ambiguity()
        .expect("this is an input ambiguity, not the RequiresU1 handoff");
    assert_eq!(ambiguity.ambiguous.len(), 1);
    assert_eq!(ambiguity.ambiguous[0].item_key, repeated);
    assert_eq!(
        ambiguity.ambiguous[0].collisions.len(),
        2,
        "both competing items come back whole, so no fact is silently lost"
    );
    // Nothing was written by the refusal.
    assert_eq!(
        rows_by_key(db, run_id),
        before,
        "a refused verdict must leave every durable row unchanged"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// A verdict item whose key the checklist does not carry must not invent a
/// ledger row and must not fail the batch: it is retained nowhere, and the rows
/// that do exist are written normally.
#[test]
fn p2b_mixed_a_verdict_item_with_no_ledger_row_neither_creates_nor_blocks_one() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let absent = "file:no-such-declared-artifact.json".to_owned();
    assert!(!keys.contains(&absent));
    let verdict = DeliveryVerdict::from_items(
        vec![unconfirmed_item(&absent), refused_item(&keys[0])],
        true,
    );
    assert_eq!(verdict.class.as_str(), "refused");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    let written = crate::runtime_host::delivery::retain_pending_typed_stop(
        db,
        run_id,
        &stop,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(
        written, 0,
        "the key is not in the ledger, so the retention write matches nothing and says so"
    );
    coordinator.terminate_with_typed_verdict(&stop).expect("refused terminates");
    let rows = rows_by_key(db, run_id);
    assert_eq!(
        rows.len(),
        keys.len(),
        "an unknown key must not add a checklist row"
    );
    assert!(
        !rows.contains_key(&absent),
        "the unknown key stays absent from the ledger"
    );
    assert_eq!(rows[&keys[0]].0, "failed", "the known refusal still landed");
    assert_eq!(
        rows[&keys[1]].0,
        "pending",
        "and the untouched checked item is untouched"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 3. The batch write's own contract
// ---------------------------------------------------------------------------

/// A batch write must report the rows it really wrote. A row that is already
/// terminal matches nothing — the retention write is scoped to `pending` — so a
/// partial write reports the real count instead of claiming the whole batch
/// landed.
#[test]
fn p2b_retention_reports_real_hits_and_never_reopens_a_terminated_row() {
    let (host, coordinator) = pair_host();
    let MixedHost { db, root, run_id, keys } = &host;
    let now = crate::database::now_ms();
    // Terminate the first row through the real terminal lane, so the row's status
    // comes from production code rather than a hand-written UPDATE.
    let verdict = DeliveryVerdict::from_items(vec![refused_item(&keys[0])], true);
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict).unwrap();
    coordinator.terminate_with_typed_verdict(&stop).expect("terminates");
    let terminal_finding = rows_by_key(db, run_id)[&keys[0]].1.clone();
    assert_eq!(rows_by_key(db, run_id)[&keys[0]].0, "failed");

    // Now ask the retention path to write both rows.
    let batch = vec![
        (keys[0].clone(), "{\"deliveryVerdict\":{\"class\":\"unconfirmed\"}}".to_owned()),
        (keys[1].clone(), "{\"deliveryVerdict\":{\"class\":\"unconfirmed\"}}".to_owned()),
    ];
    let written = crate::runtime_host::delivery::retain_pending_items(db, run_id, &batch, now)
        .unwrap();
    assert_eq!(
        written, 1,
        "only the pending row is written; the count must not claim a full batch"
    );
    let rows = rows_by_key(db, run_id);
    assert_eq!(
        rows[&keys[0]].0, "failed",
        "a terminated row is never reopened by a retention write"
    );
    assert_eq!(
        rows[&keys[0]].1, terminal_finding,
        "and its finding is left exactly as the terminal wrote it"
    );
    assert_eq!(
        retained_class(db, run_id, &keys[1]).as_deref(),
        Some("unconfirmed"),
        "the pending sibling really was written"
    );

    // A batch that matches nothing reports zero rather than an error, and an
    // empty batch is the ordinary no-op.
    let zero = crate::runtime_host::delivery::retain_pending_items(
        db,
        run_id,
        &[(keys[0].clone(), "{}".to_owned())],
        now,
    )
    .unwrap();
    assert_eq!(zero, 0, "a fully terminated batch writes nothing and says so");
    let empty = crate::runtime_host::delivery::retain_pending_items(db, run_id, &[], now).unwrap();
    assert_eq!(empty, 0, "an empty batch is a no-op, not a false success");
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}
