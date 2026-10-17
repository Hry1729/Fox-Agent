//! Package 2-B2 extended review (G): counter-examples at the **real consumer
//! boundary**.
//!
//! Each test states one invariant from `G-P2B2-extended-review-repair.md` and
//! drives the production branch that owns it. The module deliberately does not
//! re-derive a classification: it builds a verdict through the real folding core
//! (`decide_facts` → `DeliveryVerdict::from_items`), hands it to the real
//! `typed_stop_from_verdict`, and then reads back durable state — the decision
//! enum the completion lane actually branches on, the committed Kernel snapshot,
//! the committed events, the decision marks and the post-finalize ledger rows.
//!
//! The three questions the card asks here, in its own terms:
//!  1. does a `RepairNeeded` verdict reach the **repair** contract, or is it
//!     finished as a completion;
//!  2. may a terminal non-pass (`Refused`/`Invalid`/`DefiniteFailure`) be staged
//!     as a completion, and may `Confirmed`/`RepairNeeded` be terminated as
//!     `Failed`;
//!  3. when an aggregate refusal legitimately fails the Run, are the **other**
//!     items' still-unconfirmed original facts retained, or lost.

use super::*;
use crate::runtime_host::delivery::{
    decide_facts, DecisionFact, DeliveryVerdict, FactOrigin, ItemDecision,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn candidate(path: &str) -> FactOrigin {
    FactOrigin::candidate(path, &"a".repeat(64))
}

fn item(item_key: &str, facts: Vec<DecisionFact>, budget: bool) -> ItemDecision {
    ItemDecision {
        item_key: item_key.to_owned(),
        class: decide_facts(&facts, budget).class,
        facts,
        repair_budget_remaining: budget,
    }
}

/// An ordinary content mismatch with the repair budget still available: the
/// folding core must call it repairable, not definite.
fn repairable_verdict(item_key: &str) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            item_key,
            vec![DecisionFact::failed(
                candidate("out/one.json"),
                "content_mismatch",
                "成果内容与冻结契约不一致",
                true,
                None,
            )],
            true,
        )],
        true,
    )
}

fn refused_verdict(item_key: &str) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            item_key,
            vec![DecisionFact::refused(
                candidate("out/one.json"),
                "path_denied",
                "outside the authorized root",
            )],
            true,
        )],
        true,
    )
}

fn confirmed_verdict(item_key: &str) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            item_key,
            vec![DecisionFact::passed(candidate("out/one.json"))],
            true,
        )],
        true,
    )
}

fn definite_failure_verdict(item_key: &str) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![item(
            item_key,
            vec![DecisionFact::failed(
                candidate("out/one.json"),
                "content_mismatch",
                "成果内容与冻结契约不一致",
                true,
                None,
            )],
            false,
        )],
        false,
    )
}

/// A `Refused` item beside an `Unconfirmed` one: the aggregate is a determined
/// refusal, and the second item has no verdict of its own.
fn refused_beside_unconfirmed(keys: [&str; 2]) -> DeliveryVerdict {
    DeliveryVerdict::from_items(
        vec![
            item(
                keys[0],
                vec![DecisionFact::refused(
                    candidate("out/one.json"),
                    "path_denied",
                    "outside the authorized root",
                )],
                true,
            ),
            item(
                keys[1],
                vec![DecisionFact::unconfirmed(
                    FactOrigin::CheckUnbound,
                    None,
                    "内容核验没有绑定真实产物",
                    None,
                )],
                true,
            ),
        ],
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

struct GHost {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
    keys: Vec<String>,
}

fn g_host(task: &str) -> (GHost, KernelCoordinator<'static>) {
    let (clock, cancellation, db, root, run_id) = leaked_runtime(task);
    let coordinator = KernelCoordinator::start_prepared(db, clock, &run_id, cancellation).unwrap();
    let keys = db
        .delivery_checklist(&run_id)
        .unwrap()
        .into_iter()
        .map(|item| item.item_key)
        .collect();
    (
        GHost { db, root, run_id, keys },
        coordinator,
    )
}

/// A one-item JSON checklist, so the item key comes from the real seeder.
fn single_host() -> (GHost, KernelCoordinator<'static>) {
    g_host("生成 out/one.json。")
}

/// The two-item checklist the mixed-fact case needs.
fn pair_host() -> (GHost, KernelCoordinator<'static>) {
    g_host("生成 out/one.json 与 out/two.json。")
}

fn rows_by_key(db: &Database, run_id: &str) -> std::collections::BTreeMap<String, (String, String)> {
    db.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT item_key, status, COALESCE(finding_json,'') FROM delivery_checklist_items
             WHERE run_id=?1",
        )?;
        let rows = statement
            .query_map([run_id], |row| {
                Ok((row.get::<_, String>(0)?, (row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
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

fn repair_rounds(db: &Database, run_id: &str) -> i64 {
    db.delivery_repair_round_count(run_id).unwrap()
}

fn staged_rounds(db: &Database) -> usize {
    db.staged_delivery_rounds().unwrap().len()
}

fn stopped(verdict: DeliveryVerdict, keys: &[String]) -> crate::runtime_host::delivery::DeliveryStop {
    let ledger = Vec::new();
    let _ = keys;
    crate::runtime_host::delivery::typed_stop_from_verdict(&ledger, verdict)
        .unwrap_or_else(|_| panic!("a non-requires-U1 verdict must map to a typed stop"))
}

// ---------------------------------------------------------------------------
// 1. A typed RepairNeeded must land on the repair contract
// ---------------------------------------------------------------------------

/// `RepairNeeded` is not a final class: it has a bounded repair round in front of
/// it. The completion decision must therefore arm that round — the same
/// `CompletionDelivery::Repair` the legacy repair lane builds — instead of
/// returning a `Complete` that the terminal tail finishes as `Completed`.
///
/// On the original candidate this fails: the typed arm is folded into the
/// `Passed | Exhausted | Typed(_)` arm, so a repairable mismatch comes back as
/// `Complete { mark: Some }`.
#[test]
fn g_a_typed_repair_needed_must_arm_the_repair_round_not_complete() {
    let (host, coordinator) = single_host();
    let GHost { db, run_id, .. } = &host;
    let key = host.keys[0].clone();
    let verdict = repairable_verdict(&key);
    assert_eq!(
        verdict.class.as_str(),
        "repairNeeded",
        "the folding core must call a repairable mismatch repairable"
    );
    let stop = stopped(verdict, &host.keys);
    assert!(
        !stop.typed().unwrap().verdict.items[0].is_final_class(),
        "a repair round is not a final outcome"
    );

    let history: Vec<Value> = Vec::new();
    let output = serde_json::json!({
        "role": "assistant", "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    });
    let delivery = coordinator
        .completion_delivery_for_stop(&history, &output, &stop)
        .expect("a typed repair stop must still produce a completion decision");
    match &delivery {
        super::super::CompletionDelivery::Repair { mark, prompt, .. } => {
            assert!(
                mark.starts_with("delivery:completion:"),
                "the repair round rides the completing decision's own mark: {mark}"
            );
            assert!(
                prompt.contains("content_mismatch"),
                "the repair prompt must quote the real failing reason code, not a placeholder: {prompt}"
            );
        }
        super::super::CompletionDelivery::Completed { mark } => panic!(
            "a RepairNeeded verdict must not be decided as a completion \
             (mark={mark:?}); the Run would terminate `Completed` with the item repairable"
        ),
        // Added with the `Deferred` variant: a repairable delivery must never take
        // the non-terminal deferral either, so this arm keeps the test exhaustive
        // rather than letting the new variant escape the assertion.
        super::super::CompletionDelivery::Deferred { .. } => panic!(
            "a RepairNeeded verdict must arm its repair round, not be deferred"
        ),
        // Added with the `Failed` variant, for the same reason: a repairable class
        // has a repair round in front of it, so it must not take the failed
        // terminal either.
        super::super::CompletionDelivery::Failed { mark, .. } => panic!(
            "a RepairNeeded verdict must not be written as a failed Run \
             (mark={mark})"
        ),
    }
    let mark = delivery
        .mark()
        .expect("the repair round is staged under its mark")
        .to_owned();
    // Nothing has committed this mark, so the stage is speculative: drop it.
    crate::runtime_host::delivery::withdraw_outcome(db, run_id, &mark).unwrap();
    assert_eq!(staged_rounds(db), 0);
    assert!(committed_marks(db, run_id).is_empty());
    let _ = std::fs::remove_dir_all(&host.root);
    drop(coordinator);
}

/// The ledger consequence of the same decision, through the real finalize gate:
/// a repair round must appear in `delivery_repair_rounds`, which is what the
/// repair budget and the recovery lane read.
#[test]
fn g_a_typed_repair_needed_records_the_repair_round_in_its_own_ledger() {
    let (host, coordinator) = single_host();
    let GHost { db, run_id, .. } = &host;
    let key = host.keys[0].clone();
    let stop = stopped(repairable_verdict(&key), &host.keys);
    let history: Vec<Value> = Vec::new();
    let output = serde_json::json!({
        "role": "assistant", "stopReason": "stop",
        "content": [{"type": "text", "text": "已完成。"}]
    });
    let delivery = coordinator
        .completion_delivery_for_stop(&history, &output, &stop)
        .expect("completion decision");
    let mark = delivery.mark().expect("staged").to_owned();
    // The completing decision's mark, then the same finalize the lane runs.
    db.record_decision_mark_for_test(run_id, &mark, crate::database::now_ms())
        .unwrap();
    coordinator
        .finish_completion_delivery(Some(&delivery))
        .expect("finalize");
    assert_eq!(
        repair_rounds(db, run_id),
        1,
        "a RepairNeeded round must be recorded as a repair round, not finalized as an ordinary result"
    );
    let status = rows_by_key(db, run_id).get(&key).unwrap().0.clone();
    assert_ne!(status, "passed", "a repairable mismatch never passes");
    let _ = std::fs::remove_dir_all(&host.root);
    drop(coordinator);
}

// ---------------------------------------------------------------------------
// 2. Terminal classes and the failing-terminate admission
// ---------------------------------------------------------------------------

/// A terminal non-pass must never be decided as a completion: the completion
/// tail finishes a `Completed` as `RunOutcome::Completed`. It must take the
/// variant whose own arm writes the class's failed terminal instead.
///
/// **Rewritten in package 2-E.** This test used to assert that the decision
/// **refused** such a stop (`Err`), which was the 2-B2 repair-round fix for the
/// contradiction above. Refusing left the production completion consumer unable
/// to write the non-pass at all — the live lane terminated these, the completion
/// lane did not — so 2-E gave the variant its own arm. What must not change is
/// the reason the test exists: the decision must not be a completion.
#[test]
fn g_b_a_terminal_typed_non_pass_is_not_a_completion_decision() {
    let (host, coordinator) = single_host();
    let GHost { db, root, run_id, .. } = &host;
    for (label, verdict, class) in [
        ("refused", refused_verdict(&host.keys[0]), "refused"),
        (
            "definiteFailure",
            definite_failure_verdict(&host.keys[0]),
            "definiteFailure",
        ),
    ] {
        let stop = stopped(verdict, &host.keys);
        let history: Vec<Value> = Vec::new();
        let output = serde_json::json!({
            "role": "assistant", "stopReason": "stop",
            "content": [{"type": "text", "text": "已完成。"}]
        });
        let outcome = coordinator
            .completion_delivery_for_stop(&history, &output, &stop)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
        let mark = match &outcome {
            super::super::CompletionDelivery::Failed { stop, mark } => {
                assert_eq!(
                    stop.typed().expect("typed stop").verdict.class.as_str(),
                    class,
                    "{label}: the failed terminal keeps the verdict's own class"
                );
                mark
            }
            other => {
                // Named explicitly rather than `{:?}`: a `Debug` dump of the
                // verdict would print every fact, and the failure being reported is
                // which *variant* was taken, not which facts it carried.
                let kind = match other {
                    super::super::CompletionDelivery::Completed { .. } => "a completion",
                    super::super::CompletionDelivery::Repair { .. } => "a repair round",
                    super::super::CompletionDelivery::Deferred { .. } => "a deferral",
                    super::super::CompletionDelivery::Failed { .. } => unreachable!(),
                };
                panic!(
                    "{label}: a {class} verdict must not be decided as {kind} — the Run \
                     would not be failed while its delivery class says it was"
                )
            }
        };
        // The failure is staged under the mark the round's own decision will
        // carry, so a crash between stage and commit leaves the row pending
        // rather than unknown.
        assert!(
            mark.starts_with("delivery:failure:"),
            "{label}: the failure decision's own mark owns the stage ({mark})"
        );
        assert_eq!(
            staged_rounds(db),
            1,
            "{label}: a definite non-pass stages its failed terminal"
        );
        assert!(
            committed_marks(db, run_id).is_empty(),
            "{label}: nothing is committed until the round's decision lands"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}

/// The failing-terminal entry point must admit **only** the classes that own a
/// failed ruling. Excluding `RequiresU1` alone is not admission: a caller passing
/// a `Confirmed` or `RepairNeeded` stop would otherwise have the Run failed while
/// its own verdict says the opposite.
///
/// The admission takes **no** caller-supplied code or message, so "the class is
/// the only input" is enforced by the signature rather than by a convention: the
/// refusal below cannot be worked around by passing a better-looking reason.
#[test]
fn g_b_terminate_with_typed_verdict_refuses_classes_that_are_not_a_failure() {
    for (label, verdict, class) in [
        ("confirmed", confirmed_verdict("slot:json:1"), "confirmed"),
        (
            "repairNeeded",
            repairable_verdict("slot:json:1"),
            "repairNeeded",
        ),
    ] {
        let (host, coordinator) = single_host();
        let GHost { db, root, run_id, .. } = &host;
        let stop = stopped(verdict, &host.keys);
        let outcome = coordinator.terminate_with_typed_verdict(&stop);
        let error = outcome.expect_err(&format!(
            "{label}: a {class} verdict must not be admitted here — the Run would be failed \
             while its own class says otherwise"
        ));
        assert!(
            error.contains(class),
            "{label}: the refusal must name the class that refused it: {error}"
        );
        let snapshot = db.kernel_build_full_snapshot(run_id).unwrap();
        assert_ne!(
            snapshot.state, "failed",
            "{label}: the Run must not have been failed"
        );
        assert!(
            !event_types(db, run_id).iter().any(|event| event == "run.failed"),
            "{label}: no failed terminal may be emitted"
        );
        assert!(
            committed_marks(db, run_id).is_empty(),
            "{label}: and no failure mark may be committed"
        );
        let _ = std::fs::remove_dir_all(root);
        drop(coordinator);
    }
}

/// The positive half of the same admission, and the frozen code/message
/// contract: each terminal class must still terminate the real Run, and it must
/// be the class's own canonical reason code that reaches the terminal event —
/// not whatever a caller happened to pass.
#[test]
fn g_b_every_terminal_class_terminates_with_its_own_canonical_reason_code() {
    for (label, verdict, class, code) in [
        ("refused", refused_verdict("slot:json:1"), "refused", "delivery.refused"),
        (
            "definiteFailure",
            definite_failure_verdict("slot:json:1"),
            "definiteFailure",
            "delivery.definite_failure",
        ),
    ] {
        let (host, coordinator) = single_host();
        let GHost { db, root, run_id, .. } = &host;
        let stop = stopped(verdict, &host.keys);
        let typed = stop.typed().unwrap();
        let disposition = typed.verdict.disposition();
        let crate::runtime_host::delivery::DeliveryDisposition::Terminal {
            class: disposition_class,
            code: disposition_code,
            ..
        } = disposition
        else {
            panic!("{label}: a terminal class must carry a terminal disposition");
        };
        assert_eq!(
            disposition_class.as_str(),
            class,
            "{label}: the disposition class must name the verdict's own class"
        );
        assert_eq!(
            disposition_code, code,
            "{label}: the disposition must carry the class's canonical reason code"
        );
        coordinator
            .terminate_with_typed_verdict(&stop)
            .unwrap_or_else(|error| panic!("{label}: a terminal class must be admitted: {error}"));
        let events = event_types(db, run_id);
        assert!(
            events.iter().any(|event| event == "run.failed"),
            "{label}: the Run must record its failure: {events:?}"
        );
        assert!(
            !events.iter().any(|event| event == "run.completed"),
            "{label}: and must never record a completion"
        );
        assert_eq!(
            db.kernel_build_full_snapshot(run_id).unwrap().state,
            "failed",
            "{label}: the committed state is the failed terminal"
        );
        let recorded: String = db
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT json_extract(payload_json,'$.code') FROM kernel_events
                     WHERE run_id=?1 AND event_type='run.failed'",
                    [run_id.as_str()],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(
            recorded, code,
            "{label}: the class's canonical reason code must be the one the terminal event carries"
        );
        let _ = std::fs::remove_dir_all(root);
        drop(coordinator);
    }
}

// ---------------------------------------------------------------------------
// 3. Mixed facts: a determined refusal must not erase the others' facts
// ---------------------------------------------------------------------------

/// When the aggregate refusal legitimately fails the Run, the other items stay
/// unconfirmed — but their **original facts must survive**. Staging only the
/// terminal rows and never retaining the pending ones loses the refused item's
/// sibling evidence: the row keeps whatever it had, which at the failing
/// terminal is nothing at all.
#[test]
fn g_c_a_refused_aggregate_retains_the_unconfirmed_items_facts() {
    let (host, coordinator) = pair_host();
    let GHost { db, root, run_id, .. } = &host;
    assert_eq!(host.keys.len(), 2, "{:?}", host.keys);
    let verdict = refused_beside_unconfirmed([&host.keys[0], &host.keys[1]]);
    assert_eq!(verdict.class.as_str(), "refused");
    let stop = stopped(verdict, &host.keys);
    let typed = stop.typed().unwrap();
    assert_eq!(
        typed.terminal_items.len(),
        1,
        "only the refused item may enter the terminal stage"
    );
    assert_eq!(
        typed.pending_items.len(),
        1,
        "the unconfirmed item must be held on the non-terminal path"
    );

    coordinator
        .terminate_with_typed_verdict(&stop)
        .expect("a refused aggregate terminates the Run");
    let rows = rows_by_key(db, run_id);
    let (refused_status, refused_finding) = rows.get(&host.keys[0]).unwrap();
    assert_eq!(refused_status, "failed", "the refused item is the failed one");
    let refused: Value = serde_json::from_str(refused_finding).unwrap();
    assert_eq!(refused["deliveryVerdict"]["class"], "refused");
    let (unconfirmed_status, unconfirmed_finding) = rows.get(&host.keys[1]).unwrap();
    assert_eq!(
        unconfirmed_status, "pending",
        "an unconfirmed item must not be written as failed"
    );
    let retained: Value = serde_json::from_str(unconfirmed_finding).unwrap_or(Value::Null);
    assert_eq!(
        retained["deliveryVerdict"]["class"], "unconfirmed",
        "the unconfirmed item's own facts must be durably retained, not dropped: {retained}"
    );
    let stored = crate::runtime_host::delivery::decode_stored_verdict(&retained);
    assert!(
        !stored.may_be_passed(),
        "retaining the facts must never make the row a pass"
    );
    assert_eq!(
        stored.category(),
        "v2",
        "and it must stay readable as a v2 record"
    );
    let _ = std::fs::remove_dir_all(root);
    drop(coordinator);
}
