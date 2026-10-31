//! Delivery checklist persistence for ordinary file-producing tasks.
//!
//! This is the business-completion ledger, distinct from the model reply-end
//! marker (`<fox-final/>`) and from the bounded continuation review. Rows
//! bind a declared target to the real artifact and to deterministic Host
//! check results; repair rounds are counted here, never in model-failure
//! retry counters.

#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
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
            if crate::canonical_json::to_vec(&result).map(|b|b.len()>7680).unwrap_or(true) {
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
    // Package 2-B2: a typed v2 verdict is whitelisted as a **bounded projection**,
    // never passed through whole. Passing it through would let an unbounded fact
    // list exceed the finding capacity silently, and dropping it would leave the
    // reader unable to tell a v2 record from a legacy one.
    if let Some(projected)=project_verdict(source.get("deliveryVerdict")) {
        let bounded=matches!(projected,Some(_));
        if let Some(projected)=projected {
            result.insert("deliveryVerdict".into(),projected);
        }
        // Capacity is checked before accepting: an over-budget projection is
        // recorded as explicitly missing instead of being silently truncated.
        if !bounded || crate::canonical_json::to_vec(&result).map(|b|b.len()>7680).unwrap_or(true) {
            result.remove("deliveryVerdict");
            result.insert("deliveryVerdictOmitted".into(),json!(true));
        }
    }
    Some(Value::Object(result))
}

/// Project a persisted v2 verdict envelope into a bounded, readable form.
///
/// `None` means "no envelope present" and `Some(None)` means "present but its
/// shape is not one this reader can decode" — the distinction matters, because
/// the caller records the second case as an explicit omission rather than
/// pretending the record never had a verdict.
fn project_verdict(value: Option<&Value>) -> Option<Option<Value>> {
    let value=value?;
    let envelope=value.as_object()?;
    let mut result=serde_json::Map::new();
    for field in ["verdictSchemaVersion","verdictEnvelopeVersion","itemCount"] {
        if let Some(number)=envelope.get(field).and_then(Value::as_u64) {
            result.insert(field.into(),json!(number));
        }
    }
    for field in ["itemKey","class","aggregateClass"] {
        if let Some(text)=envelope.get(field).and_then(Value::as_str) {
            result.insert(field.into(),json!(bounded_display_text(text,512)));
        }
    }
    for field in ["terminal","repairBudgetRemaining"] {
        if let Some(flag)=envelope.get(field).and_then(Value::as_bool) {
            result.insert(field.into(),json!(flag));
        }
    }
    let facts=envelope.get("facts").and_then(Value::as_array);
    if let Some(facts)=facts {
        let total=facts.len();
        // A bounded number of facts, each bounded field by field. `factCount` is
        // always recorded so a truncated list is never mistaken for a complete one.
        let kept: Vec<Value>=facts.iter().take(32).filter_map(|fact| {
            let source=fact.as_object()?;let mut entry=serde_json::Map::new();
            for field in ["class","origin","ruleId","check","reasonCode","limitKind","equivalence","path","sha256","expected","actual"] {
                if let Some(text)=source.get(field).and_then(Value::as_str) {
                    entry.insert(field.into(),json!(bounded_display_text(text,512)));
                }
            }
            for field in ["checkerVersion","limit"] {
                if let Some(number)=source.get(field).and_then(Value::as_u64) {
                    entry.insert(field.into(),json!(number));
                }
            }
            for field in ["blocking","needsRepair"] {
                if let Some(flag)=source.get(field).and_then(Value::as_bool) {
                    entry.insert(field.into(),json!(flag));
                }
            }
            if let Some(detail)=source.get("detail").and_then(Value::as_str) {
                entry.insert("detail".into(),json!(bounded_display_text(detail,512)));
            }
            if let Some(locator)=source.get("locator") {
                if let Ok(text)=crate::canonical_json::to_string(locator) {
                    entry.insert("locator".into(),json!(bounded_display_text(&text,512)));
                }
            }
            (!entry.is_empty()).then_some(Value::Object(entry))
        }).collect();
        result.insert("factCount".into(),json!(total));
        result.insert("facts".into(),Value::Array(kept));
        if facts.len()>32 {result.insert("factsTruncated".into(),json!(true));}
    }
    Some(Some(Value::Object(result)))
}

/// A single latest Run, owned by this conversation. All callers use the same
/// persisted rows; absent files and unbound/non-file items remain visible.
pub(super) fn query_current_checklist_view(connection:&rusqlite::Connection,conversation:&str,run:Option<&str>)
    ->rusqlite::Result<(Vec<super::super::DeliveryChecklistView>,bool)> {
    let Some(run)=run else {return Ok((Vec::new(),false));};
    let mut statement=connection.prepare("SELECT i.item_key,i.target_path,
        (SELECT a.id FROM artifacts a WHERE a.id=i.artifact_id AND a.conversation_id=?1),
        i.display_name,i.status,i.finding_json,i.checked_at,i.requirements_json
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
        // The row's own frozen contract column, read in the **same** statement as
        // its finding so the identity and the verdict always describe one row and
        // no second per-artifact query is issued.
        let requirements:Option<String>=row.get(7)?;
        // Packages 2-B2 / 2-C1: a v2 record's own facts decide whether it may be
        // shown as a pass. The durable columns are read **before** projection,
        // because the projection truncates the fact list and could not support a
        // complete verdict. `KeepStoredStatus` means the row carries no pass
        // obligation this rule can judge: a historical record keeps the status it
        // was written with, so introducing v2 cannot mass-downgrade legacy rows.
        //
        // This is a **downgrade guard**, not a re-derivation. Only a row that
        // claims `passed` while its own durable record does not support a pass is
        // withheld. Re-deriving every status here would overwrite the write side's
        // own verdicts: a legitimate definite non-pass is stored as `failed` and
        // must keep reading as `failed`, and `RepairNeeded` (also stored `failed`)
        // means this round's check did not pass while the Run stays repairable.
        //
        // 2-C1 adds the second, independent reason a pass may not stand: the row's
        // own contract says it owes a v2 record and that record is gone. Reading
        // only the finding column could not tell that row apart from a legal
        // legacy one, so a stripped v2 row used to keep displaying `passed`.
        // The decision itself lives in one shared rule
        // (`delivery::passed_qualification_of_row`) so this view, the artifact
        // view, the finalize defence and the progress summary cannot drift apart
        // about what may count as a pass.
        let qualification = crate::runtime_host::delivery::passed_qualification_of_row(
            &status,
            raw.as_deref(),
            requirements.as_deref(),
            &item_key,
        );
        let status=match status.as_str() {
            // The one downgrade: the row says passed and its own durable record
            // says no. It becomes the existing non-terminal `pending` rather than
            // `failed`: an unusable or contradictory record is *unconfirmed*, and
            // claiming it failed would report a verdict no checker produced.
            "passed" if qualification.withdraws_pass() => "pending".to_owned(),
            // Everything else keeps the status it was written with: a legitimate
            // `failed` (Refused/Invalid/DefiniteFailure/RepairNeeded) stays
            // `failed`, a legitimate `passed` stays `passed`, and `pending` is
            // never upgraded to a pass by the reader.
            "pending"|"passed"|"failed" => status,
            _ => "pending".into(),
        };
        Ok(super::super::DeliveryChecklistView{run_id:run.into(),item_key:bounded_display_text(&item_key,2048),
            description:bounded_display_text(&row.get::<_,String>(3)?,512),artifact_kind:kind.into(),
            target_path:target_path.map(|s|bounded_display_text(&s,2048)),artifact_id:row.get(2)?,read_version,
            status,finding,checked_at:row.get(6)?})
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

    pub(crate) fn should_abort() -> bool {
        DONE.with(|done| {
            let applied = done.get() + 1;
            done.set(applied);
            ABORT_AFTER.with(|limit| limit.get().is_some_and(|limit| applied > limit))
        })
    }
}

impl Database {
    /// Insert checklist items that were not planned yet. Existing items (a
    /// repaired run may re-check the same target) keep their identity.
    /// Test fixture: historical checklist rows for the retained read model
    /// (no production writer remains).
    #[cfg(test)]
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
                        crate::canonical_json::to_string(&seed.checks).unwrap_or_else(|_| "[]".into()),
                        crate::canonical_json::to_string(&seed.requirements).unwrap_or_else(|_| "[]".into()),
                        now,
                    ],
                )?;
            }
            transaction.commit()?;
            Ok(())
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

}
