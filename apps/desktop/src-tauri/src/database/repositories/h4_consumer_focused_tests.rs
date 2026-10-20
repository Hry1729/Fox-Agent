//! H4 focused repair: one pass-display rule, two real consumers.
//!
//! Card `G-P2B2-H4-focused-repair.md` §2–§3. H returned two defects:
//!
//! * **RETURN-1** — the checklist projection mapped *every* `Some(false)` to
//!   `pending`, so a legitimate `Refused`/`Invalid`/`DefiniteFailure` the write
//!   side had correctly stored as `failed` was re-displayed as "not verified".
//!   The product's most important conclusion ("this did not pass") was erased.
//! * **RETURN-2** — `artifact_delivery_view` never consulted the pass rule at all,
//!   so for one row the checklist said `pending` while the artifact view said
//!   `passed`.
//!
//! Both are now decided by one shared guard,
//! `delivery::passed_display_is_withheld`. These tests drive the **real** entries:
//! `Database::load_conversation` (the checklist projection) and
//! `Database::artifact_for_conversation` (the artifact view), over a real on-disk
//! database, a real file and a real `managed_file_versions` binding — not an
//! in-memory schema and not an internal helper.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::Database;
use crate::database::now_ms;

/// A task whose checklist seeds exactly one promised artifact.
const TASK: &str = "请生成 out/deliverable.json";

/// One real, end-to-end delivery world on disk.
///
/// Everything the two consumers read is really in the database and really on the
/// filesystem: the artifact row, the `managed_file_versions` row whose `after_hash`
/// matches the bytes on disk, the checklist row bound to both, and the
/// conversation/run the views are keyed by.
struct World {
    db: Database,
    root: std::path::PathBuf,
    conversation_id: String,
    run_id: String,
    artifact_id: String,
    /// The artifact's absolute storage path.
    path: std::path::PathBuf,
    /// The one checklist row this artifact is bound to.
    item_key: String,
    /// The `managed_file_versions` id the finding must name for the artifact view
    /// to accept the verdict as being about the *current* version.
    version_id: String,
}

fn world(tag: &str) -> World {
    let root = std::env::temp_dir().join(format!("fox-h4-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("out")).unwrap();
    let path = root.join("out").join("deliverable.json");
    std::fs::write(&path, br#"{"ok":true}"#).unwrap();
    let (hash, size) = crate::runtime_host::managed_files::hash_file(&path).unwrap();

    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
            None,
        )
        .unwrap();
    let run_id = db
        .create_run(&conversation.id, TASK, None)
        .unwrap()
        .run
        .id;

    // Seed the checklist from the task, so the promised item is the real one the
    // delivery lane would have produced.
    let seeds = crate::runtime_host::delivery::expectations_from_task(TASK);
    assert!(
        !seeds.is_empty(),
        "the fixture task must promise at least one artifact: {seeds:?}"
    );
    db.seed_delivery_checklist(&run_id, &seeds, now_ms())
        .unwrap();
    let item_key = seeds[0].item_key.clone();

    let artifact_id = "artifact-1".to_owned();
    db.with_connection(|connection| {
        // Point the seeded row at the real file and the real artifact.
        connection.execute(
            "UPDATE delivery_checklist_items SET target_path=?1, artifact_id=?2, display_name='deliverable.json'
             WHERE run_id=?3 AND item_key=?4",
            rusqlite::params![path.to_string_lossy(), artifact_id, run_id, item_key],
        )?;
        connection.execute(
            "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,
                artifact_class,artifact_origin,storage_path,media_type,byte_size,sha256,status,
                created_at,updated_at)
             VALUES (?1,?2,?3,'deliverable.json','created_file','deliverable','project',?4,
                NULL,?5,?6,'ready',1,1)",
            rusqlite::params![
                artifact_id,
                conversation.id,
                run_id,
                path.to_string_lossy(),
                size as i64,
                hash
            ],
        )?;
        // The durable version binding the artifact view requires: same storage
        // path, same conversation/run, and `after_hash` equal to the bytes on disk.
        connection.execute(
            "INSERT INTO managed_file_versions(id,conversation_id,run_id,tool_call_id,tool,
                storage_path,display_name,version_no,change_kind,after_hash,after_size,created_at)
             VALUES ('v1',?1,?2,'tool-1','write_file',?3,'deliverable.json',1,'created',?4,?5,1)",
            rusqlite::params![
                conversation.id,
                run_id,
                path.to_string_lossy(),
                hash,
                size as i64
            ],
        )?;
        Ok(())
    })
    .unwrap();

    // The user message that makes the artifact `user_request` rather than
    // `host_rule`; the artifact view reads it through the real path either way.
    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,metadata_json,created_at,updated_at)
             VALUES ('m1',?1,?2,'user','chat',?3,'complete',1,'{}',1,1)",
            rusqlite::params![conversation.id, run_id, TASK],
        )?;
        Ok(())
    })
    .unwrap();

    World {
        db,
        root,
        conversation_id: conversation.id,
        run_id,
        artifact_id,
        path,
        item_key,
        version_id: "v1".to_owned(),
    }
}

impl World {
    /// The `boundPath` the artifact view compares against the artifact's storage
    /// path. The real writer stores the candidate path, and the view normalises
    /// absolute/relative forms, so either spelling works; this uses the absolute
    /// path the fixture actually wrote.
    fn bound_path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
    /// Write the checklist row's durable status and finding directly.
    ///
    /// This is the only way to create the *damaged* shape H probed: a real writer
    /// cannot produce it, because `stage_outcome` derives `passed` and
    /// `finding_json` from the same in-memory stop. Writing it here is not
    /// "writing the expected answer first" — the assertion is about what the two
    /// **readers** then do with those bytes, and both are driven through their
    /// real entries below.
    fn store(&self, status: &str, finding: Option<serde_json::Value>) {
        self.db
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE delivery_checklist_items SET status=?1, finding_json=?2
                     WHERE run_id=?3 AND item_key=?4",
                    rusqlite::params![
                        status,
                        finding.map(|value| value.canonical_string()),
                        self.run_id,
                        self.item_key
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// The checklist row as the real projection reports it.
    fn checklist_status(&self) -> String {
        self.db
            .load_conversation(&self.conversation_id)
            .unwrap()
            .delivery_checklist
            .into_iter()
            .find(|view| view.item_key == self.item_key)
            .expect("the projection carries the promised item")
            .status
    }

    /// The artifact as the real artifact entry reports it.
    fn artifact_status(&self) -> String {
        self.db
            .artifact_for_conversation(&self.conversation_id, &self.artifact_id)
            .unwrap()
            .expect("the artifact exists")
            .delivery
            .expect("a deliverable project artifact carries a delivery view")
            .verification_status
    }
}

/// A placeholder digest for a `candidate` fact's `sha256`.
///
/// The decoder requires one, so a fact without it decodes as `Corrupt` and would
/// make every case look like an envelope problem. The value is irrelevant to the
/// pass rule — only `class`/`aggregateClass` and the fact classes are compared —
/// so a well-formed constant is enough.
const FACT_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The durable finding shape the real writer produces: the item's own binding keys
/// at the top level, plus the self-contained v2 verdict envelope.
///
/// Reproducing the binding keys matters for the artifact view, which reads
/// `boundPath`/`managedVersionId` to decide whether the verdict is about *this*
/// version. A finding carrying only the envelope would report `stale` for every
/// case and quietly turn the whole artifact half of this suite into a test of the
/// version check rather than of the pass rule.
fn finding(envelope: serde_json::Value, path: &str, version_id: &str) -> serde_json::Value {
    serde_json::json!({
        "boundPath": path,
        "managedVersionId": version_id,
        "reason": null,
        "deliveryVerdict": envelope
    })
}

/// A well-formed v2 envelope declaring `class`, with the given facts.
fn envelope(
    item_key: &str,
    class: &str,
    aggregate: &str,
    facts: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "verdictSchemaVersion": 2,
        "verdictEnvelopeVersion": 2,
        "itemKey": item_key,
        "class": class,
        "aggregateClass": aggregate,
        "itemCount": 1,
        "terminal": matches!(class, "confirmed" | "refused" | "invalid" | "definiteFailure"),
        "repairBudgetRemaining": true,
        "facts": facts
    })
}

/// A `confirmed` envelope whose own facts say `failed`: the self-contradiction a
/// truncated, mis-merged or hand-edited row can carry.
fn contradicted_pass(item_key: &str, path: &str, version_id: &str) -> serde_json::Value {
    finding(
        envelope(
            item_key,
            "confirmed",
            "confirmed",
            vec![serde_json::json!({
                "class": "failed",
                "origin": "candidate",
                "ruleId": "content",
                "reasonCode": "sheet_names_mismatch",
                "path": path,
                "sha256": FACT_SHA,
                "blocking": true,
                "needsRepair": true
            })],
        ),
        path,
        version_id,
    )
}

/// A legitimate pass: well-formed `confirmed` with a supporting `passed` fact.
fn legitimate_pass(item_key: &str, path: &str, version_id: &str) -> serde_json::Value {
    finding(
        envelope(
            item_key,
            "confirmed",
            "confirmed",
            vec![serde_json::json!({
                "class": "passed",
                "origin": "candidate",
                "ruleId": "content",
                "reasonCode": "content_matches",
                "path": path,
                "sha256": FACT_SHA,
                "blocking": false,
                "needsRepair": false
            })],
        ),
        path,
        version_id,
    )
}

/// A legitimate definite non-pass whose facts support it.
///
/// The *fact* class is the vocabulary's own (`refused`/`invalid`/`failed`); the
/// item's declared class is the folded outcome the write side would have stored.
fn definite_non_pass(
    item_key: &str,
    class: &str,
    path: &str,
    version_id: &str,
) -> serde_json::Value {
    let (fact_class, rule, reason) = match class {
        "refused" => ("refused", "policy", "path_denied"),
        "invalid" => ("invalid", "schema", "contract_invalid"),
        _ => ("failed", "content", "definite_failure"),
    };
    finding(
        envelope(
            item_key,
            class,
            class,
            vec![serde_json::json!({
                "class": fact_class,
                "origin": "candidate",
                "ruleId": rule,
                "reasonCode": reason,
                "path": path,
                "sha256": FACT_SHA,
                "blocking": true,
                "needsRepair": false
            })],
        ),
        path,
        version_id,
    )
}

/// A `RepairNeeded` envelope: this round's check did not pass, but the item is
/// repairable, so the write side stores `failed` while the Run stays resumable.
///
/// The fact is a blocking failed-with-repair, which is exactly what folds to
/// `RepairNeeded`; the item's `terminal` is false because it is not a final class.
fn repair_needed(item_key: &str, path: &str, version_id: &str) -> serde_json::Value {
    finding(
        envelope(
            item_key,
            "repairNeeded",
            "repairNeeded",
            vec![serde_json::json!({
                "class": "failed",
                "origin": "candidate",
                "ruleId": "content",
                "reasonCode": "repairable_mismatch",
                "path": path,
                "sha256": FACT_SHA,
                "blocking": true,
                "needsRepair": true
            })],
        ),
        path,
        version_id,
    )
}

/// A bare legacy record: no v2 envelope at all, but a real version binding.
fn legacy_finding(path: &str, version_id: &str) -> serde_json::Value {
    serde_json::json!({
        "boundPath": path,
        "managedVersionId": version_id,
        "reason": null
    })
}

/// Everything the fixture must establish before any assertion, proven explicitly
/// rather than assumed: the checklist row exists and the artifact view can reach
/// its real `passed` branch (card §3 item 3 — the positive case must be shown to
/// reach `verification_status=passed` rather than falling to `stale`/`unavailable`).
#[test]
fn h4f_the_fixture_reaches_the_real_passed_branch() {
    let world = world("fixture");
    world.store(
        "passed",
        Some(legitimate_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_eq!(
        world.checklist_status(),
        "passed",
        "the checklist projection reports the legitimate pass"
    );
    assert_eq!(
        world.artifact_status(),
        "passed",
        "the fixture reaches the artifact view's real `passed` branch rather than \
         falling into stale/unavailable earlier"
    );
}

// ---------------------------------------------------------------------------
// RETURN-1: the checklist projection is a downgrade guard, not a re-derivation
// ---------------------------------------------------------------------------

/// H's `h4r_a_legitimate_definite_non_pass_is_still_shown_as_failed`, through the
/// real `load_conversation`.
///
/// The write side stores these as `failed`; the reader must not wash them into
/// `pending`. H observed all three classes failing; all three are asserted here.
#[test]
fn h4f_a_legitimate_definite_non_pass_still_reads_as_failed() {
    let world = world("definite");
    let path = world.bound_path();
    for class in ["refused", "invalid", "definiteFailure"] {
        world.store(
            "failed",
            Some(definite_non_pass(
                &world.item_key,
                class,
                &path,
                &world.version_id,
            )),
        );
        assert_eq!(
            world.checklist_status(),
            "failed",
            "`{class}` is a legitimate definite non-pass and must keep reading as \
             `failed`, not be washed into `pending`"
        );
        assert_eq!(
            world.artifact_status(),
            "failed",
            "the artifact view agrees for `{class}`: a real failure, not stale"
        );
    }
}

/// `RepairNeeded` is stored `failed` because this round's check did not pass, but
/// it is repairable and does not make the Run's terminal state a failure.
#[test]
fn h4f_repair_needed_reads_as_failed_and_stays_repairable() {
    let world = world("repair");
    world.store(
        "failed",
        Some(repair_needed(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_eq!(
        world.checklist_status(),
        "failed",
        "RepairNeeded means this round's check did not pass; it is not silently \
         downgraded to `pending`"
    );
    assert_eq!(
        world.artifact_status(),
        "failed",
        "the artifact view does not demote a real failure to stale/unavailable"
    );
}

/// A contradicted pass is withheld — the original H4 read defense, preserved.
#[test]
fn h4f_a_contradicted_stored_pass_is_not_projected_as_passed() {
    let world = world("contradicted");
    world.store(
        "passed",
        Some(contradicted_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_eq!(
        world.checklist_status(),
        "pending",
        "a row claiming passed whose own facts say no is withheld, not displayed \
         as a pass and not invented into `failed`"
    );
    assert_ne!(
        world.artifact_status(),
        "passed",
        "the artifact view withholds the same pass, so the two views agree"
    );
}

/// A `pending` row is never upgraded to a pass by the reader, even when its facts
/// are locally legitimate.
#[test]
fn h4f_a_pending_row_is_never_upgraded_by_the_reader() {
    let world = world("pending");
    world.store(
        "pending",
        Some(legitimate_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_eq!(
        world.checklist_status(),
        "pending",
        "the reader never promotes a pending row, however good its facts look"
    );
}

/// A legacy record with no v2 envelope keeps whatever status its own path gave it.
#[test]
fn h4f_a_legacy_row_keeps_its_own_status() {
    let world = world("legacy");
    world.store(
        "passed",
        Some(legacy_finding(&world.bound_path(), &world.version_id)),
    );
    assert_eq!(
        world.checklist_status(),
        "passed",
        "no envelope means the v2 rule does not apply; historical v1 rows are not \
         mass-downgraded"
    );
    assert_eq!(
        world.artifact_status(),
        "passed",
        "the artifact view treats a legacy row the same way"
    );
}

// ---------------------------------------------------------------------------
// RETURN-2: the artifact view joins the same rule, and identity still outranks it
// ---------------------------------------------------------------------------

/// A contradicted pass is withheld by the artifact view too — the bypass H found
/// statically, now verified dynamically through `artifact_for_conversation`.
#[test]
fn h4f_the_artifact_view_withholds_a_contradicted_pass() {
    let world = world("artifact-contradicted");
    world.store(
        "passed",
        Some(contradicted_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_ne!(
        world.artifact_status(),
        "passed",
        "the artifact view must not report a pass the checklist projection withheld"
    );
}

/// A verdict naming a **different** managed version is an identity failure: it
/// must report `stale` even when its own facts are a legitimate pass, so the fact
/// guard can never upgrade a version mismatch into a pass.
#[test]
fn h4f_a_version_mismatch_outranks_the_fact_guard() {
    let world = world("version-mismatch");
    // A legitimate pass, but bound to some other version id.
    world.store(
        "passed",
        Some(legitimate_pass(
            &world.item_key,
            &world.bound_path(),
            "some-other-version",
        )),
    );
    assert_eq!(
        world.artifact_status(),
        "stale",
        "the verdict is about another version, so it cannot testify about this one"
    );
    // The checklist projection is version-agnostic: it reports the row it holds.
    assert_eq!(world.checklist_status(), "passed");
}

/// A **real** version change still reports `stale`, and the fact guard does not
/// override it: identity failures keep their priority.
#[test]
fn h4f_a_real_version_drift_still_reports_stale() {
    let world = world("stale");
    // A legitimate pass, then the bytes on disk change away from `after_hash`.
    world.store(
        "passed",
        Some(legitimate_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_eq!(world.artifact_status(), "passed");
    std::fs::write(&world.path, br#"{"ok":false}"#).unwrap();
    assert_eq!(
        world.artifact_status(),
        "stale",
        "the artifact changed since the recorded version, so the old pass cannot \
         testify about the current bytes"
    );
    // The checklist was never about the bytes on disk, so it keeps its stored
    // verdict; the identity check lives in the artifact view.
    assert_eq!(world.checklist_status(), "passed");
}

/// A legitimate `failed` is never demoted to `stale` by this change.
#[test]
fn h4f_a_legitimate_failed_is_not_demoted_to_stale() {
    let world = world("failed-not-stale");
    world.store(
        "failed",
        Some(definite_non_pass(
            &world.item_key,
            "refused",
            &world.bound_path(),
            &world.version_id,
        )),
    );
    let status = world.artifact_status();
    assert_eq!(
        status, "failed",
        "a legitimate failure must stay a failure, not become an identity problem"
    );
    assert_ne!(status, "stale");
    assert_ne!(status, "unavailable");
}

/// The two views agree for every row of the frozen matrix, and the agreement
/// survives a real database reopen.
#[test]
fn h4f_the_two_views_agree_across_every_recorded_status() {
    let world = world("matrix");
    let key = world.item_key.clone();
    let path = world.bound_path();
    let version = world.version_id.clone();
    let cases: Vec<(&str, &str, Option<serde_json::Value>, bool)> = vec![
        (
            "passed",
            "legitimate pass",
            Some(legitimate_pass(&key, &path, &version)),
            true,
        ),
        (
            "passed",
            "contradicted pass",
            Some(contradicted_pass(&key, &path, &version)),
            false,
        ),
        (
            "failed",
            "refused",
            Some(definite_non_pass(&key, "refused", &path, &version)),
            false,
        ),
        (
            "failed",
            "invalid",
            Some(definite_non_pass(&key, "invalid", &path, &version)),
            false,
        ),
        (
            "failed",
            "definiteFailure",
            Some(definite_non_pass(&key, "definiteFailure", &path, &version)),
            false,
        ),
        (
            "failed",
            "repairNeeded",
            Some(repair_needed(&key, &path, &version)),
            false,
        ),
        (
            "pending",
            "pending",
            Some(legitimate_pass(&key, &path, &version)),
            false,
        ),
        ("passed", "legacy", Some(legacy_finding(&path, &version)), true),
    ];
    for (stored, label, finding, should_pass) in &cases {
        world.store(stored, finding.clone());
        let checklist = world.checklist_status();
        let artifact = world.artifact_status();
        assert_eq!(
            checklist == "passed",
            artifact == "passed",
            "`{label}`: the two views must agree on the pass question \
             (checklist={checklist}, artifact={artifact})"
        );
        assert_eq!(
            checklist == "passed",
            *should_pass,
            "`{label}`: expected pass={should_pass}, checklist={checklist}"
        );
    }

    // Re-open the database and re-assert: the decision is durable, not cached.
    let reopened = Database::open(world.root.join("facts.db")).unwrap();
    for (stored, label, finding, should_pass) in &cases {
        world.store(stored, finding.clone());
        let checklist = reopened
            .load_conversation(&world.conversation_id)
            .unwrap()
            .delivery_checklist
            .into_iter()
            .find(|view| view.item_key == world.item_key)
            .expect("the reopened projection carries the promised item")
            .status;
        let artifact = reopened
            .artifact_for_conversation(&world.conversation_id, &world.artifact_id)
            .unwrap()
            .expect("the artifact exists after reopen")
            .delivery
            .expect("a deliverable project artifact carries a delivery view")
            .verification_status;
        assert_eq!(
            checklist == "passed",
            artifact == "passed",
            "`{label}` after reopen: the two views must still agree \
             (checklist={checklist}, artifact={artifact})"
        );
        assert_eq!(
            checklist == "passed",
            *should_pass,
            "`{label}` after reopen: expected pass={should_pass}, checklist={checklist}"
        );
    }
}
