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
