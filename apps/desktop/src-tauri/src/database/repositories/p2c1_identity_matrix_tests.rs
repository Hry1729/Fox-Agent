//! Package 2-C1 §4a: the real-database identity/consumer matrix.
//!
//! Every case here runs against a **real on-disk SQLite database**, a real file,
//! a real `managed_file_versions` binding, and the **real** consumer entries
//! (`Database::load_conversation`, `Database::artifact_for_conversation`). No
//! in-memory schema and no internal helper stands in for a consumer.
//!
//! Rows covered: legacy in all three statuses, legal Host v2, only the envelope
//! deleted, the whole finding column deleted, an unparseable requirement column,
//! a spec that declares v2 with an illegal extra version marker, a v2 declaration
//! last among several requirements, two items with opposite contracts in one Run,
//! a contract/verdict identity conflict, and facts past the display truncation
//! point.
//!
//! The requirement column is built through the **real** `DeliveryRequirement` /
//! `RequirementKind` serializer, never by string-matching a `schemaVersion`, so
//! these fixtures cannot drift from what the identity rule actually reads.
//!
//! Two rows are the ones H's B-段 probe found wrong on the old candidate: only the
//! envelope deleted, and the whole finding column gone. For the first, the
//! checklist projection is the discriminating consumer (pre-fix it read `passed`);
//! for the second the finding column is gone entirely, so the artifact view
//! refuses independently through its own version binding while the checklist
//! projection is again the one that must be corrected. The pre-fix counterexample
//! itself was taken on the old binary and is recorded in the round's log rather
//! than asserted here, because the old binary is a different artifact.

use super::Database;
use crate::database::now_ms;

/// A task whose checklist seeds exactly one promised artifact.
const TASK: &str = "请生成 out/deliverable.json";

/// A second promised artifact path, so one Run can hold two opposite contracts.
const SECOND_ITEM: &str = "slot:json:manual";

/// The frozen `Contract` payload tag is `type`, and the requirement envelope's own
/// tag is `kind` — two different keys, which is exactly the kind of shape guessing
/// the identity rule must not do.
fn unbound_rule() -> serde_json::Value {
    serde_json::json!({"type": "unbound", "reason": "未绑定"})
}

/// A legal legacy content contract: `schemaVersion 1`, as the only real producer
/// (`delivery_content_requirements::add`) writes it.
fn legacy_spec() -> serde_json::Value {
    serde_json::json!({"schemaVersion": 1, "rule": unbound_rule()})
}

/// A legal Host v2 contract: `schemaVersion 2` plus the strict envelope, with the
/// executable rule under the **same** `rule` key the v1 payload uses.
fn host_v2_spec() -> serde_json::Value {
    serde_json::json!({
        "schemaVersion": 2,
        "envelopeVersion": 2,
        "checkerVersion": 2,
        "ruleId": "content:concat",
        "policy": {"comparison": "textSegments", "eof": "allowSingleTrailingLf"},
        "rule": unbound_rule(),
    })
}

/// Serialize one `ContentContract` requirement through the real stored types.
fn requirements_json(spec: serde_json::Value) -> String {
    requirements_json_many(vec![spec])
}

fn requirements_json_many(specs: Vec<serde_json::Value>) -> String {
    use crate::database::{DeliveryRequirement, RequirementKind};
    let requirements: Vec<DeliveryRequirement> = specs
        .into_iter()
        .enumerate()
        .map(|(index, spec)| DeliveryRequirement {
            item_key: None,
            id: format!("c-{index}"),
            kind: RequirementKind::ContentContract { spec },
            source_text: "内容契约".to_owned(),
        })
        .collect();
    serde_json::to_string(&requirements).unwrap()
}

/// One real, end-to-end delivery world on disk.
struct MatrixWorld {
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

fn world(tag: &str) -> MatrixWorld {
    let root = std::env::temp_dir().join(format!("fox-p2c1-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("out")).unwrap();
    let path = root.join("out").join("deliverable.json");
    std::fs::write(&path, br#"{"ok":true}"#).unwrap();
    let (hash, size) = crate::runtime_host::managed_files::hash_file(&path).unwrap();

    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), None)
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
        "the fixture task must promise at least one artifact"
    );
    db.seed_delivery_checklist(&run_id, &seeds, now_ms())
        .unwrap();
    assert_eq!(seeds.len(), 1, "the fixture relies on a single promised item");
    let item_key = seeds[0].item_key.clone();

    let artifact_id = "artifact-1".to_owned();
    db.with_connection(|connection| {
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

    db.with_connection(|connection| {
        connection.execute(
            "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,metadata_json,created_at,updated_at)
             VALUES ('m1',?1,?2,'user','chat',?3,'complete',1,'{}',1,1)",
            rusqlite::params![conversation.id, run_id, TASK],
        )?;
        Ok(())
    })
    .unwrap();

    MatrixWorld {
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

impl MatrixWorld {
    fn bound_path(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }

    /// Set this row's own contract column, leaving its finding and status alone.
    fn contract(&self, requirements: String) {
        self.db
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE delivery_checklist_items SET requirements_json=?1
                     WHERE run_id=?2 AND item_key=?3",
                    rusqlite::params![requirements, self.run_id, self.item_key],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// Write the durable status and finding columns directly.
    ///
    /// This is the only way to create the damaged shapes the card names: a real
    /// writer cannot produce them, because it derives the status and the finding
    /// from the same in-memory stop. Writing them here is not "writing the expected
    /// answer first" — the assertions are about what the two **readers** do with
    /// those bytes, and both are driven through their real entries below.
    fn store(&self, status: &str, finding: Option<serde_json::Value>) {
        self.db
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE delivery_checklist_items SET status=?1, finding_json=?2
                     WHERE run_id=?3 AND item_key=?4",
                    rusqlite::params![
                        status,
                        finding.map(|value| value.to_string()),
                        self.run_id,
                        self.item_key
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    /// The checklist row as the real projection reports it.
    fn checklist_status(&self) -> String {        self.db
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

    /// Assert the row's pass answer the way the card requires: the two display
    /// consumers must agree about the one row.
    fn assert_pass(&self, expected: bool, label: &str) {
        let checklist = self.checklist_status();
        let artifact = self.artifact_status();
        let window = self.db.with_connection(|connection| {
            let record = super::query_artifacts_window(connection, &self.conversation_id, 1, None)?
                .into_iter().find(|record| record.id == self.artifact_id).expect("real artifact in query window");
            Ok(record.delivery.expect("real window delivery view").verification_status)
        }).unwrap();
        assert_eq!(window == "passed", expected, "{label}: artifact query window ({window})");
        assert_eq!(
            checklist == "passed",
            expected,
            "`{label}`: checklist projection (got {checklist})"
        );
        assert_eq!(
            artifact == "passed",
            expected,
            "`{label}`: artifact view (got {artifact})"
        );
    }
}

/// A placeholder digest for a `candidate` fact's `sha256`. The decoder requires
/// one, so a fact without it would decode as `Corrupt` and make every case look
/// like an envelope problem; the value is irrelevant to the pass rule.
const FACT_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The durable finding shape the real writer produces: the item's own binding keys
/// at the top level plus the self-contained v2 verdict envelope.
fn finding(envelope: serde_json::Value, path: &str, version_id: &str) -> serde_json::Value {
    serde_json::json!({
        "boundPath": path,
        "managedVersionId": version_id,
        "reason": null,
        "deliveryVerdict": envelope
    })
}

/// A well-formed v2 envelope declaring `class`, with the given facts.
fn envelope(item_key: &str, class: &str, facts: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "verdictSchemaVersion": 2,
        "verdictEnvelopeVersion": 2,
        "itemKey": item_key,
        "class": class,
        "aggregateClass": class,
        "itemCount": 1,
        "terminal": matches!(class, "confirmed" | "refused" | "invalid" | "definiteFailure"),
        "repairBudgetRemaining": true,
        "facts": facts
    })
}

fn passed_fact(path: &str) -> serde_json::Value {
    serde_json::json!({
        "class": "passed", "origin": "candidate", "ruleId": "content",
        "reasonCode": "content_matches", "path": path, "sha256": FACT_SHA,
        "blocking": false, "needsRepair": false
    })
}

/// A legitimate pass: a well-formed `confirmed` envelope with a supporting fact.
fn legitimate_pass(item_key: &str, path: &str, version_id: &str) -> serde_json::Value {
    finding(
        envelope(item_key, "confirmed", vec![passed_fact(path)]),
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

// ---------------------------------------------------------------------------
// Legacy rows: the three statuses keep their own status
// ---------------------------------------------------------------------------

/// With a legal legacy contract, the historical statuses must survive untouched.
/// This is the no-mass-downgrade guarantee, and the row most at risk from an
/// over-eager identity rule.
#[test]
fn m_legacy_rows_in_all_three_statuses_keep_their_own_status() {
    let world = world("legacy-three");
    world.contract(requirements_json(legacy_spec()));
    for (stored, expected_pass) in [("passed", true), ("failed", false), ("pending", false)] {
        world.store(stored, Some(legacy_finding(&world.bound_path(), &world.version_id)));
        world.assert_pass(expected_pass, &format!("legacy contract, stored `{stored}`"));
    }
}

/// The column's own `DEFAULT '[]'` is legal history, not damage: a `passed` legacy
/// row must not be withdrawn by an empty contract.
#[test]
fn m_an_empty_requirement_column_is_a_legal_legacy_row() {
    let world = world("legacy-empty");
    world.contract("[]".to_owned());
    world.store("passed", Some(legacy_finding(&world.bound_path(), &world.version_id)));
    world.assert_pass(true, "empty requirement column");
}

// ---------------------------------------------------------------------------
// Legal Host v2: the row that is allowed to keep its pass
// ---------------------------------------------------------------------------

/// A legal Host v2 contract beside a complete, trustworthy verdict keeps its pass
/// — and this is the positive case proving the suite reaches the real `passed`
/// branch instead of reporting `stale`/`unavailable` for every row.
#[test]
fn m_a_legal_host_v2_row_with_a_sound_verdict_keeps_its_pass() {
    let world = world("hostv2-pass");
    world.contract(requirements_json(host_v2_spec()));
    world.store(
        "passed",
        Some(legitimate_pass(&world.item_key, &world.bound_path(), &world.version_id)),
    );
    world.assert_pass(true, "legal Host v2 with a sound verdict");
}

/// The pre-fix defect, case 1: only the **envelope** is deleted. The contract
/// column still says this row owes a v2 record, so the pass may not stand.
///
/// On the old candidate this row kept reading `passed` from the checklist
/// projection, because the finding column alone cannot tell it apart from a legal
/// legacy row.
#[test]
fn m_a_v2_row_that_lost_only_its_envelope_cannot_keep_its_pass() {
    let world = world("lost-envelope");
    world.contract(requirements_json(host_v2_spec()));
    // The binding keys survive; only `deliveryVerdict` is gone.
    world.store(
        "passed",
        Some(serde_json::json!({
            "boundPath": world.bound_path(),
            "managedVersionId": world.version_id,
            "reason": null
        })),
    );
    assert_eq!(
        world.checklist_status(),
        "pending",
        "an unconfirmable pass becomes the existing non-terminal `pending`, not an invented `failed`"
    );
    assert_eq!(
        world.artifact_status(),
        "unverified",
        "the artifact view withholds the same pass on the same row"
    );
}

/// The pre-fix defect, case 2: the **whole finding column** is gone (`NULL`).
///
/// The contract still identifies the row as v2, so the declared pass cannot be
/// confirmed. The checklist projection is the discriminating consumer here — with
/// the binding column empty the artifact view refuses independently through its own
/// version comparison, which is why the assertion is on the checklist status.
#[test]
fn m_a_v2_row_whose_whole_finding_column_is_deleted_cannot_keep_its_pass() {
    let world = world("lost-column");
    world.contract(requirements_json(host_v2_spec()));
    world.store("passed", None);
    assert_eq!(
        world.checklist_status(),
        "pending",
        "a v2 row with no record at all cannot keep its declared pass"
    );
    assert_ne!(
        world.artifact_status(),
        "passed",
        "the artifact view never shows a pass without a bound record"
    );
}

// ---------------------------------------------------------------------------
// Damage that must not read as legal v1
// ---------------------------------------------------------------------------

/// Unparseable requirement bytes are `Indeterminate`, never "legal v1". The card
/// forbids defaulting corruption to the permissive reading.
///
/// The finding here is envelope-less on purpose: a **present** decodable envelope
/// decides its own pass question before the identity is consulted (see
/// `m_a_corrupt_contract_beside_a_sound_envelope_still_keeps_its_pass`), so a sound
/// envelope would mask the very classification this test is about.
#[test]
fn m_unparseable_requirements_without_a_usable_record_never_default_to_a_legal_pass() {
    let world = world("bad-json");
    world.db
        .with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET requirements_json='{not json'
                 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![world.run_id, world.item_key],
            )?;
            Ok(())
        })
        .unwrap();
    world.store(
        "passed",
        Some(legacy_finding(&world.bound_path(), &world.version_id)),
    );
    assert_eq!(
        world.checklist_status(),
        "pending",
        "damage yields an unconfirmed status, never a definite failure"
    );
    assert_ne!(world.artifact_status(), "passed");

    // The contrast that makes this a statement about corruption rather than about
    // the guard being trigger-happy: the legal legacy bytes keep the same pass.
    // `world` is shadowed by the binding above, so the helper is named explicitly.
    let control = self::world("bad-json-control");
    control.contract(requirements_json(legacy_spec()));
    control.store(
        "passed",
        Some(legacy_finding(&control.bound_path(), &control.version_id)),
    );
    control.assert_pass(true, "the same row with legal legacy bytes");
}

/// A v2 spec that declares **the selector** but carries an illegal envelope marker
/// is `Invalid` under the strict Host decode, so it is not a legal v1 row either.
///
/// Envelope-less finding, for the reason given above: the illegal declaration has to
/// reach the identity to be judged, and a decodable envelope would decide first.
#[test]
fn m_a_v2_spec_with_an_illegal_envelope_marker_is_not_legal_v1() {
    let world = world("illegal-envelope");
    let mut spec = host_v2_spec();
    spec["envelopeVersion"] = serde_json::json!(1);
    world.contract(requirements_json(spec));
    world.store(
        "passed",
        Some(legacy_finding(&world.bound_path(), &world.version_id)),
    );
    world.assert_pass(false, "v2 spec declaring envelopeVersion 1");
}

/// The same for an illegal checker marker: the row declares v2 in its selector and
/// fails the strict marker check, which is damage rather than a legacy record.
#[test]
fn m_a_v2_spec_with_an_illegal_checker_marker_is_not_legal_v1() {
    let world = world("illegal-checker");
    let mut spec = host_v2_spec();
    spec["checkerVersion"] = serde_json::json!(1);
    world.contract(requirements_json(spec));
    world.store(
        "passed",
        Some(legacy_finding(&world.bound_path(), &world.version_id)),
    );
    world.assert_pass(false, "v2 spec declaring checkerVersion 1");
}

/// The explicit boundary of the identity rule: a **present, decodable** envelope
/// governs its own pass question, so a damaged contract column beside a sound record
/// does not withdraw a pass.
///
/// This is deliberate rather than incidental. The identity is a *fallback for an
/// absent record*; letting a corrupt contract column override a self-describing
/// envelope would withdraw legitimate passes for damage that says nothing about
/// whether the work was done, and the reader must not invent a verdict from it.
#[test]
fn m_a_corrupt_contract_beside_a_sound_envelope_still_keeps_its_pass() {
    let world = world("corrupt-contract-sound-envelope");
    world.db
        .with_connection(|connection| {
            connection.execute(
                "UPDATE delivery_checklist_items SET requirements_json='{not json'
                 WHERE run_id=?1 AND item_key=?2",
                rusqlite::params![world.run_id, world.item_key],
            )?;
            Ok(())
        })
        .unwrap();
    world.store(
        "passed",
        Some(legitimate_pass(
            &world.item_key,
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(true, "corrupt contract beside a sound self-describing envelope");
}

/// The v2 declaration is **last** among several requirements. Any one requirement
/// puts the row in v2 scope — not merely the first — so a legal v1 sibling must not
/// mask it.
#[test]
fn m_a_v2_declaration_last_among_several_requirements_still_scopes_the_row() {
    let world = world("v2-last");
    world.contract(requirements_json_many(vec![legacy_spec(), host_v2_spec()]));
    world.store(
        "passed",
        Some(serde_json::json!({
            "boundPath": world.bound_path(),
            "managedVersionId": world.version_id,
            "reason": null
        })),
    );
    world.assert_pass(false, "v2 declaration last among several requirements");
}

/// Two items in one Run with opposite contracts: identity is per row, so the v2
/// item is withheld while the legacy item keeps its status.
#[test]
fn m_two_items_with_opposite_contracts_are_decided_independently() {
    let world = world("opposite");
    world.contract(requirements_json(legacy_spec()));
    world.store(
        "passed",
        Some(legacy_finding(&world.bound_path(), &world.version_id)),
    );
    world.assert_pass(true, "the legacy item keeps its pass");

    // A second row on the same Run, in v2 scope, with no sound verdict of its own.
    world
        .db
        .with_connection(|connection| {
            connection.execute(
                "INSERT INTO delivery_checklist_items
                 (run_id,item_key,target_path,artifact_id,display_name,checks_json,status,
                  finding_json,checked_at,updated_at,requirements_json)
                 VALUES (?1,?2,NULL,NULL,'second.json','[\"exists\"]','passed',NULL,1,1,?3)",
                rusqlite::params![world.run_id, SECOND_ITEM, requirements_json(host_v2_spec())],
            )?;
            Ok(())
        })
        .unwrap();

    let views = world
        .db
        .load_conversation(&world.conversation_id)
        .unwrap()
        .delivery_checklist;
    let legacy_view = views
        .iter()
        .find(|view| view.item_key == world.item_key)
        .expect("the legacy item is projected");
    let second_view = views
        .iter()
        .find(|view| view.item_key == SECOND_ITEM)
        .expect("the second item is projected");
    assert_eq!(
        legacy_view.status, "passed",
        "the legacy row's own contract keeps its pass"
    );
    assert_eq!(
        second_view.status, "pending",
        "the v2 row's pass is withheld; the two rows are decided independently"
    );
}

/// A verdict on a row whose own contract reads v1 is still governed by the
/// **present** envelope: the envelope decides the pass question in both directions
/// and the identity is not consulted. This is the conflict case, and its outcome is
/// explicit rather than a silent switch of identity — every live v2 verdict sits on
/// such a row today, so the opposite reading would mass-withdraw legitimate passes.
#[test]
fn m_a_present_envelope_governs_even_when_the_contract_reads_v1() {
    let world = world("conflict");
    world.contract(requirements_json(legacy_spec()));
    world.store(
        "passed",
        Some(legitimate_pass(&world.item_key, &world.bound_path(), &world.version_id)),
    );
    world.assert_pass(true, "present sound envelope on a v1 contract");
}

/// And the same row with a contradicting envelope: the present record still governs,
/// so the pass is withheld for the **fact** reason rather than the identity one. A
/// row cannot escape a contradicted envelope by carrying a v1 contract.
#[test]
fn m_a_contradicting_envelope_is_withheld_on_a_v1_contract_too() {
    let world = world("conflict-contradicted");
    world.contract(requirements_json(legacy_spec()));
    world.store(
        "passed",
        Some(finding(
            envelope(
                &world.item_key,
                "confirmed",
                vec![serde_json::json!({
                    "class": "failed", "origin": "candidate", "ruleId": "content",
                    "reasonCode": "sheet_names_mismatch", "path": world.bound_path(),
                    "sha256": FACT_SHA, "blocking": true, "needsRepair": false
                })],
            ),
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(false, "contradicted envelope on a v1 contract");
}

// ---------------------------------------------------------------------------
// Facts past the display truncation point
// ---------------------------------------------------------------------------

/// A compact passing fact. `checkUnbound` carries neither path nor sha256, so forty
/// of them stay inside the projection's byte budget and the binder truncates by
/// **count** — which is what makes the assertion below about the raw read rather
/// than about an over-budget omission.
fn compact_passed_fact(index: usize) -> serde_json::Value {
    serde_json::json!({
        "class": "passed", "origin": "checkUnbound", "ruleId": format!("content:{index}"),
        "blocking": false, "needsRepair": false
    })
}

/// The projection caps its visible fact list at 32 and records `factsTruncated`.
/// The consumers must decide from the **raw** durable bytes, so a pass supported by
/// a fact past the cap must still be confirmed.
#[test]
fn m_a_pass_supported_past_the_display_truncation_point_is_still_confirmed() {
    let world = world("beyond-cap");
    world.contract(requirements_json(host_v2_spec()));
    // 40 facts: the projection keeps 32, so the last one is invisible to a
    // truncated read. All of them pass, so the complete list folds to `Confirmed`.
    let facts: Vec<serde_json::Value> = (0..40).map(compact_passed_fact).collect();
    world.store(
        "passed",
        Some(finding(
            envelope(&world.item_key, "confirmed", facts),
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(true, "passing fact past the display truncation point");

    // The projection really did truncate by count, which is what makes the
    // assertion above about the raw bytes rather than about a complete projection.
    let projected = world
        .db
        .load_conversation(&world.conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == world.item_key)
        .and_then(|view| view.finding)
        .expect("the projected finding is readable");
    let verdict = &projected["deliveryVerdict"];
    assert_eq!(
        verdict["factsTruncated"],
        serde_json::json!(true),
        "the display projection truncated the fact list: {projected}"
    );
    assert_eq!(
        verdict["factCount"],
        serde_json::json!(40),
        "the projection records the true count beside the bounded list"
    );
    assert_eq!(
        verdict["facts"].as_array().map(Vec::len),
        Some(32),
        "the visible list is the bounded one"
    );
}

/// The discriminating direction: a **contradiction** sits past the cap. A truncated
/// read would see only passing facts and keep the pass; the raw read must withhold
/// it. This is the case that proves the raw bytes are what the rule decides on.
#[test]
fn m_a_contradiction_past_the_display_truncation_point_is_still_withheld() {
    let world = world("contradiction-beyond-cap");
    world.contract(requirements_json(host_v2_spec()));
    let mut facts: Vec<serde_json::Value> = (0..39).map(compact_passed_fact).collect();
    facts.push(serde_json::json!({
        "class": "failed", "origin": "checkUnbound", "ruleId": "content:last",
        "blocking": true, "needsRepair": false
    }));
    world.store(
        "passed",
        Some(finding(
            envelope(&world.item_key, "confirmed", facts),
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(false, "contradiction past the display truncation point");

    // The contradiction is genuinely invisible in the projection: it is the 40th
    // fact and the binder kept 32.
    let projected = world
        .db
        .load_conversation(&world.conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == world.item_key)
        .and_then(|view| view.finding)
        .expect("the projected finding is readable");
    let visible = projected["deliveryVerdict"]["facts"]
        .as_array()
        .expect("the bounded list is present");
    assert!(
        !visible.iter().any(|fact| fact["blocking"] == serde_json::json!(true)),
        "the blocking fact is past the cap, so a truncated read could not see it"
    );
}

// ---------------------------------------------------------------------------
// Durability
// ---------------------------------------------------------------------------

/// The decision is a read of durable columns, not a cache: re-open the database and
/// re-assert both the withdrawn pass and the kept one.
#[test]
fn m_the_identity_decision_survives_a_real_reopen() {
    let world = world("reopen");
    world.contract(requirements_json(host_v2_spec()));
    world.store("passed", None);
    assert_eq!(world.checklist_status(), "pending", "before reopen");

    let reopened = Database::open(world.root.join("facts.db")).unwrap();
    let checklist = reopened
        .load_conversation(&world.conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == world.item_key)
        .expect("the reopened projection carries the item")
        .status;
    assert_eq!(
        checklist, "pending",
        "the withdrawn pass is durable across a real reopen"
    );
    assert_ne!(
        reopened
            .artifact_for_conversation(&world.conversation_id, &world.artifact_id)
            .unwrap()
            .expect("the artifact exists after reopen")
            .delivery
            .expect("a deliverable project artifact carries a delivery view")
            .verification_status,
        "passed",
        "the artifact view agrees after reopen"
    );
}

// ---------------------------------------------------------------------------
// B. Row-level `itemKey` binding
//
// H's F-1: a decodable envelope that names another item used to govern anyway, so
// all three surfaces reported `passed` on a record that was never about this row.
// These cases keep the same file, the same hash and the same version, and differ
// only in the key — which is exactly the counterexample the card asks to preserve
// before proving the withdrawal.
// ---------------------------------------------------------------------------

/// The old defect, kept as the shape under test: everything about this record is a
/// legitimate pass except the `itemKey` inside the envelope.
#[test]
fn b_an_envelope_naming_another_item_cannot_keep_this_rows_pass() {
    let world = world("b-foreign-key");
    world.contract(requirements_json(host_v2_spec()));
    // Same path, same version, same facts as `legitimate_pass`; only the key
    // differs, so nothing but the row binding can explain the outcome.
    world.store(
        "passed",
        Some(legitimate_pass(
            "file:someone-elses.json",
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(false, "envelope names another item");

    // The control: the identical record under this row's own key does pass, so the
    // withdrawal cannot be satisfied by refusing every envelope.
    let control = self::world("b-own-key");
    control.contract(requirements_json(host_v2_spec()));
    control.store(
        "passed",
        Some(legitimate_pass(
            &control.item_key,
            &control.bound_path(),
            &control.version_id,
        )),
    );
    control.assert_pass(true, "the same record under its own key");
}

/// Two items in one Run, with opposite identities: the foreign record on item A is
/// this row's own legitimate record for item B, so the keys cannot be proven by
/// borrowing across items.
#[test]
fn b_two_items_with_opposite_keys_do_not_borrow_each_others_evidence() {
    let world = world("b-two-items");
    // Item A is written `passed` while carrying item B's envelope.
    world.contract(requirements_json(host_v2_spec()));
    world.store(
        "passed",
        Some(legitimate_pass(
            "file:b.json",
            &world.bound_path(),
            &world.version_id,
        )),
    );
    world.assert_pass(false, "item A holds item B's record");

    // The mirror: item B holds item A's record. The same pair of keys, swapped,
    // must withdraw on the other side too, so neither key is privileged.
    let mirror = self::world("b-two-items-mirror");
    mirror.contract(requirements_json(host_v2_spec()));
    mirror.store(
        "passed",
        Some(legitimate_pass(
            "file:a.json",
            &mirror.bound_path(),
            &mirror.version_id,
        )),
    );
    mirror.assert_pass(false, "item B holds item A's record");
}

/// The row binding survives a real reopen, so it is a durable reading of the
/// columns rather than a property of one in-memory projection.
#[test]
fn b_the_row_binding_survives_a_real_reopen() {
    let world = world("b-reopen");
    world.contract(requirements_json(host_v2_spec()));
    world.store(
        "passed",
        Some(legitimate_pass(
            "file:other.json",
            &world.bound_path(),
            &world.version_id,
        )),
    );
    assert_ne!(world.checklist_status(), "passed", "before reopen");

    let reopened = Database::open(world.root.join("facts.db")).unwrap();
    let checklist = reopened
        .load_conversation(&world.conversation_id)
        .unwrap()
        .delivery_checklist
        .into_iter()
        .find(|view| view.item_key == world.item_key)
        .expect("the reopened projection carries the item")
        .status;
    assert_ne!(
        checklist, "passed",
        "the foreign record is still not this row's evidence after reopen"
    );
}

/// A legal legacy row is untouched by the row binding: it carries no envelope, so
/// there is no `itemKey` to disagree with, and it keeps the status it was written
/// with. This is the guard against the binding becoming a mass downgrade.
#[test]
fn b_a_legacy_row_without_an_envelope_keeps_its_own_status() {
    let world = world("b-legacy");
    world.contract("[]".to_owned());
    for stored in ["passed", "failed", "pending"] {
        world.store(stored, None);
        assert_eq!(
            world.checklist_status(),
            stored,
            "a legacy row keeps `{stored}`: there is no envelope key to compare"
        );
    }
}

#[test]
fn b_missing_illegal_and_foreign_envelope_keys_withhold_all_real_read_entries() {
    let world=world("b-unusable-key");world.contract(requirements_json(host_v2_spec()));
    let positive=legitimate_pass(&world.item_key,&world.bound_path(),&world.version_id);
    world.store("passed",Some(positive.clone()));world.assert_pass(true,"same-binding positive control");
    for value in [serde_json::Value::Null,serde_json::json!(""),serde_json::json!(7),serde_json::json!("foreign"),serde_json::json!("bad\nkey")] {
        let mut damaged=positive.clone();damaged["deliveryVerdict"]["itemKey"]=value;
        world.store("passed",Some(damaged));world.assert_pass(false,"missing/illegal/foreign key");
        assert_eq!(world.artifact_status(),"unverified","same path/hash/version must reach qualification, never early stale");
    }
    let mut missing=positive;missing["deliveryVerdict"].as_object_mut().unwrap().remove("itemKey");
    world.store("passed",Some(missing));world.assert_pass(false,"key field removed");
}
