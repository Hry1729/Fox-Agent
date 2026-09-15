//! Delivery checklist persistence for ordinary file-producing tasks.
//!
//! This is the business-completion ledger, distinct from the model reply-end
//! marker (`<fox-final/>`) and from the bounded continuation review. Rows
//! bind a declared target to the real artifact and to deterministic Host
//! check results; repair rounds are counted here, never in model-failure
//! retry counters.

use super::*;

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
}

impl Database {
    /// Insert checklist items that were not planned yet. Existing items (a
    /// repaired run may re-check the same target) keep their identity.
    pub(crate) fn seed_delivery_checklist(
        &self,
        run_id: &str,
        seeds: &[DeliveryChecklistSeed],
        now: i64,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            for seed in seeds {
                conn.execute(
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

    pub(crate) fn record_delivery_check(
        &self,
        run_id: &str,
        item_key: &str,
        passed: bool,
        finding: Option<&str>,
        now: i64,
    ) -> Result<(), String> {
        self.with_connection(|conn| {
            let affected = conn.execute(
                "UPDATE delivery_checklist_items
                 SET status=?1, finding_json=?2, checked_at=?3, updated_at=?3
                 WHERE run_id=?4 AND item_key=?5",
                params![
                    if passed { "passed" } else { "failed" },
                    finding,
                    now,
                    run_id,
                    item_key,
                ],
            )?;
            if affected == 0 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(())
        })
    }

    pub(crate) fn delivery_checklist(
        &self,
        run_id: &str,
    ) -> Result<Vec<DeliveryChecklistItem>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(
                "SELECT item_key, target_path, artifact_id, display_name, checks_json,
                        status, finding_json, checked_at, updated_at
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
            conn.execute(
                "UPDATE delivery_checklist_items
                 SET target_path=?1,
                     artifact_id=COALESCE(?2, artifact_id),
                     updated_at=?3
                 WHERE run_id=?4 AND item_key=?5 AND target_path IS NULL",
                params![target_path, artifact_id, now_ms(), run_id, item_key],
            )?;
            Ok(())
        })
    }
}
