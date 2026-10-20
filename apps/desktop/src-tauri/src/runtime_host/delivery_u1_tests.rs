//! Package 2-D U1: the acceptance rule, its freshness checks and the surfaces
//! that must not be fooled by one.
//!
//! # What these tests prove
//!
//! An acceptance is a durable statement about a **specific** artifact: these
//! bytes, this managed version, this frozen contract, under this project
//! authorization. Every test below changes one of those and asserts the
//! acceptance stops being effective — or, where the change is only "we could not
//! establish the current state", that it is reported as unconfirmed rather than
//! as drift.
//!
//! # What they deliberately do not prove
//!
//! * That an acceptance is a verification. `finding_json` is asserted
//!   byte-identical before and after, and the row's class is asserted unchanged.
//! * That two columns written by a human are atomic. The transaction scope is
//!   "one writer on one database", stated as such.
//! * Anything about multiple processes or unlimited concurrency.
//!
//! The Run-lifecycle half (single entry edge, exits, fail-closed preconditions)
//! lives in `fox-agent-kernel`'s `delivery_review_tests`.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::*;
use crate::database::{
    AcceptanceWrite, CURRENT_U1_AWARE_SCHEMA_VERSION, Database, DeliveryChecklistSeed,
    DeliveryRequirement, DowngradeBlocker, RequirementKind, delivery_review_downgrade_blocker,
};
use crate::runtime_host::delivery::delivery_u1::{
    delivery_acceptance_state, effective_delivery_acceptance, parse_acceptance_decision,
    resolve_delivery_acceptance, AcceptanceIneffective,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const ACTOR: &str = "window:main";

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// A real database, a real Run, a real frozen contract row and a real candidate
/// file whose content genuinely satisfies it — so the only thing standing
/// between the fixture and `Confirmed` is whatever the test changes.
/// A minimal, well-formed fact as `decode_verdict` reads it. `origin` is a bare
/// token (`spec`, `checkUnbound`, …), not an object — the durable wire shape.
fn fact(class: &str, detail: &str) -> Value {
    json!({"class":class,"origin":"spec","detail":detail})
}

/// A refused fact, so a test can prove a security/trust refusal is not a risk a
/// user may assume.
fn refused_fact(reason_code: &str) -> Value {
    json!({"class":"refused","origin":"candidate","path":"out.txt",
        "sha256":digest(b"a.txt\nalpha"),"reasonCode":reason_code,"detail":"拒绝"})
}

struct Fixture {
    root: std::path::PathBuf,
    database: Database,
    conversation: String,
    run: String,
    item: String,
    spec: Value,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "p2d-u1-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("in")).unwrap();
        std::fs::write(root.join("in/a.txt"), b"alpha").unwrap();
        // A candidate that really is the frozen concat of `in/a.txt`.
        std::fs::write(root.join("out.txt"), b"a.txt\nalpha").unwrap();
        let database = Database::open(root.join("fox.db")).unwrap();
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some("read_only"),
            )
            .unwrap();
        let run = database
            .create_run(&conversation.id, "合并授权来源", None)
            .unwrap()
            .run
            .id;
        let conversation = conversation.id;
        use fox_engine_protocol::{
            ExecutionAuthority, FrozenPermission, PermissionMode, ResourceExecutor,
            RunControlBinding, TimeBudgets,
        };
        let permission = FrozenPermission {
            mode: PermissionMode::Allow,
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
            approval_epoch: None,
        };
        database
            .freeze_run_control(&RunControlBinding {
                schema_version: 1,
                run_id: run.clone(),
                conversation_id: conversation.clone(),
                engine_id: "pi".into(),
                execution_profile_id: "legacy".into(),
                authority: ExecutionAuthority::Authoritative,
                read_only_executor: ResourceExecutor::Rust,
                permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
                permission,
                budgets: TimeBudgets::default(),
            })
            .unwrap();
        let spec = json!({"schemaVersion":2,"envelopeVersion":2,"checkerVersion":2,
            "ruleId":"content:concat",
            "rule":{"type":"concat","sources":[{"path":"in/a.txt","sha256":digest(b"alpha"),
                "bytes":5,"firstLine":1,"lastLine":1}],"selected":["in/a.txt"],
                "selection_order":"bytes_ascending","tie_order":"filename_ascending",
                "merge_order":"filename_ascending","read_only":true}});
        let fixture = Self {
            root,
            database,
            conversation,
            run,
            item: "file:out.txt".into(),
            spec,
        };
        fixture.seed_row();
        fixture
    }

    fn seed_row(&self) {
        let seed = DeliveryChecklistSeed {
            item_key: self.item.clone(),
            target_path: Some("out.txt".into()),
            artifact_id: None,
            display_name: "合并产物".into(),
            checks: vec![],
            requirements: vec![DeliveryRequirement {
                item_key: Some(self.item.clone()),
                id: "content:concat".into(),
                kind: RequirementKind::ContentContract {
                    spec: self.spec.clone(),
                },
                source_text: "冻结合同".into(),
            }],
        };
        self.database
            .seed_delivery_checklist(&self.run, &[seed], crate::database::now_ms())
            .unwrap();
    }

    /// Run the real internal entry so the row carries a real, verified-but-
    /// incomplete v2 verdict. The fixture's candidate satisfies the concat, so
    /// the class here is `Confirmed`; tests that need `Unconfirmed` rewrite the
    /// contract to something with an unimplemented obligation.
    fn verify(&self) {
        let result = super::super::content_requirements::classify_v2_requirement(
            &self.database,
            &self.run,
            &self.item,
            &self.spec,
            Some(&self.root.join("out.txt")),
            &self.finding(),
            true,
        );
        assert_eq!(result.class, DeliveryClass::Confirmed, "{result:?}");
    }

    fn finding(&self) -> Value {
        let bytes = std::fs::read(self.root.join("out.txt")).unwrap();
        json!({"checkedHash":digest(&bytes),"boundPath":"out.txt","sourceRunId":self.run})
    }

    /// Rewrite the frozen contract to one whose obligations cannot all be
    /// discharged, so the row lands on `Unconfirmed` — the only class U1 accepts.
    fn make_unverified(&self) {
        let spec = json!({"schemaVersion":2,"envelopeVersion":2,"checkerVersion":2,
            "ruleId":"content:concat",
            "rule":{"type":"factQualifiers","sources":[{"path":"in/a.txt",
                "sha256":digest(b"alpha"),"bytes":5,"firstLine":1,"lastLine":1}]}});
        let requirements = crate::canonical_json::to_string(&[DeliveryRequirement {
            item_key: Some(self.item.clone()),
            id: "content:qualifiers".into(),
            kind: RequirementKind::ContentContract { spec },
            source_text: "限定语来源".into(),
        }])
        .unwrap();
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET requirements_json=?3 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![self.run, self.item, requirements],
            )?;
            Ok(())
        }).unwrap();
    }

    /// Persist a v2 verdict envelope on the row, as the terminal write would.
    fn store_verdict(&self, class: &str, facts: Vec<Value>) {
        let verdict = json!({
            "verdictSchemaVersion":2,"verdictEnvelopeVersion":2,
            "itemKey":self.item,"class":class,"aggregateClass":class,
            "itemCount":1,"terminal":false,"facts":facts,
        });
        let mut finding = self.finding();
        finding["deliveryVerdict"] = verdict;
        self.database.with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET status='pending', finding_json=?3 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![self.run, self.item, finding.canonical_string()],
            )?;
            Ok(())
        }).unwrap();
    }

    fn accept(&self) -> Result<crate::database::AcceptanceWrite, String> {
        resolve_delivery_acceptance(
            &self.database,
            &self.run,
            &self.item,
            DeliveryAcceptanceDecision::Accept,
            ACTOR,
            crate::database::now_ms(),
        )
        .map_err(|error| error.to_string())
    }

    fn effective(&self) -> Result<Option<EffectiveAcceptance>, AcceptanceIneffective> {
        effective_delivery_acceptance(&self.database, &self.run, &self.item)
    }

    /// A real managed-file version for the candidate, written with the
    /// **absolute** `storage_path` the real write gate records.
    fn seed_managed_version(&self, bytes: &[u8]) -> String {
        let absolute = self.root.join("out.txt");
        let version_id = format!("mv-{}", self.item);
        self.database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO managed_file_versions
                 (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
                  version_no, change_kind, before_hash, before_size, after_hash, after_size,
                  backup_path, restored_from_id, created_at)
                 VALUES (?1, ?2, ?3, 'call-1', 'write_file', ?4, 'out.txt', 1, 'created',
                         NULL, NULL, ?5, ?6, NULL, NULL, 1)",
                rusqlite::params![
                    version_id,
                    self.conversation,
                    self.run,
                    absolute.to_string_lossy(),
                    digest(bytes),
                    bytes.len() as i64
                ],
            )?;
            Ok(())
        }).unwrap();
        version_id
    }

    fn status(&self) -> String {
        self.database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM delivery_checklist_items WHERE run_id=?1 AND item_key=?2",
                    rusqlite::params![self.run, self.item],
                    |row| row.get::<_, String>(0),
                )
            })
            .unwrap()
    }

    fn finding_json(&self) -> Option<String> {
        self.database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT finding_json FROM delivery_checklist_items WHERE run_id=?1 AND item_key=?2",
                    rusqlite::params![self.run, self.item],
                    |row| row.get::<_, Option<String>>(0),
                )
            })
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

// ---------------------------------------------------------------------------
// The decision vocabulary
// ---------------------------------------------------------------------------

#[test]
fn the_decision_vocabulary_has_no_default() {
    assert_eq!(
        parse_acceptance_decision("accept"),
        Some(DeliveryAcceptanceDecision::Accept)
    );
    assert_eq!(
        parse_acceptance_decision("revoke"),
        Some(DeliveryAcceptanceDecision::Revoke)
    );
    // An unrecognised string is `None`, never a silent "accept".
    assert_eq!(parse_acceptance_decision(""), None);
    assert_eq!(parse_acceptance_decision("approve"), None);
    assert_eq!(parse_acceptance_decision("ACCEPT"), None);
}

// ---------------------------------------------------------------------------
// A legacy / historical row is never an acceptable Unconfirmed
// ---------------------------------------------------------------------------

#[test]
fn a_row_with_no_verdict_envelope_cannot_be_accepted() {
    let fixture = Fixture::new("no-envelope");
    // Seeded, never verified: the row has no v2 envelope at all.
    assert_eq!(fixture.status(), "pending");
    let error = fixture.accept().unwrap_err();
    assert!(
        error.contains("不是可核验的 v2 同一行信封"),
        "{error}"
    );
    assert!(fixture.effective().unwrap().is_none());
}

#[test]
fn a_corrupt_envelope_cannot_be_accepted() {
    let fixture = Fixture::new("corrupt-envelope");
    fixture.make_unverified();
    let mut finding = fixture.finding();
    // An envelope with an unreadable fact list: `decode_verdict` returns Corrupt.
    finding["deliveryVerdict"] = json!({"verdictSchemaVersion":2,"verdictEnvelopeVersion":2,
        "itemKey":fixture.item,"class":"unconfirmed","aggregateClass":"unconfirmed",
        "itemCount":1,"terminal":false,"facts":"not-an-array"});
    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET finding_json=?3 WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![fixture.run, fixture.item, finding.canonical_string()],
        )?;
        Ok(())
    }).unwrap();
    let error = fixture.accept().unwrap_err();
    assert!(error.contains("不是可核验的 v2 同一行信封"), "{error}");
}

#[test]
fn an_envelope_naming_another_item_cannot_be_accepted() {
    let fixture = Fixture::new("foreign-key");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "限定语未实现，义务未履行")],
    );
    // Rewrite only the envelope's itemKey, leaving the row key alone.
    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET finding_json = json_set(finding_json, '$.deliveryVerdict.itemKey', 'file:another.txt') WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![fixture.run, fixture.item],
        )?;
        Ok(())
    }).unwrap();
    let error = fixture.accept().unwrap_err();
    assert!(error.contains("不是可核验的 v2 同一行信封"), "{error}");
}

#[test]
fn a_confirmed_row_needs_no_acceptance_and_cannot_take_one() {
    let fixture = Fixture::new("confirmed-row");
    fixture.verify();
    fixture.store_verdict(
        "confirmed",
        vec![fact("passed", "已通过")],
    );
    let error = fixture.accept().unwrap_err();
    assert!(error.contains("只有未核验项可被接受"), "{error}");
}

#[test]
fn a_row_blocked_by_a_refusal_cannot_be_accepted() {
    let fixture = Fixture::new("blocked-refusal");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![refused_fact("delivery.security.readonly_input_write")],
    );
    let error = fixture.accept().unwrap_err();
    assert!(error.contains("不可由用户假设的风险"), "{error}");
}

// ---------------------------------------------------------------------------
// The happy path, and what an acceptance does NOT change
// ---------------------------------------------------------------------------

#[test]
fn a_first_acceptance_is_recorded_and_the_finding_is_untouched() {
    let fixture = Fixture::new("first-accept");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "限定语未实现，义务未履行")],
    );
    let before = fixture.finding_json();

    let written = fixture.accept().unwrap();
    assert_eq!(written, crate::database::AcceptanceWrite::Written);

    let effective = fixture.effective().unwrap().expect("an acceptance stands");
    assert_eq!(effective.run_id, fixture.run);
    assert_eq!(effective.item_key, fixture.item);
    assert_eq!(effective.accepted_by, ACTOR);
    assert_eq!(effective.candidate_sha256, digest(b"a.txt\nalpha"));
    assert!(!effective.spec_hash.is_empty());

    // The acceptance changes nothing about the verification.
    assert_eq!(fixture.finding_json(), before, "finding_json is byte-identical");
    assert_eq!(fixture.status(), "pending", "the row is still pending");
}

#[test]
fn a_repeated_identical_acceptance_is_a_no_op() {
    let fixture = Fixture::new("repeat-accept");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);
    let first = fixture.effective().unwrap().unwrap();

    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Unchanged);
    let second = fixture.effective().unwrap().unwrap();
    assert_eq!(first.accepted_at, second.accepted_at, "not rewritten");
    assert_eq!(first.accepted_by, second.accepted_by);

    // And the audit holds exactly one `accepted` row, not two.
    let count: i64 = fixture.database.with_connection(|connection| {
        connection.query_row(
            "SELECT COUNT(*) FROM delivery_acceptance_audit WHERE run_id=?1 AND item_key=?2 AND action='accepted'",
            rusqlite::params![fixture.run, fixture.item],
            |row| row.get(0),
        )
    }).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn a_revocation_withdraws_the_acceptance_and_a_re_acceptance_is_a_new_decision() {
    let fixture = Fixture::new("revoke-reaccept");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);
    let first_at = fixture.effective().unwrap().unwrap().accepted_at;

    let revoked = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Revoke,
        ACTOR,
        first_at + 10,
    )
    .unwrap();
    assert_eq!(revoked, crate::database::AcceptanceWrite::Written);
    assert!(fixture.effective().unwrap().is_none(), "withdrawn");

    // A repeated revocation with nothing in between is a no-op, so `revoked_at`
    // cannot drift forward and misreport when the user changed their mind.
    let repeat = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Revoke,
        ACTOR,
        first_at + 15,
    )
    .unwrap();
    assert_eq!(repeat, crate::database::AcceptanceWrite::Unchanged);

    // A re-acceptance is a new decision: it writes, and the first one is still in
    // the audit rather than having been overwritten.
    let reaccepted = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Accept,
        ACTOR,
        first_at + 20,
    )
    .unwrap();
    assert_eq!(reaccepted, crate::database::AcceptanceWrite::Written);
    let second = fixture.effective().unwrap().expect("stands again");
    assert_eq!(second.accepted_at, first_at + 20);

    let accepted_audits: i64 = fixture.database.with_connection(|connection| {
        connection.query_row(
            "SELECT COUNT(*) FROM delivery_acceptance_audit WHERE run_id=?1 AND item_key=?2 AND action='accepted'",
            rusqlite::params![fixture.run, fixture.item],
            |row| row.get(0),
        )
    }).unwrap();
    assert_eq!(accepted_audits, 2, "both decisions are in the audit");

    // A revocation after a re-acceptance IS a new decision: the re-acceptance
    // cleared `revoked_at`, so there is something to withdraw again.
    let revoked_again = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Revoke,
        ACTOR,
        first_at + 30,
    )
    .unwrap();
    assert_eq!(revoked_again, crate::database::AcceptanceWrite::Written);
    assert!(fixture.effective().unwrap().is_none());
}

#[test]
fn a_rejection_is_recorded_without_an_acceptance_and_is_idempotent() {
    let fixture = Fixture::new("reject");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    let before = fixture.finding_json();

    let written = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Reject,
        ACTOR,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(written, crate::database::AcceptanceWrite::Written);
    assert!(fixture.effective().unwrap().is_none(), "a rejection is not an acceptance");
    assert_eq!(fixture.finding_json(), before);

    // `accepted_at` must stay 0: a rejected item was never accepted, and a
    // non-zero value would read back as one.
    let accepted_at: i64 = fixture.database.with_connection(|connection| {
        connection.query_row(
            "SELECT accepted_at FROM delivery_acceptances WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![fixture.run, fixture.item],
            |row| row.get(0),
        )
    }).unwrap();
    assert_eq!(accepted_at, 0);

    let again = resolve_delivery_acceptance(
        &fixture.database,
        &fixture.run,
        &fixture.item,
        DeliveryAcceptanceDecision::Reject,
        ACTOR,
        crate::database::now_ms(),
    )
    .unwrap();
    assert_eq!(again, crate::database::AcceptanceWrite::Unchanged);
}

#[test]
fn the_actor_must_be_a_trusted_session_identifier() {
    let fixture = Fixture::new("actor");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    for bad in ["", "   "] {
        let error = resolve_delivery_acceptance(
            &fixture.database,
            &fixture.run,
            &fixture.item,
            DeliveryAcceptanceDecision::Accept,
            bad,
            crate::database::now_ms(),
        )
        .unwrap_err();
        assert!(matches!(error, AcceptanceResolveError::Storage(_)), "{error:?}");
    }
    assert!(fixture.effective().unwrap().is_none(), "nothing was written");
}

// ---------------------------------------------------------------------------
// Freshness: every one of these must withdraw the acceptance
// ---------------------------------------------------------------------------

#[test]
fn rewriting_the_candidate_bytes_withdraws_the_acceptance() {
    let fixture = Fixture::new("byte-drift");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);
    assert!(fixture.effective().unwrap().is_some());

    // Rewrite the file but leave every database column alone — exactly the case
    // a column comparison would miss.
    std::fs::write(fixture.root.join("out.txt"), b"tampered").unwrap();

    let reason = fixture.effective().unwrap_err();
    assert_eq!(reason, AcceptanceIneffective::CandidateDrift {
        expected: digest(b"a.txt\nalpha"),
        actual: digest(b"tampered"),
    });
    assert!(reason.is_definite());
}

#[test]
fn rewriting_the_contract_withdraws_the_acceptance() {
    let fixture = Fixture::new("contract-drift");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);

    // The row's own frozen contract column is replaced.
    let requirements = crate::canonical_json::to_string(&[DeliveryRequirement {
        item_key: Some(fixture.item.clone()),
        id: "content:other".into(),
        kind: RequirementKind::ContentContract {
            spec: json!({"schemaVersion":2,"rule":{"type":"unbound","reason":"x"}}),
        },
        source_text: "换合同".into(),
    }])
    .unwrap();
    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET requirements_json=?3 WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![fixture.run, fixture.item, requirements],
        )?;
        Ok(())
    }).unwrap();

    let reason = fixture.effective().unwrap_err();
    assert!(matches!(reason, AcceptanceIneffective::ContractDrift { .. }), "{reason:?}");
}

#[test]
fn terminating_the_row_withdraws_the_acceptance() {
    let fixture = Fixture::new("row-terminated");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);

    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET status='failed' WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![fixture.run, fixture.item],
        )?;
        Ok(())
    }).unwrap();

    let reason = fixture.effective().unwrap_err();
    assert_eq!(
        reason,
        AcceptanceIneffective::RowNotPending { status: "failed".to_owned() }
    );
}

#[test]
fn withdrawing_the_project_authorization_makes_the_acceptance_unconfirmed_not_drift() {
    let fixture = Fixture::new("auth-withdrawn");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );
    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);

    // Remove the project access the acceptance was made under.
    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE conversations SET project_root=NULL, project_id=NULL WHERE id=?1",
            rusqlite::params![fixture.conversation],
        )?;
        Ok(())
    }).unwrap();

    let reason = fixture.effective().unwrap_err();
    assert_eq!(reason, AcceptanceIneffective::AuthorizationWithdrawn);
    // Not a definite drift: we could not establish the current state, so we must
    // not claim the bytes changed.
    assert!(!reason.is_definite());
}

// ---------------------------------------------------------------------------
// The item-detail surface
// ---------------------------------------------------------------------------

#[test]
fn the_item_detail_surface_keeps_verification_and_acceptance_apart() {
    let fixture = Fixture::new("detail");
    fixture.make_unverified();
    fixture.store_verdict(
        "unconfirmed",
        vec![fact("unconfirmed", "义务未履行")],
    );

    let before = delivery_acceptance_state(&fixture.database, &fixture.run, &fixture.item).unwrap();
    assert_eq!(before.verification_class, "unconfirmed");
    assert!(before.accepted.is_none());
    assert!(!before.rejected);
    assert!(before.blocked.is_none(), "an unverified row may be accepted");

    assert_eq!(fixture.accept().unwrap(), crate::database::AcceptanceWrite::Written);
    let after = delivery_acceptance_state(&fixture.database, &fixture.run, &fixture.item).unwrap();
    // The verification class is unchanged by the acceptance.
    assert_eq!(after.verification_class, "unconfirmed");
    assert!(after.accepted.is_some());
    assert!(after.blocked.is_none());
}

#[test]
fn the_item_detail_surface_reports_a_blocked_row_without_an_acceptance() {
    let fixture = Fixture::new("detail-blocked");
    fixture.verify();
    fixture.store_verdict(
        "confirmed",
        vec![fact("passed", "已通过")],
    );
    let state = delivery_acceptance_state(&fixture.database, &fixture.run, &fixture.item).unwrap();
    assert_eq!(state.verification_class, "confirmed");
    assert!(state.accepted.is_none());
    assert_eq!(
        state.blocked.as_ref().map(|blocked| blocked.kind.as_str()),
        Some("class_not_unconfirmed"),
        "the wire shape carries the stable token the frontend switches on"
    );
    assert!(
        state
            .blocked
            .as_ref()
            .is_some_and(|blocked| blocked.detail.contains("只有未核验项可被接受")),
        "and the display line the frontend renders"
    );
}

#[test]
fn a_missing_row_reads_as_absent_rather_than_panicking() {
    let fixture = Fixture::new("missing-row");
    let state = delivery_acceptance_state(&fixture.database, &fixture.run, "file:nope.txt").unwrap();
    assert!(state.accepted.is_none());
    assert_eq!(
        state.blocked.as_ref().map(|blocked| blocked.kind.as_str()),
        Some("absent")
    );
}

// ---------------------------------------------------------------------------
// Package 2-D repair, D-4: the version identity hits the real ledger
// ---------------------------------------------------------------------------

#[test]
fn d4_a_relative_target_path_binds_the_real_managed_version() {
    // The D-4 defect: `expectations_from_task` seeds a **relative** `target_path`
    // while `managed_file_versions.storage_path` is absolute, so the binding read
    // missed and the whole version criterion was skipped.
    let fixture = Fixture::new("d4-relative");
    fixture.make_unverified();
    fixture.store_verdict("unconfirmed", vec![fact("unconfirmed", "义务未履行")]);
    let version_id = fixture.seed_managed_version(b"a.txt\nalpha");
    assert_eq!(fixture.status(), "pending");

    assert_eq!(fixture.accept().unwrap(), AcceptanceWrite::Written);
    let row = fixture
        .database
        .read_delivery_acceptance_row(&fixture.run, &fixture.item)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        row.accepted_version_id.as_deref(),
        Some(version_id.as_str()),
        "the acceptance binds the version the real write gate recorded"
    );
    assert!(fixture.effective().unwrap().is_some(), "and it stands");
}

#[test]
fn d4_an_absolute_target_path_binds_the_same_object() {
    let fixture = Fixture::new("d4-absolute");
    fixture.make_unverified();
    fixture.store_verdict("unconfirmed", vec![fact("unconfirmed", "义务未履行")]);
    let version_id = fixture.seed_managed_version(b"a.txt\nalpha");
    fixture.database.with_connection(|connection| {
        connection.execute(
            "UPDATE delivery_checklist_items SET target_path=?3 WHERE run_id=?1 AND item_key=?2",
            rusqlite::params![
                fixture.run,
                fixture.item,
                fixture.root.join("out.txt").to_string_lossy()
            ],
        )?;
        Ok(())
    }).unwrap();

    assert_eq!(fixture.accept().unwrap(), AcceptanceWrite::Written);
    let row = fixture
        .database
        .read_delivery_acceptance_row(&fixture.run, &fixture.item)
        .unwrap()
        .expect("the row exists");
    assert_eq!(
        row.accepted_version_id.as_deref(),
        Some(version_id.as_str()),
        "the absolute form resolves to the same ledger row"
    );
    assert!(fixture.effective().unwrap().is_some());
}

#[test]
fn d4_a_new_version_of_the_same_bytes_withdraws_the_acceptance() {
    let fixture = Fixture::new("d4-version-bump");
    fixture.make_unverified();
    fixture.store_verdict("unconfirmed", vec![fact("unconfirmed", "义务未履行")]);
    let first = fixture.seed_managed_version(b"a.txt\nalpha");
    assert_eq!(fixture.accept().unwrap(), AcceptanceWrite::Written);
    assert!(fixture.effective().unwrap().is_some());

    // A second version of the SAME bytes: same content, different version identity.
    let second = format!("{first}-v2");
    fixture.database.with_connection(|connection| {
        connection.execute(
            "INSERT INTO managed_file_versions
             (id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
              version_no, change_kind, before_hash, before_size, after_hash, after_size,
              backup_path, restored_from_id, created_at)
             VALUES (?1, ?2, ?3, 'call-2', 'write_file', ?4, 'out.txt', 2, 'modified',
                     NULL, NULL, ?5, ?6, NULL, NULL, 2)",
            rusqlite::params![
                second,
                fixture.conversation,
                fixture.run,
                fixture.root.join("out.txt").to_string_lossy(),
                digest(b"a.txt\nalpha"),
                b"a.txt\nalpha".len() as i64
            ],
        )?;
        Ok(())
    }).unwrap();

    assert_eq!(
        fixture.effective().unwrap_err(),
        AcceptanceIneffective::VersionDrift
    );
}

#[test]
fn d4_a_candidate_with_no_managed_version_is_not_silently_treated_as_versionless() {
    // A candidate never written through the managed-file gate has no ledger row.
    // That is a legitimate "no version" — distinct from a lookup that failed,
    // which is what stops a broken path from slipping through the criterion.
    let fixture = Fixture::new("d4-no-version");
    fixture.make_unverified();
    fixture.store_verdict("unconfirmed", vec![fact("unconfirmed", "义务未履行")]);
    assert_eq!(fixture.accept().unwrap(), AcceptanceWrite::Written);

    let row = fixture
        .database
        .read_delivery_acceptance_row(&fixture.run, &fixture.item)
        .unwrap()
        .expect("the row exists");
    assert_eq!(row.accepted_version_id, None);
    assert!(
        fixture.effective().unwrap().is_some(),
        "a genuinely versionless candidate is acceptable"
    );
}

// ---------------------------------------------------------------------------
// Package 2-D repair, D-3: the blocked reason's wire shape
// ---------------------------------------------------------------------------

#[test]
fn d3_the_blocked_reason_serializes_into_the_shape_the_frontend_reads() {
    // The D-3 defect: `AcceptanceIneffective` is an external-tagged enum, so a
    // direct serialization produced `"Absent"` for the unit variant and
    // `{"row_not_pending":{…}}` for the rest — neither has the `kind`/`detail`
    // pair the frontend reads, so every specific reason was unreachable.
    let fixture = Fixture::new("d3-shape");
    fixture.verify();
    fixture.store_verdict("confirmed", vec![fact("passed", "已通过")]);
    let state = delivery_acceptance_state(&fixture.database, &fixture.run, &fixture.item).unwrap();

    let wire = serde_json::to_value(&state).unwrap();
    let blocked = wire["blocked"].as_object().expect("an object, not a bare string");
    assert_eq!(
        blocked.get("kind").and_then(Value::as_str),
        Some("class_not_unconfirmed"),
        "the token the frontend switches on"
    );
    assert!(
        blocked
            .get("detail")
            .and_then(Value::as_str)
            .is_some_and(|detail| detail.contains("只有未核验项可被接受")),
        "and the display line it renders"
    );

    // Every variant must round-trip through the DTO, so no reason can silently
    // become the generic fallback.
    for reason in [
        AcceptanceIneffective::Absent,
        AcceptanceIneffective::RowNotPending { status: "failed".to_owned() },
        AcceptanceIneffective::NotAVerifiableRecord,
        AcceptanceIneffective::ClassNotUnconfirmed { class: "confirmed".to_owned() },
        AcceptanceIneffective::BlockedByFact { detail: "d".to_owned() },
        AcceptanceIneffective::CandidateDrift { expected: "e".to_owned(), actual: "a".to_owned() },
        AcceptanceIneffective::CandidateUnreadable { detail: "d".to_owned() },
        AcceptanceIneffective::VersionDrift,
        AcceptanceIneffective::ContractDrift { expected: "e".to_owned(), actual: "a".to_owned() },
        AcceptanceIneffective::AuthorizationWithdrawn,
    ] {
        let dto = reason.blocked();
        assert_eq!(dto.kind, reason.as_str());
        assert!(!dto.detail.is_empty(), "{reason:?} has no display line");
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["kind"], json!(reason.as_str()));
        assert!(value["detail"].is_string());
    }
}

// ---------------------------------------------------------------------------
// Package 2-D repair, D-5: the executable downgrade pre-check
// ---------------------------------------------------------------------------

#[test]
fn d5_the_downgrade_precheck_refuses_an_ahead_schema() {
    // A database with the Kernel tables but no acceptance tables: the parked
    // check is a legitimate "no", so the schema version is the refusal.
    let fixture = Fixture::new("d5-schema");
    fixture.database.with_connection(|connection| {
        connection.execute_batch(
            "DROP TABLE delivery_acceptances; DROP TABLE delivery_acceptance_audit;",
        )?;
        Ok(())
    }).unwrap();
    let blocker = fixture
        .database
        .delivery_review_downgrade_blocker(1)
        .expect("a database ahead of the target is refused");
    assert!(
        matches!(blocker, DowngradeBlocker::SchemaAhead { .. }),
        "got {blocker:?}"
    );
}

#[test]
fn d5_the_downgrade_precheck_refuses_the_acceptance_tables_even_with_no_parked_run() {
    // "No parked Run" is necessary but not sufficient: the tables exist
    // independently, and an older binary would silently drop every acceptance.
    let fixture = Fixture::new("d5-tables");
    let blocker = fixture
        .database
        .delivery_review_downgrade_blocker(CURRENT_U1_AWARE_SCHEMA_VERSION - 1)
        .expect("the tables alone block the downgrade");
    assert!(
        matches!(
            blocker,
            DowngradeBlocker::SchemaAhead { .. } | DowngradeBlocker::AcceptanceTablesPresent
        ),
        "got {blocker:?}"
    );
}

#[test]
fn d5_the_downgrade_precheck_never_mistakes_a_database_error_for_permission() {
    // A database error must be reported as `CheckFailed`, never as "no blocker".
    let connection = rusqlite::Connection::open_in_memory().unwrap();
    let blocker = delivery_review_downgrade_blocker(&connection, CURRENT_U1_AWARE_SCHEMA_VERSION);
    assert!(
        matches!(blocker, Some(DowngradeBlocker::CheckFailed { .. })),
        "an unreadable database is not permission to downgrade: {blocker:?}"
    );
}





