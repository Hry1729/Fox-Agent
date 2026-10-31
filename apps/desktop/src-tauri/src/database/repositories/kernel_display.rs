//! Durable display content, separate from Kernel execution facts and replay inputs.
use rusqlite::{params, OptionalExtension, Transaction};
use serde_json::Value;

/// Only a Host-registered, byte-checked Office version can create an Office
/// artifact. Connector result JSON is intentionally absent from this record.
pub(super) struct VerifiedOfficeArtifact {
    pub version_id: String,
    pub tool: String,
    pub path: String,
    pub name: String,
    pub change_kind: String,
    pub bytes: i64,
    pub sha: String,
}

pub(super) fn verified_office_artifact(
    tx: &Transaction<'_>,
    run: &str,
    call: &str,
) -> rusqlite::Result<Option<VerifiedOfficeArtifact>> {
    tx.query_row(
        "SELECT id,tool,storage_path,display_name,change_kind,after_size,after_hash
         FROM managed_file_versions
         WHERE run_id=?1 AND tool_call_id=?2 AND source_kind='office_connector'
           AND source_verified=1 AND after_size IS NOT NULL AND after_hash IS NOT NULL
         ORDER BY version_no DESC LIMIT 1",
        params![run, call],
        |row| Ok(VerifiedOfficeArtifact {
            version_id: row.get(0)?,
            tool: row.get(1)?,
            path: row.get(2)?,
            name: row.get(3)?,
            change_kind: row.get(4)?,
            bytes: row.get(5)?,
            sha: row.get(6)?,
        }),
    ).optional()
}

pub(super) fn builtin_office_render(input: &Value) -> bool {
    input.get("serverId").and_then(Value::as_str) == Some(crate::office::SERVER_ID)
        && input.get("tool").and_then(Value::as_str) == Some("office_render")
}

pub(super) fn verified_preview_bytes(meta: &Value) -> Option<(String, String, i64)> {
    let path = meta.get("storagePath")?.as_str()?;
    let claimed_sha = meta.get("afterHash")?.as_str()?;
    let claimed_size = meta.get("afterSize")?.as_i64()?;
    let (actual_sha, actual_size) = crate::runtime_host::managed_files::hash_file(std::path::Path::new(path))?;
    if actual_size != claimed_size || actual_sha != claimed_sha.strip_prefix("sha256:").unwrap_or(claimed_sha) {
        return None;
    }
    Some((path.to_owned(), actual_sha, actual_size))
}

pub(super) fn explicit_user_purpose(
    tx: &Transaction<'_>,
    run: &str,
    project_root: Option<&std::path::Path>,
    path: &std::path::Path,
) -> rusqlite::Result<Option<crate::runtime_host::artifact_store::ArtifactClass>> {
    let Some(root) = project_root else { return Ok(None) };
    let mut current = run.to_owned();
    // A continuation inherits the original request's file purpose. This is
    // bounded so damaged ancestry cannot trap a read projection.
    for _ in 0..32 {
        let task: Option<String> = tx.query_row(
            "SELECT content FROM messages WHERE run_id=?1 AND role='user' ORDER BY ordinal LIMIT 1",
            [&current], |row| row.get(0),
        ).optional()?;
        if let Some(purpose) = task.as_deref().and_then(|text| {
            crate::runtime_host::delivery::explicit_file_purpose(text, root, path)
        }) {
            return Ok(Some(purpose));
        }
        let parent: Option<String> = tx.query_row(
            "SELECT continued_from_run_id FROM kernel_runs WHERE run_id=?1",
            [&current], |row| row.get(0),
        ).optional()?.flatten();
        let Some(parent) = parent else { break };
        current = parent;
    }
    Ok(None)
}

