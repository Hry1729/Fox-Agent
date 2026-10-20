//! Package 2-D repair, D-1/D-2: the user-visible loop, on the real lanes.
//!
//! H's review found that a `RequiresU1` verdict parked the Run but nothing ever
//! **ended** it: accepting every item left `kernel_runs.state` at
//! `awaiting_delivery_review` for ever, so the product combination the user chose
//! ("all items accepted → Completed") was unreachable. These tests close that.
//!
//! Every test drives the **real** shared tail to reach the parked state
//! (`completion_tail_for_test`, the same seam the H1/H3/H4 suites use) and the
//! **real** durable decision commit to leave it (`kernel_commit_decision`, via
//! `commit_delivery_acceptance_terminal`). Nothing here writes `kernel_runs.state`
//! with SQL.
//!
//! The concurrency tests are bounded and deterministic: one database, one writer,
//! and a state change injected between the admission read and the commit. That is
//! the whole of the claimed scope — single-writer atomicity on one database — and
//! the tests say so rather than implying more.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::*;
use crate::kernel::RunController;
use crate::runtime_host::delivery::delivery_u1::{
    commit_delivery_acceptance_terminal, effective_delivery_acceptance,
    evaluate_delivery_acceptance_admission, resolve_delivery_acceptance,
    AcceptanceIneffective, AcceptanceTerminal, AdmissionRefusal, DeliveryAcceptanceDecision,
};
use crate::runtime_host::kernel_coordinator::tests::p2b_consumer_h_repair_tests::Host;
use crate::database::{CURRENT_U1_AWARE_SCHEMA_VERSION, DowngradeBlocker};
use sha2::{Digest, Sha256};

const ACTOR: &str = "window:main";

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// The unverified verdict the fixture row carries: nothing was bound to a real
/// artifact, which is the one class U1 accepts.
fn unconfirmed_verdict(item_key: &str) -> crate::runtime_host::delivery::DeliveryVerdict {
    crate::runtime_host::delivery::DeliveryVerdict::from_items(
        vec![crate::runtime_host::delivery::ItemDecision {
            item_key: item_key.to_owned(),
            class: crate::runtime_host::delivery::decide_facts(
                &[crate::runtime_host::delivery::DecisionFact::unconfirmed(
                    crate::runtime_host::delivery::FactOrigin::CheckUnbound,
                    None,
                    "内容核验没有绑定真实产物",
                    None,
                )],
                true,
            )
            .class,
            facts: vec![crate::runtime_host::delivery::DecisionFact::unconfirmed(
                crate::runtime_host::delivery::FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            )],
            repair_budget_remaining: true,
        }],
        true,
    )
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

fn run_state(db: &Database, run_id: &str) -> String {
    db.kernel_build_full_snapshot(run_id).unwrap().state
}

fn row_of(db: &Database, run_id: &str, item_key: &str) -> (String, String, String) {
    db.with_connection(|connection| {
        Ok(connection.query_row(
            "SELECT item_key, status, COALESCE(finding_json,'')
             FROM delivery_checklist_items WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![run_id, item_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )?)
    })
    .unwrap()
}

/// A fixture with one unverified row, a real candidate file and a real
/// managed-file version for it.
///
/// The version is written with the **absolute** `storage_path` the real write gate
/// records — the shape that makes the D-4 version criterion reachable at all (the
/// checklist row's `target_path` is project-relative).
struct ReviewFixture {
    db: &'static Database,
    root: std::path::PathBuf,
    run_id: String,
    conversation_id: String,
    item_key: String,
    version_id: String,
}

impl ReviewFixture {
    fn new(label: &str) -> (Self, KernelCoordinator<'static>) {
        let (host, coordinator) = super::super::tests::p2b_consumer_h_repair_tests::host();
        let Host {
            db,
            root,
            run_id,
            conversation_id,
            keys,
        } = host;
        let item_key = keys[0].clone();

        // A real candidate file, bound to the row, with a real managed version.
        std::fs::write(root.join("proof.csv"), b"segment,value\na,1\n").unwrap();
        let absolute = root.join("proof.csv");
        let version_id = format!("mv-{label}");
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET target_path='proof.csv' WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![run_id, item_key],
            )?;
            connection.execute(
                "INSERT INTO managed_file_versions
                 (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
                  version_no, change_kind, before_hash, before_size, after_hash, after_size,
                  backup_path, restored_from_id, created_at)
                 VALUES (?1, ?2, ?3, 'call-1', 'write_file', ?4, 'proof.csv', 1, 'created',
                         NULL, NULL, ?5, ?6, NULL, NULL, 1)",
                rusqlite::params![
                    version_id,
                    conversation_id,
                    run_id,
                    absolute.to_string_lossy(),
                    digest(b"segment,value\na,1\n"),
                    b"segment,value\na,1\n".len() as i64
                ],
            )?;
            // The durable v2 envelope naming this row, with an unverified class.
            connection.execute(
                "UPDATE delivery_checklist_items SET status='pending', finding_json=?3 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![
                    run_id,
                    item_key,
                    serde_json::json!({
                        "checkedHash": digest(b"segment,value\na,1\n"),
                        "boundPath": "proof.csv",
                        "deliveryVerdict": {
                            "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                            "itemKey": item_key, "class": "unconfirmed",
                            "aggregateClass": "unconfirmed", "itemCount": 1,
                            "terminal": false,
                            "facts": [{"class": "unconfirmed", "origin": "spec",
                                       "detail": "内容核验没有绑定真实产物"}]
                        }
                    })
                    .canonical_string()
                ],
            )?;
            Ok(())
        })
        .unwrap();

        let fixture = ReviewFixture {
            db,
            root: root.clone(),
            run_id: run_id.clone(),
            conversation_id: conversation_id.clone(),
            item_key,
            version_id,
        };
        (fixture, coordinator)
    }

    /// Park the Run through the real shared completion tail.
    fn park(&self, coordinator: &KernelCoordinator<'static>) {
        let ledger = self.db.delivery_checklist(&self.run_id).unwrap();
        let refusal = crate::runtime_host::delivery::typed_stop_from_verdict(
            &ledger,
            unconfirmed_verdict(&self.item_key),
        )
        .expect_err("a requires-U1 verdict must not produce a routable stop");
        let stop = crate::runtime_host::delivery::typed_stop_bypassing_the_refusal_for_test(
            &ledger,
            refusal.expect_requires_u1().clone(),
        );
        coordinator
            .completion_tail_for_test(&stop)
            .expect("the shared tail parks the Run");
        assert_eq!(run_state(self.db, &self.run_id), "awaiting_delivery_review");
    }

    fn decide(&self, decision: DeliveryAcceptanceDecision) -> crate::database::AcceptanceWrite {
        resolve_delivery_acceptance(
            self.db,
            &self.run_id,
            &self.item_key,
            decision,
            ACTOR,
            crate::database::now_ms(),
        )
        .expect("the decision is legal")
    }

    /// The same decision, but reporting the refusal instead of panicking.
    ///
    /// Used where the refusal IS the point: an acceptance whose terminal could not
    /// be committed is a real outcome the caller has to see, not a test failure.
    fn try_decide(
        &self,
        decision: DeliveryAcceptanceDecision,
    ) -> Result<crate::database::AcceptanceWrite, crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError>
    {
        resolve_delivery_acceptance(
            self.db,
            &self.run_id,
            &self.item_key,
            decision,
            ACTOR,
            crate::database::now_ms(),
        )
    }

    fn accept(&self) -> crate::database::AcceptanceWrite {
        self.decide(DeliveryAcceptanceDecision::Accept)
    }

    /// Record the acceptance **without** attempting the terminal.
    ///
    /// Exists for the interleaving tests, which need the acceptance committed
    /// and the Run still parked so the terminal can be attempted separately. It
    /// uses the real DB write path, not a fixture-side SQL update.
    fn record_acceptance_only(&self) -> crate::database::AcceptanceWrite {
        let absolute = self.root.join("proof.csv");
        let row = self
            .db
            .read_delivery_acceptance_row(&self.run_id, &self.item_key)
            .unwrap()
            .expect("the row exists");
        let binding = self
            .db
            .delivery_acceptance_binding(&self.run_id, &self.item_key, &row, Some(&absolute))
            .expect("the binding resolves");
        let now = crate::database::now_ms();
        self.db
            .write_delivery_acceptance(&self.run_id, &self.item_key, &binding, ACTOR, now)
            .expect("the acceptance is legal")
    }

    fn revoke(&self) -> crate::database::AcceptanceWrite {
        self.decide(DeliveryAcceptanceDecision::Revoke)
    }

    fn reject(&self) -> crate::database::AcceptanceWrite {
        self.decide(DeliveryAcceptanceDecision::Reject)
    }

    /// Whether the Run has reached a terminal state, read through the same
    /// production helper the guard uses.
    fn run_is_terminal(&self) -> bool {
        crate::runtime_host::delivery::delivery_u1::run_is_terminal(self.db, &self.run_id)
    }

    /// The acceptance that currently stands for this item, if any.
    fn accepted_acceptance(
        &self,
    ) -> Option<crate::runtime_host::delivery::delivery_u1::EffectiveAcceptance> {
        effective_delivery_acceptance(self.db, &self.run_id, &self.item_key).unwrap()
    }

    /// The state the item-detail surface would render, which is what the UI's
    /// button logic reads.
    fn state(&self) -> crate::runtime_host::delivery::delivery_u1::AcceptanceState {
        crate::runtime_host::delivery::delivery_u1::delivery_acceptance_state(
            self.db,
            &self.run_id,
            &self.item_key,
        )
        .unwrap()
    }

    fn payload(&self, event_type: &str) -> serde_json::Value {
        let mut raw: Option<String> = None;
        self.db
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT payload_json FROM kernel_events
                     WHERE run_id=?1 AND event_type=?2 ORDER BY seq DESC LIMIT 1",
                )?;
                let mut rows = statement.query(rusqlite::params![self.run_id, event_type])?;
                if let Some(row) = rows.next()? {
                    raw = Some(row.get::<_, String>(0)?);
                }
                Ok(())
            })
            .unwrap();
        serde_json::from_str(&raw.expect("the event exists")).unwrap()
    }

    /// A cancel command built through the real controller, for the one test that
    /// needs a Run ended by something other than an acceptance.
    fn cancel_command(&self) -> crate::kernel::KernelPersistCommand {
        let rehydrated = self.db.kernel_rehydrate(&self.run_id).unwrap().unwrap();
        let mut controller = RunController::rehydrate(rehydrated).unwrap();
        let effects = controller.request_cancel();
        controller.persist_command(&effects)
    }
}

// ---------------------------------------------------------------------------
// D-1: accepting ends the Run; rejecting fails it
// ---------------------------------------------------------------------------

#[test]
fn d1_accepting_the_only_item_completes_the_run_through_the_real_commit() {
    let (fixture, coordinator) = ReviewFixture::new("d1-complete");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);

    // The Run really ended, through `kernel_commit_decision` — not a SQL update.
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");

    // Exactly one terminal event, and it carries the user-acceptance marker.
    let events = event_types(fixture.db, &fixture.run_id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "run.completed")
            .count(),
        1
    );
    assert!(!events.iter().any(|event| event == "run.failed"));

    let accepted = fixture.payload("run.delivery_accepted");
    assert_eq!(accepted["userAccepted"], serde_json::json!(true));
    assert_eq!(accepted["acceptanceClass"], serde_json::json!("unconfirmed"));
    let items = accepted["acceptances"].as_array().expect("acceptances");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["itemKey"], serde_json::json!(fixture.item_key));
    assert_eq!(items[0]["actor"], serde_json::json!(ACTOR));
    assert_eq!(items[0]["acceptanceClass"], serde_json::json!("unconfirmed"));

    // The verification result is untouched: still `pending`, still `unconfirmed`.
    let (_, status, finding) = row_of(fixture.db, &fixture.run_id, &fixture.item_key);
    assert_eq!(status, "pending");
    let finding: serde_json::Value = serde_json::from_str(&finding).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d1_rejecting_the_item_fails_the_run_with_the_canonical_reason() {
    let (fixture, coordinator) = ReviewFixture::new("d1-reject");
    fixture.park(&coordinator);
    assert_eq!(fixture.reject(), crate::database::AcceptanceWrite::Written);

    assert_eq!(run_state(fixture.db, &fixture.run_id), "failed");
    let failed = fixture.payload("run.failed");
    assert_eq!(failed["code"], serde_json::json!("delivery.rejected_by_user"));
    assert!(failed["message"].as_str().unwrap().contains(&fixture.item_key));

    // A rejection is an execution-layer conclusion: no `DecisionFact` was written
    // and the durable class is unchanged.
    let (_, status, finding) = row_of(fixture.db, &fixture.run_id, &fixture.item_key);
    assert_eq!(status, "pending");
    let finding: serde_json::Value = serde_json::from_str(&finding).unwrap();
    assert_eq!(finding["deliveryVerdict"]["class"], "unconfirmed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d1_an_unaccepted_item_keeps_the_run_parked() {
    let (fixture, coordinator) = ReviewFixture::new("d1-outstanding");
    fixture.park(&coordinator);

    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("an outstanding item keeps the Run parked");
    assert_eq!(admission, AdmissionRefusal::OutstandingItems);
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review"
    );
    assert!(!event_types(fixture.db, &fixture.run_id)
        .iter()
        .any(|event| event == "run.completed"));

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d1_a_definite_failure_sibling_keeps_the_run_parked_even_when_the_other_item_is_accepted() {
    let (fixture, coordinator) = ReviewFixture::new("d1-sibling");
    let sibling = "file:sibling.csv".to_owned();
    fixture
        .db
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO delivery_checklist_items
                 (run_id,item_key,target_path,artifact_id,display_name,checks_json,
                  requirements_json,status,finding_json,updated_at)
                 VALUES (?1,?2,'sibling.csv',NULL,'兄弟项','[]','[]','pending',?3,1)",
                rusqlite::params![
                    fixture.run_id,
                    sibling,
                    serde_json::json!({
                        "checkedHash": digest(b"x"), "boundPath": "sibling.csv",
                        "deliveryVerdict": {
                            "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                            "itemKey": sibling, "class": "definiteFailure",
                            "aggregateClass": "definiteFailure", "itemCount": 1,
                            "terminal": false,
                            "facts": [{"class": "failed", "origin": "spec", "detail": "确定失败"}]
                        }
                    })
                    .canonical_string()
                ],
            )?;
            Ok(())
        })
        .unwrap();
    fixture.park(&coordinator);

    // The acceptable item is accepted, and the caller is told the Run did NOT
    // end: a silent success here would be the exact hole the repair closes.
    let refused = fixture.try_decide(DeliveryAcceptanceDecision::Accept).expect_err("a blocked sibling keeps the Run parked");
    let message = refused.to_string();
    assert!(
        message.contains("blocked_sibling"),
        "the refusal token reaches the caller: {message}"
    );
    // The acceptance itself was recorded and stands — only the terminal was refused.
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_some());
    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("a blocked sibling keeps the Run parked");
    assert_eq!(admission, AdmissionRefusal::BlockedSibling);
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d1_a_repairable_sibling_keeps_the_run_parked() {
    let (fixture, coordinator) = ReviewFixture::new("d1-repairable");
    let sibling = "file:repairable.csv".to_owned();
    fixture
        .db
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO delivery_checklist_items
                 (run_id,item_key,target_path,artifact_id,display_name,checks_json,
                  requirements_json,status,finding_json,updated_at)
                 VALUES (?1,?2,'repairable.csv',NULL,'可修复项','[]','[]','pending',?3,1)",
                rusqlite::params![
                    fixture.run_id,
                    sibling,
                    serde_json::json!({
                        "checkedHash": digest(b"x"), "boundPath": "repairable.csv",
                        "deliveryVerdict": {
                            "verdictSchemaVersion": 2, "verdictEnvelopeVersion": 2,
                            "itemKey": sibling, "class": "repairNeeded",
                            "aggregateClass": "repairNeeded", "itemCount": 1,
                            "terminal": false,
                            "facts": [{"class": "failed", "origin": "spec",
                                       "detail": "可修复", "blocking": true}]
                        }
                    })
                    .canonical_string()
                ],
            )?;
            Ok(())
        })
        .unwrap();
    fixture.park(&coordinator);

    let refused = fixture.try_decide(DeliveryAcceptanceDecision::Accept).expect_err("a repairable sibling keeps the Run parked");
    let message = refused.to_string();
    assert!(
        message.contains("repairable_sibling"),
        "the refusal token reaches the caller: {message}"
    );
    // The acceptance stands; only the terminal was refused.
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_some());
    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("a repairable sibling keeps the Run parked");
    assert_eq!(admission, AdmissionRefusal::RepairableSibling);
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d1_a_run_that_is_not_parked_is_refused_rather_than_completed() {
    let (fixture, coordinator) = ReviewFixture::new("d1-not-parked");
    // Never parked.
    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("a running Run has no acceptance to aggregate");
    assert_eq!(admission, AdmissionRefusal::NotParked);

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// D-2: bounded, deterministic interleavings
// ---------------------------------------------------------------------------

#[test]
fn d2_a_candidate_that_drifts_after_the_acceptance_withdraws_the_admission() {
    // The check-then-commit window: the acceptance is committed, the candidate
    // changes, and only then does anything try to end the Run. The admission must
    // refuse, so a stale acceptance cannot complete a Run whose evidence moved.
    //
    // The acceptance is written through the real DB path and the terminal is
    // committed separately, which is exactly the interleaving: a real `accept`
    // request does both in one call and so cannot expose the window.
    let (fixture, coordinator) = ReviewFixture::new("d2-drift");
    fixture.park(&coordinator);
    fixture.record_acceptance_only();

    std::fs::write(fixture.root.join("proof.csv"), b"segment,value\na,2\n").unwrap();

    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("a drifted candidate withdraws the admission");
    assert_eq!(admission, AdmissionRefusal::OutstandingItems);

    // And the terminal commit refuses too, rather than ending a Run whose
    // evidence has moved underneath it.
    let outcome = commit_delivery_acceptance_terminal(
        fixture.db,
        &fixture.run_id,
        crate::database::now_ms(),
    )
    .expect("the commit itself must not error");
    assert!(
        matches!(outcome, AcceptanceTerminal::Refused { .. }),
        "got {outcome:?}"
    );
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_only_one_legal_terminal_is_ever_written() {
    let (fixture, coordinator) = ReviewFixture::new("d2-single");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");
    let events_before = event_types(fixture.db, &fixture.run_id);

    // A second attempt to end an already-terminal Run writes nothing.
    let outcome = commit_delivery_acceptance_terminal(
        fixture.db,
        &fixture.run_id,
        crate::database::now_ms(),
    )
    .expect("the commit itself must not error");
    assert!(
        matches!(
            outcome,
            AcceptanceTerminal::Refused {
                refusal: AdmissionRefusal::NotParked,
                ..
            }
        ),
        "got {outcome:?}"
    );
    assert_eq!(event_types(fixture.db, &fixture.run_id), events_before);
    assert_eq!(
        event_types(fixture.db, &fixture.run_id)
            .iter()
            .filter(|event| event.as_str() == "run.completed")
            .count(),
        1,
        "exactly one terminal event"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_a_revocation_committed_first_is_not_overwritten_by_a_stale_completion() {
    let (fixture, coordinator) = ReviewFixture::new("d2-revoke-first");
    fixture.park(&coordinator);
    fixture.record_acceptance_only();
    assert_eq!(fixture.revoke(), crate::database::AcceptanceWrite::Written);

    // The revocation is committed, so the admission refuses and no completion can
    // be written over it.
    let admission = evaluate_delivery_acceptance_admission(fixture.db, &fixture.run_id)
        .expect_err("a revoked acceptance cannot end the Run");
    assert_eq!(admission, AdmissionRefusal::OutstandingItems);
    let outcome = commit_delivery_acceptance_terminal(
        fixture.db,
        &fixture.run_id,
        crate::database::now_ms(),
    )
    .expect("the commit itself must not error");
    assert!(
        matches!(outcome, AcceptanceTerminal::Refused { .. }),
        "got {outcome:?}"
    );
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review"
    );
    assert!(!event_types(fixture.db, &fixture.run_id)
        .iter()
        .any(|event| event == "run.completed"));

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_a_completion_committed_first_leaves_a_later_revocation_to_the_audit_only() {
    let (fixture, coordinator) = ReviewFixture::new("d2-complete-first");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");
    let events_before = event_types(fixture.db, &fixture.run_id);

    // Revoking after the completion changes the current acceptance and appends an
    // audit row. The terminal event is history and is not rewritten, and the Run
    // is not restarted.
    assert_eq!(fixture.revoke(), crate::database::AcceptanceWrite::Written);
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "completed",
        "no terminal rewrite"
    );
    assert_eq!(
        event_types(fixture.db, &fixture.run_id),
        events_before,
        "no new terminal event"
    );
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_none());

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_a_repeated_accept_after_completion_is_refused_not_silently_recorded() {
    // Before R-1 this was a no-op; the guard makes it a refusal, because a
    // repeated accept on a finished Run would otherwise record a NEW acceptance
    // nobody asked for and then drop the benign `NotParked` terminal refusal.
    let (fixture, coordinator) = ReviewFixture::new("d2-repeat");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");
    let events_before = event_types(fixture.db, &fixture.run_id);

    let refused = fixture
        .try_decide(DeliveryAcceptanceDecision::Accept)
        .expect_err("a completed Run takes no new acceptance");
    assert!(
        matches!(
            refused,
            crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError::RunTerminal { .. }
        ),
        "got {refused:?}"
    );
    assert_eq!(event_types(fixture.db, &fixture.run_id), events_before);
    // The acceptance that ended the Run is still the one that stands.
    assert!(fixture.accepted_acceptance().is_some());

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_a_refused_terminal_tells_the_caller_apart_from_the_benign_race() {
    // Two different "the terminal did not commit" situations, which must stay
    // distinguishable:
    //
    // * the Run has **ended** — a new accept/reject is refused outright (R-1),
    //   because recording one would silently accumulate acceptances on a finished
    //   Run that nobody asked for;
    // * the Run is in flight but no longer parked (`cancelling`) — the acceptance
    //   is still legal, and the benign `NotParked` refusal is not reported as a
    //   problem, because the Run is still moving.
    let (fixture, coordinator) = ReviewFixture::new("d2-refusal-kinds");
    fixture.park(&coordinator);

    // In flight but not parked: `request_cancel` lands on `cancelling`, which is
    // not terminal. The acceptance stays legal.
    let _ = fixture
        .db
        .kernel_commit_decision(
            &fixture.run_id,
            crate::database::now_ms(),
            &fixture.cancel_command(),
        )
        .expect("the cancellation commits");
    assert_ne!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review",
        "the Run is no longer parked, so no terminal can be committed for it"
    );
    assert!(
        !fixture.run_is_terminal(),
        "but it has not ended, so a new acceptance is still legal"
    );
    assert_eq!(
        fixture.accept(),
        crate::database::AcceptanceWrite::Written,
        "the benign refusal is not reported to the caller as a problem"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// R-1: after the Run ends, only revoke remains
// ---------------------------------------------------------------------------

#[test]
fn r1_a_completed_run_refuses_a_new_accept_or_reject_but_still_allows_revoke() {
    let (fixture, coordinator) = ReviewFixture::new("r1-completed");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");

    // The Run has ended: a NEW accept or reject is refused, not silently recorded.
    let refused = fixture
        .try_decide(DeliveryAcceptanceDecision::Accept)
        .expect_err("a completed Run takes no new acceptance");
    assert!(
        matches!(
            refused,
            crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError::RunTerminal { .. }
        ),
        "got {refused:?}"
    );
    let refused = fixture
        .try_decide(DeliveryAcceptanceDecision::Reject)
        .expect_err("a completed Run takes no new rejection");
    assert!(
        matches!(
            refused,
            crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError::RunTerminal { .. }
        ),
        "got {refused:?}"
    );

    // The acceptance the user made while it was parked is still there and still
    // withdrawable — that is the whole point of R-1.
    assert!(fixture.accepted_acceptance().is_some());
    assert_eq!(
        fixture.revoke(),
        crate::database::AcceptanceWrite::Written,
        "revoke stays legal after the Run ends"
    );
    assert!(fixture.accepted_acceptance().is_none());

    // And the historical terminal is untouched.
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");
    let events = event_types(fixture.db, &fixture.run_id);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.as_str() == "run.completed")
            .count(),
        1
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn r1_a_revoked_acceptance_on_a_completed_run_can_be_re_accepted_only_before_it_ends() {
    // The mirror image: while the Run is parked, revoke then re-accept is legal
    // (a new decision, the audit keeps the first). Once it has ended, the
    // re-accept is refused, so a revocation on a finished Run is final.
    let (fixture, coordinator) = ReviewFixture::new("r1-revoke-final");
    fixture.park(&coordinator);

    // While the Run is parked, the revoke/re-accept cycle is live. The acceptance
    // is recorded without attempting the terminal — which is exactly the window
    // this test is about, and what a real `accept` request cannot expose because
    // it does both in one call.
    fixture.record_acceptance_only();
    assert_eq!(
        run_state(fixture.db, &fixture.run_id),
        "awaiting_delivery_review",
        "the Run stays parked, so the revoke/re-accept cycle is live"
    );
    assert_eq!(fixture.revoke(), crate::database::AcceptanceWrite::Written);
    fixture.record_acceptance_only();
    assert!(
        fixture.accepted_acceptance().is_some(),
        "a parked Run may be re-accepted after a revocation"
    );

    // Now it ends. The revoke-then-reaccept cycle is closed for good.
    let _ = commit_delivery_acceptance_terminal(
        fixture.db,
        &fixture.run_id,
        crate::database::now_ms(),
    )
    .expect("the terminal commits");
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");
    assert_eq!(fixture.revoke(), crate::database::AcceptanceWrite::Written);
    let refused = fixture
        .try_decide(DeliveryAcceptanceDecision::Accept)
        .expect_err("a completed Run takes no new acceptance");
    assert!(
        matches!(
            refused,
            crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError::RunTerminal { .. }
        ),
        "got {refused:?}"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn r1_a_rejected_run_refuses_a_new_accept_and_has_nothing_to_withdraw() {
    // A failed Run has no acceptance to withdraw, and takes no new decision. The
    // refusal must be about the Run, not about the row — the row itself is still
    // a perfectly acceptable `unconfirmed` one.
    let (fixture, coordinator) = ReviewFixture::new("r1-failed");
    fixture.park(&coordinator);
    assert_eq!(fixture.reject(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "failed");

    let refused = fixture
        .try_decide(DeliveryAcceptanceDecision::Accept)
        .expect_err("a failed Run takes no new acceptance");
    assert!(
        matches!(
            refused,
            crate::runtime_host::delivery::delivery_u1::AcceptanceResolveError::RunTerminal { .. }
        ),
        "got {refused:?}"
    );
    // Nothing to withdraw: the row was rejected, never accepted. `write_delivery_revocation`
    // still stamps `revoked_at` (a pre-existing wart, out of this card's scope), but
    // the effective acceptance is and stays absent either way.
    fixture.revoke();
    assert!(
        fixture.accepted_acceptance().is_none(),
        "a rejected row never gains an acceptance"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn r1_the_state_surface_reports_the_terminal_flag_the_ui_switches_on() {
    // The frontend needs to know the Run ended, or it would keep offering
    // accept/reject on a finished Run. This is the field it reads.
    let (fixture, coordinator) = ReviewFixture::new("r1-state-flag");
    fixture.park(&coordinator);
    let state = fixture.state();
    assert!(!state.run_terminal, "a parked Run is not terminal");

    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    let state = fixture.state();
    assert!(state.run_terminal, "the completed Run reports terminal");
    assert!(state.accepted.is_some(), "and the acceptance is still visible");
    assert_eq!(state.verification_class, "unconfirmed", "still not passed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d2_a_rejection_committed_first_is_not_overwritten_by_a_stale_acceptance() {    let (fixture, coordinator) = ReviewFixture::new("d2-reject-first");
    fixture.park(&coordinator);
    assert_eq!(fixture.reject(), crate::database::AcceptanceWrite::Written);
    assert_eq!(run_state(fixture.db, &fixture.run_id), "failed");

    // The Run is terminal, so nothing may end it again.
    let outcome = commit_delivery_acceptance_terminal(
        fixture.db,
        &fixture.run_id,
        crate::database::now_ms(),
    )
    .expect("the commit itself must not error");
    assert!(
        matches!(
            outcome,
            AcceptanceTerminal::Refused {
                refusal: AdmissionRefusal::NotParked,
                ..
            }
        ),
        "got {outcome:?}"
    );
    assert_eq!(run_state(fixture.db, &fixture.run_id), "failed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// D-4: the version identity hits the real ledger, on both path shapes
// ---------------------------------------------------------------------------

#[test]
fn d4_a_relative_target_path_binds_the_real_managed_version() {
    // The D-4 defect: the checklist row's `target_path` is project-relative while
    // `managed_file_versions.storage_path` is absolute, so the binding read missed
    // and the whole version criterion was skipped.
    let (fixture, coordinator) = ReviewFixture::new("d4-relative");
    fixture.park(&coordinator);

    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    let row = fixture
        .db
        .read_delivery_acceptance_row(&fixture.run_id, &fixture.item_key)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        row.accepted_version_id.as_deref(),
        Some(fixture.version_id.as_str()),
        "the acceptance binds the version the real write gate recorded"
    );
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_some());
    // And the Run really completed, which the broken version criterion made
    // impossible.
    assert_eq!(run_state(fixture.db, &fixture.run_id), "completed");

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d4_an_absolute_target_path_binds_the_same_object() {
    let (fixture, coordinator) = ReviewFixture::new("d4-absolute");
    fixture
        .db
        .with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET target_path=?3 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![
                    fixture.run_id,
                    fixture.item_key,
                    fixture.root.join("proof.csv").to_string_lossy()
                ],
            )?;
            Ok(())
        })
        .unwrap();
    fixture.park(&coordinator);

    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    let row = fixture
        .db
        .read_delivery_acceptance_row(&fixture.run_id, &fixture.item_key)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        row.accepted_version_id.as_deref(),
        Some(fixture.version_id.as_str()),
        "the absolute form resolves to the same ledger row"
    );
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_some());

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d4_a_new_version_of_the_same_bytes_withdraws_the_acceptance() {
    let (fixture, coordinator) = ReviewFixture::new("d4-version-bump");
    fixture.park(&coordinator);
    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    assert!(effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap()
        .is_some());

    // A second version of the SAME bytes: same content, different version identity.
    let second = format!("{}-v2", fixture.version_id);
    fixture
        .db
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO managed_file_versions
                 (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
                  version_no, change_kind, before_hash, before_size, after_hash, after_size,
                  backup_path, restored_from_id, created_at)
                 VALUES (?1, ?2, ?3, 'call-2', 'write_file', ?4, 'proof.csv', 2, 'modified',
                         NULL, NULL, ?5, ?6, NULL, NULL, 2)",
                rusqlite::params![
                    second,
                    fixture.conversation_id,
                    fixture.run_id,
                    fixture.root.join("proof.csv").to_string_lossy(),
                    digest(b"segment,value\na,1\n"),
                    b"segment,value\na,1\n".len() as i64
                ],
            )?;
            Ok(())
        })
        .unwrap();

    let reason = effective_delivery_acceptance(fixture.db, &fixture.run_id, &fixture.item_key)
        .unwrap_err();
    assert_eq!(reason, AcceptanceIneffective::VersionDrift);

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

#[test]
fn d4_another_projects_same_named_path_does_not_bind() {
    // A same-named path in another project must not resolve to this project's
    // version: the identity is the authorized root plus the relative path, not the
    // bare name.
    let (fixture, coordinator) = ReviewFixture::new("d4-other-project");
    let other_root = fixture.root.join("elsewhere");
    std::fs::create_dir_all(&other_root).unwrap();
    fixture
        .db
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO managed_file_versions
                 (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
                  version_no, change_kind, before_hash, before_size, after_hash, after_size,
                  backup_path, restored_from_id, created_at)
                 VALUES ('mv-other', ?1, ?2, 'call-x', 'write_file', ?3, 'proof.csv', 1, 'created',
                         NULL, NULL, ?4, ?5, NULL, NULL, 1)",
                rusqlite::params![
                    fixture.conversation_id,
                    fixture.run_id,
                    other_root.join("proof.csv").to_string_lossy(),
                    digest(b"someone elses bytes"),
                    b"someone elses bytes".len() as i64
                ],
            )?;
            Ok(())
        })
        .unwrap();
    fixture.park(&coordinator);

    assert_eq!(fixture.accept(), crate::database::AcceptanceWrite::Written);
    let row = fixture
        .db
        .read_delivery_acceptance_row(&fixture.run_id, &fixture.item_key)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        row.accepted_version_id.as_deref(),
        Some(fixture.version_id.as_str()),
        "the in-project version binds, not the same-named path elsewhere"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}

// ---------------------------------------------------------------------------
// D-5: the executable downgrade pre-check, on a real parked Run
// ---------------------------------------------------------------------------

#[test]
fn d5_the_downgrade_precheck_refuses_a_real_parked_run() {
    let (fixture, coordinator) = ReviewFixture::new("d5-parked");
    fixture.park(&coordinator);

    // Any target below this binary's own schema version is refused, and the
    // parked Run is reported first because it is the most specific reason.
    let blocker = fixture
        .db
        .delivery_review_downgrade_blocker(CURRENT_U1_AWARE_SCHEMA_VERSION - 1)
        .expect("a parked Run blocks the downgrade");
    assert!(
        matches!(blocker, DowngradeBlocker::ParkedRuns { .. }),
        "got {blocker:?}"
    );

    // And a target that understands this schema version is not blocked by the
    // schema version itself — the tables and the parked Run are the reasons, and
    // each has its own test.
    let same_schema = fixture
        .db
        .delivery_review_downgrade_blocker(CURRENT_U1_AWARE_SCHEMA_VERSION);
    assert!(
        matches!(same_schema, Some(DowngradeBlocker::ParkedRuns { .. })),
        "got {same_schema:?}"
    );

    drop(coordinator);
    let _ = std::fs::remove_dir_all(&fixture.root);
}


