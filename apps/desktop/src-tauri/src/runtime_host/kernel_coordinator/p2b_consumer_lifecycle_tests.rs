//! Package 2-B2 extended review (§1.5): transaction, restart and re-execution.
//!
//! The delivery write is deliberately two-phase — the verdicts are staged under a
//! mark, the round decision commits that mark, and only then may the stage become
//! the ledger's history. The card asks for the three windows that creates, each
//! observed from a **reopened database** rather than from an in-memory handle:
//!
//! 1. staged, but the decision's mark never committed;
//! 2. the mark committed, but the finalize never ran;
//! 3. the finalize started and died partway through its own transaction.
//!
//! Plus the properties the two-phase write exists to give: the same mark replayed
//! finalizes nothing twice, an old mark cannot consume a newer round's stage, a
//! retention write never reopens a terminated row, and an aborted stage batch
//! leaves no partial stage behind.
//!
//! Fault-injection levels, stated because the card requires it: the two aborts
//! below are `#[cfg(test)]`-only hooks **inside** the production transactions
//! (`database::stage_fault`, `database::finalize_fault`). They do not model a
//! power loss at an arbitrary instruction; they model the caller losing the
//! process at one documented point of the write, which is what makes the
//! transaction's own rollback the thing under test. Neither hook exists in a
//! production build, and neither is reachable from the delivery gate's callers.

use super::*;
use crate::runtime_host::delivery::{
    decide_facts, DecisionFact, DeliveryVerdict, DeliveryStop, FactOrigin, ItemDecision,
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

fn unconfirmed_item(item_key: &str) -> ItemDecision {
    item(
        item_key,
        vec![DecisionFact::unconfirmed(
            FactOrigin::CheckUnbound,
            None,
            "内容核验没有绑定真实产物",
            None,
        )],
        true,
    )
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

/// The clock, cancellation and database a real coordinator borrows, with the
/// given task's promised artifacts seeded exactly as a production start seeds
/// them. Leaked for the reason the neighbouring consumer tests leak them:
/// `KernelCoordinator<'a>` holds `&'a` references.
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

struct LifecycleHost {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
    keys: Vec<String>,
}

fn lifecycle_host(task: &str) -> (LifecycleHost, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime(task);
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    let keys = db
        .delivery_checklist(&run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    (
        LifecycleHost { db, root, run_id, keys },
        coordinator,
    )
}

fn single_host() -> (LifecycleHost, KernelCoordinator<'static>) {
    lifecycle_host("生成 out/one.json。")
}

/// Two items whose verdict stages **both** rows, so a mid-batch abort has a
/// second item to die on: `Confirmed` beside a repairable mismatch still folds to
/// `RepairNeeded`, and both classes may enter the terminal stage.
fn pair_host() -> (LifecycleHost, KernelCoordinator<'static>) {
    lifecycle_host("生成 out/one.json 与 out/two.json。")
}

fn output() -> Value {
    serde_json::json!({
        "role": "assistant", "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    })
}

fn stop_for(verdict: DeliveryVerdict) -> DeliveryStop {
    crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect("a terminal or repairable verdict maps to a stop")
}

/// The durable rows, keyed, as `(status, finding, checked_at)`.
fn rows_by_key(
    db: &Database,
    run_id: &str,
) -> std::collections::BTreeMap<String, (String, String, Option<i64>)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status, COALESCE(finding_json,''), checked_at
             FROM delivery_checklist_items WHERE run_id=?1",
        )?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    (
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                    ),
                ))
            })?
            .collect::<rusqlite::Result<std::collections::BTreeMap<_, _>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn class_of(finding: &str) -> Option<String> {
    let value: Value = serde_json::from_str(finding).ok()?;
    value["deliveryVerdict"]["class"].as_str().map(str::to_owned)
}

/// Reopen the run's database the way the app's next start does.
fn reopen(root: &std::path::Path) -> Database {
    Database::open(root.join("facts.db")).unwrap()
}

// ---------------------------------------------------------------------------
// 1. The three interruption points
// ---------------------------------------------------------------------------

/// Interruption point 1: staged, then the decision that owned the stage never
/// committed its mark.
///
/// The stage must not become history. A startup recovery withdraws it, the rows
/// stay exactly as the abandoned round found them, and no repair round is
/// recorded — read back from a **reopened** database, so the assertion is about
/// durable bytes rather than a live handle.
#[test]
fn p2b_lifecycle_a_stage_without_its_committed_mark_is_withdrawn_on_reopen() {
    let (host, coordinator) = single_host();
    let LifecycleHost { db, root, run_id, keys } = &host;
    let stop = stop_for(DeliveryVerdict::from_items(
        vec![repairable_item(&keys[0])],
        true,
    ));

    let delivery = coordinator
        .completion_delivery_for_stop(&[], &output(), &stop)
        .expect("a repairable stop arms its repair round");
    let mark = delivery.mark().expect("staged").to_owned();

    // Fact 1: the stage is durable, and its mark is *not* committed.
    assert_eq!(db.staged_delivery_rounds().unwrap().len(), 1);
    assert!(!db.decision_mark_committed(run_id, &mark).unwrap());
    // Fact 2: nothing was written to the ledger before the decision landed.
    let before = rows_by_key(db, run_id);
    assert_eq!(before[&keys[0]].0, "pending", "the stage wrote no verdict");
    assert_eq!(class_of(&before[&keys[0]].1), None);
    assert_eq!(db.delivery_repair_round_count(run_id).unwrap(), 0);

    drop(coordinator);
    let reopened = reopen(&root);

    // The startup sweep is the real recovery path; it must refuse this stage.
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        0,
        "a stage whose decision never committed finalizes nothing"
    );
    assert!(
        reopened.staged_delivery_rounds().unwrap().is_empty(),
        "the uncommitted stage is withdrawn rather than left to be found later"
    );
    let after = rows_by_key(&reopened, run_id);
    assert_eq!(after[&keys[0]].0, "pending");
    assert_eq!(
        after[&keys[0]].1, before[&keys[0]].1,
        "the abandoned round left the finding byte-for-byte unchanged"
    );
    assert_eq!(
        reopened.delivery_repair_round_count(run_id).unwrap(),
        0,
        "a round that never committed must not spend the repair budget"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Interruption point 2: the mark committed, the finalize never ran.
///
/// Recovery may finish it — the committed mark is the proof that a decision
/// authorized these verdicts — and doing it twice must not write twice. The
/// second pass is checked on `checked_at`, which only changes on a real write.
#[test]
fn p2b_lifecycle_a_committed_stage_is_finalized_once_and_replays_as_a_no_op() {
    let (host, coordinator) = single_host();
    let LifecycleHost { db, root, run_id, keys } = &host;
    let stop = stop_for(DeliveryVerdict::from_items(
        vec![repairable_item(&keys[0])],
        true,
    ));
    let delivery = coordinator
        .completion_delivery_for_stop(&[], &output(), &stop)
        .expect("repair round");
    let mark = delivery.mark().expect("staged").to_owned();

    // The decision's own commit, then the crash: the finalize is never reached.
    db.record_decision_mark_for_test(run_id, &mark, crate::database::now_ms())
        .unwrap();
    assert!(db.decision_mark_committed(run_id, &mark).unwrap());
    drop(coordinator);

    let reopened = reopen(&root);
    assert_eq!(rows_by_key(&reopened, run_id)[&keys[0]].0, "pending");
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        1,
        "the committed stage is recovered"
    );
    let after = rows_by_key(&reopened, run_id);
    assert_ne!(
        after[&keys[0]].0, "passed",
        "a repairable mismatch is never recovered as a pass"
    );
    assert_eq!(
        class_of(&after[&keys[0]].1).as_deref(),
        Some("repairNeeded"),
        "the recovered row still carries the class it was decided under"
    );
    assert!(after[&keys[0]].2.is_some(), "the decision instant is recorded");
    assert_eq!(
        reopened.delivery_repair_round_count(run_id).unwrap(),
        1,
        "the recovery records the repair round exactly once"
    );
    let decided_at = after[&keys[0]].2;

    // Replaying the same committed mark, and the startup sweep over a ledger that
    // no longer has a stage, are both no-ops.
    assert_eq!(
        crate::runtime_host::delivery::finalize_outcome(
            &reopened,
            run_id,
            &mark,
            crate::database::now_ms()
        )
        .unwrap(),
        0,
        "a replayed finalize writes nothing"
    );
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        0
    );
    let replayed = rows_by_key(&reopened, run_id);
    assert_eq!(
        replayed[&keys[0]].2, decided_at,
        "a replay must not restamp the instant the verdict was really decided"
    );
    assert_eq!(
        reopened.delivery_repair_round_count(run_id).unwrap(),
        1,
        "and must not add a second repair round"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Interruption point 3: the finalize starts and dies partway through its own
/// transaction.
///
/// The abort is injected after one item update, so the rollback is the property
/// under test: the stage must survive intact with its committed mark and **no**
/// item may be half-finalized. Recovery then writes the whole set.
#[test]
fn p2b_lifecycle_a_mid_finalize_death_rolls_back_and_recovery_finishes_the_round() {
    let (host, coordinator) = pair_host();
    let LifecycleHost { db, root, run_id, keys } = &host;
    assert_eq!(keys.len(), 2);
    let stop = stop_for(DeliveryVerdict::from_items(
        vec![confirmed_item(&keys[0]), repairable_item(&keys[1])],
        true,
    ));
    let typed = stop.typed().expect("typed stop");
    assert_eq!(
        typed.terminal_items.len(),
        2,
        "both rows are stageable, so the abort has a second item to die on"
    );
    let delivery = coordinator
        .completion_delivery_for_stop(&[], &output(), &stop)
        .expect("repair round");
    let mark = delivery.mark().expect("staged").to_owned();
    db.record_decision_mark_for_test(run_id, &mark, crate::database::now_ms())
        .unwrap();

    // Die inside the finalize transaction once the first item was updated.
    crate::database::finalize_fault::abort_after(1);
    let aborted = coordinator.finish_completion_delivery(Some(&delivery));
    crate::database::finalize_fault::disarm();
    assert!(aborted.is_err(), "the injected death must really interrupt it");

    // The transaction rolled back as a whole: nothing half-written.
    for key in keys.iter() {
        assert_eq!(
            rows_by_key(db, run_id)[key].0, "pending",
            "{key} must not be half-finalized"
        );
    }
    assert_eq!(db.delivery_repair_round_count(run_id).unwrap(), 0);
    assert_eq!(
        db.staged_delivery_rounds().unwrap().len(),
        1,
        "the committed stage survives for the next start to finish"
    );
    drop(coordinator);

    let reopened = reopen(&root);
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        2,
        "recovery writes the whole recovered set"
    );
    let rows = rows_by_key(&reopened, run_id);
    assert_eq!(rows[&keys[0]].0, "passed", "the confirmed row is the pass");
    assert_eq!(
        rows[&keys[1]].0, "failed",
        "the repairable row is a recorded non-pass, not a pass"
    );
    assert_eq!(
        class_of(&rows[&keys[1]].1).as_deref(),
        Some("repairNeeded")
    );
    assert_eq!(reopened.delivery_repair_round_count(run_id).unwrap(), 1);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 2. Mark scoping and the stage batch's own transaction
// ---------------------------------------------------------------------------

/// An old mark must never consume a newer round's stage.
///
/// Round A stages under a mark that never commits; round B replaces the stage and
/// commits its own mark. Finalizing under A's mark must write nothing at all —
/// neither A's verdicts nor B's — because the stage now belongs to B.
#[test]
fn p2b_lifecycle_an_old_mark_never_consumes_a_newer_stage() {
    let (host, coordinator) = pair_host();
    let LifecycleHost { db, root, run_id, keys } = &host;

    let stop_a = stop_for(DeliveryVerdict::from_items(
        vec![repairable_item(&keys[0])],
        true,
    ));
    let round_a = coordinator
        .completion_delivery_for_stop(&[], &output(), &stop_a)
        .expect("round A");
    let mark_a = round_a.mark().expect("staged").to_owned();

    // Round B supersedes A's stage: only the newest round can still be waiting.
    let stop_b = stop_for(DeliveryVerdict::from_items(
        vec![repairable_item(&keys[1])],
        true,
    ));
    let round_b = coordinator
        .completion_delivery_for_stop(&[], &output(), &stop_b)
        .expect("round B");
    let mark_b = round_b.mark().expect("staged").to_owned();
    assert_ne!(mark_a, mark_b);
    db.record_decision_mark_for_test(run_id, &mark_b, crate::database::now_ms())
        .unwrap();
    assert!(!db.decision_mark_committed(run_id, &mark_a).unwrap());

    // A's mark names no stage any more, and A's decision never committed.
    assert_eq!(
        crate::runtime_host::delivery::finalize_outcome(
            db,
            run_id,
            &mark_a,
            crate::database::now_ms()
        )
        .unwrap(),
        0,
        "the superseded mark finalizes nothing"
    );
    let staged = db.staged_delivery_rounds().unwrap();
    assert_eq!(
        staged,
        vec![(run_id.clone(), mark_b.clone())],
        "B's stage is untouched by A's finalize"
    );
    assert_eq!(db.delivery_repair_round_count(run_id).unwrap(), 0);

    // B's own finalize writes B's verdict and only B's.
    assert_eq!(
        crate::runtime_host::delivery::finalize_outcome(
            db,
            run_id,
            &mark_b,
            crate::database::now_ms()
        )
        .unwrap(),
        1
    );
    let rows = rows_by_key(db, run_id);
    assert_eq!(
        rows[&keys[0]].0, "pending",
        "A's item was never authorized by a committed decision"
    );
    assert_eq!(rows[&keys[1]].0, "failed");
    assert_eq!(db.delivery_repair_round_count(run_id).unwrap(), 1);
    let _ = std::fs::remove_dir_all(root);
}

/// The stage batch is one transaction: an abort partway through leaves **no**
/// partial stage, so a later reader cannot see half a round's verdicts.
#[test]
fn p2b_lifecycle_an_aborted_stage_batch_leaves_no_partial_stage() {
    let (host, coordinator) = pair_host();
    let LifecycleHost { db, root, run_id, keys } = &host;
    let stop = stop_for(DeliveryVerdict::from_items(
        vec![confirmed_item(&keys[0]), repairable_item(&keys[1])],
        true,
    ));
    assert_eq!(stop.typed().unwrap().terminal_items.len(), 2);

    crate::database::stage_fault::abort_after(1);
    let aborted = coordinator.completion_delivery_for_stop(&[], &output(), &stop);
    crate::database::stage_fault::disarm();
    assert!(aborted.is_err(), "the injected death must interrupt the stage");

    assert!(
        db.staged_delivery_rounds().unwrap().is_empty(),
        "the first item's insert must not survive its sibling's abort"
    );
    for key in keys.iter() {
        assert_eq!(rows_by_key(db, run_id)[key].0, "pending");
    }
    assert_eq!(db.delivery_repair_round_count(run_id).unwrap(), 0);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 3. Retention across a reopen
// ---------------------------------------------------------------------------

/// A retention write is scoped to `pending` rows, and that scoping is durable:
/// after a database reopen it still must not reopen a row a real decision already
/// terminated, and it must report the partial write honestly.
#[test]
fn p2b_lifecycle_retention_never_reopens_a_terminated_row_across_reopen() {
    let (host, coordinator) = pair_host();
    let LifecycleHost { db, root, run_id, keys } = &host;

    // Terminate the first row through the real failed-terminal admission.
    let stop = stop_for(DeliveryVerdict::from_items(
        vec![refused_item(&keys[0])],
        true,
    ));
    coordinator
        .terminate_with_typed_verdict(&stop)
        .expect("a refusal terminates the Run");
    let terminal = rows_by_key(db, run_id);
    assert_eq!(terminal[&keys[0]].0, "failed");
    let terminal_finding = terminal[&keys[0]].1.clone();
    drop(coordinator);

    // A later round holds facts for both items: a known pass for the row that is
    // already terminated, and an unknown sibling. Reopened, so the scoping is
    // tested against durable state rather than a cached handle.
    let reopened = reopen(&root);
    let verdict = DeliveryVerdict::from_items(
        vec![confirmed_item(&keys[0]), unconfirmed_item(&keys[1])],
        true,
    );
    assert!(verdict.requires_u1());
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        &reopened,
        run_id,
        &reopened.delivery_checklist(run_id).unwrap(),
        &verdict,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(
        written, 1,
        "only the pending sibling is written — a partial batch must not report a full save"
    );

    let rows = rows_by_key(&reopened, run_id);
    assert_eq!(
        rows[&keys[0]].0, "failed",
        "a terminated row is never reopened by retention"
    );
    assert_eq!(
        rows[&keys[0]].1, terminal_finding,
        "and its finding is left exactly as the terminal wrote it"
    );
    assert_eq!(rows[&keys[1]].0, "pending");
    assert_eq!(class_of(&rows[&keys[1]].1).as_deref(), Some("unconfirmed"));
    assert!(
        reopened.staged_delivery_rounds().unwrap().is_empty(),
        "retention stages nothing"
    );
    assert_eq!(reopened.delivery_repair_round_count(run_id).unwrap(), 0);
    let _ = std::fs::remove_dir_all(root);
}

/// A withheld aggregate's retention must survive a reopen — including the facts of
/// the item it *could* decide.
///
/// `Confirmed` beside `Unconfirmed` folds to `Unconfirmed`, which has no stop at
/// all: nothing is staged, so the only durable record is the retention write. Read
/// back from a reopened database, **both** rows must still carry their own class.
/// A retention path that kept only the rows it was about to leave non-terminal
/// would lose the known item here and nowhere else, because this is the only shape
/// in which a decidable item reaches retention without a terminal of its own.
#[test]
fn p2b_lifecycle_a_withheld_aggregate_keeps_every_items_facts_across_reopen() {
    let (host, coordinator) = pair_host();
    let LifecycleHost { db, root, run_id, keys } = &host;

    let verdict = DeliveryVerdict::from_items(
        vec![confirmed_item(&keys[0]), unconfirmed_item(&keys[1])],
        true,
    );
    assert!(
        verdict.requires_u1(),
        "one unknown item holds the whole aggregate back"
    );
    let withheld = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        verdict,
    )
    .expect_err("an unconfirmed aggregate has no stop to route");
    let withheld = withheld.expect_requires_u1();
    assert_eq!(
        withheld.pending_items().count(),
        1,
        "only the unknown item is pending; the known one is not 'pending', it is simply undecided"
    );
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        withheld,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(
        written, 2,
        "both rows carry facts the ledger must keep: the known item is not dropped for being decidable"
    );
    drop(coordinator);

    let reopened = reopen(&root);
    let rows = rows_by_key(&reopened, run_id);
    for key in keys.iter() {
        assert_eq!(
            rows[key].0, "pending",
            "a withheld aggregate writes no terminal for any item"
        );
    }
    assert_eq!(class_of(&rows[&keys[0]].1).as_deref(), Some("confirmed"));
    assert_eq!(class_of(&rows[&keys[1]].1).as_deref(), Some("unconfirmed"));
    assert!(
        reopened.staged_delivery_rounds().unwrap().is_empty(),
        "nothing was staged, so a reopen has no stage to resolve"
    );
    assert_eq!(
        crate::runtime_host::delivery::finalize_staged_outcomes(&reopened).unwrap(),
        0
    );
    let after_recovery = rows_by_key(&reopened, run_id);
    assert_eq!(
        after_recovery[&keys[0]].1, rows[&keys[0]].1,
        "the sweep must not touch a ledger with no stage"
    );
    let _ = std::fs::remove_dir_all(root);
}
