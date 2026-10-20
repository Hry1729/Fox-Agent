//! Acceptance tests for the H1 / H3 / H4 repairs (card `G-P2B2-H-return-repair`).
//!
//! H's probes (`h-p2b2-snapshot`, `p2b_consumer_h_acceptance_tests.rs`) established
//! three RETURN defects against the G-repaired candidate. H's files are kept
//! read-only as evidence of the **pre-repair** state, so the acceptance assertions
//! live here instead. They are deliberately written against the same routes H used,
//! so a reader can compare them side by side:
//!
//! * **H1** — a `RequiresU1` delivery on the completion lane must not complete the
//!   Run. H drove a real stop through `completion_decision_for_test`; the same
//!   route is exercised here, plus a check that no *other* tail finalizes the Run.
//! * **H3** — two items sharing an `item_key` must not silently lose one item's
//!   facts. H's probe asserted "both facts are durably retained"; this card chose
//!   the explicit-refusal route, so the acceptance asserts "refused, both facts
//!   intact in the error, zero writes, zero terminals".
//! * **H4** — a record whose own facts do not support `Confirmed` must not be
//!   written as passed by the real write path, nor displayed as passed by the real
//!   read path.
//!
//! Every assertion reads durable database state or a real `load_conversation`
//! projection. Nothing here asserts an enum conversion alone.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::*;
use crate::runtime_host::delivery::{DecisionFact, DeliveryVerdict, FactOrigin, ItemDecision};
use crate::runtime_host::kernel_coordinator::live::StopFollowup;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn candidate(path: &str) -> FactOrigin {
    FactOrigin::candidate(path, &"a".repeat(64))
}

fn item(item_key: &str, facts: Vec<DecisionFact>, budget: bool) -> ItemDecision {
    ItemDecision {
        item_key: item_key.to_owned(),
        class: crate::runtime_host::delivery::decide_facts(&facts, budget).class,
        facts,
        repair_budget_remaining: budget,
    }
}

/// A `Confirmed` item, built through the real folding core with an all-passed fact.
fn confirmed_item(item_key: &str, path: &str) -> ItemDecision {
    item(item_key, vec![DecisionFact::passed(candidate(path))], true)
}

/// An `Unconfirmed` item: nothing was bound to a real artifact.
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

/// The one legal class with no stop to route.
fn requires_u1_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(vec![unconfirmed_item("slot:xlsx:1")], true)
}

fn leaked_runtime() -> (
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
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let db: &'static Database = Box::leak(Box::new(db));
    (clock, cancellation, db, root, run_id)
}

/// The real-Host fixture: a real Run, a real frozen scope and the real
/// coordinator. Shared with the package-2-D lane tests (`delivery_review_lane_tests`),
/// which need exactly this shape to reach the parked state through the real tail.
pub(super) struct Host {
    pub(super) db: &'static Database,
    pub(super) root: std::path::PathBuf,
    pub(super) run_id: String,
    pub(super) conversation_id: String,
    pub(super) keys: Vec<String>,
}

/// The real-Host fixture: a real Run, a real frozen scope and the real
/// coordinator. Shared with the package-2-D lane tests
/// (`delivery_review_lane_tests`), which need exactly this shape to reach the
/// parked state through the real shared tail.
pub(super) fn host() -> (Host, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime();
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    let keys: Vec<String> = db
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
        Host { db, root, run_id, conversation_id, keys },
        coordinator,
    )
}

/// The outward checklist projection the real UI reads.
fn projected(
    db: &Database,
    conversation_id: &str,
    run_id: &str,
) -> Vec<crate::database::DeliveryChecklistView> {
    db.load_conversation(conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .filter(|view| view.run_id == run_id)
        .collect()
}

/// One projected row by key.
fn view_of(
    db: &Database,
    conversation_id: &str,
    run_id: &str,
    item_key: &str,
) -> crate::database::DeliveryChecklistView {
    projected(db, conversation_id, run_id)
        .into_iter()
        .find(|view| view.item_key == item_key)
        .expect("the projected checklist carries this item")
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

fn terminal_state(db: &Database, run_id: &str) -> String {
    db.kernel_build_full_snapshot(run_id).unwrap().state
}

fn committed_marks(db: &Database, run_id: &str) -> Vec<String> {
    db.with_connection(|connection| {
        let mut statement =
            connection.prepare("SELECT mark FROM kernel_decision_marks WHERE run_id=?1")?;
        let rows = statement
            .query_map([run_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn rows(db: &Database, run_id: &str) -> Vec<(String, String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status, COALESCE(finding_json,'') FROM delivery_checklist_items
             WHERE run_id=?1 ORDER BY item_key",
        )?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    })
    .unwrap()
}

fn staged_rounds(db: &Database, run_id: &str) -> Vec<String> {
    db.staged_delivery_rounds()
        .unwrap()
        .into_iter()
        .filter(|(row_run, _)| row_run == run_id)
        .map(|(_, mark)| mark)
        .collect()
}

/// Reopen the run's database the way the app's next start does.
fn reopen(root: &std::path::Path) -> Database {
    Database::open(root.join("facts.db")).unwrap()
}

/// Write a finding straight onto the checklist rows, as a corrupted or
/// hand-edited database would.
fn force_finding(db: &Database, run_id: &str, finding: &serde_json::Value, status: &str) {
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET finding_json=?1, status=?2 WHERE run_id=?3",
            rusqlite::params![finding.canonical_string(), status, run_id],
        )?;
        Ok(())
    })
    .unwrap()
}

/// A v2 envelope that claims `confirmed` while carrying a `failed` fact.
///
/// Built by hand on purpose: this is the "durable bytes disagree with themselves"
/// shape, which no correct writer produces but a truncated, mis-merged or
/// hand-edited row can. Both the write path and the read path must survive it.
fn contradictory_finding(item_key: &str) -> serde_json::Value {
    serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": item_key,
            "class": "confirmed",
            "aggregateClass": "confirmed",
            "itemCount": 1,
            "terminal": true,
            "repairBudgetRemaining": true,
            "facts": [{
                "class": "failed",
                "origin": "candidate",
                "ruleId": "content",
                "reasonCode": "sheet_names_mismatch",
                "path": "out/one.xlsx",
                "sha256": "a".repeat(64),
                "blocking": true,
                "needsRepair": true
            }]
        }
    })
}

// ---------------------------------------------------------------------------
// H1: the completion lane must not complete a RequiresU1 delivery
// ---------------------------------------------------------------------------

/// H1, through the route H used: the unverified delivery keeps its facts and the
/// Run acquires no terminal.
///
/// Three things are asserted at once, because each alone is passable by a wrong
/// fix:
///
/// 1. the Run is **not** completed (and not failed, and not cancelled);
/// 2. the unverified item's facts **are** durable — the deferral is not a licence
///    to drop them;
/// 3. nothing was staged and no mark was committed, so no decision claims to own
///    verdicts that were never written.
///
/// **The retention is not called by the test.** Card §2 forbids "由夹具手工调用
/// retain 来掩盖实现没保存": if this test called `retain_pending_typed_outcome`
/// itself, assertion 2 would only prove that the test can write — the exact defect
/// H proved would survive. The whole tail is therefore driven through
/// `completion_tail_for_test`, so the only writer of the facts is production code
/// (`completion_delivery_for_stop`'s `RequiresU1` arm).
#[test]
fn h1_repair_a_requires_u1_delivery_defers_without_completing_the_run() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let ledger = db.delivery_checklist(run_id).unwrap();
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, requires_u1_verdict())
        .expect_err("a requires-U1 verdict must not produce a stop");
    assert!(stop.is_requires_u1(), "the valid handoff, not an ambiguity");
    let deferred =
        crate::runtime_host::delivery::typed_stop_bypassing_the_refusal_for_test(&ledger, stop.expect_requires_u1().clone());

    // The real shared tail — decision, retention and terminal branch — with no
    // retention performed by the fixture.
    let mark = coordinator
        .completion_tail_for_test(&deferred)
        .expect("a requires-U1 delivery is deferred, not refused, by the shared tail");
    assert!(
        mark.is_none(),
        "a deferred delivery carries no mark, got {mark:?}"
    );

    // 1. No terminal state, and no terminal event of any kind.
    let events = event_types(db, run_id);
    assert!(
        !events.iter().any(|event| event == "run.completed"),
        "a RequiresU1 delivery must never report the Run as completed ({events:?})"
    );
    assert!(
        !events
            .iter()
            .any(|event| event == "run.failed" || event == "run.cancelled"),
        "deferral is not a failure either ({events:?})"
    );
    let state = terminal_state(db, run_id);
    assert_ne!(state, "completed", "the Run must not be completed");
    assert_ne!(state, "failed", "the Run must not be failed");

    // 2. The facts survived on the pending path.
    let rows = rows(db, run_id);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].1, "pending", "the unverified row stays pending");
    let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");
    assert_eq!(
        crate::runtime_host::delivery::decode_stored_verdict(&finding).category(),
        "v2",
        "the retained row is a well-formed v2 record, not a bare legacy row"
    );

    // 3. Nothing staged, nothing committed.
    assert!(
        staged_rounds(db, run_id).is_empty(),
        "a deferred delivery stages no round"
    );
    assert!(
        committed_marks(db, run_id).is_empty(),
        "no decision owned verdicts that were never staged"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The same class through the **real shared completion tail** both production
/// callers use.
///
/// This is the route where the old code returned `Complete { mark: None }` and the
/// caller's `else` then wrote `terminate(Completed)`. Driving
/// `completion_tail_for_test` commits the effect list the production
/// `completion_effects` function actually returns, so the assertion is about the
/// code the two real callers run — not about a branch the test chose. If the
/// deferred case ever produced a terminal effect again, a `run.completed` event
/// would appear here.
///
/// The stop is built through the real encoder rather than the refusing constructor,
/// because the refusal is exactly what keeps this shape out of production: to test
/// the tail's behaviour on a deferred delivery, the test must construct the
/// deferred decision directly. What the assertion then proves is that **the tail
/// itself** does not terminate — so even a future producer that did construct this
/// shape could not complete an unverified Run.
#[test]
fn h1_repair_the_shared_completion_tail_writes_no_terminal_for_a_deferred_delivery() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let ledger = db.delivery_checklist(run_id).unwrap();
    let verdict = requires_u1_verdict();
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, verdict)
        .expect_err("the real constructor refuses a RequiresU1 stop");
    let verdict = stop.expect_requires_u1().clone();
    let deferred = crate::runtime_host::delivery::typed_stop_bypassing_the_refusal_for_test(
        &ledger,
        verdict,
    );

    // The real tail, committed through the real decision transaction.
    let mark = coordinator
        .completion_tail_for_test(&deferred)
        .expect("the shared tail must handle a deferred delivery");
    assert!(
        mark.is_none(),
        "a deferred delivery stages nothing and so carries no mark, got {mark:?}"
    );

    // The durable consequence: no terminal, nothing staged, still non-terminal.
    let events = event_types(db, run_id);
    assert!(
        !events.iter().any(|event| event == "run.completed"),
        "the shared completion tail must not complete a deferred delivery ({events:?})"
    );
    assert!(
        !events
            .iter()
            .any(|event| event == "run.failed" || event == "run.cancelled"),
        "deferral is not a failure either ({events:?})"
    );
    assert!(committed_marks(db, run_id).is_empty());
    assert!(staged_rounds(db, run_id).is_empty());
    let state = terminal_state(db, run_id);
    assert_ne!(state, "completed");
    assert_ne!(state, "failed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// No other tail may finalize the Run after the deferred branch declines to.
///
/// Card §2 requires this explicitly: "核一次返回后是否有其他 tail/finalize/fallback
/// 继续终结". The check is that a deferred round leaves the Run exactly where it
/// was, and that a fresh real snapshot still reports no terminal.
#[test]
fn h1_repair_nothing_downstream_finalizes_a_deferred_round() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let before = terminal_state(db, run_id);
    let before_rows = rows(db, run_id);
    let ledger = db.delivery_checklist(run_id).unwrap();
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, requires_u1_verdict())
        .expect_err("no stop");
    let deferred = crate::runtime_host::delivery::typed_stop_bypassing_the_refusal_for_test(
        &ledger,
        stop.expect_requires_u1().clone(),
    );
    // The real tail again: no fixture-side retention, so the "nothing downstream
    // finalizes" claim is about production code.
    coordinator
        .completion_tail_for_test(&deferred)
        .expect("the shared tail defers a requires-U1 delivery");
    let after = terminal_state(db, run_id);
    // Package 2-D U1 changed this assertion on purpose. It used to read
    // `assert_eq!(before, after, "a deferred round must leave the Run's state
    // exactly where it was")`, which encoded the **pre-2-D** behaviour: the tail
    // returned an empty effect list, so the Run stayed `running`. That was not a
    // neutral outcome — the Host drive loop then found nothing dispatchable and
    // failed the Run with `kernel.no_progress`. The invariant is now the one the
    // card asks for: the deferred round moves the Run to a state it can actually
    // wait in, and that state is not terminal.
    assert_eq!(before, "running", "the fixture starts from a running Run");
    assert_eq!(
        after, "awaiting_delivery_review",
        "a deferred round must park the Run, not leave it running with nothing to do"
    );
    assert!(
        !matches!(after.as_str(), "completed" | "failed" | "cancelled"),
        "the Run is still non-terminal after deferral, got `{after}`"
    );
    // H5's 残留缺口 (P0-7): "完成道拒绝后该行未被改动" is two separable facts, and
    // both are asserted here. The deferral legitimately writes the item's *pending*
    // facts (requirement 2 above), so byte-identity is not the invariant. What must
    // hold is that no terminal decision was written onto the row: the status is
    // still `pending`, the durable class is still the non-terminal `unconfirmed`,
    // and the row was not turned into a pass or a failure.
    let after_rows = rows(db, run_id);
    assert_eq!(
        after_rows.len(),
        before_rows.len(),
        "deferring must not add or remove ledger rows"
    );
    assert_eq!(
        after_rows[0].0, before_rows[0].0,
        "the deferred row keeps its key"
    );
    assert_eq!(
        after_rows[0].1, "pending",
        "the deferred row is still pending, never passed or failed"
    );
    let finding: serde_json::Value = serde_json::from_str(&after_rows[0].2).unwrap();
    assert_eq!(
        finding["deliveryVerdict"]["class"], "unconfirmed",
        "the row still carries the non-terminal class, not a terminal verdict"
    );
    assert_eq!(
        finding["deliveryVerdict"]["terminal"], false,
        "the deferred row is explicitly non-terminal"
    );
    drop(coordinator);

    // Card §5 merges the caller's quantity shortfall check into this atomicity
    // test: "保留数量不足的调用者行为检查与本轮原子性测试合并，不仅测试 usize 值".
    // The caller-visible contract of the deferral is not the retention's `usize`
    // but that the Run is left resumable with its facts intact — a caller that
    // short-changed the deferral would have terminated instead.
    //
    // Re-open the database so the assertion is about durable state rather than the
    // in-process cache, then re-read both facts through the real projection.
    let reopened = reopen(root);
    let reopened_rows = rows(&reopened, run_id);
    assert_eq!(
        reopened_rows.len(),
        1,
        "the deferred row survives a real reopen"
    );
    assert_eq!(
        reopened_rows[0].1, "pending",
        "the reopened row is still pending, so the caller was not told 'done'"
    );
    let state_after_reopen = reopened.kernel_build_full_snapshot(run_id).unwrap().state;
    assert!(
        !matches!(
            state_after_reopen.as_str(),
            "completed" | "failed" | "cancelled"
        ),
        "the Run is still non-terminal after a real reopen, got `{state_after_reopen}`"
    );
    drop(reopened);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// H3: an ambiguous item_key is refused before any write
// ---------------------------------------------------------------------------

/// H3, the acceptance this card chose: explicit refusal, not silent dedupe.
///
/// H's probe asserted "both items' facts must be durably retained" and observed
/// `written=1`. This card took the other route H offered: refuse the input. The
/// acceptance is therefore the **negation of a successful save**, proven three
/// ways, because a refusal that still wrote something would pass a naive
/// `is_err()` check:
///
/// 1. the constructor returns the structured refusal, naming the conflicting key
///    and carrying **both** items with their complete facts;
/// 2. nothing at all was staged, committed or terminated;
/// 3. the durable rows are unchanged.
#[test]
fn h3_repair_a_repeated_item_key_is_refused_before_any_write() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let before = rows(db, run_id);

    // H's exact shape: one key, two items, different bindings and classes.
    let verdict = DeliveryVerdict::from_items(
        vec![
            confirmed_item("slot:xlsx:1", "distinct-A.xlsx"),
            unconfirmed_item("slot:xlsx:1"),
        ],
        true,
    );
    let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect_err("a repeated item_key must not produce a stop");
    let ambiguity = refusal
        .ambiguity()
        .expect("this refusal is an input ambiguity, not a RequiresU1 handoff");

    // 1. The error names the key and keeps both items' facts.
    assert_eq!(ambiguity.ambiguous.len(), 1, "one key is duplicated");
    let collision = &ambiguity.ambiguous[0];
    assert_eq!(collision.item_key, "slot:xlsx:1");
    assert_eq!(
        collision.collisions.len(),
        2,
        "both competing items come back whole, not just the winner"
    );
    let classes: Vec<&str> = collision
        .collisions
        .iter()
        .map(|item| item.class.as_str())
        .collect();
    assert!(
        classes.contains(&"confirmed") && classes.contains(&"unconfirmed"),
        "neither item's class may be dropped: {classes:?}"
    );
    let bindings: Vec<String> = collision
        .collisions
        .iter()
        .flat_map(|item| item.facts.iter())
        .filter_map(|fact| match &fact.origin {
            FactOrigin::Candidate { path, .. } | FactOrigin::Source { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();
    assert!(
        bindings.iter().any(|path| path == "distinct-A.xlsx"),
        "the would-be-overwritten item's binding must survive in the error: {bindings:?}"
    );

    // 2. No write, no terminal.
    assert!(
        staged_rounds(db, run_id).is_empty(),
        "a refused verdict stages nothing at all"
    );
    assert!(committed_marks(db, run_id).is_empty());
    let events = event_types(db, run_id);
    assert!(
        !events
            .iter()
            .any(|event| event == "run.completed" || event == "run.failed"),
        "a refused input must not be terminated as this or that ({events:?})"
    );

    // 3. The ledger is unchanged.
    assert_eq!(
        rows(db, run_id),
        before,
        "the refusal must leave the durable rows unchanged"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The refusal must be the **same** one from the retention entry, which must not
/// save a partial batch and call it done.
#[test]
fn h3_repair_the_retention_entry_refuses_the_same_ambiguous_verdict() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let verdict = DeliveryVerdict::from_items(
        vec![
            confirmed_item("slot:xlsx:1", "distinct-A.xlsx"),
            unconfirmed_item("slot:xlsx:1"),
        ],
        true,
    );
    let retention = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        &verdict,
        crate::database::now_ms(),
    )
    .expect_err("retention must refuse an ambiguous verdict");
    assert!(
        retention.is_refused_input(),
        "the retention failure is a refused input, not a storage error: {retention}"
    );
    assert_eq!(
        retention.ambiguity().unwrap().ambiguous[0].item_key,
        "slot:xlsx:1"
    );

    // No partial save: the untouched row is still pending.
    assert!(
        rows(db, run_id).iter().all(|row| row.1 == "pending"),
        "no row was written by the refused retention"
    );
    assert_eq!(rows(db, run_id).len(), 1);

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The control group: **distinct** keys are not ambiguous and both facts must be
/// retainable.
///
/// Without this, the refusal above could be "reject everything", which would be a
/// different defect.
#[test]
fn h3_control_distinct_keys_are_never_refused() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    assert!(keys.len() >= 1, "the fixture seeds one row: {keys:?}");
    // Add a second real row so two distinct keys exist in the ledger.
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成两份 Excel 成果");
    if keys.len() < 2 {
        db.seed_delivery_checklist(run_id, &seeds, crate::database::now_ms())
            .unwrap();
    }
    let keys: Vec<String> = db
        .delivery_checklist(run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    assert!(keys.len() >= 2, "the fixture needs two rows, got {keys:?}");
    let verdict = DeliveryVerdict::from_items(
        vec![
            confirmed_item(&keys[0], "distinct-A.xlsx"),
            unconfirmed_item(&keys[1]),
        ],
        true,
    );
    // Distinct keys: the encoder must not refuse. The aggregate is `Unconfirmed`
    // (one unknown sibling), so the *disposition* is the valid `RequiresU1`
    // handoff — what must not happen is an ambiguity refusal.
    let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict.clone())
        .expect_err("an unconfirmed aggregate has no stop to route");
    assert!(
        refusal.is_requires_u1(),
        "distinct item_keys are not ambiguous: the refusal must be the valid RequiresU1 \
         handoff, got {refusal}"
    );
    assert!(
        refusal.ambiguity().is_none(),
        "a verdict whose keys are all distinct names no ambiguous key"
    );
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        &verdict,
        crate::database::now_ms(),
    )
    .expect("distinct keys must retain");
    assert_eq!(
        written, 2,
        "both items' facts are retained when the keys really are different"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// An identical duplicate is refused too: the rule is "one key, one decision",
/// not "one key, one *distinct* decision".
///
/// This makes the input contract explicit rather than leaving a reader to guess
/// whether byte-identical repeats are tolerated.
#[test]
fn h3_repair_even_an_identical_duplicate_key_is_refused() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, .. } = &host;
    let verdict = DeliveryVerdict::from_items(
        vec![
            confirmed_item("slot:xlsx:1", "same.xlsx"),
            confirmed_item("slot:xlsx:1", "same.xlsx"),
        ],
        true,
    );
    let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect_err("an identical duplicate key is still two decisions about one row");
    assert_eq!(
        refusal.ambiguity().unwrap().ambiguous[0].collisions.len(),
        2
    );
    assert!(staged_rounds(db, run_id).is_empty());
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// H4: the facts decide the pass on the real write and read paths
// ---------------------------------------------------------------------------

/// H4 **write** side: the real stage → mark → finalize path must not write
/// `passed` for a record its own facts contradict.
///
/// H showed the old finalizer took `passed` straight from the stage row and never
/// looked at the finding.
#[test]
fn h4_repair_the_real_finalize_never_writes_passed_for_contradicted_facts() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    let key = keys[0].clone();
    let mark = "delivery:completion:h4-write";
    let finding = contradictory_finding(&key);
    match crate::runtime_host::delivery::decode_stored_verdict(&finding) {
        crate::runtime_host::delivery::StoredVerdict::V2(stored) => assert_eq!(stored.item_key, key),
        other => panic!("the contradicted fixture must be decodable and bound to this row: {other:?}"),
    }
    // Stage through the real repository call the pipeline uses, with the flag and
    // the finding disagreeing.
    db.stage_delivery_outcome(
        run_id,
        mark,
        &[crate::database::StagedDeliveryItem {
            item_key: key.clone(),
            passed: true,
            finding_json: finding.canonical_string(),
        }],
        None,
        crate::database::now_ms(),
    )
    .unwrap();
    db.record_decision_mark_for_test(run_id, mark, crate::database::now_ms())
        .unwrap();
    db.finalize_staged_delivery_outcome(run_id, mark, crate::database::now_ms())
        .unwrap();
    let rows = rows(db, run_id);
    assert_eq!(
        rows[0].1, "failed",
        "a record whose own facts say failed must not be written as passed"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// H4 **read** side: an already-stored contradictory row must not be *displayed*
/// as passed by the real projection.
///
/// This is the read-defence injection card :37 requires ("读取已存在的矛盾或损坏v2行
/// 时仍必须保守；只堵新写入不够"). The row is written straight into the column,
/// bypassing the write-side repair, so only the projection can correct it — and the
/// real `load_conversation` is what is read back.
#[test]
fn h4_repair_a_stored_contradictory_row_is_not_projected_as_passed() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, keys } = &host;
    let key = keys[0].clone();
    force_finding(db, run_id, &contradictory_finding(&key), "passed");
    // The raw column really does say passed...
    assert_eq!(rows(db, run_id)[0].1, "passed", "the injection landed");

    // ...and the real projection must refuse to show it as one.
    let view = view_of(db, conversation_id, run_id, &key);
    assert_ne!(
        view.status, "passed",
        "a record contradicted by its own facts must not be displayed as passed"
    );
    assert_eq!(
        view.status, "pending",
        "an unconfirmable record is shown as unconfirmed, never as failed"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// H4's required positive case: a legitimate pass must still read as a pass.
///
/// Without it, the test above could be satisfied by rejecting everything.
#[test]
fn h4_repair_a_legitimate_pass_still_reads_as_passed() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, keys } = &host;
    let key = keys[0].clone();
    let verdict = DeliveryVerdict::from_items(vec![confirmed_item(&key, "out/one.xlsx")], true);
    assert_eq!(verdict.class.as_str(), "confirmed");
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        verdict,
    )
    .expect("a confirmed verdict produces a stop");
    // Stage and finalize through the real path, so the row is written by the code
    // under test rather than by the test.
    coordinator
        .completion_decision_for_test(&stop)
        .expect("the completion lane accepts a confirmed verdict");
    assert_eq!(
        rows(db, run_id)[0].1,
        "passed",
        "a real pass is still written as passed"
    );
    let view = view_of(db, conversation_id, run_id, &key);
    assert_eq!(
        view.status, "passed",
        "a legitimate pass must still be displayed as passed"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// A `Confirmed` item beside an `Unconfirmed` sibling keeps its own row's truth.
///
/// The aggregate is `Unconfirmed`, so the delivery cannot complete — but the item
/// that really was verified must not be dragged down with it (card :39: "合法 item
/// Confirmed + aggregate Refused/RepairNeeded 仍可显示该 item 通过").
#[test]
fn h4_repair_a_confirmed_item_survives_an_unconfirmed_aggregate() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, .. } = &host;
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成两份 Excel 成果");
    db.seed_delivery_checklist(run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let keys: Vec<String> = db
        .delivery_checklist(run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    assert!(keys.len() >= 2, "need two rows, got {keys:?}");
    let verdict = DeliveryVerdict::from_items(
        vec![
            confirmed_item(&keys[0], "out/one.xlsx"),
            unconfirmed_item(&keys[1]),
        ],
        true,
    );
    assert_eq!(
        verdict.class.as_str(),
        "unconfirmed",
        "one unknown item holds the aggregate back"
    );
    // Mark the genuinely verified item's own row as passed; leave the unknown one
    // pending. The aggregate is still unconfirmed.
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET status='passed' WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![run_id, keys[0]],
        )?;
        Ok(())
    })
    .unwrap();
    let confirmed = view_of(db, conversation_id, run_id, &keys[0]);
    assert_eq!(
        confirmed.status, "passed",
        "the item that really was verified keeps its pass"
    );
    let unknown = view_of(db, conversation_id, run_id, &keys[1]);
    assert_ne!(unknown.status, "passed", "the unknown sibling is not a pass");
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The read defence must not fire on a **legacy** row: no v2 envelope keeps the
/// status its own path gave it, so introducing v2 cannot mass-downgrade history.
#[test]
fn h4_repair_a_legacy_row_without_an_envelope_keeps_its_status() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, keys } = &host;
    force_finding(
        db,
        run_id,
        &serde_json::json!({"reasonCode":"ok","verificationStatus":"verified"}),
        "passed",
    );
    let view = view_of(db, conversation_id, run_id, &keys[0]);
    assert_eq!(
        view.status, "passed",
        "a legacy record with no v2 envelope is not this check's business"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// An **undecodable** v2 envelope is unconfirmable, not failed.
///
/// Card :37 requires the corrupt cases to report "cannot confirm" rather than
/// inventing a definite failure.
#[test]
fn h4_repair_a_corrupt_v2_envelope_is_unconfirmable_not_failed() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, keys } = &host;
    let corrupt = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 99,
            "verdictEnvelopeVersion": 99,
            "itemKey": "slot:xlsx:1",
            "class": "confirmed",
            "facts": []
        }
    });
    force_finding(db, run_id, &corrupt, "passed");
    let view = view_of(db, conversation_id, run_id, &keys[0]);
    assert_ne!(view.status, "passed", "an undecodable record is not a pass");
    assert_eq!(
        view.status, "pending",
        "an unknown-version record is unconfirmed, not a definite failure"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// A v2 record with **no facts at all** is unconfirmable, not a pass.
///
/// A `Confirmed` claim with an empty fact list is the "nothing observed" shape; the
/// fold turns an empty fact list into `Unconfirmed`, so the row must not pass.
#[test]
fn h4_repair_a_confirmed_claim_with_no_facts_is_not_a_pass() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, conversation_id, keys } = &host;
    let empty = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2,
            "verdictEnvelopeVersion": 2,
            "itemKey": "slot:xlsx:1",
            "class": "confirmed",
            "aggregateClass": "confirmed",
            "itemCount": 1,
            "terminal": true,
            "repairBudgetRemaining": true,
            "facts": []
        }
    });
    force_finding(db, run_id, &empty, "passed");
    let view = view_of(db, conversation_id, run_id, &keys[0]);
    assert_eq!(
        view.status, "pending",
        "a pass claimed with no facts is unconfirmed, not shown as passed"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn b_corrupt_candidate_fact_stays_pending_on_real_finalize() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    let key=keys[0].clone();let mut finding=contradictory_finding(&key);
    finding["deliveryVerdict"]["facts"][0].as_object_mut().unwrap().remove("sha256");
    assert!(matches!(crate::runtime_host::delivery::decode_stored_verdict(&finding),
        crate::runtime_host::delivery::StoredVerdict::Corrupt { .. }));
    let mark="delivery:completion:b-corrupt-fact";let now=crate::database::now_ms();
    db.stage_delivery_outcome(run_id,mark,&[crate::database::StagedDeliveryItem {item_key:key,passed:true,finding_json:finding.canonical_string()}],None,now).unwrap();
    db.record_decision_mark_for_test(run_id,mark,now).unwrap();db.finalize_staged_delivery_outcome(run_id,mark,now).unwrap();
    assert_eq!(rows(db,run_id)[0].1,"pending","corrupt evidence cannot manufacture a definite failure");
    drop(coordinator);let _=std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// Package 2-D U1: the parked state, on both lanes
// ---------------------------------------------------------------------------

fn unconfirmed_verdict(keys: &[String]) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        keys.iter()
            .map(|key| ItemDecision {
                item_key: key.clone(),
                class: crate::runtime_host::delivery::decide_facts(
                    &[DecisionFact::unconfirmed(
                        FactOrigin::CheckUnbound,
                        None,
                        "内容核验没有绑定真实产物",
                        None,
                    )],
                    true,
                )
                .class,
                facts: vec![DecisionFact::unconfirmed(
                    FactOrigin::CheckUnbound,
                    None,
                    "内容核验没有绑定真实产物",
                    None,
                )],
                repair_budget_remaining: true,
            })
            .collect::<Vec<_>>(),
        true,
    )
}

/// A `RequiresU1` stop, built through the real refusal and the real test bypass.
fn requires_u1_stop(db: &Database, run_id: &str, keys: &[String]) -> crate::runtime_host::delivery::DeliveryStop {
    let ledger = db.delivery_checklist(run_id).unwrap();
    let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(
        &ledger,
        unconfirmed_verdict(keys),
    )
    .expect_err("a requires-U1 verdict must not produce a routable stop");
    assert!(refusal.is_requires_u1(), "the valid handoff, not an ambiguity");
    crate::runtime_host::delivery::typed_stop_bypassing_the_refusal_for_test(
        &ledger,
        refusal.expect_requires_u1().clone(),
    )
}

#[test]
fn d1_a_requires_u1_delivery_parks_the_run_instead_of_leaving_it_running() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    let deferred = requires_u1_stop(db, run_id, keys);

    // The real shared tail — retention, completion decision and terminal branch —
    // with no retention performed by the fixture.
    let mark = coordinator
        .completion_tail_for_test(&deferred)
        .expect("a requires-U1 delivery is deferred by the shared tail");
    assert!(mark.is_none(), "a deferred delivery stages nothing");

    // 1. The Run is parked on the new state. Before this package the tail
    //    returned an empty effect list, which left the Run `running` with nothing
    //    to dispatch — and the Host loop then failed it with `kernel.no_progress`.
    assert_eq!(
        terminal_state(db, run_id),
        "awaiting_delivery_review",
        "the Run must have a state to wait in"
    );

    // 2. The park event names why it is waiting and which items it is waiting on,
    //    so the durable progress row is derived from the decision that parked it.
    let events = event_types(db, run_id);
    assert!(
        events.iter().any(|event| event == "run.awaiting_delivery_review"),
        "{events:?}"
    );
    let parked: String = db
        .with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT payload_json FROM kernel_events
                 WHERE run_id=?1 AND event_type='run.awaiting_delivery_review'
                 ORDER BY seq DESC LIMIT 1",
            )?;
            let mut rows = statement.query([run_id.as_str()])?;
            let raw = rows.next()?.map(|row| row.get::<_, String>(0)).transpose()?;
            Ok(raw.ok_or_else(|| rusqlite::Error::InvalidQuery)?)
        })
        .unwrap();
    let parked: serde_json::Value = serde_json::from_str(&parked).unwrap();
    assert!(
        !parked["reason"].as_str().unwrap_or_default().is_empty(),
        "the park names why it is waiting"
    );
    assert_eq!(
        parked["outstandingItems"].as_array().map(Vec::len),
        Some(keys.len()),
        "every unverified item is listed"
    );

    // 3. No terminal of any kind.
    assert!(!events.iter().any(|event| event == "run.completed"));
    assert!(!events.iter().any(|event| event == "run.failed"));
    assert!(!events.iter().any(|event| event == "run.cancelled"));

    // 4. The unverified rows stay pending with their facts, and the Run is
    //    continuable through the durable progress row the UI reads.
    for (_, status, finding) in rows(db, run_id) {
        assert_eq!(status, "pending");
        let finding: serde_json::Value = serde_json::from_str(&finding).unwrap();
        assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");
    }
    let progress = db
        .kernel_continuable_run(&host.conversation_id, run_id)
        .expect("a parked Run must offer a continuation");
    assert_eq!(progress.pause_reason, "delivery_review");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn d2_the_live_lane_maps_a_requires_u1_verdict_to_a_park_not_an_error() {
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    let typed = match requires_u1_stop(db, run_id, keys) {
        crate::runtime_host::delivery::DeliveryStop::Typed(typed) => typed,
        other => panic!("expected a typed stop, got {other:?}"),
    };

    // Before this package the live lane returned `Err` here, which failed the
    // round and therefore the Run.
    let (_, followup) = coordinator
        .typed_live_followup_for_route(
            crate::runtime_host::delivery::DeliveryStop::Typed(typed),
            &[],
            &serde_json::json!({"role":"assistant","content":[]}),
        )
        .expect("a requires-U1 verdict maps to a park, not an error");
    assert!(
        matches!(followup, StopFollowup::AwaitDeliveryReview { .. }),
        "a requires-U1 verdict must map to the park variant"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn d3_a_repairable_verdict_still_arms_its_repair_round_and_is_not_parked() {
    // The negative control for D1: a `RepairNeeded` verdict must route to the
    // repair lane, not the park. Otherwise a repairable delivery would silently
    // stop asking the model to fix it.
    let (host, coordinator) = host();
    let Host { db, root, run_id, keys, .. } = &host;
    let key = keys[0].clone();
    let blocking = DecisionFact::failed(
        FactOrigin::candidate("out/report.xlsx", &"a".repeat(64)),
        "delivery.check.content_mismatch",
        "内容不符",
        true,
        None,
    );
    let repairable = DeliveryVerdict::from_items(
        vec![ItemDecision {
            item_key: key,
            class: crate::runtime_host::delivery::decide_facts(&[blocking.clone()], true).class,
            facts: vec![blocking],
            repair_budget_remaining: true,
        }],
        true,
    );
    assert_eq!(
        repairable.class,
        crate::runtime_host::delivery::DeliveryClass::RepairNeeded
    );
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        repairable,
    )
    .expect("a repairable verdict is routable");

    // The route is the whole invariant: `Repair`, never `RequiresU1`. Driving the
    // repair round itself needs a valid model input, which is the repair lane's
    // own subject and is covered by its own tests.
    assert_eq!(
        stop.route(),
        crate::runtime_host::delivery::DeliveryRoute::Repair,
        "a repairable delivery must still arm its repair round"
    );
    assert!(
        !matches!(
            stop.route(),
            crate::runtime_host::delivery::DeliveryRoute::RequiresU1 { .. }
        ),
        "a repairable delivery must not be parked"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}
