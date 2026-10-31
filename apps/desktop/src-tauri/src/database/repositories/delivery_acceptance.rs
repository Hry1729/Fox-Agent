//! Package 2-D U1: the durable store for a user's decision about an
//! **unverified** delivery.
//!
//! This module is deliberately **SQL only**. The rule that decides whether an
//! acceptance still stands — the row's class, the candidate's current bytes, the
//! managed-file version, the frozen contract, the project authorization — lives
//! in `runtime_host::delivery`, because every one of those checks needs the typed
//! verification vocabulary (`StoredVerdict`, `DeliveryClass`, `DecisionFact`) and
//! the authorized reader, and re-exporting all of that into the database layer
//! would widen a surface for no benefit.
//!
//! What this module owns:
//!
//! * reading one row's acceptance state **together with** its checklist row, so
//!   the two can never describe different rows;
//! * writing an acceptance, a rejection or a revocation, each in one transaction;
//! * appending to the append-only audit table.
//!
//! It never decides anything. In particular it never reports a row as passed:
//! `acceptance_class` may only be `'unconfirmed'`, enforced by the schema.

use super::Database;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;

/// One checklist row joined to its acceptance projection, read in a single
/// statement so a later read cannot observe half of the pair.
#[derive(Debug, Clone)]
pub(crate) struct AcceptanceRow {
    pub run_id: String,
    pub item_key: String,
    pub status: String,
    /// The row's own frozen contract column, verbatim.
    pub requirements_json: String,
    /// The row's bound target path, if it has one.
    pub target_path: Option<String>,
    /// The row's durable finding, verbatim.
    pub finding_json: Option<String>,
    /// The hash recorded when the acceptance was made.
    pub accepted_sha256: String,
    /// The managed-file version recorded when the acceptance was made.
    pub accepted_version_id: Option<String>,
    /// The frozen contract hash recorded when the acceptance was made.
    pub accepted_spec_hash: String,
    pub accepted_by: String,
    pub accepted_at: i64,
    pub rejected_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// The identity a new acceptance must bind, taken from the row's own columns.
///
/// The managed-version identity is resolved here — but through the **absolute**
/// storage path the caller passes in, because that is the form
/// `managed_file_versions.storage_path` is written in. Resolving it from the raw
/// `target_path` is the D-4 defect: a relative `target_path` missed (so the
/// version criterion was skipped) and an absolute one made the freshness
/// re-check miss (so an acceptance could never stand).
#[derive(Debug, Clone)]
pub(crate) struct AcceptanceBinding {
    pub conversation_id: String,
    pub candidate_path: String,
    pub candidate_sha256: String,
    pub managed_version_id: Option<String>,
    pub spec_hash: String,
    pub rule_id: Option<String>,
    pub checker_version: i64,
}

/// What the write actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AcceptanceWrite {
    /// A row was inserted or updated.
    Written,
    /// The request was already satisfied; nothing changed.
    Unchanged,
}

impl Database {
    /// Read the checklist row and its acceptance projection together.
    ///
    /// A missing row returns `Ok(None)` — that is "this item has no acceptance
    /// state", which is a legitimate answer. A **missing table** is not: that is
    /// a database error and must surface as one, because silently treating it as
    /// "no acceptance" would let an unmigrated database look like a database
    /// where nobody ever accepted anything.
    pub(crate) fn read_delivery_acceptance_row(
        &self,
        run_id: &str,
        item_key: &str,
    ) -> Result<Option<AcceptanceRow>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT c.run_id, c.item_key, c.status, c.requirements_json,
                            c.target_path, c.finding_json,
                            COALESCE(a.candidate_sha256, ''),
                            a.managed_version_id,
                            COALESCE(a.spec_hash, ''),
                            COALESCE(a.accepted_by, ''),
                            COALESCE(a.accepted_at, 0),
                            a.rejected_at, a.revoked_at
                     FROM delivery_checklist_items c
                     LEFT JOIN delivery_acceptances a
                       ON a.run_id = c.run_id AND a.item_key = c.item_key
                     WHERE c.run_id = ?1 AND c.item_key = ?2",
                    params![run_id, item_key],
                    |row| {
                        Ok(AcceptanceRow {
                            run_id: row.get(0)?,
                            item_key: row.get(1)?,
                            status: row.get(2)?,
                            requirements_json: row.get(3)?,
                            target_path: row.get(4)?,
                            finding_json: row.get(5)?,
                            accepted_sha256: row.get(6)?,
                            accepted_version_id: row.get(7)?,
                            accepted_spec_hash: row.get(8)?,
                            accepted_by: row.get(9)?,
                            accepted_at: row.get(10)?,
                            rejected_at: row.get(11)?,
                            revoked_at: row.get(12)?,
                        })
                    },
                )
                .optional()
        })
        .map_err(|error| error.to_string())
    }

    /// Derive the identity an acceptance must bind, from the row's own frozen
    /// columns. Nothing is invented: a missing hash stays empty rather than being
    /// filled with a placeholder the caller might mistake for a real one.
    ///
    /// `absolute_candidate` is the candidate resolved under the current
    /// authorized root — the same form `managed_file_versions.storage_path` is
    /// written in. Passing `None` (an unresolvable candidate) yields no version
    /// binding, which the freshness rule then reports as `CandidateUnreadable`
    /// rather than silently skipping the version criterion.
    pub(crate) fn delivery_acceptance_binding(
        &self,
        run_id: &str,
        item_key: &str,
        row: &AcceptanceRow,
        absolute_candidate: Option<&std::path::Path>,
    ) -> Result<AcceptanceBinding, String> {
        self.with_connection(|connection| {
            let conversation_id: String = connection
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1",
                    params![run_id],
                    |row| row.get(0),
                )
                .optional()?
                .unwrap_or_default();
            let finding: Option<serde_json::Value> = row
                .finding_json
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok());
            let candidate_path = row
                .target_path
                .clone()
                .or_else(|| {
                    finding
                        .as_ref()
                        .and_then(|value| value.get("boundPath"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            let candidate_sha256 = if row.accepted_sha256.is_empty() {
                finding
                    .as_ref()
                    .and_then(|value| value.get("checkedHash"))
                    .and_then(serde_json::Value::as_str)
                    .map(|hash| hash.trim_start_matches("sha256:").to_owned())
                    .unwrap_or_default()
            } else {
                row.accepted_sha256.clone()
            };
            let requirements: serde_json::Value =
                serde_json::from_str(&row.requirements_json).unwrap_or(serde_json::json!({}));
            let spec_hash = digest_hex(
                crate::canonical_json::to_vec(&requirements)
                    .unwrap_or_default()
                    .as_slice(),
            );
            let rule = requirements.get("rule");
            let rule_id = rule
                .and_then(|value| value.get("ruleId"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let checker_version = rule
                .and_then(|value| value.get("checkerVersion"))
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(1);
            // The version identity, resolved through the absolute storage path
            // the real write gate records. An empty candidate path has nothing
            // to resolve and is reported by the freshness rule, not skipped.
            let managed_version_id = match absolute_candidate {
                Some(absolute) if !absolute.as_os_str().is_empty() => connection
                    .query_row(
                        "SELECT id FROM managed_file_versions
                         WHERE run_id = ?1 AND storage_path = ?2
                         ORDER BY version_no DESC LIMIT 1",
                        params![run_id, absolute.to_string_lossy()],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?,
                _ => None,
            };
            let _ = item_key;
            Ok(AcceptanceBinding {
                conversation_id,
                candidate_path,
                candidate_sha256,
                managed_version_id,
                spec_hash,
                rule_id,
                checker_version,
            })
        })
    }

    /// Write (or refresh) the current acceptance for one item.
    ///
    /// `rejected_at` and `revoked_at` are cleared, because an acceptance and a
    /// rejection are mutually exclusive current states. The audit table keeps
    /// both; only this projection is exclusive.
    pub(crate) fn write_delivery_acceptance(
        &self,
        run_id: &str,
        item_key: &str,
        binding: &AcceptanceBinding,
        actor: &str,
        now: i64,
    ) -> Result<AcceptanceWrite, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let changed = transaction
                .execute(
                    "INSERT INTO delivery_acceptances
                     (run_id, item_key, conversation_id, accepted_by, accepted_at,
                      candidate_path, candidate_sha256, managed_version_id, spec_hash,
                      rule_id, checker_version, acceptance_class,
                      rejected_at, revoked_at, revoke_reason)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'unconfirmed',
                             NULL, NULL, NULL)
                     ON CONFLICT(run_id, item_key) DO UPDATE SET
                         accepted_by=excluded.accepted_by,
                         accepted_at=excluded.accepted_at,
                         candidate_path=excluded.candidate_path,
                         candidate_sha256=excluded.candidate_sha256,
                         managed_version_id=excluded.managed_version_id,
                         spec_hash=excluded.spec_hash,
                         rule_id=excluded.rule_id,
                         checker_version=excluded.checker_version,
                         rejected_at=NULL,
                         revoked_at=NULL,
                         revoke_reason=NULL",
                    params![
                        run_id,
                        item_key,
                        binding.conversation_id,
                        actor,
                        now,
                        binding.candidate_path,
                        binding.candidate_sha256,
                        binding.managed_version_id,
                        binding.spec_hash,
                        binding.rule_id,
                        binding.checker_version,
                    ],
                )?;
            transaction.commit()?;
            Ok(if changed == 0 {
                AcceptanceWrite::Unchanged
            } else {
                AcceptanceWrite::Written
            })
        })
    }

    /// Record that the user looked at this item and said no.
    ///
    /// `accepted_at` is written as `0`, never as `now`: that column means
    /// "accepted", and a rejected item was not accepted. Inventing an
    /// `accepted_at` here would make a rejection read back as an acceptance.
    pub(crate) fn write_delivery_rejection(
        &self,
        run_id: &str,
        item_key: &str,
        binding: &AcceptanceBinding,
        actor: &str,
        now: i64,
    ) -> Result<AcceptanceWrite, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let changed = transaction
                .execute(
                    "INSERT INTO delivery_acceptances
                     (run_id, item_key, conversation_id, accepted_by, accepted_at,
                      candidate_path, candidate_sha256, spec_hash,
                      rule_id, checker_version, acceptance_class, rejected_at,
                      revoked_at, revoke_reason)
                     VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8, ?9, 'unconfirmed',
                             ?10, NULL, NULL)
                     ON CONFLICT(run_id, item_key) DO UPDATE SET
                         rejected_at=excluded.rejected_at,
                         revoked_at=NULL,
                         revoke_reason=NULL",
                    params![
                        run_id,
                        item_key,
                        binding.conversation_id,
                        actor,
                        binding.candidate_path,
                        binding.candidate_sha256,
                        binding.spec_hash,
                        binding.rule_id,
                        binding.checker_version,
                        now,
                    ],
                )?;
            transaction.commit()?;
            Ok(if changed == 0 {
                AcceptanceWrite::Unchanged
            } else {
                AcceptanceWrite::Written
            })
        })
    }

    /// Withdraw an acceptance that currently stands. Scoped to
    /// `revoked_at IS NULL`, so a repeated revocation is a no-op and a revocation
    /// can never be applied twice (which would move `revoked_at` forward and
    /// misreport when the user actually changed their mind).
    pub(crate) fn write_delivery_revocation(
        &self,
        run_id: &str,
        item_key: &str,
        now: i64,
    ) -> Result<AcceptanceWrite, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let changed = transaction
                .execute(
                    "UPDATE delivery_acceptances
                     SET revoked_at = ?3, revoke_reason = '用户撤销接受'
                     WHERE run_id = ?1 AND item_key = ?2 AND revoked_at IS NULL",
                    params![run_id, item_key, now],
                )?;
            transaction.commit()?;
            Ok(if changed == 0 {
                AcceptanceWrite::Unchanged
            } else {
                AcceptanceWrite::Written
            })
        })
    }

    /// Append one audit row. Never updates or deletes: the audit is the only
    /// place the first acceptance survives a later re-acceptance.
    pub(crate) fn append_delivery_acceptance_audit(
        &self,
        run_id: &str,
        item_key: &str,
        action: &str,
        actor: &str,
        at: i64,
        reason: Option<&str>,
        snapshot: Option<&str>,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO delivery_acceptance_audit
                     (run_id, item_key, action, actor, at, reason, object_snapshot)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![run_id, item_key, action, actor, at, reason, snapshot],
                )
                .map(|_| ())
        })
        .map_err(|error| error.to_string())
    }
}

impl AcceptanceRow {
    /// The candidate path this row's own columns name: its bound `target_path`,
    /// falling back to the finding's `boundPath`.
    ///
    /// One definition, used by both the binding and the freshness rule, so the
    /// two cannot disagree about which path an acceptance refers to.
    pub(crate) fn candidate_path(&self) -> String {
        self.target_path
            .clone()
            .or_else(|| {
                self.finding_json
                    .as_deref()
                    .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
                    .and_then(|value| {
                        value
                            .get("boundPath")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
            })
            .unwrap_or_default()
    }
}

fn digest_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

