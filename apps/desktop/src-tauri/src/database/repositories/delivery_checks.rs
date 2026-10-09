//! Delivery checklist persistence for ordinary file-producing tasks.
//!
//! This is the business-completion ledger, distinct from the model reply-end
//! marker (`<fox-final/>`) and from the bounded continuation review. Rows
//! bind a declared target to the real artifact and to deterministic Host
//! check results; repair rounds are counted here, never in model-failure
//! retry counters.

use super::*;

fn bounded_display_text(value: &str, limit: usize) -> String {
    value.chars().filter(|c| !c.is_control() || matches!(c,'\n'|'\t')).take(limit).collect()
}

fn safe_finding(value: &Value) -> Option<Value> {
    let source=value.as_object()?;
    let mut result=serde_json::Map::new();
    let requirements=source.get("requirementFindings").or_else(||source.get("requirements")).and_then(Value::as_array);
    let semantic=requirements.filter(|entries|!entries.is_empty()).map(|entries| {
        if entries.iter().any(|entry|matches!(entry["state"].as_str().or_else(||entry["status"].as_str()),Some("failed"|"unmet"))) {"unmet"}
        else if entries.iter().any(|entry|!matches!(entry["state"].as_str().or_else(||entry["status"].as_str()),Some("passed"|"met"))) {"unverified"}
        else {"met"}
    });
    let mut requirements_truncated=requirements.is_some_and(|entries|entries.len()>16);
    for key in ["reasonCode","verificationStatus","semanticStatus","reason","expectedTarget","targetDirectory",
        "boundPath","managedVersionId","managedVersionNo","sourceRunId","sourceToolCallId","checkedHash","provenance","cancelled",
        "actualWrittenPaths","requirementFindings"] {
        let value=source.get(key).or_else(||(key=="requirementFindings").then(||source.get("requirements")).flatten());
        let Some(value)=value else {continue;};
        let projected=match value {
            Value::String(text)=>Some(Value::String(bounded_display_text(text,if key=="reason" {1536}else{512}))),
            Value::Number(number) if key=="managedVersionNo"=>Some(Value::Number(number.clone())),
            Value::Bool(value) if key=="cancelled"=>Some(Value::Bool(*value)),
            Value::Array(values) if key=="actualWrittenPaths"=>Some(json!(values.iter().take(8)
                .filter_map(Value::as_str).map(|s|bounded_display_text(s,512)).collect::<Vec<_>>())),
            Value::Array(values) if key=="requirementFindings"=>Some(Value::Array(values.iter().take(16).filter_map(|finding| {
                let source=finding.as_object()?;let mut entry=serde_json::Map::new();
                for field in ["id","requirementId","kind","check","status","state","reasonCode","reason","verificationStatus","sourcePath","sourceHash"] {
                    if let Some(text)=source.get(field).and_then(Value::as_str) {entry.insert(field.into(),json!(bounded_display_text(text,512)));}
                }
                (!entry.is_empty()).then_some(Value::Object(entry))
            }).collect())),
            _=>None,
        };
        if let Some(projected)=projected {
            result.insert(key.into(),projected);
            if serde_json::to_vec(&result).map(|b|b.len()>7680).unwrap_or(true) {
                result.remove(key);
                if key=="requirementFindings" {requirements_truncated=true;}
            }
        }
    }
    if let Some(semantic)=semantic {
        result.insert("semanticStatus".into(),json!(semantic));
        if semantic=="unmet" {result.insert("verificationStatus".into(),json!("failed"));}
        else if semantic=="unverified" {result.insert("verificationStatus".into(),json!("unverified"));}
    }
    if requirements_truncated {result.insert("requirementsTruncated".into(),json!(true));}
    Some(Value::Object(result))
}

/// A single latest Run, owned by this conversation. All callers use the same
/// persisted rows; absent files and unbound/non-file items remain visible.
pub(super) fn query_current_checklist_view(connection:&rusqlite::Connection,conversation:&str,run:Option<&str>)
    ->rusqlite::Result<(Vec<super::super::DeliveryChecklistView>,bool)> {
    let Some(run)=run else {return Ok((Vec::new(),false));};
    let mut statement=connection.prepare("SELECT i.item_key,i.target_path,
        (SELECT a.id FROM artifacts a WHERE a.id=i.artifact_id AND a.conversation_id=?1),
        i.display_name,i.status,i.finding_json,i.checked_at
        FROM delivery_checklist_items i JOIN runs r ON r.id=i.run_id
        WHERE r.conversation_id=?1 AND i.run_id=?2 ORDER BY i.item_key LIMIT 257")?;
    let mut rows=statement.query_map(params![conversation,run],|row| {
        let item_key:String=row.get(0)?;let target_path:Option<String>=row.get(1)?;
        let raw:Option<String>=row.get(5)?;
        let finding=raw.as_deref().map(|body|serde_json::from_str::<Value>(body).ok().and_then(|v|safe_finding(&v))
            .unwrap_or_else(||json!({"reasonCode":"delivery.invalid_finding","verificationStatus":"unverified","reason":"持久核验记录无法安全读取"})));
        let read_version=finding.as_ref().and_then(|v|v["checkedHash"].as_str()).filter(|value| {
            let digest=value.strip_prefix("sha256:").unwrap_or(value);digest.len()==64 && digest.bytes().all(|c|c.is_ascii_hexdigit())
        }).map(str::to_owned);
        let extension=target_path.as_deref().and_then(|path|std::path::Path::new(path).extension()).and_then(|s|s.to_str())
            .or_else(||item_key.strip_prefix("slot:").and_then(|rest|rest.split(':').next())).unwrap_or("").to_ascii_lowercase();
        let kind=if item_key.starts_with("recognition:") {"non_file"}else{match extension.as_str() {
            "xlsx"|"xls"|"xlsm"=>"spreadsheet","docx"|"doc"=>"word","pptx"|"ppt"=>"slides","pdf"=>"pdf",
            "csv"|"tsv"=>"csv","json"=>"json","txt"|"md"|"html"=>"text","png"|"svg"|"jpg"=>"image",_=>"unknown"}};
        let status:String=row.get(4)?;
        Ok(super::super::DeliveryChecklistView{run_id:run.into(),item_key:bounded_display_text(&item_key,2048),
            description:bounded_display_text(&row.get::<_,String>(3)?,512),artifact_kind:kind.into(),
            target_path:target_path.map(|s|bounded_display_text(&s,2048)),artifact_id:row.get(2)?,read_version,
            status:if matches!(status.as_str(),"pending"|"passed"|"failed") {status}else{"pending".into()},finding,checked_at:row.get(6)?})
    })?.collect::<rusqlite::Result<Vec<_>>>()?;
    let truncated=rows.len()>256;rows.truncate(256);Ok((rows,truncated))
}

/// One structured, machine-decidable demand taken from the task text and stored
/// with the checklist item that has to satisfy it.
///
/// Only requirements a deterministic checker can decide are stored as verified
/// checks; a demand that cannot be parsed leaves this list empty and is reported
/// as 未核验 instead of being assumed met.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeliveryRequirement {
    /// Checklist item this requirement belongs to; `None` means it spans items.
    #[serde(default)]
    pub item_key: Option<String>,
    pub id: String,
    pub kind: RequirementKind,
    /// The phrase in the task that produced this requirement.
    pub source_text: String,
}

/// What a requirement demands. Deliberately small and structural: a counted
/// number of chart parts inside the OOXML package, named sections that must
/// exist, or a ratio that must agree with a total the task itself states.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum RequirementKind {
    /// Host placement metadata for an unnamed artifact. It is not a content
    /// requirement and must not replace or erase the other requirements.
    TargetDirectory { directory: String },
    /// A real file-output intent whose complete target set was not recognized.
    /// This is a non-file state, never an invented filename to repair.
    RecognitionUnresolved { reason: String },
    /// Versioned Host-frozen content contract. The runtime consumer validates
    /// its finite schema; old semantic requirements remain separately readable.
    ContentContract { spec: serde_json::Value },
    Charts {
        count: usize,
        /// The task demanded the charts per worksheet ("每个 Sheet 配一张图表").
        /// A whole-file count cannot verify that demand, so the checker compares
        /// against the artifact's own sheet count. Defaulted so requirements
        /// stored before this field existed still load.
        #[serde(default)]
        per_sheet: bool,
    },
    Sections { names: Vec<String> },
    RatioConsistency { numerator: u64, total: u64 },
    /// A statistic that must be recomputed from the source data the task named:
    /// the row distribution of `label_header` in `source` (optionally restricted
    /// to one `sheet`). The expectation is derived from the source at check time
    /// — never from the artifact, never from the model's own summary — and the
    /// `source_sha256` recorded at bind time reports drift.
    SourceDistribution {
        source: String,
        /// Other readings of the same mention, tried in order when `source`
        /// names no existing file. Empty once the requirement is bound.
        #[serde(default)]
        alternates: Vec<String>,
        source_sha256: String,
        label_header: String,
        sheet: Option<String>,
    },
    /// The task demanded statistics but did not name both a source file and the
    /// column to aggregate, so no expectation can be derived. It is reported as
    /// 未核验 with the demand text rather than being quietly dropped.
    SourceStatsUnbound { demand: String },
    /// A machine field convention the task stated for **one** JSON artifact.
    /// `target_path` is the declared artifact the rule was bound to; it is
    /// `None` when the Host could not identify one, and the rule is then
    /// reported as 未核验 instead of being applied to unrelated JSON.
    FieldConvention {
        #[serde(default)]
        target_path: Option<String>,
        marker_field: String,
        marker_value: String,
        evidence_field: String,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct DeliveryChecklistSeed {
    pub item_key: String,
    pub target_path: Option<String>,
    pub artifact_id: Option<String>,
    pub display_name: String,
    /// Planned deterministic checks, e.g. ["exists","parseable","stats"].
    pub checks: Vec<String>,
    /// Structured demands this item has to satisfy, derived from the task text
    /// by the Host. Empty means the task stated none in a machine-decidable
    /// form; the delivery report must then mark those demands unverified rather
    /// than passed.
    pub requirements: Vec<DeliveryRequirement>,
}

/// One Host-verified generated file recorded by a settled tool result.
#[derive(Debug, Clone)]
pub(crate) struct DeliveryArtifactRow {
    pub id: String,
    pub storage_path: String,
    pub sha256: Option<String>,
    pub created_at: i64,
}

/// One computed verdict, before the round decision that consumes it commits.
///
/// Staged rows are the durable hand-off between "the delivery gate decided this"
/// and "the Run's decision is durable": they let the ledger write happen on
/// either side of a crash without ever producing a completed Run with pending
/// business rows.
#[derive(Debug, Clone)]
pub(crate) struct StagedDeliveryItem {
    pub item_key: String,
    pub passed: bool,
    pub finding_json: String,
}

#[derive(Debug, Clone)]
pub(crate) struct DeliveryChecklistItem {
    pub item_key: String,
    pub target_path: Option<String>,
    pub artifact_id: Option<String>,
    pub display_name: String,
    pub checks: Vec<String>,
    pub status: String,
    pub finding: Option<String>,
    pub checked_at: Option<i64>,
    pub updated_at: i64,
    /// The interrupted Run this requirement was inherited from, when this Run
    /// resumes an earlier task of the same conversation.
    pub inherited_from_run_id: Option<String>,
}

/// One managed write this Run committed to a path, as recorded by the Host's own
/// file-version ledger. This is the write *receipt*: it names the tool that
/// committed the bytes and the hash they had afterwards, so a delivery check can
/// prove the current content came from this Run instead of trusting a
/// modification time.
#[derive(Debug, Clone)]
pub(crate) struct RunWriteReceipt {
    pub version_id: String,
    pub version_no: i64,
    pub source_run_id: String,
    pub tool_call_id: Option<String>,
    pub storage_path: String,
    pub change_kind: String,
    pub tool: String,
    pub after_hash: Option<String>,
    pub created_at: i64,
}

/// Test-only fault injection for the second phase of the delivery write.
///
/// The window this models is real: the round decision is durable, the process
/// dies before the staged verdicts become final. It is thread-local so parallel
/// tests cannot inject into each other, and it is absent from production builds.
#[cfg(test)]
pub(crate) mod finalize_fault {
    use std::cell::Cell;

    thread_local! {
        /// Abort the finalize after this many item updates.
        static ABORT_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
        static DONE: Cell<usize> = const { Cell::new(0) };
    }

    /// Arm the fault: the next finalize aborts once `count` updates were applied,
    /// exactly like a process death inside the transaction (which therefore
    /// rolls back as a whole — the caller only observes the abort).
    pub(crate) fn abort_after(count: usize) {
        ABORT_AFTER.with(|value| value.set(Some(count)));
        DONE.with(|value| value.set(0));
    }

    pub(crate) fn disarm() {
        ABORT_AFTER.with(|value| value.set(None));
        DONE.with(|value| value.set(0));
    }

    /// Called before each item update; returns true when this update must abort.
    pub(crate) fn should_abort() -> bool {
        DONE.with(|done| {
            let applied = done.get() + 1;
            done.set(applied);
            ABORT_AFTER.with(|limit| limit.get().is_some_and(|limit| applied > limit))
        })
    }
}

/// Test-only fault injection for the first phase of the delivery write: the
/// whole stage batch must be one transaction, so an abort at item N leaves no
/// partial stage behind.
#[cfg(test)]
pub(crate) mod stage_fault {
    use std::cell::Cell;

    thread_local! {
        static ABORT_AFTER: Cell<Option<usize>> = const { Cell::new(None) };
        static DONE: Cell<usize> = const { Cell::new(0) };
    }

    pub(crate) fn abort_after(count: usize) {
        ABORT_AFTER.with(|value| value.set(Some(count)));
        DONE.with(|value| value.set(0));
    }

    pub(crate) fn disarm() {
        ABORT_AFTER.with(|value| value.set(None));
        DONE.with(|value| value.set(0));
    }

    pub(crate) fn should_abort() -> bool {
        DONE.with(|done| {
            let applied = done.get() + 1;
            done.set(applied);
            ABORT_AFTER.with(|limit| limit.get().is_some_and(|limit| applied > limit))
        })
    }
}

/// One artifact's comparable path: separators unified, the Windows verbatim prefix
/// removed, case folded (Windows paths are case-insensitive), `./` dropped.
fn normalize_delivery_path(named: &str) -> String {
    let unified = named.trim().replace('\\', "/");
    let without_prefix = unified
        .strip_prefix("//?/")
        .or_else(|| unified.strip_prefix("\\?\\"))
        .unwrap_or(&unified);
    let trimmed = without_prefix.trim_start_matches("./");
    let mut normalized = trimmed.to_ascii_lowercase();
    while normalized.contains("//") {
        normalized = normalized.replace("//", "/");
    }
    normalized.trim_end_matches('/').to_owned()
}

/// True when two normalized artifact paths name the same file: equal, or one is the
/// other's path suffix ("summary.md" is the same artifact as "out/summary.md", while
/// "out/progress.csv" stays distinct).
fn same_delivery_artifact(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    if left == right {
        return true;
    }
    let (short, long) = if left.len() <= right.len() { (left, right) } else { (right, left) };
    long.ends_with(short) && long.as_bytes()[long.len() - short.len() - 1] == b'/'
}

/// Cancellation is not a verification verdict. Preserve pending and checked_at
/// while attaching a reason to every item which still needs verification.
pub(super) fn cancelled_unverified(transaction: &rusqlite::Transaction<'_>, run_id: &str, now: i64) -> rusqlite::Result<()> {
    transaction.execute("UPDATE delivery_checklist_items SET finding_json=?2, updated_at=?3
        WHERE run_id=?1 AND status='pending' AND checked_at IS NULL AND finding_json IS NULL",
        params![run_id, serde_json::json!({"reasonCode":"delivery.cancelled_unverified",
            "reason":"运行已取消，此交付项未完成核验","verificationStatus":"unverified","cancelled":true}).to_string(), now])?;
    Ok(())
}

impl Database {
    /// Read per-item placement metadata without Run-wide semantic de-duplication.
    pub(crate) fn delivery_item_requirements(&self,run_id:&str,item_key:&str)->Result<Vec<DeliveryRequirement>,String> {
        self.with_connection(|conn| {
            let body:String=conn.query_row("SELECT requirements_json FROM delivery_checklist_items WHERE run_id=?1 AND item_key=?2",
                params![run_id,item_key],|row|row.get(0))?;
            serde_json::from_str(&body).map_err(|_|rusqlite::Error::InvalidParameterName("invalid frozen delivery requirements".into()))
        })
    }
    /// An unnamed artifact is bound to the immediate frozen output directory,
    /// never another directory with a similarly named or same-type file.
    pub(crate) fn delivery_target_matches_directory(target:&str,directory:&str)->bool {
        let target=target.replace('\\',"/");let directory=directory.replace('\\',"/");
        let directory=directory.trim_end_matches('/');
        let safe=|value:&str|!value.starts_with('/') && !value.contains(':') && value.split('/').all(|part|!part.is_empty() && part!="." && part!="..");
        if !safe(&target) || directory!="." && !safe(directory) {return false;}
        let parent=target.rsplit_once('/').map(|(parent,_)|parent).unwrap_or("");
        parent.eq_ignore_ascii_case(if directory=="." {""} else {directory})
    }
    /// Insert checklist items that were not planned yet. Existing items (a
    /// repaired run may re-check the same target) keep their identity.
    pub(crate) fn seed_delivery_checklist(
        &self,
        run_id: &str,
        seeds: &[DeliveryChecklistSeed],
        now: i64,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            let transaction=conn.unchecked_transaction()?;
            for seed in seeds {
                transaction.execute(
                    "INSERT OR IGNORE INTO delivery_checklist_items(
                        run_id, item_key, target_path, artifact_id, display_name,
                        checks_json, requirements_json, status, updated_at)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8)",
                    params![
                        run_id,
                        seed.item_key,
                        seed.target_path,
                        seed.artifact_id,
                        seed.display_name,
                        serde_json::to_string(&seed.checks).unwrap_or_else(|_| "[]".into()),
                        serde_json::to_string(&seed.requirements).unwrap_or_else(|_| "[]".into()),
                        now,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// The structured requirements of a Run's delivery checklist.
    ///
    /// Read back at the stop gate so the requirements that were SEEDED from the
    /// task are exactly the ones verified — a task edit or a later heuristic
    /// change can never retroactively alter what an in-flight Run promised.
    pub(crate) fn delivery_requirements(
        &self,
        run_id: &str,
    ) -> Result<Vec<DeliveryRequirement>, String> {        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT requirements_json FROM delivery_checklist_items
                 WHERE run_id=?1 ORDER BY item_key",
            )?;
            let rows = stmt
                .query_map(params![run_id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut requirements = Vec::new();
            for body in rows {
                let stored: Vec<DeliveryRequirement> =
                    serde_json::from_str(&body).unwrap_or_default();
                for mut item in stored {
                    // Placement and recognition belong to their original item,
                    // not to the de-duplicated Run-wide semantic demands.
                    if matches!(item.kind,RequirementKind::TargetDirectory{..}|RequirementKind::RecognitionUnresolved{..}|RequirementKind::ContentContract{..}) {continue;}
                    item.item_key = None;
                    if !requirements
                        .iter()
                        .any(|existing: &DeliveryRequirement| existing.id == item.id)
                    {
                        requirements.push(item);
                    }
                }
            }
            Ok(requirements)
        })
    }

    /// The immediately preceding Run of this conversation that ended without a
    /// delivery verdict, when it promised deliverables.
    ///
    /// This is what a user-authored resume continues: the same conversation (and
    /// therefore the same project root) and nothing older. A Run that completed its
    /// delivery — or that promised nothing — is not a resume source.
    pub(crate) fn previous_unfinished_run(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT r.id FROM runs r
                  WHERE r.conversation_id = ?1
                    AND r.id <> ?2
                    AND r.status IN ('cancelled', 'failed', 'interrupted')
                    AND EXISTS(
                          SELECT 1 FROM delivery_checklist_items i WHERE i.run_id = r.id)
                  ORDER BY r.created_at DESC, r.rowid DESC
                  LIMIT 1",
                params![conversation_id, run_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }

    /// Copy a previous Run's promised deliverables into this Run as **new, pending**
    /// requirements, recording where they came from.
    ///
    /// Deliberately not copied: `artifact_id` (the old artifact belongs to the old
    /// Run; this Run must verify the current file version), the old status, the old
    /// finding and `checked_at`. `INSERT OR IGNORE` on (run_id, item_key) is what
    /// keeps re-seeding or a recovery from duplicating a row.
    pub(crate) fn inherit_delivery_checklist(
        &self,
        run_id: &str,
        source_run_id: &str,
        now: i64,
    ) -> Result<usize, String> {
        self.with_connection(|conn| {
            // The declared spellings differ between rounds ("summary.md" in the
            // interrupted Run, "file:out%2fsummary.md" here), so key equality alone
            // left the same file in the checklist twice. Compare normalized paths and
            // treat one as the same artifact when it is the other's path suffix.
            let mut current: Vec<(String, String)> = Vec::new();
            {
                let mut statement = conn.prepare(
                    "SELECT item_key, COALESCE(target_path, display_name)
                       FROM delivery_checklist_items WHERE run_id = ?1",
                )?;
                let rows = statement.query_map(params![run_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?;
                for row in rows {
                    current.push(row?);
                }
            }
            let mut source: Vec<(String, Option<String>, String, String, String, String)> = Vec::new();
            {
                let mut statement = conn.prepare(
                    "SELECT item_key, target_path, display_name, checks_json,
                            requirements_json, COALESCE(target_path, display_name)
                       FROM delivery_checklist_items WHERE run_id = ?1",
                )?;
                let rows = statement.query_map(params![source_run_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?;
                for row in rows {
                    source.push(row?);
                }
            }
            let mut inserted = 0usize;
            for (item_key, target_path, display_name, checks, requirements, named) in source {
                let candidate = normalize_delivery_path(&named);
                let duplicate = current.iter().any(|(existing_key, existing_named)| {
                    existing_key == &item_key
                        || same_delivery_artifact(&normalize_delivery_path(existing_named), &candidate)
                });
                if duplicate {
                    continue;
                }
                let changed = conn.execute(
                    "INSERT OR IGNORE INTO delivery_checklist_items(
                         run_id, item_key, target_path, artifact_id, display_name,
                         checks_json, requirements_json, status, updated_at,
                         inherited_from_run_id)
                     VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, 'pending', ?7, ?8)",
                    params![run_id, item_key, target_path, display_name, checks, requirements, now, source_run_id],
                )?;
                inserted += changed;
                current.push((item_key, named));
            }
            Ok(inserted)
        })
    }

    /// Link this Run to the interrupted task it resumes.
    ///
    /// The link scopes delivery *evidence* (managed write receipts) to this task, so
    /// an inherited requirement can be verified against the file the task itself
    /// wrote earlier. It never widens write permission: the write gate derives
    /// read-only inputs from the current task text, not from this link.
    pub(crate) fn link_resumed_task(
        &self,
        run_id: &str,
        source_run_id: &str,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            conn.execute(
                "UPDATE kernel_runs SET continued_from_run_id = ?2
                  WHERE run_id = ?1 AND continued_from_run_id IS NULL",
                params![run_id, source_run_id],
            )?;
            Ok(())
        })
    }

    /// The Run this one continues, when this Run is a continuation.
    ///
    /// Delivery evidence (managed write receipts) is scoped to the Run that
    /// committed it, while a continuation re-verifies its parent's checklist
    /// under a new run id. Following this link is what lets a continuation keep
    /// accepting its own task's earlier writes **and** keeps every other Run's
    /// writes out.
    pub(crate) fn continued_from_run_id(&self, run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT continued_from_run_id FROM kernel_runs WHERE run_id=?1",
                params![run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(Into::into)
        })
    }

    /// Stage one round's delivery verdicts **before** the decision that consumes
    /// them commits, bound to that decision's mark.
    ///
    /// The verdict for a checklist row is computed from durable facts and the
    /// filesystem at the model stop. Staging it closes the window between "the
    /// decision is durable" and "the verdicts are durable" **without** letting an
    /// uncommitted decision be finalized later: recovery only acts on a stage
    /// whose mark is present in `kernel_decision_marks`, and a mark is written by
    /// the decision's own transaction.
    ///
    /// The whole batch is one transaction — a failure partway through leaves no
    /// partial stage — and it replaces any previous stage of the same Run, since
    /// only the newest round's verdicts can still be waiting for a decision.
    pub(crate) fn stage_delivery_outcome(
        &self,
        run_id: &str,
        decision_mark: &str,
        items: &[StagedDeliveryItem],
        findings_json: Option<&str>,
        now: i64,
    ) -> Result<(), String> {
        if decision_mark.trim().is_empty() {
            return Err("delivery stage requires a non-empty decision mark".into());
        }
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM delivery_outcome_stage WHERE run_id=?1",
                params![run_id],
            )?;
            transaction.execute(
                "DELETE FROM delivery_repair_stage WHERE run_id=?1",
                params![run_id],
            )?;
            for item in items {
                #[cfg(test)]
                if stage_fault::should_abort() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                transaction.execute(
                    "INSERT INTO delivery_outcome_stage(
                        run_id, item_key, passed, finding_json, checked_at, updated_at, decision_mark)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
                    params![
                        run_id,
                        item.item_key,
                        item.passed as i64,
                        item.finding_json,
                        now,
                        decision_mark,
                    ],
                )?;
            }
            if let Some(findings) = findings_json {
                transaction.execute(
                    "INSERT INTO delivery_repair_stage(run_id, findings_json, staged_at, decision_mark)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![run_id, findings, now, decision_mark],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })
    }

    /// Drop a staged round that must never be finalized: the decision it belonged
    /// to was superseded (steering took the round over) or its commit failed.
    ///
    /// Scoped by mark so a newer round's stage is never withdrawn by a stale
    /// caller, and idempotent.
    pub(crate) fn withdraw_staged_delivery_outcome(
        &self,
        run_id: &str,
        decision_mark: &str,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            conn.execute(
                "DELETE FROM delivery_outcome_stage WHERE run_id=?1 AND decision_mark=?2",
                params![run_id, decision_mark],
            )?;
            conn.execute(
                "DELETE FROM delivery_repair_stage WHERE run_id=?1 AND decision_mark=?2",
                params![run_id, decision_mark],
            )?;
            Ok(())
        })
    }

    /// True when the given decision mark was committed with its decision.
    pub(crate) fn decision_mark_committed(
        &self,
        run_id: &str,
        decision_mark: &str,
    ) -> Result<bool, String> {
        self.with_connection(|conn| {
            Ok(conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM kernel_decision_marks WHERE run_id=?1 AND mark=?2)",
                params![run_id, decision_mark],
                |row| row.get::<_, bool>(0),
            )?)
        })
    }

    /// Test-only stand-in for the decision commit's mark write.
    ///
    /// In production the mark is written by `kernel_commit_*` inside the
    /// decision's own transaction. Tests that drive the delivery ledger without a
    /// real round commit call this to state "the decision that owns this mark
    /// committed", so the gate under test is the production one.
    #[cfg(test)]
    pub(crate) fn record_decision_mark_for_test(
        &self,
        run_id: &str,
        decision_mark: &str,
        now: i64,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO kernel_decision_marks(run_id, mark, created_at)
                 VALUES (?1, ?2, ?3)",
                params![run_id, decision_mark, now],
            )?;
            Ok(())
        })
    }

    /// Staged rounds still waiting, as `(run_id, decision_mark)`.
    ///
    /// A lingering stage row means the second phase never ran. Whether it may be
    /// finalized is decided by [`Self::decision_mark_committed`], never by the
    /// row's presence alone.
    pub(crate) fn staged_delivery_rounds(&self) -> Result<Vec<(String, String)>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT run_id, decision_mark FROM delivery_outcome_stage
                 GROUP BY run_id, decision_mark
                 UNION
                 SELECT run_id, decision_mark FROM delivery_repair_stage
                 GROUP BY run_id, decision_mark
                 ORDER BY 1, 2",
            )?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Finalize one round's staged verdicts: the item outcomes, their findings
    /// and (for a repair round) the repair ledger row become durable together.
    ///
    /// The write is gated **inside this transaction** on the decision mark having
    /// been committed, and scoped to that mark, so:
    /// * a stage whose decision never committed (crash, cancellation, superseded
    ///   round) writes nothing — the caller then withdraws it;
    /// * a newer round's stage is never finalized by an older round's caller;
    /// * re-running it is a no-op: rows already carrying the verdict, and an
    ///   existing repair round, are left alone.
    pub(crate) fn finalize_staged_delivery_outcome(
        &self,
        run_id: &str,
        decision_mark: &str,
        now: i64,
    ) -> Result<usize, String> {
        if decision_mark.trim().is_empty() {
            return Err("delivery finalize requires a non-empty decision mark".into());
        }
        self.with_connection(|conn| {
            let transaction = conn.unchecked_transaction()?;
            let committed: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM kernel_decision_marks WHERE run_id=?1 AND mark=?2)",
                params![run_id, decision_mark],
                |row| row.get(0),
            )?;
            if !committed {
                // Nothing to finalize: the decision this stage belongs to never
                // landed. The caller decides whether to withdraw the stage.
                transaction.rollback()?;
                return Ok(0);
            }
            let staged: Vec<(String, i64, String)> = {
                let mut stmt = transaction.prepare(
                    "SELECT item_key, passed, finding_json FROM delivery_outcome_stage
                     WHERE run_id=?1 AND decision_mark=?2 ORDER BY item_key",
                )?;
                let rows = stmt
                    .query_map(params![run_id, decision_mark], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows
            };
            for (item_key, passed, finding) in &staged {
                #[cfg(test)]
                if finalize_fault::should_abort() {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                let status = if *passed != 0 { "passed" } else { "failed" };
                // Only a row that has not already received this exact verdict is
                // written, so `checked_at` keeps the instant the verdict was
                // really decided and a replayed finalizer writes nothing.
                transaction.execute(
                    "UPDATE delivery_checklist_items
                     SET status=?1, finding_json=?2, checked_at=?3, updated_at=?3
                     WHERE run_id=?4 AND item_key=?5
                       AND (status<>?1 OR finding_json IS NOT ?2)",
                    params![status, finding, now, run_id, item_key],
                )?;
            }
            let findings: Option<String> = transaction
                .query_row(
                    "SELECT findings_json FROM delivery_repair_stage
                     WHERE run_id=?1 AND decision_mark=?2",
                    params![run_id, decision_mark],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(findings) = &findings {
                transaction.execute(
                    "INSERT OR IGNORE INTO delivery_repair_rounds(run_id, round, findings_json, created_at)
                     SELECT ?1, COALESCE(MAX(round),0)+1, ?2, ?3 FROM delivery_repair_rounds WHERE run_id=?1",
                    params![run_id, findings, now],
                )?;
            }
            transaction.execute(
                "DELETE FROM delivery_outcome_stage WHERE run_id=?1 AND decision_mark=?2",
                params![run_id, decision_mark],
            )?;
            transaction.execute(
                "DELETE FROM delivery_repair_stage WHERE run_id=?1 AND decision_mark=?2",
                params![run_id, decision_mark],
            )?;
            transaction.commit()?;
            Ok(staged.len())
        })
    }

    pub(crate) fn delivery_checklist(
        &self,
        run_id: &str,
    ) -> Result<Vec<DeliveryChecklistItem>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT item_key, target_path, artifact_id, display_name, checks_json,
                        status, finding_json, checked_at, updated_at, inherited_from_run_id
                 FROM delivery_checklist_items WHERE run_id=?1 ORDER BY item_key",
            )?;
            let rows = stmt
                .query_map(params![run_id], |row| {
                    let checks: String = row.get(4)?;
                    Ok(DeliveryChecklistItem {
                        item_key: row.get(0)?,
                        target_path: row.get(1)?,
                        artifact_id: row.get(2)?,
                        display_name: row.get(3)?,
                        checks: serde_json::from_str(&checks).unwrap_or_default(),
                        status: row.get(5)?,
                        finding: row.get(6)?,
                        checked_at: row.get(7)?,
                        updated_at: row.get(8)?,
                        inherited_from_run_id: row.get(9)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub(crate) fn record_delivery_repair_round(
        &self,
        run_id: &str,
        findings_json: &str,
        now: i64,
    ) -> Result<i64, String> {
        self.with_connection(|conn| {
            let round = conn.query_row(
                "SELECT COALESCE(MAX(round),0)+1 FROM delivery_repair_rounds WHERE run_id=?1",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )?;
            conn.execute(
                "INSERT INTO delivery_repair_rounds(run_id, round, findings_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![run_id, round, findings_json, now],
            )?;
            Ok(round)
        })
    }

    pub(crate) fn delivery_repair_round_count(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT COUNT(*) FROM delivery_repair_rounds WHERE run_id=?1",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
    }

    /// Wall-clock creation time of the Run; filesystem artifacts older than it
    /// (plus clock wobble) are never treated as this Run's deliverables.
    pub(crate) fn run_created_at(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT created_at FROM runs WHERE id=?1",
                params![run_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(Into::into)
        })
    }

    /// The submitted task text a Run was started with.
    ///
    /// Per-Run file roles (read-only input vs declared output) are derived from
    /// the text the user actually submitted, never from the current conversation
    /// state: a later message cannot retroactively widen what a running Run was
    /// allowed to write. Continuations and child Runs walk up their own chain,
    /// and the conversation's earliest user message is the last resort.
    pub(crate) fn run_task_text(&self, run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            let mut current = Some(run_id.to_owned());
            for _ in 0..8 {
                let Some(id) = current else { break };
                let text: Option<String> = conn
                    .query_row(
                        "SELECT content FROM messages
                         WHERE run_id=?1 AND role='user' AND TRIM(content)<>''
                         ORDER BY ordinal ASC LIMIT 1",
                        params![id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(text) = text {
                    return Ok(Some(text));
                }
                current = conn
                    .query_row(
                        "SELECT CASE
                                  WHEN parent_run_id IS NOT NULL AND parent_run_id<>?1 THEN parent_run_id
                                  WHEN root_run_id IS NOT NULL AND root_run_id<>?1 THEN root_run_id
                                END
                         FROM runs WHERE id=?1",
                        params![id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten();
            }
            let text: Option<String> = conn
                .query_row(
                    "SELECT content FROM messages
                     WHERE conversation_id=(SELECT conversation_id FROM runs WHERE id=?1)
                       AND role='user' AND TRIM(content)<>''
                     ORDER BY ordinal ASC LIMIT 1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(text)
        })
    }

    /// Host-verified files produced by settled tool results of this Run, in
    /// production order.
    pub(crate) fn run_artifacts(
        &self,
        run_id: &str,
    ) -> Result<Vec<DeliveryArtifactRow>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, storage_path, sha256, created_at FROM artifacts
                 WHERE run_id=?1 AND status='ready' ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt
                .query_map(params![run_id], |row| {
                    Ok(DeliveryArtifactRow {
                        id: row.get(0)?,
                        storage_path: row.get(1)?,
                        sha256: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Managed write receipts this Run committed, in commit order. A receipt
    /// names the path, the tool and the hash the bytes had afterwards; the
    /// delivery check compares that hash with the file on disk, so a refreshed
    /// modification time alone can never pass for a completed write.
    pub(crate) fn run_write_receipts(&self, run_id: &str) -> Result<Vec<RunWriteReceipt>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT id, version_no, run_id, tool_call_id, storage_path, change_kind, tool, after_hash, created_at
                 FROM managed_file_versions WHERE run_id=?1 ORDER BY created_at ASC, rowid ASC",
            )?;
            let rows = stmt
                .query_map(params![run_id], |row| {
                    Ok(RunWriteReceipt {
                        version_id: row.get(0)?,
                        version_no: row.get(1)?,
                        source_run_id: row.get(2)?,
                        tool_call_id: row.get(3)?,
                        storage_path: row.get(4)?,
                        change_kind: row.get(5)?,
                        tool: row.get(6)?,
                        after_hash: row.get(7)?,
                        created_at: row.get(8)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// Durably bind a pathless slot to the first matching real artifact. Only
    /// fills an unbound item, so repeated verification and reopened Runs keep
    /// checking the same file.
    pub(crate) fn bind_delivery_item(
        &self,
        run_id: &str,
        item_key: &str,
        target_path: &str,
        artifact_id: Option<&str>,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            let transaction=conn.unchecked_transaction()?;
            let (body,existing):(String,Option<String>)=transaction.query_row("SELECT requirements_json,target_path FROM delivery_checklist_items WHERE run_id=?1 AND item_key=?2",
                params![run_id,item_key],|row|Ok((row.get(0)?,row.get(1)?)))?;
            let requirements:Vec<DeliveryRequirement>=serde_json::from_str(&body)
                .map_err(|_|rusqlite::Error::InvalidParameterName("invalid frozen delivery requirements".into()))?;
            if requirements.iter().any(|requirement|match &requirement.kind {
                RequirementKind::TargetDirectory{directory}=>!Self::delivery_target_matches_directory(target_path,directory),
                RequirementKind::RecognitionUnresolved{..}=>true,_=>false}) {
                return Err(rusqlite::Error::InvalidParameterName("delivery target differs from the frozen directory or is unresolved".into()));
            }
            if existing.as_deref().is_some_and(|path|!path.eq_ignore_ascii_case(target_path)) {
                return Err(rusqlite::Error::InvalidParameterName("delivery item is already bound to another target".into()));
            }
            transaction.execute(
                "UPDATE delivery_checklist_items
                 SET target_path=?1,
                     artifact_id=COALESCE(?2, artifact_id),
                     updated_at=?3
                 WHERE run_id=?4 AND item_key=?5 AND target_path IS NULL",
                params![target_path, artifact_id, now_ms(), run_id, item_key],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }
}
