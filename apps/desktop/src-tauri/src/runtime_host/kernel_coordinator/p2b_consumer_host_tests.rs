//! Package 2-B2 consumer tests through the **real Host and coordinator**.
//!
//! The card requires the typed verdict to be consumed through the actual
//! `stage → terminate` ordering, at a real coordinator, against a real database,
//! asserting the actual Kernel result, the events, the decision mark and the
//! post-finalize rows. A test that only converts an enum proves none of that, so
//! nothing here exercises the mapping in isolation: every assertion reads durable
//! state after the coordinator has committed.
//!
//! Both lanes that classify the same verdict are covered, because they are
//! separate code paths:
//! * the completion lane (`stage_completion_delivery` → `CompletionDelivery`);
//! * the length-recovery lane (`length_recovery_delivery` → instruction kind).

use super::*;
use crate::runtime_host::delivery::{DecisionFact, DeliveryVerdict, FactOrigin, ItemDecision};

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

/// A trust refusal: never repairable, and a definite non-pass.
fn refused_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            "slot:xlsx:1",
            vec![DecisionFact::refused(
                candidate("AGV汇总.xlsx"),
                "path_denied",
                "outside the authorized root",
            )],
            true,
        )],
        true,
    )
}

/// An unusable frozen spec — a spec problem, not a content problem.
fn invalid_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            "slot:xlsx:1",
            vec![DecisionFact::invalid_with_code(
                "delivery.unreadable_contract",
                "内容契约版本不支持",
            )],
            true,
        )],
        true,
    )
}

/// A real content mismatch with the repair budget already spent, so the fact that
/// was repairable a round ago is now a definite failure. It must never be
/// reported as `Invalid`.
fn definite_failure_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            "slot:xlsx:1",
            vec![DecisionFact::failed(
                candidate("AGV汇总.xlsx"),
                "sheet_names_mismatch",
                "工作表名称不一致",
                true,
                None,
            )],
            false,
        )],
        false,
    )
}

/// A `Confirmed` item, built through the real folding core with an all-passed
/// fact set.
///
/// This is **constructed data**: it proves the consumer's `Confirmed → Accept`
/// mapping and the completion lane's `passed` write. It is not proof that any
/// frozen contract was fully verified — reaching `Confirmed` through the real
/// classifier requires a caller-declared `Coverage`, which production still
/// leaves `Unstated`. See `p2b_consumer_boundary_tests` for that route.
fn confirmed_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            "slot:xlsx:1",
            vec![DecisionFact::passed(candidate("AGV汇总.xlsx"))],
            true,
        )],
        true,
    )
}

/// Nothing was bound to a real artifact: not a pass, not a failure.
fn requires_u1_verdict() -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            "slot:xlsx:1",
            vec![DecisionFact::unconfirmed(
                FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            )],
            true,
        )],
        true,
    )
}

/// The clock, cancellation and database a real coordinator borrows. All three
/// are leaked for the reason the existing live delivery tests leak the first
/// two: `KernelCoordinator<'a>` holds `&'a` references, so a fixture that
/// returned them by value could not hand out a coordinator at all.
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
    // The fixture bypasses the runtime_host run start, so the authoritative
    // delivery checklist is seeded the same way production start seeds it.
    let seeds = crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1, "{seeds:?}");
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let db: &'static Database = Box::leak(Box::new(db));
    (clock, cancellation, db, root, run_id)
}

/// A real Run with the standard one-item Excel checklist and a prepared
/// coordinator.
struct TypedHost {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
}

fn typed_host() -> (TypedHost, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime();
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    (TypedHost { db, root, run_id }, coordinator)
}

/// The checklist rows as durable `(item_key, status, finding)` triples.
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

/// Every committed Kernel event type for this Run, in sequence order.
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

/// Every committed decision mark, so a test can prove which decision owned the
/// staged verdicts.
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

fn has_terminal(events: &[String]) -> bool {
    events
        .iter()
        .any(|event| event == "run.completed" || event == "run.failed" || event == "run.cancelled")
}

/// Write an arbitrary finding straight onto the one checklist row.
fn force_finding(db: &Database, run_id: &str, finding: &serde_json::Value, status: &str) {
    db.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET finding_json=?1, status=?2 WHERE run_id=?3",
            rusqlite::params![finding.to_string(), status, run_id],
        )?;
        Ok(())
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// 1. Definite non-passes through the real terminate lane
// ---------------------------------------------------------------------------

/// Every definite non-pass must reach the Run's real failed terminal, with the
/// verdict staged under the mark the failing decision committed and the item
/// finalized afterwards.
#[test]
fn p2b_typed_definite_non_passes_terminate_the_real_run_and_finalize_their_rows() {
    for (label, verdict, expected_class) in [
        ("refused", refused_verdict(), "refused"),
        ("invalid", invalid_verdict(), "invalid"),
        (
            "definiteFailure",
            definite_failure_verdict(),
            "definiteFailure",
        ),
    ] {
        let (host, coordinator) = typed_host();
        let TypedHost { db, root, run_id } = &host;
        let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
            &db.delivery_checklist(run_id).unwrap(),
            verdict,
        )
        .expect("a definite non-pass maps to a typed stop");
        let typed = stop.typed().expect("typed stop");
        assert_eq!(typed.verdict.class.as_str(), expected_class);
        assert!(!typed.verdict.requires_u1());

        coordinator
            .terminate_with_typed_verdict(&stop)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        drop(coordinator);

        // The real Kernel result: failed, not completed.
        let snapshot = db.kernel_build_full_snapshot(run_id).unwrap();
        assert_eq!(
            snapshot.state, "failed",
            "{label}: a definite non-pass must terminate as failed"
        );

        // The events: the real failed terminal, and no completion.
        let events = event_types(db, run_id);
        assert!(
            !events.iter().any(|event| event == "run.completed"),
            "{label}: a definite non-pass must not emit run.completed ({events:?})"
        );
        assert!(
            events.iter().any(|event| event == "run.failed"),
            "{label}: the terminal is the real failed event ({events:?})"
        );

        // The mark: the failure stage was owned by a committed decision.
        let marks = committed_marks(db, run_id);
        assert_eq!(marks.len(), 1, "{label}: exactly one decision mark");
        assert!(
            marks[0].starts_with("delivery:failure:"),
            "{label}: the verdict is owned by the failing decision ({marks:?})"
        );

        // Post-finalize rows: the row carries its class and is not passed.
        let rows = rows(db, run_id);
        assert_eq!(rows.len(), 1, "{label}: one checklist row");
        assert_eq!(rows[0].0, "slot:xlsx:1");
        assert_eq!(
            rows[0].1, "failed",
            "{label}: the terminal stage maps passed=0 to failed"
        );
        let finding: serde_json::Value =
            serde_json::from_str(&rows[0].2).expect("valid finding");
        assert_eq!(
            finding["deliveryVerdict"]["class"], expected_class,
            "{label}: the class survives the durable round trip"
        );
        assert_eq!(finding["deliveryVerdict"]["terminal"], true);
        assert_eq!(
            finding["deliveryVerdict"]["aggregateClass"], expected_class,
            "{label}: the aggregate class is retained separately from the item class"
        );
        assert!(
            db.staged_delivery_rounds().unwrap().is_empty(),
            "{label}: a finalized round leaves no stage behind"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The three classes must not collapse into one another: the retained class and
/// the concrete check reason stay specific to the fact that produced them.
#[test]
fn p2b_typed_classes_do_not_collapse_and_the_retained_facts_stay_specific() {
    for (label, verdict, expected_fact_class, expected_reason) in [
        ("refused", refused_verdict(), "refused", "path_denied"),
        (
            "invalid",
            invalid_verdict(),
            "invalid",
            "delivery.unreadable_contract",
        ),
        (
            "definiteFailure",
            definite_failure_verdict(),
            "failed",
            "sheet_names_mismatch",
        ),
    ] {
        let (host, coordinator) = typed_host();
        let TypedHost { db, root, run_id } = &host;
        let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
            &db.delivery_checklist(run_id).unwrap(),
            verdict,
        )
        .expect("typed stop");
        coordinator
            .terminate_with_typed_verdict(&stop)
            .unwrap();
        drop(coordinator);

        let rows = rows(db, run_id);
        let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
        let facts = finding["deliveryVerdict"]["facts"]
            .as_array()
            .expect("facts array");
        assert_eq!(facts.len(), 1, "{label}: the one fact is retained");
        assert_eq!(
            facts[0]["class"], expected_fact_class,
            "{label}: the fact keeps its own class"
        );
        assert_eq!(
            facts[0]["reasonCode"], expected_reason,
            "{label}: the concrete check reason is preserved verbatim"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// A definite non-pass must not go through `fail()`, because that path would run
/// `evaluate_stop` again and could stage a *different* verdict than the one the
/// caller classified. The typed entry point must stage exactly one round, owned
/// by exactly one mark, and leave no abandoned stage.
#[test]
fn p2b_typed_termination_stages_exactly_one_round_owned_by_one_mark() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        refused_verdict(),
    )
    .expect("typed stop");
    coordinator
        .terminate_with_typed_verdict(&stop)
        .unwrap();
    drop(coordinator);

    // One decision, one mark, no leftover stage — the two-phase protocol's
    // postcondition, asserted on the real ledger rather than on the enum.
    assert_eq!(committed_marks(db, run_id).len(), 1);
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    assert_eq!(
        db.delivery_checklist(run_id).unwrap()[0].checked_at.is_some(),
        true,
        "a finalized verdict carries when it was verified"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The completion lane's typed arms, driven through the real staging/finalize
/// edge with an already computed stop.
///
/// A `Confirmed` verdict completes and finalizes its row as `passed`. Nothing
/// else does.
///
/// A definite non-pass takes this lane, and takes it to the **failed** terminal.
///
/// **Documented behaviour change (package 2-E).** This test previously asserted
/// that a `Refused` and a `DefiniteFailure` verdict were **refused** by this lane
/// and had to be terminated through `terminate_with_typed_verdict` from the caller.
/// That was the 2-B2 repair-round fix for a real defect — the completion lane's
/// terminal write is `terminate(RunOutcome::Completed)`, so a typed non-pass that
/// reached it reported a **completed** Run for a delivery the ledger had just
/// marked `failed`.
///
/// Refusing was the right ruling but the wrong layer: it left the production
/// completion consumer with no way to write the non-pass at all, so the only
/// callers that terminated these were tests and the live lane. Package 2-E closes
/// that gap by giving the variant its own arm, so this lane now writes the failed
/// terminal the class chose — still never a completion.
///
/// The row status is now right for the stated reason too: it is `failed` because
/// this lane finalized it as a definite non-pass, not because the finalize maps
/// `passed = 0` to `failed` by accident.
#[test]
fn p2b_completion_lane_stages_and_finalizes_confirmed_and_definite_non_passes() {
    for (label, verdict, expected_status, expected_class) in [
        ("confirmed", confirmed_verdict(), "passed", "confirmed"),
        ("refused", refused_verdict(), "failed", "refused"),
        (
            "definiteFailure",
            definite_failure_verdict(),
            "failed",
            "definiteFailure",
        ),
    ] {
        let (host, coordinator) = typed_host();
        let TypedHost { db, root, run_id } = &host;
        let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
            &db.delivery_checklist(run_id).unwrap(),
            verdict,
        )
        .expect("typed stop");
        if expected_status != "passed" {
            // Driven through the **real shared tail**, not the staging seam alone:
            // what a definite non-pass must do is terminate the Run, and that
            // happens in `completion_effects`. The staging itself is asserted where
            // it is decided, in
            // `g_b_a_terminal_typed_non_pass_is_not_a_completion_decision`.
            let mark = coordinator
                .completion_tail_for_test(&stop)
                .unwrap_or_else(|error| panic!("{label}: {error}"));
            assert!(
                mark.as_deref()
                    .is_some_and(|mark| mark.starts_with("delivery:failure:")),
                "{label}: the failure decision owns the staged verdict ({mark:?})"
            );
            assert_eq!(
                committed_marks(db, run_id).len(),
                1,
                "{label}: the failure stage is owned by one committed decision"
            );
            let snapshot = db.kernel_build_full_snapshot(run_id).unwrap();
            assert_eq!(
                snapshot.state, "failed",
                "{label}: a definite non-pass must terminate the Run `failed`"
            );
            let events = event_types(db, run_id);
            assert!(
                !events.iter().any(|event| event == "run.completed"),
                "{label}: and must never emit run.completed ({events:?})"
            );
            assert!(
                events.iter().any(|event| event == "run.failed"),
                "{label}: the terminal is the real failed event ({events:?})"
            );
            let rows = rows(db, run_id);
            assert_eq!(
                rows[0].1, expected_status,
                "{label}: the failure lane finalizes the row from the verdict"
            );
            let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
            assert_eq!(
                finding["deliveryVerdict"]["class"], expected_class,
                "{label}: the class survives the failure lane"
            );
            assert!(
                db.staged_delivery_rounds().unwrap().is_empty(),
                "{label}: the failure stage was finalized, not abandoned"
            );
            let _ = std::fs::remove_dir_all(root);
            drop(coordinator);
            continue;
        }
        let mark = coordinator
            .completion_decision_for_test(&stop)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert!(
            mark.as_deref()
                .is_some_and(|mark| mark.starts_with("delivery:completion:")),
            "{label}: the completion decision owns the staged verdict ({mark:?})"
        );
        drop(coordinator);

        // The completion decision committed its own mark, and the finalize wrote
        // the row under it.
        assert_eq!(committed_marks(db, run_id).len(), 1, "{label}");
        let rows = rows(db, run_id);
        assert_eq!(
            rows[0].1, expected_status,
            "{label}: the completion lane finalizes the row from the verdict"
        );
        let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
        assert_eq!(
            finding["deliveryVerdict"]["class"], expected_class,
            "{label}: the class survives the completion lane"
        );
        assert!(
            db.staged_delivery_rounds().unwrap().is_empty(),
            "{label}: the completion stage was finalized, not abandoned"
        );
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The completion lane must stage **nothing** for a requires-U1 verdict: no mark,
/// no row write, and therefore no completion. This is the assertion that keeps an
/// unverified delivery from riding a passing round into `run.completed`.
#[test]
fn p2b_completion_lane_stages_nothing_for_a_requires_u1_verdict() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        requires_u1_verdict(),
    )
    .expect_err("no stop");
    let returned = returned.expect_requires_u1();

    // The retention path keeps the facts on the pending row without a terminal.
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(written, 1, "the unconfirmed row is retained");

    // No decision owned this: nothing was staged, no mark was written, and the
    // row is still pending rather than passed or failed.
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    assert!(committed_marks(db, run_id).is_empty());
    let rows = rows(db, run_id);
    assert_eq!(
        rows[0].1, "pending",
        "an unconfirmed result is not a completed pass and not a failed terminal"
    );
    let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");
    assert_eq!(finding["deliveryVerdict"]["terminal"], false);
    let events = event_types(db, run_id);
    assert!(
        !has_terminal(&events),
        "an unconfirmed verdict emits no terminal event ({events:?})"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// A requires-U1 verdict has no stop at all, so the terminal lane refuses it by
/// type rather than by caller convention, and building the verdict leaves the Run
/// without any terminal.
#[test]
fn p2b_a_requires_u1_verdict_never_acquires_a_terminal_or_a_stage() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        requires_u1_verdict(),
    )
    .expect_err("a requires-U1 verdict must not produce a stop");
    assert!(returned.is_requires_u1());
    let returned = returned.expect_requires_u1();
    assert_eq!(returned.class.as_str(), "unconfirmed");

    // Building the verdict staged nothing, committed nothing and terminated
    // nothing.
    assert!(committed_marks(db, run_id).is_empty());
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    let events = event_types(db, run_id);
    assert!(
        !has_terminal(&events),
        "an unconfirmed verdict emits no terminal event ({events:?})"
    );
    assert!(
        !matches!(
            db.kernel_build_full_snapshot(run_id).unwrap().state.as_str(),
            "completed" | "failed"
        ),
        "the Run is left without a terminal"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 2. Non-terminal retention on the real ledger
// ---------------------------------------------------------------------------

/// An unconfirmed delivery must be retained on the existing non-terminal path:
/// pending, with its facts, and with no terminal event.
#[test]
fn p2b_unconfirmed_is_retained_pending_with_its_facts_and_no_terminal() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        requires_u1_verdict(),
    )
    .expect_err("no stop");
    let returned = returned.expect_requires_u1();
    let written = crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(written, 1, "the unconfirmed row is retained");

    let rows = rows(db, run_id);
    assert_eq!(
        rows[0].1, "pending",
        "an unconfirmed result stays pending: not a pass and not a failed terminal"
    );
    let finding: serde_json::Value = serde_json::from_str(&rows[0].2).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");
    assert_eq!(
        finding["deliveryVerdict"]["terminal"], false,
        "the retained envelope says the item is not a final class"
    );
    assert!(
        finding["deliveryVerdict"]["facts"]
            .as_array()
            .is_some_and(|facts| !facts.is_empty()),
        "the facts are retained, not summarized away"
    );
    // No decision owned this: nothing was staged and no mark was written.
    assert!(db.staged_delivery_rounds().unwrap().is_empty());
    assert!(committed_marks(db, run_id).is_empty());
    let events = event_types(db, run_id);
    assert!(
        !has_terminal(&events),
        "retention writes no terminal event ({events:?})"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The retained class must survive reopening the database, so a later U1 reads
/// durable bytes rather than an in-memory handle.
#[test]
fn p2b_unconfirmed_retention_reloads_as_a_non_pass() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        requires_u1_verdict(),
    )
    .expect_err("no stop");
    let returned = returned.expect_requires_u1();
    crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    drop(coordinator);

    let reopened = Database::open(root.join("facts.db")).unwrap();
    let reloaded = reopened.delivery_checklist(run_id).unwrap();
    assert_eq!(reloaded.len(), 1);
    let finding: serde_json::Value =
        serde_json::from_str(reloaded[0].finding.as_deref().expect("finding")).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");
    assert_eq!(
        reloaded[0].status, "pending",
        "an unconfirmed result is never written as passed and never flipped to failed"
    );
    let stored = crate::runtime_host::delivery::decode_stored_verdict(&finding);
    assert!(
        !stored.may_be_passed(),
        "a reloaded unconfirmed record is not a pass"
    );
    assert_eq!(stored.category(), "v2", "it is a well-formed v2 record");
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 3. Length-recovery lane
// ---------------------------------------------------------------------------

/// The recovery instruction must classify every typed class truthfully, and must
/// not report a definite failure as merely unverified.
#[test]
fn p2b_length_recovery_classifies_every_typed_class_without_conflating_them() {
    for (label, verdict, expected) in [
        (
            "refused",
            refused_verdict(),
            LengthRecoveryDeliveryKind::IncompleteDeliverables,
        ),
        (
            "invalid",
            invalid_verdict(),
            LengthRecoveryDeliveryKind::IncompleteDeliverables,
        ),
        (
            "definiteFailure",
            definite_failure_verdict(),
            LengthRecoveryDeliveryKind::IncompleteDeliverables,
        ),
    ] {
        let (host, coordinator) = typed_host();
        let TypedHost { db, root, run_id } = &host;
        let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
            &db.delivery_checklist(run_id).unwrap(),
            verdict,
        )
        .expect("typed stop");
        let facts = coordinator.length_recovery_for_test(&stop).unwrap();
        assert_eq!(facts.kind, expected, "{label}: recovery kind");
        assert_eq!(facts.passed, 0, "{label}: nothing passed");
        assert_eq!(facts.failed, 0, "{label}: nothing was finalized yet");
        assert!(
            !facts.outstanding.is_empty(),
            "{label}: the outstanding item is named from the real checklist"
        );
        let instruction = facts.instruction();
        assert!(
            !instruction.contains("全部通过"),
            "{label}: the instruction must not claim the delivery passed"
        );
        drop(coordinator);
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The `Confirmed` branch of the same lane. It is stated separately because it is
/// the one class whose instruction *does* say the delivery passed — so the
/// negative assertion the loop above makes is deliberately not applied here, and
/// leaving this branch out would let a `Confirmed` verdict be reported as
/// incomplete without any test noticing.
#[test]
fn p2b_length_recovery_reports_a_confirmed_delivery_as_verified() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let stop = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        confirmed_verdict(),
    )
    .expect("typed stop");
    let facts = coordinator.length_recovery_for_test(&stop).unwrap();
    assert_eq!(
        facts.kind,
        LengthRecoveryDeliveryKind::VerifiedDeliverables,
        "a confirmed verdict is the verified-deliverables kind"
    );
    let instruction = facts.instruction();
    assert!(
        instruction.contains("全部通过"),
        "a confirmed delivery is the one case that may say so: {instruction}"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The unconfirmed case on its own, through the same lane: it must be reported as
/// **unverified**, which is neither a pass nor a failed deliverable. This cannot
/// be folded into the loop above, because a requires-U1 verdict has no stop.
#[test]
fn p2b_length_recovery_reports_an_unconfirmed_delivery_as_unverified_not_failed() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    // Retain the unconfirmed verdict, then let the real gate evaluate the Run as
    // it now stands: the row is still pending, so nothing is verified.
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(run_id).unwrap(),
        requires_u1_verdict(),
    )
    .expect_err("no stop");
    let returned = returned.expect_requires_u1();
    assert_eq!(
        returned.class.as_str(),
        "unconfirmed",
        "the recovery lane's class comes from the verdict, not from raw status counts"
    );
    crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        run_id,
        &db.delivery_checklist(run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();

    let stop = crate::runtime_host::delivery::evaluate_stop(
        db,
        root.to_str(),
        run_id,
    )
    .unwrap();
    let facts = coordinator.length_recovery_for_test(&stop).unwrap();
    assert_eq!(
        facts.kind,
        LengthRecoveryDeliveryKind::UnverifiedArtifacts,
        "an unconfirmed delivery is unverified, not a failed deliverable"
    );
    assert_eq!(facts.failed, 0);
    assert_eq!(facts.passed, 0);
    assert!(facts.pending > 0, "the retained row is still pending");
    let instruction = facts.instruction();
    assert!(
        instruction.contains("尚未通过核验"),
        "the instruction must say the content is unverified: {instruction}"
    );
    assert!(
        !instruction.contains("不合格"),
        "it must not describe unverified content as failed: {instruction}"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

// ---------------------------------------------------------------------------
// 4. Version isolation on the real ledger
// ---------------------------------------------------------------------------

/// A historical v1 row must keep its own mapping: with no verdict envelope it is
/// legacy, not a wholesale `Unconfirmed` downgrade, and not a pass.
#[test]
fn p2b_legacy_rows_keep_their_mapping_and_are_never_read_as_v2() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    let legacy = serde_json::json!({
        "reasonCode": "delivery.missing_artifact",
        "verificationStatus": "unverified",
    });
    force_finding(db, run_id, &legacy, "failed");

    let reloaded = db.delivery_checklist(run_id).unwrap();
    let finding: serde_json::Value =
        serde_json::from_str(reloaded[0].finding.as_deref().unwrap()).unwrap();
    let stored = crate::runtime_host::delivery::decode_stored_verdict(&finding);
    assert_eq!(
        stored.category(),
        "legacy",
        "a row without an envelope stays legacy instead of being downgraded"
    );
    assert!(!stored.may_be_passed());
    // Its own status is untouched: the legacy mapping still decides it.
    assert_eq!(reloaded[0].status, "failed");
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// A v2 row whose envelope is missing, unknown, corrupted or truncated must not
/// become a pass, and must not be rewritten into a fabricated refusal either.
#[test]
fn p2b_corrupt_v2_rows_in_the_real_ledger_never_become_passes() {
    for (label, corrupt) in [
        (
            "envelope-missing-version",
            serde_json::json!({"deliveryVerdict": {"itemKey": "slot:xlsx:1", "class": "confirmed"}}),
        ),
        (
            "unknown-version",
            serde_json::json!({"deliveryVerdict": {
                "verdictSchemaVersion": 9, "verdictEnvelopeVersion": 9,
                "itemKey": "slot:xlsx:1", "class": "confirmed",
                "aggregateClass": "confirmed", "itemCount": 1, "terminal": true,
                "facts": [],
            }, "reasonCode": "legacy.survives"}),
        ),
        (
            "unknown-class",
            serde_json::json!({"deliveryVerdict": {
                "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                "itemKey": "slot:xlsx:1", "class": "probablyFine",
                "aggregateClass": "confirmed", "itemCount": 1, "terminal": true,
                "facts": [],
            }}),
        ),
        (
            "truncated-fact",
            serde_json::json!({"deliveryVerdict": {
                "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                "itemKey": "slot:xlsx:1", "class": "confirmed",
                "aggregateClass": "confirmed", "itemCount": 1, "terminal": true,
                "facts": [{"class": "passed", "origin": "candidate"}],
            }}),
        ),
        (
            "not-an-envelope-object",
            serde_json::json!({"deliveryVerdict": "confirmed"}),
        ),
    ] {
        let (host, coordinator) = typed_host();
        let TypedHost { db, root, run_id } = &host;
        force_finding(db, run_id, &corrupt, "pending");

        let reloaded = db.delivery_checklist(run_id).unwrap();
        let raw = reloaded[0].finding.as_deref().unwrap_or("{}");
        let finding: serde_json::Value = serde_json::from_str(raw).unwrap();
        let stored = crate::runtime_host::delivery::decode_stored_verdict(&finding);
        assert!(
            !stored.may_be_passed(),
            "{label}: a corrupt v2 record must never read as passed"
        );
        assert_ne!(
            stored.category(),
            "v2",
            "{label}: and it must not be reported as a valid v2 verdict"
        );
        assert_ne!(
            reloaded[0].status, "passed",
            "{label}: the stored status must not claim a pass either"
        );
        // A trustworthy independent fact beside a broken envelope is still kept.
        if label == "unknown-version" {
            assert_eq!(
                finding["reasonCode"], "legacy.survives",
                "the independent fact is not discarded with the envelope"
            );
        }
        drop(coordinator);
        let _ = std::fs::remove_dir_all(root);
    }
}

/// The `safe_finding` capacity boundary, on the **projected** read path a real
/// caller uses. An over-large envelope must be recorded as omitted rather than
/// silently dropped, and must never read as accepted.
///
/// This reads through `load_conversation`, which is where the Host-owned bounded
/// projection actually runs. Reading `delivery_checklist` would prove nothing
/// about the boundary, because that path returns the stored bytes unprojected.
#[test]
fn p2b_an_oversized_verdict_is_omitted_explicitly_not_silently_accepted() {
    let (host, coordinator) = typed_host();
    let TypedHost { db, root, run_id } = &host;
    // A verdict whose facts exceed the finding capacity, built from the real
    // envelope shape so the real projection runs rather than JSON filler.
    let facts: Vec<serde_json::Value> = (0..200)
        .map(|index| {
            serde_json::json!({
                "class": "unconfirmed",
                "origin": "spec",
                "detail": "x".repeat(512),
                "reasonCode": format!("delivery.fact_{index}"),
            })
        })
        .collect();
    let oversized = serde_json::json!({
        "deliveryVerdict": {
            "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
            "itemKey": "slot:xlsx:1", "class": "unconfirmed",
            "aggregateClass": "unconfirmed", "itemCount": 1, "terminal": false,
            "facts": facts,
        }
    });
    force_finding(db, run_id, &oversized, "pending");

    let conversation_id: String = db
        .with_connection(|connection| {
            Ok(connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1",
                [run_id.as_str()],
                |row| row.get(0),
            )?)
        })
        .unwrap();
    let detail = db.load_conversation(&conversation_id).unwrap();
    let view = detail
        .delivery_checklist
        .iter()
        .find(|view| view.run_id == *run_id)
        .expect("the projected checklist carries this Run");
    let finding = view.finding.clone().unwrap_or(serde_json::json!({}));

    assert!(
        !matches!(view.status.as_str(), "passed"),
        "an oversized record must not be projected as passed"
    );
    let stored = crate::runtime_host::delivery::decode_stored_verdict(&finding);
    assert!(
        !stored.may_be_passed(),
        "and the projected record must not decode as a pass either: {finding}"
    );
    // Whatever the projection kept, the fact that it could not carry the whole
    // envelope must be visible instead of silently absent.
    let omitted = finding["deliveryVerdictOmitted"] == serde_json::json!(true)
        || finding["deliveryVerdict"]["factsTruncated"] == serde_json::json!(true);
    assert!(
        omitted,
        "an over-capacity envelope must be recorded as omitted or truncated, not dropped invisibly: {finding}"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}
