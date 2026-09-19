//! Append-only registry of managed file versions (before/after hashes and
//! backup locations) produced by Host-intercepted writes: write_file,
//! edit_file and the fox-office connector. Restore is a pure file operation
//! against a registered snapshot; it never replays tools or creates Runs.

use super::*;

/// Provenance of one registered version. Only `HostCapture` and `OfficeConnector`
/// rows may be restored: both describe a write the Host itself performed or
/// verified, never a fact that merely arrived in a tool result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManagedFileSource {
    /// `write_file` / `edit_file` through a Host-intercepted, already approved
    /// dispatch: the Host hashed and copied the target itself.
    HostCapture,
    /// `office_create` / `office_edit` on the frozen built-in Office connector,
    /// cross-checked by the Host against the verified target and the file on
    /// disk before this row was written.
    OfficeConnector,
    /// Restore output produced by this module's own pure file copy.
    Restore,
    /// A row written before provenance tracking existed. Its hashes are shown
    /// but its backup is never trusted to reconstruct content.
    LegacyUnknown,
}

impl ManagedFileSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ManagedFileSource::HostCapture => "host_capture",
            ManagedFileSource::OfficeConnector => "office_connector",
            ManagedFileSource::Restore => "restore",
            ManagedFileSource::LegacyUnknown => "legacy_unknown",
        }
    }

    fn from_str(value: &str) -> ManagedFileSource {
        match value {
            "host_capture" => ManagedFileSource::HostCapture,
            "office_connector" => ManagedFileSource::OfficeConnector,
            "restore" => ManagedFileSource::Restore,
            _ => ManagedFileSource::LegacyUnknown,
        }
    }

    /// Whether this row's own recorded bytes may be used as restore input.
    pub(crate) fn is_verified(self) -> bool {
        !matches!(self, ManagedFileSource::LegacyUnknown)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedFileVersionInput<'a> {
    pub conversation_id: &'a str,
    pub run_id: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
    pub tool: &'a str,
    pub storage_path: &'a str,
    pub display_name: &'a str,
    pub change_kind: &'a str, // created | modified | restored
    pub before_hash: Option<&'a str>,
    pub before_size: Option<i64>,
    pub after_hash: Option<&'a str>,
    pub after_size: Option<i64>,
    pub backup_path: Option<&'a str>,
    /// Host-owned snapshot of the content this row records (`after_hash`).
    pub after_backup_path: Option<&'a str>,
    pub restored_from_id: Option<&'a str>,
    pub source: ManagedFileSource,
}

#[derive(Debug, Clone)]
pub(crate) struct ManagedFileVersion {
    pub id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool: String,
    pub storage_path: String,
    pub display_name: String,
    pub version_no: i64,
    pub change_kind: String,
    pub before_hash: Option<String>,
    pub before_size: Option<i64>,
    pub after_hash: Option<String>,
    pub after_size: Option<i64>,
    pub backup_path: Option<String>,
    pub after_backup_path: Option<String>,
    pub restored_from_id: Option<String>,
    pub source: ManagedFileSource,
    pub source_verified: bool,
    pub created_at: i64,
}

impl ManagedFileVersion {
    /// Bytes this row records: exactly `after_hash` / `after_size`.
    pub(crate) fn recorded_size(&self) -> Option<i64> {
        self.after_size
    }
}

const SELECT_COLUMNS: &str = "id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
            version_no, change_kind, before_hash, before_size, after_hash, after_size,
            backup_path, after_backup_path, restored_from_id, source_kind, source_verified, created_at";

impl Database {
    /// Register one version. `version_no` is derived per storage path inside
    /// the same transaction, so concurrent writes cannot alias a number.
    pub(crate) fn register_managed_file_version(
        &self,
        input: &ManagedFileVersionInput<'_>,
        now: i64,
    ) -> Result<String, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            let version_no = tx.query_row(
                "SELECT COALESCE(MAX(version_no),0)+1 FROM managed_file_versions
                 WHERE storage_path=?1",
                params![input.storage_path],
                |row| row.get::<_, i64>(0),
            )?;
            let id = format!("file-version:{}", uuid::Uuid::new_v4());
            tx.execute(
                "INSERT INTO managed_file_versions(
                    id, conversation_id, run_id, tool_call_id, tool, storage_path, display_name,
                    version_no, change_kind, before_hash, before_size, after_hash, after_size,
                    backup_path, after_backup_path, restored_from_id, source_kind, source_verified,
                    created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
                params![
                    id,
                    input.conversation_id,
                    input.run_id,
                    input.tool_call_id,
                    input.tool,
                    input.storage_path,
                    input.display_name,
                    version_no,
                    input.change_kind,
                    input.before_hash,
                    input.before_size,
                    input.after_hash,
                    input.after_size,
                    input.backup_path,
                    input.after_backup_path,
                    input.restored_from_id,
                    input.source.as_str(),
                    i64::from(input.source.is_verified()),
                    now,
                ],
            )?;
            tx.commit()?;
            Ok(id)
        })
    }

    pub(crate) fn managed_file_version(&self, id: &str) -> Result<Option<ManagedFileVersion>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM managed_file_versions WHERE id=?1"
            ))?;
            Ok(stmt.query_row(params![id], row_to_managed_version).optional()?)
        })
    }

    pub(crate) fn managed_file_versions(
        &self,
        conversation_id: &str,
        storage_path: Option<&str>,
    ) -> Result<Vec<ManagedFileVersion>, String> {
        self.with_connection(|conn| {
            let (sql, path_param): (String, Option<&str>) = if storage_path.is_some() {
                (
                    format!(
                        "SELECT {SELECT_COLUMNS}
                         FROM managed_file_versions
                         WHERE conversation_id=?1 AND storage_path=?2 ORDER BY version_no"
                    ),
                    storage_path,
                )
            } else {
                (
                    format!(
                        "SELECT {SELECT_COLUMNS}
                         FROM managed_file_versions
                         WHERE conversation_id=?1 ORDER BY storage_path, version_no"
                    ),
                    None,
                )
            };
            let mut stmt = conn.prepare(&sql)?;
            let rows = if let Some(path) = path_param {
                stmt.query_map(params![conversation_id, path], row_to_managed_version)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            } else {
                stmt.query_map(params![conversation_id], row_to_managed_version)?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            Ok(rows)
        })
    }

    /// Every version of one storage path in ascending version order, regardless
    /// of the conversation that registered it: restore-time integrity checks need
    /// the neighbouring rows, not only the requester's own history.
    pub(crate) fn managed_file_versions_for_path(
        &self,
        storage_path: &str,
    ) -> Result<Vec<ManagedFileVersion>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM managed_file_versions
                 WHERE storage_path=?1 ORDER BY version_no"
            ))?;
            let rows = stmt
                .query_map(params![storage_path], row_to_managed_version)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    pub(crate) fn latest_managed_file_version(
        &self,
        conversation_id: &str,
        storage_path: &str,
    ) -> Result<Option<ManagedFileVersion>, String> {
        self.with_connection(|conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SELECT_COLUMNS} FROM managed_file_versions
                 WHERE conversation_id=?1 AND storage_path=?2 ORDER BY version_no DESC LIMIT 1"
            ))?;
            Ok(stmt
                .query_row(params![conversation_id, storage_path], row_to_managed_version)
                .optional()?)
        })
    }

    /// Whether any Host ledger still names this exact path.
    ///
    /// Consulted before an abandoned commit staging file is reclaimed: bytes an
    /// artifact row or a version/recovery row still references are never
    /// deleted, however abandoned they look. This is a required check, not a
    /// heuristic — an unanswerable query is treated as "still referenced" by
    /// the caller.
    pub(crate) fn path_is_referenced_by_host_ledger(&self, path: &str) -> Result<bool, String> {
        self.with_connection(|conn| {
            let referenced: bool = conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM artifacts WHERE storage_path = ?1
                    UNION ALL
                    SELECT 1 FROM managed_file_versions
                     WHERE storage_path = ?1 OR backup_path = ?1 OR after_backup_path = ?1
                )",
                params![path],
                |row| row.get(0),
            )?;
            Ok(referenced)
        })
    }

    /// The conversation's already-assigned deliverable folder for this project,
    /// or `None` when it has never produced a deliverable here.
    ///
    /// Read back on every prompt build, so a task continued on a later day (or
    /// after an application restart) keeps the folder it started with.
    pub(crate) fn deliverable_folder(
        &self,
        conversation_id: &str,
        project_key: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            Ok(conn
                .query_row(
                    "SELECT folder_name FROM deliverable_placements
                     WHERE conversation_id=?1 AND project_key=?2",
                    params![conversation_id, project_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?)
        })
    }

    /// Assign the conversation's deliverable folder for this project, or return
    /// the one already assigned.
    ///
    /// `candidate` is only used the first time. `INSERT OR IGNORE` against the
    /// composite primary key makes the assignment idempotent under concurrent
    /// Runs: whichever Run commits first wins and every other Run reads that
    /// same row back, so two simultaneous requests cannot end up in two
    /// folders. `now` is passed in so the caller keeps one clock.
    ///
    /// `acceptable` re-validates the stored value on every read. A record that
    /// is no longer a usable single path component is replaced — and only when
    /// it still holds exactly the rejected value, so a concurrent repair cannot
    /// clobber a name another Run just stored. Returns `None` when no acceptable
    /// name could be established, which the caller must treat as "no deliverable
    /// root" rather than falling back to an unvalidated value.
    pub(crate) fn ensure_deliverable_folder(
        &self,
        conversation_id: &str,
        project_key: &str,
        candidate: &str,
        acceptable: &dyn Fn(&str) -> bool,
        now: i64,
    ) -> Result<Option<String>, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT OR IGNORE INTO deliverable_placements(
                    conversation_id, project_key, folder_name, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)",
                params![conversation_id, project_key, candidate, now],
            )?;
            let stored: String = tx.query_row(
                "SELECT folder_name FROM deliverable_placements
                 WHERE conversation_id=?1 AND project_key=?2",
                params![conversation_id, project_key],
                |row| row.get(0),
            )?;
            if acceptable(&stored) {
                tx.commit()?;
                return Ok(Some(stored));
            }
            tx.execute(
                "UPDATE deliverable_placements SET folder_name=?1, updated_at=?2
                 WHERE conversation_id=?3 AND project_key=?4 AND folder_name=?5",
                params![candidate, now, conversation_id, project_key, stored],
            )?;
            tx.commit()?;
            Ok(acceptable(candidate).then(|| candidate.to_owned()))
        })
    }
}

fn row_to_managed_version(row: &rusqlite::Row<'_>) -> rusqlite::Result<ManagedFileVersion> {
    let source_kind: String = row.get(16)?;
    Ok(ManagedFileVersion {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        run_id: row.get(2)?,
        tool_call_id: row.get(3)?,
        tool: row.get(4)?,
        storage_path: row.get(5)?,
        display_name: row.get(6)?,
        version_no: row.get(7)?,
        change_kind: row.get(8)?,
        before_hash: row.get(9)?,
        before_size: row.get(10)?,
        after_hash: row.get(11)?,
        after_size: row.get(12)?,
        backup_path: row.get(13)?,
        after_backup_path: row.get(14)?,
        restored_from_id: row.get(15)?,
        source: ManagedFileSource::from_str(&source_kind),
        source_verified: row.get::<_, i64>(17)? != 0,
        created_at: row.get(18)?,
    })
}
