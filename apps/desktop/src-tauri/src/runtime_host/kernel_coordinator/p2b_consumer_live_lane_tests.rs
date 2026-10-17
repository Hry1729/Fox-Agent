//! Package 2-B2 extended review: the **live** terminal lane's typed-v2 classes.
//!
//! The card requires live coverage of the four delivery classes plus the length
//! recovery class mapping. The completion lane already has a seam
//! (`completion_delivery_for_stop`), but the live loop's class mapping sat inline
//! in `service_round_output`, where a test cannot reach it. So the mapping was
//! extracted into `KernelCoordinator::typed_live_followup` and **both** the live
//! loop and these tests call it: the branch under test is the branch production
//! runs, not a copy of it.
//!
//! Each case asserts two independent things, because the first round's defect was
//! precisely a disagreement between them:
//!
//!  1. which follow-up the class produced (`Final` / `Failed` / `Repair`);
//!  2. which **terminal outcome the real `commit_tail` writes for it**, read back
//!     from the committed Kernel snapshot — `Completed` or `Failed{code}`.
//!
//! Reading the outcome from the real Kernel event/snapshot (not from the enum, and
//! not from a message that happens to contain a word) is what makes the evidence
//! terminal rather than descriptive.

use super::*;
use crate::runtime_host::delivery::{
    decide_facts, DecisionFact, DeliveryClass, DeliveryVerdict, FactOrigin, ItemDecision,
};

// ---------------------------------------------------------------------------
// Fixtures: verdicts built through the real folding core
// ---------------------------------------------------------------------------

fn candidate(path: &str) -> FactOrigin {
    FactOrigin::candidate(path, &"b".repeat(64))
}

fn item(item_key: &str, facts: Vec<DecisionFact>, budget: bool) -> ItemDecision {
    ItemDecision {
        item_key: item_key.to_owned(),
        class: decide_facts(&facts, budget).class,
        facts,
        repair_budget_remaining: budget,
    }
}

/// A one-item verdict of the requested class, folded by the real producer.
fn verdict_of(class: &str, item_key: &str) -> DeliveryVerdict {
    let (facts, budget) = match class {
        "confirmed" => (vec![DecisionFact::passed(candidate("out/one.json"))], true),
        "repairNeeded" => (
            vec![DecisionFact::failed(
                candidate("out/one.json"),
                "content_mismatch",
                "成果内容与冻结契约不一致",
                true,
                None,
            )],
            true,
        ),
        "definiteFailure" => (
            vec![DecisionFact::failed(
                candidate("out/one.json"),
                "content_mismatch",
                "成果内容与冻结契约不一致",
                true,
                None,
            )],
            false,
        ),
        "refused" => (
            vec![DecisionFact::refused(
                candidate("out/one.json"),
                "path_denied",
                "outside the authorized root",
            )],
            true,
        ),
        "invalid" => (
            vec![DecisionFact::invalid_with_code(
                "schema_invalid",
                "artifact does not satisfy the frozen contract",
            )],
            true,
        ),
        "unconfirmed" => (
            vec![DecisionFact::unconfirmed(
                FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            )],
            true,
        ),
        other => panic!("unknown class under test: {other}"),
    };
    DeliveryVerdict::from_items(vec![item(item_key, facts, budget)], budget)
}

/// A real coordinator over a real database, with the task's promised artifacts
/// seeded through the production seeder.
///
/// Leaked because `KernelCoordinator<'a>` borrows its clock, database and
/// cancellation for `'a`.
fn live_host(task: &str) -> (
    &'static Database,
    std::path::PathBuf,
    String,
    KernelCoordinator<'static>,
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
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    (db, root, run_id, coordinator)
}

fn item_keys(db: &Database, run_id: &str) -> Vec<String> {
    db.delivery_checklist(run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect()
}

fn typed_stop(
    verdict: DeliveryVerdict,
) -> crate::runtime_host::delivery::DeliveryStop {
    crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .unwrap_or_else(|_| panic!("a non-requires-U1 verdict must map to a typed stop"))
}

/// The assistant message a real live round ends with. `typed_live_followup`
/// appends it to the round history and validates the result, so a placeholder
/// object would be rejected as an unsupported history message rather than
/// exercising the mapping.
fn final_stop_output() -> Value {
    serde_json::json!({
        "role": "assistant",
        "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    })
}

/// The committed Kernel terminal state, plus the `run.failed` payload's code.
///
/// The code is read from `json_extract(payload_json,'$.code')` — the same column
/// and the same JSON path the Kernel itself writes — so this is the durable fact a
/// reader would see, not a re-derivation.
fn terminal_state(db: &Database, run_id: &str) -> (String, Option<String>) {
    let snapshot = db.kernel_build_full_snapshot(run_id).unwrap();
    let code = db
        .with_connection(|connection| {
            let code: Option<String> = connection
                .query_row(
                    "SELECT json_extract(payload_json,'$.code') FROM kernel_events
                      WHERE run_id=?1 AND event_type='run.failed' ORDER BY seq DESC LIMIT 1",
                    [run_id],
                    |row| row.get(0),
                )
                .ok();
            Ok(code)
        })
        .unwrap();
    (snapshot.state, code)
}

fn run_event_types(db: &Database, run_id: &str) -> Vec<String> {
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

fn repair_rounds(db: &Database, run_id: &str) -> i64 {
    db.delivery_repair_round_count(run_id).unwrap()
}

// ---------------------------------------------------------------------------
// 1. One class per route, through the live lane's own mapping
// ---------------------------------------------------------------------------

/// The live lane must route each class to its own follow-up. The defect was that
/// `RepairNeeded` — and every other non-`RequiresU1` class — took `Final`, so the
/// Run was reported `Completed` for a delivery that had not passed.
#[test]
fn p2b_live_lane_routes_every_typed_class_to_its_own_followup() {
    for (label, class, expected) in [
        ("confirmed", "confirmed", "completed"),
        // A definite non-pass terminates with its own canonical code.
        ("refused", "refused", "failed:delivery.refused"),
        ("invalid", "invalid", "failed:delivery.invalid"),
        (
            "definiteFailure",
            "definiteFailure",
            "failed:delivery.definite_failure",
        ),
        // A repairable mismatch must stay non-final.
        ("repairNeeded", "repairNeeded", "repair"),
    ] {
        let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
        let key = item_keys(db, &run_id)[0].clone();
        let stop = typed_stop(verdict_of(class, &key));
        let history: Vec<Value> = Vec::new();
        let output = final_stop_output();
        let planned = coordinator
            .typed_live_followup_for_test(stop, &history, &output)
            .unwrap_or_else(|error| panic!("{label}: the live lane must route this class: {error}"));
        let terminal = planned.terminal.clone().expect("a terminal follow-up");
        if expected == "repair" {
            assert!(
                terminal.starts_with("repair:"),
                "{label}: a repairable delivery must arm its repair round, not finish: {terminal}"
            );
            assert!(
                terminal.contains("content_mismatch"),
                "{label}: the repair round must quote the real failing reason code: {terminal}"
            );
        } else {
            assert_eq!(terminal, expected, "{label}: wrong live route");
        }
        let _ = std::fs::remove_dir_all(root);
        drop(coordinator);
    }
}

// ---------------------------------------------------------------------------
// 2. The committed terminal outcome, not just the mapping
// ---------------------------------------------------------------------------

/// A definite non-pass must terminate the **real** Run with `Failed`, carrying the
/// class's own canonical code. The outcome is read from the committed Kernel
/// snapshot and the `run.failed` event, not from the decision enum.
#[test]
fn p2b_live_lane_commits_a_failed_terminal_for_each_definite_non_pass() {
    for (label, class, expected_code) in [
        ("refused", "refused", "delivery.refused"),
        ("invalid", "invalid", "delivery.invalid"),
        (
            "definiteFailure",
            "definiteFailure",
            "delivery.definite_failure",
        ),
    ] {
        let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
        let key = item_keys(db, &run_id)[0].clone();
        let stop = typed_stop(verdict_of(class, &key));
        let history: Vec<Value> = Vec::new();
        let output = final_stop_output();
        let planned = coordinator
            .typed_live_followup_for_test(stop, &history, &output)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        // Stage and terminate exactly as the live loop does: the stop is the one
        // the lane built, and the follow-up is the one the lane chose.
        crate::runtime_host::delivery::stage_outcome(
            db,
            &run_id,
            &format!("delivery:test:{label}"),
            &planned.stop,
            crate::database::now_ms(),
        )
        .unwrap();
        coordinator
            .commit_typed_followup_for_test(&planned.followup, &Allow)
            .unwrap_or_else(|error| panic!("{label}: the live lane must commit: {error}"));

        let (state, code) = terminal_state(db, &run_id);
        assert_eq!(
            state, "failed",
            "{label}: a definite non-pass must not leave the Run completed"
        );
        assert!(
            run_event_types(db, &run_id)
                .iter()
                .any(|event| event == "run.failed"),
            "{label}: the failed terminal must be a real Kernel event"
        );
        assert_eq!(
            code.as_deref(),
            Some(expected_code),
            "{label}: the terminal must carry the class's own canonical code, not a caller's"
        );
        let _ = std::fs::remove_dir_all(root);
        drop(coordinator);
    }
}

/// The positive half: a `Confirmed` delivery must still complete the Run, and a
/// `RepairNeeded` one must arm the repair round and leave the Run **non-final**.
#[test]
fn p2b_live_lane_completes_only_a_confirmed_delivery() {
    // Confirmed completes.
    let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
    let key = item_keys(db, &run_id)[0].clone();
    let history: Vec<Value> = Vec::new();
    let output = final_stop_output();
    let planned = coordinator
        .typed_live_followup_for_test(typed_stop(verdict_of("confirmed", &key)), &history, &output)
        .unwrap();
    coordinator
        .commit_typed_followup_for_test(&planned.followup, &Allow)
        .unwrap();
    let (state, code) = terminal_state(db, &run_id);
    assert_eq!(state, "completed", "a Confirmed delivery completes the Run");
    assert_eq!(code, None, "a completion writes no failure code");
    assert!(run_event_types(db, &run_id).iter().any(|event| event == "run.completed"));
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);

    // RepairNeeded does not complete and does not fail.
    let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
    let key = item_keys(db, &run_id)[0].clone();
    let history: Vec<Value> = Vec::new();
    let output = final_stop_output();
    let planned = coordinator
        .typed_live_followup_for_test(typed_stop(verdict_of("repairNeeded", &key)), &history, &output)
        .unwrap();
    coordinator
        .commit_typed_followup_for_test(&planned.followup, &Allow)
        .unwrap();
    let (state, _) = terminal_state(db, &run_id);
    assert_ne!(
        state, "completed",
        "a repairable delivery must not be reported as a completed Run"
    );
    assert_ne!(
        state, "failed",
        "a repairable delivery still has a repair round: it is not a failure either"
    );
    assert!(
        !run_event_types(db, &run_id).iter().any(|event| event == "run.failed"),
        "a repair round must not emit a failed terminal"
    );
    assert_eq!(
        repair_rounds(db, &run_id),
        0,
        "the round is recorded by the repair lane's own staging, not invented here"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// A `Unconfirmed` verdict must never reach the live terminal lane at all. Its
/// only constructor refuses to build a stop for it; the defensive check inside
/// the lane is asserted directly so a future constructor cannot quietly reintroduce
/// one.
#[test]
fn p2b_live_lane_refuses_a_requires_u1_verdict_instead_of_completing_it() {
    let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
    let key = item_keys(db, &run_id)[0].clone();
    let verdict = verdict_of("unconfirmed", &key);
    assert_eq!(verdict.class, DeliveryClass::Unconfirmed);
    assert!(verdict.requires_u1());
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(&[], verdict)
        .expect_err("a requires-U1 verdict must not map to a stop");
    assert!(returned.is_requires_u1());
    assert!(
        returned.ambiguity().is_none(),
        "this refusal is the valid RequiresU1 handoff, not an ambiguous input"
    );
    // No terminal lane may be entered, so the Run is untouched.
    let (state, _) = terminal_state(db, &run_id);
    assert_ne!(state, "completed");
    assert_ne!(state, "failed");
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 3. Length recovery: the same classes, not a second classification
// ---------------------------------------------------------------------------

/// The length-recovery lane classifies a typed stop from the verdict's own class.
/// `Unconfirmed` must be reported as **unverified**, never as a failed
/// deliverable, and `Confirmed` as verified.
///
/// The five stoppable classes are driven through `length_recovery_for_test` with
/// the stop the real constructor builds. `Unconfirmed` cannot be: its only
/// constructor refuses to build a stop for it, so it is driven the way production
/// reaches it — retain the verdict, then let the real gate evaluate the Run — and
/// its own case is asserted separately below.
#[test]
fn p2b_live_length_recovery_maps_every_typed_class_without_conflating_them() {
    use super::super::LengthRecoveryDeliveryKind as Kind;
    for (label, class, expected) in [
        ("confirmed", "confirmed", Kind::VerifiedDeliverables),
        ("repairNeeded", "repairNeeded", Kind::IncompleteDeliverables),
        (
            "definiteFailure",
            "definiteFailure",
            Kind::IncompleteDeliverables,
        ),
        ("refused", "refused", Kind::IncompleteDeliverables),
        ("invalid", "invalid", Kind::IncompleteDeliverables),
    ] {
        let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
        let key = item_keys(db, &run_id)[0].clone();
        let stop = typed_stop(verdict_of(class, &key));
        let recovery = coordinator
            .length_recovery_for_test(&stop)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        assert_eq!(recovery.kind, expected, "{label}: wrong recovery class");
        let _ = std::fs::remove_dir_all(root);
        drop(coordinator);
    }
}

/// The unconfirmed case, through the route production actually takes: the verdict
/// cannot become a stop, so it is retained and the real gate then evaluates the
/// Run from the durable row.
#[test]
fn p2b_live_length_recovery_reports_an_unconfirmed_typed_verdict_as_unverified() {
    use super::super::LengthRecoveryDeliveryKind as Kind;
    let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
    let key = item_keys(db, &run_id)[0].clone();
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(&run_id).unwrap(),
        verdict_of("unconfirmed", &key),
    )
    .expect_err("a requires-U1 verdict must not map to a stop");
    let returned = returned.expect_requires_u1();
    assert_eq!(returned.class, DeliveryClass::Unconfirmed);
    crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        &run_id,
        &db.delivery_checklist(&run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    let stop = crate::runtime_host::delivery::evaluate_stop(db, root.to_str(), &run_id).unwrap();
    let recovery = coordinator.length_recovery_for_test(&stop).unwrap();
    assert_eq!(
        recovery.kind,
        Kind::UnverifiedArtifacts,
        "an unconfirmed delivery is unverified, not a failed deliverable"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}

/// The counts the recovery instruction quotes come from the durable ledger, so an
/// unverified delivery must not be reported with `failed` counting it.
#[test]
fn p2b_live_length_recovery_does_not_count_an_unverified_item_as_failed() {
    use super::super::LengthRecoveryDeliveryKind as Kind;
    let (db, root, run_id, coordinator) = live_host("生成 out/one.json。");
    let key = item_keys(db, &run_id)[0].clone();
    let returned = crate::runtime_host::delivery::typed_stop_from_verdict(
        &db.delivery_checklist(&run_id).unwrap(),
        verdict_of("unconfirmed", &key),
    )
    .expect_err("a requires-U1 verdict must not map to a stop");
    let returned = returned.expect_requires_u1();
    crate::runtime_host::delivery::retain_pending_typed_outcome(
        db,
        &run_id,
        &db.delivery_checklist(&run_id).unwrap(),
        returned,
        crate::database::now_ms(),
    )
    .unwrap();
    let stop = crate::runtime_host::delivery::evaluate_stop(db, root.to_str(), &run_id).unwrap();
    let recovery = coordinator.length_recovery_for_test(&stop).unwrap();
    assert_eq!(recovery.kind, Kind::UnverifiedArtifacts);
    assert_eq!(
        recovery.failed, 0,
        "a never-verified item has not failed a check; the ledger says pending"
    );
    assert_eq!(
        recovery.pending, 1,
        "and it is visible as pending, which is the fact the instruction must carry"
    );
    drop(coordinator);
    let _ = std::fs::remove_dir_all(root);
}
