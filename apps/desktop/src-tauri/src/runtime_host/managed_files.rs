//! Capture and restore of user-file versions produced by Host-intercepted
//! writes (`write_file`, `edit_file`, the fox-office connector).
//!
//! Invariants:
//! * Registration happens around a write that already passed every frozen
//!   Kernel/Legacy admission check. Version bookkeeping is best-effort: a hash
//!   or registry failure never undoes or fails an authorized write.
//! * A registered **version is a content version**: `version_no`, the size and
//!   hash shown in the UI, and the bytes a restore writes are the same content.
//!   Each recorded write therefore keeps two Host-owned snapshots: the bytes
//!   *before* it (undo input, referenced by the *next* version) and the bytes
//!   *after* it (this version's own content).
//! * Restore is a pure file copy against a registered snapshot. It NEVER replays
//!   tools, creates Runs, grants permissions, or touches model state.
//! * Before a restore overwrites anything, the current file is compared with
//!   the latest registered `after_hash`. A mismatch (user edit, another task,
//!   deletion) blocks the restore unless the caller explicitly forces it, and
//!   the current bytes are themselves backed up first.
//! * Only writes the Host performed or verified are registered at all, and the
//!   restore path re-verifies the bytes it is about to copy. `force` overrides
//!   *content drift only*: it never bypasses conversation ownership, provenance,
//!   or snapshot integrity.
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::database::{
    now_ms, Database, ManagedFileSource, ManagedFileVersion, ManagedFileVersionInput,
};

pub(crate) fn hash_file(path: &Path) -> Option<(String, i64)> {
    let bytes = fs::read(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Some((hex::encode(hasher.finalize()), bytes.len() as i64))
}

fn hash_bytes(bytes: &[u8]) -> (String, i64) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    (hex::encode(hasher.finalize()), bytes.len() as i64)
}

/// Copy `source` into the Host-owned backup directory and return the new path.
fn store_snapshot(backups_dir: &Path, source: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(backups_dir)
        .map_err(|error| format!("cannot create managed-file backup directory: {error}"))?;
    let backup = backups_dir.join(format!("managed-{}.foxbak", uuid::Uuid::new_v4()));
    fs::copy(source, &backup)
        .map_err(|error| format!("failed to snapshot the managed file: {error}"))?;
    Ok(backup)
}

/// Pre-write snapshot. When the target already exists its bytes are copied
/// into the Host backup directory before any write touches the file.
pub(crate) struct BeforeCapture {
    pub storage_path: String,
    pub display_name: String,
    pub change_kind: &'static str, // created | modified
    pub before_hash: Option<String>,
    pub before_size: Option<i64>,
    pub backup_path: Option<PathBuf>,
}

pub(crate) fn capture_before(
    backups_dir: &Path,
    project_root: &Path,
    target: &Path,
) -> Result<BeforeCapture, String> {
    let existed = target.is_file();
    let (before_hash, before_size, backup_path) = if existed {
        let (hash, size) = hash_file(target)
            .ok_or_else(|| "cannot hash the existing managed file".to_string())?;
        let backup = store_snapshot(backups_dir, target)?;
        (Some(hash), Some(size), Some(backup))
    } else {
        (None, None, None)
    };
    let storage_path = target.to_string_lossy().into_owned();
    let display_name = target
        .strip_prefix(project_root)
        .map(|relative| relative.to_string_lossy().into_owned())
        .unwrap_or_else(|_| storage_path.clone());
    Ok(BeforeCapture {
        storage_path,
        display_name,
        change_kind: if existed { "modified" } else { "created" },
        before_hash,
        before_size,
        backup_path,
    })
}

/// Hash the post-write file, snapshot its content as this version's own bytes,
/// and append the version row.
///
/// The content snapshot is what makes "restore version *n*" mean *this*
/// content: `version n`'s bytes are kept next to `version n`'s row instead of
/// being inferred from a neighbouring row's undo copy. Errors are returned so
/// the caller can log them; callers must not fail the tool result because of
/// bookkeeping.
pub(crate) fn record_after(
    database: &Database,
    backups_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: Option<&str>,
    tool: &str,
    capture: BeforeCapture,
) -> Result<String, String> {
    let target = PathBuf::from(&capture.storage_path);
    let (after_hash, after_size) = hash_file(&target)
        .ok_or_else(|| "managed target is missing after the write".to_string())?;
    // A missing content snapshot must not silently degrade the row into
    // "unrestorable": fail the registration so the caller logs an honest gap.
    let after_backup = store_snapshot(backups_dir, &target)?;
    database.register_managed_file_version(
        &ManagedFileVersionInput {
            conversation_id,
            run_id: Some(run_id),
            tool_call_id,
            tool,
            storage_path: &capture.storage_path,
            display_name: &capture.display_name,
            change_kind: capture.change_kind,
            before_hash: capture.before_hash.as_deref(),
            before_size: capture.before_size,
            after_hash: Some(&after_hash),
            after_size: Some(after_size),
            backup_path: capture.backup_path.as_deref().and_then(Path::to_str),
            after_backup_path: after_backup.to_str(),
            restored_from_id: None,
            source: ManagedFileSource::HostCapture,
        },
        now_ms(),
    )
}

/// Register an Office-connector write whose backup the connector itself
/// created next to the document. `office_details` carries the hashes.
///
/// The caller must already have cross-checked every field against the Host's
/// own facts (verified tool, frozen connector identity, prepared target, and
/// the bytes on disk); this function only stores the result.
pub(crate) fn record_office_write(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: Option<&str>,
    details: &OfficeWriteDetails<'_>,
) -> Result<String, String> {
    database.register_managed_file_version(
        &ManagedFileVersionInput {
            conversation_id,
            run_id: Some(run_id),
            tool_call_id,
            tool: details.tool,
            storage_path: details.storage_path,
            display_name: details.display_name,
            change_kind: details.change_kind,
            before_hash: details.before_hash,
            before_size: details.before_size,
            after_hash: Some(details.after_hash),
            after_size: Some(details.after_size),
            backup_path: details.backup_path,
            after_backup_path: details.after_backup_path,
            restored_from_id: None,
            source: ManagedFileSource::OfficeConnector,
        },
        now_ms(),
    )
}

pub(crate) struct OfficeWriteDetails<'a> {
    pub tool: &'a str,
    pub storage_path: &'a str,
    pub display_name: &'a str,
    pub change_kind: &'a str,
    pub before_hash: Option<&'a str>,
    pub before_size: Option<i64>,
    pub after_hash: &'a str,
    pub after_size: i64,
    pub backup_path: Option<&'a str>,
    /// Host-copied snapshot of the saved document.
    pub after_backup_path: Option<&'a str>,
}

/// The Host-verified managed write behind one frozen dispatch.
///
/// A managed write is identified from Host facts only: the real dispatch tool,
/// the frozen connector identity and scope, and the target the Host itself
/// admits for this dispatch — never from a tool result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ManagedWrite {
    /// `write_file` / `edit_file`: the Host copies and hashes the file.
    HostFile {
        target: PathBuf,
        verified: VerifiedWriteTarget,
    },
    /// A built-in Office mutation. The production dispatch is always the MCP
    /// wrapper (`call_mcp_tool` with `serverId = fox-office`); the inner
    /// operation name and its arguments live in the wrapper's own input.
    Office {
        inner_tool: String,
        verified: VerifiedWriteTarget,
    },
}

/// Resolve the inner Office mutation of a `call_mcp_tool` dispatch.
///
/// Every condition is a Host fact, not a claim from the result:
/// * the wrapper really names the built-in connector;
/// * that connector is in the Run's *frozen* scope (its definition hash was
///   frozen at Run creation, so a swapped connector cannot register);
/// * the inner operation is authorized by the frozen scope as well;
/// * only the two document-mutating operations produce versions.
///
/// A generic MCP server never reaches the `Some` branch, even when its result
/// carries byte-identical `foxManagedFile` JSON.
pub(crate) fn frozen_office_operation<'a>(
    scope: &crate::database::KernelHostScope,
    tool: &str,
    input: &'a serde_json::Value,
) -> Option<(String, &'a serde_json::Value)> {
    use serde_json::Value;
    if tool != "call_mcp_tool" {
        return None;
    }
    let server_id = input.get("serverId").and_then(Value::as_str)?;
    if server_id != crate::office::SERVER_ID {
        return None;
    }
    if !scope.mcp_server_hashes.contains_key(server_id) {
        return None;
    }
    let inner = input.get("tool").and_then(Value::as_str)?;
    if !scope.office_tools.contains(inner) {
        return None;
    }
    if !matches!(inner, "office_create" | "office_edit") {
        return None;
    }
    Some((
        inner.to_owned(),
        input.get("arguments").unwrap_or(&Value::Null),
    ))
}

/// Classify one frozen dispatch as a managed write, using only Host-verified
/// facts: the frozen permission root, the frozen connector scope and the
/// dispatch's own arguments.
pub(crate) fn verified_managed_write(
    root: &Path,
    permission_mode: &str,
    scope: &crate::database::KernelHostScope,
    tool: &str,
    input: &serde_json::Value,
) -> Option<ManagedWrite> {
    let root_string = root.to_string_lossy().into_owned();
    match tool {
        "write_file" | "edit_file" => {
            let target = crate::tool_host::prepare(tool, input, &root_string)
                .ok()?
                .target_path()?
                .to_path_buf();
            Some(ManagedWrite::HostFile {
                verified: VerifiedWriteTarget::new(root, &target),
                target,
            })
        }
        "call_mcp_tool" => {
            let (inner_tool, arguments) = frozen_office_operation(scope, tool, input)?;
            // The Host re-runs the admission the gateway will run, so the target
            // used for registration is the one this dispatch was approved for.
            let target = crate::office::prepare(
                &inner_tool,
                arguments,
                Some(&root_string),
                permission_mode,
            )
            .ok()?
            .target_path()?
            .to_path_buf();
            Some(ManagedWrite::Office {
                inner_tool,
                verified: VerifiedWriteTarget::new(root, &target),
            })
        }
        _ => None,
    }
}

/// Verify and register the managed-file declaration of a completed Office
/// mutation.
/// Both dispatch paths — the Kernel gateway's `call_mcp_tool` wrapper and the
/// Legacy MCP proxy — call this one function with the target they verified
/// themselves, so "may this become a restorable version?" has exactly one
/// implementation. `inner_tool` is the Office operation that actually ran
/// (`office_create` / `office_edit`), taken from the wrapper's own input, never
/// from the result.
pub(crate) fn record_office_write_from_result(
    database: &Database,
    backups_dir: &Path,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: Option<&str>,
    verified_target: &VerifiedWriteTarget,
    inner_tool: &str,
    result: &serde_json::Value,
) -> Result<(), String> {
    let details = result
        .get("details")
        .and_then(serde_json::Value::as_object)
        .and_then(|details| details.get("foxManagedFile"))
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "the Office result carries no managed-file declaration".to_string())?;
    let verified = verify_office_write(backups_dir, verified_target, inner_tool, details)?;
    record_office_write(
        database,
        conversation_id,
        run_id,
        tool_call_id,
        &OfficeWriteDetails {
            tool: &verified.tool,
            storage_path: &verified.storage_path,
            display_name: &verified.display_name,
            change_kind: &verified.change_kind,
            before_hash: verified.before_hash.as_deref(),
            before_size: verified.before_size,
            after_hash: &verified.after_hash,
            after_size: verified.after_size,
            backup_path: verified.backup_path.as_deref(),
            after_backup_path: Some(&verified.after_backup_path),
        },
    )?;
    Ok(())
}

/// What the shared execution seam needs to decide and record a managed write.
///
/// Deliberately a handful of Host facts rather than a whole binding: the seam is
/// called by the desktop Host *and* by the real-task evaluation, and both must
/// classify a write from the same facts.
#[derive(Clone, Copy)]
pub(crate) struct ManagedExecutionContext<'a> {
    pub database: &'a Database,
    pub backups_dir: &'a Path,
    pub conversation_id: &'a str,
    pub run_id: &'a str,
    pub project_root: Option<&'a str>,
    pub permission_mode: &'a str,
    pub scope: &'a crate::database::KernelHostScope,
}

/// Run one Host dispatch with managed-file bookkeeping around it.
///
/// This is the single execution seam for a file-mutating dispatch: it classifies
/// the write from **Host facts** (never from the result), protects the current
/// bytes before the write, runs the dispatch, and registers the resulting content
/// version. It is deliberately UI-free so the real-task evaluation drives the
/// same path as the desktop Host: an evaluation that called the gateway directly
/// would execute real Office writes while registering nothing, which made
/// "the eval wrote a document" and "the user can restore that document" two
/// different things.
pub(crate) fn execute_with_managed_versions<F>(
    context: ManagedExecutionContext<'_>,
    tool: &str,
    input: &serde_json::Value,
    tool_call_id: Option<&str>,
    execute: F,
) -> Result<serde_json::Value, String>
where
    F: FnOnce() -> Result<serde_json::Value, String>,
{
    let managed = context.project_root.and_then(|root| {
        verified_managed_write(
            Path::new(root),
            context.permission_mode,
            context.scope,
            tool,
            input,
        )
    });
    let capture = match &managed {
        Some(ManagedWrite::HostFile { target, .. }) => capture_before(
            context.backups_dir,
            Path::new(context.project_root.unwrap_or_default()),
            target,
        )
        .ok(),
        _ => None,
    };
    let outcome = execute();
    if let Ok(ref result) = outcome {
        let business_ok = result.get("isError").and_then(serde_json::Value::as_bool) != Some(true);
        // Bookkeeping is best-effort: a hash or registry failure never turns an
        // authorized write into a failed tool call.
        if business_ok {
            if let Some(capture) = capture {
                if let Err(error) = record_after(
                    context.database,
                    context.backups_dir,
                    context.conversation_id,
                    context.run_id,
                    tool_call_id,
                    tool,
                    capture,
                ) {
                    eprintln!(
                        "managed-file version registration failed after {tool} {}: {error}",
                        tool_call_id.unwrap_or("?")
                    );
                }
            }
            if let Some(ManagedWrite::Office {
                inner_tool,
                verified,
            }) = &managed
            {
                if let Err(reason) = record_office_write_from_result(
                    context.database,
                    context.backups_dir,
                    context.conversation_id,
                    context.run_id,
                    tool_call_id,
                    verified,
                    inner_tool,
                    result,
                ) {
                    eprintln!(
                        "refused an unverified Office managed-file declaration for {inner_tool} \
                         {}: {reason}",
                        tool_call_id.unwrap_or("?")
                    );
                }
            }
        }
    }
    outcome
}

/// Where a version's content comes from, plus why it cannot be recovered when
/// no source verifies.
#[derive(Debug, Clone)]
pub(crate) struct RestoreSource {
    /// File holding the exact registered bytes.
    pub path: PathBuf,
    pub sha256: String,
    pub size: i64,
    /// `version` = the row's own content snapshot; `undo_of_next` = the
    /// pre-write copy taken by this path's next recorded write, which holds
    /// this version's bytes.
    pub origin: &'static str,
    /// Which row the bytes were read for, for auditing a legacy recovery.
    pub origin_version_no: i64,
}

/// Why one version cannot be restored, in user-facing terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RestoreBlocker {
    /// The row predates content snapshots.
    LegacyWithoutSnapshot,
    /// The row's own facts were never verified by the Host.
    UnverifiedOrigin,
    /// Registered hashes resolve to a file whose bytes do not match.
    SnapshotMissing,
    /// The bytes that were found are not the bytes this version recorded.
    SnapshotMismatch,
}

impl RestoreBlocker {
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            RestoreBlocker::LegacyWithoutSnapshot => {
                "该版本记录于内容快照启用之前，其自身内容无法验证，不能用于恢复（记录保持不变）"
            }
            RestoreBlocker::UnverifiedOrigin => {
                "该版本的内容来源未经 Host 验证，不能用于恢复"
            }
            RestoreBlocker::SnapshotMissing => "该版本的内容快照文件已丢失或不可读",
            RestoreBlocker::SnapshotMismatch => {
                "该版本的内容快照与登记的哈希/大小不一致，拒绝用不确定的内容覆盖文件"
            }
        }
    }
}

/// Resolve the registered content of one version.
///
/// Order of evidence, all hash-checked against the row's own `after_hash`:
/// 1. the row's own content snapshot;
/// 2. the pre-write copy taken by the next recorded write of the same path,
///    which by definition holds the bytes the next row replaced — i.e. this
///    row's content. Its `before_hash`/`before_size` must equal this row's
///    `after_hash`/`after_size` so the two rows provably describe one state.
///
/// Nothing is guessed: when neither source verifies, the caller refuses.
pub(crate) fn resolve_restore_source(
    database: &Database,
    version: &ManagedFileVersion,
) -> Result<RestoreSource, String> {
    resolve_restore_source_inner(database, version)
        .map_err(|blocker| blocker.reason().to_owned())
}

/// Why one version cannot be restored, or `None` when its content resolves.
///
/// The UI and the restore path call this same function, so a selectable version
/// is always a restorable one.
pub(crate) fn restore_blocker(
    database: &Database,
    version: &ManagedFileVersion,
) -> Option<String> {
    resolve_restore_source_inner(database, version)
        .err()
        .map(|blocker| blocker.reason().to_owned())
}

fn resolve_restore_source_inner(
    database: &Database,
    version: &ManagedFileVersion,
) -> Result<RestoreSource, RestoreBlocker> {
    if !version.source_verified {
        return Err(RestoreBlocker::UnverifiedOrigin);
    }
    let Some(expected_hash) = version.after_hash.as_deref() else {
        return Err(RestoreBlocker::SnapshotMissing);
    };

    let mut candidates: Vec<(PathBuf, &'static str, i64)> = Vec::new();
    if let Some(path) = version.after_backup_path.as_deref() {
        candidates.push((PathBuf::from(path), "version", version.version_no));
    }
    let mut has_verified_undo = false;
    let rows = database
        .managed_file_versions_for_path(&version.storage_path)
        .map_err(|_| RestoreBlocker::SnapshotMissing)?;
    for row in rows {
        if row.version_no <= version.version_no {
            continue;
        }
        if row.before_hash.as_deref() == Some(expected_hash)
            && row.before_size == version.after_size
        {
            if row.source_verified {
                has_verified_undo = true;
            }
            if let Some(path) = row.backup_path.as_deref() {
                candidates.push((PathBuf::from(path), "undo_of_next", row.version_no));
            }
        }
    }

    for (path, origin, origin_version_no) in candidates {
        let Some((actual_hash, actual_size)) = hash_file(&path) else {
            continue;
        };
        if actual_hash == expected_hash && Some(actual_size) == version.after_size {
            return Ok(RestoreSource {
                path,
                sha256: actual_hash,
                size: actual_size,
                origin,
                origin_version_no,
            });
        }
    }
    if version.after_backup_path.is_some() {
        return Err(RestoreBlocker::SnapshotMismatch);
    }
    if has_verified_undo {
        return Err(RestoreBlocker::SnapshotMissing);
    }
    Err(RestoreBlocker::LegacyWithoutSnapshot)
}

/// Result of a successful restore: the newly registered `restored` row.
pub(crate) fn restore_version(
    database: &Database,
    backups_dir: &Path,
    conversation_id: &str,
    version_id: &str,
    force: bool,
) -> Result<ManagedFileVersion, String> {
    let version = database
        .managed_file_version(version_id)?
        .ok_or_else(|| "managed file version not found".to_string())?;
    if version.conversation_id != conversation_id {
        return Err("managed file version belongs to another conversation".into());
    }
    // The exact bytes this version records, verified against its own hash. A
    // version is never restored by copying some other version's backup.
    let source = resolve_restore_source(database, &version)?;

    let target = PathBuf::from(&version.storage_path);
    let latest = database.latest_managed_file_version(conversation_id, &version.storage_path)?;
    let current = hash_file(&target);
    if !force {
        match (&latest, &current) {
            (Some(latest), Some((hash, _))) if Some(hash.as_str()) == latest.after_hash.as_deref() => {}
            (Some(_), None) => {
                return Err(format!(
                    "{} was deleted after the last recorded change; re-select it with force to restore anyway",
                    version.display_name
                ));
            }
            _ => {
                return Err(format!(
                    "{} changed outside the recorded task history (another edit, another task, or an external program); \
                     re-select it with force to overwrite the current contents",
                    version.display_name
                ));
            }
        }
    }

    // The current bytes always get their own safety backup before overwrite.
    let safety_backup = if let Some((ref hash, ref size)) = current {
        let safety = store_snapshot(backups_dir, &target)?;
        Some((safety, hash.clone(), *size))
    } else {
        None
    };

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot recreate the target folder: {error}"))?;
    }
    fs::copy(&source.path, &target).map_err(|error| {
        format!("failed to write the restored contents: {error}")
    })?;
    let (after_hash, after_size) = hash_file(&target)
        .ok_or_else(|| "restored target is unreadable".to_string())?;
    // Post-condition: the file now holds exactly the selected content version.
    if after_hash != source.sha256 || after_size != source.size {
        return Err(
            "restored file does not match the selected version; the copy was not applied".into(),
        );
    }
    // Restored content is itself a version of this file: snapshot it so this
    // restore is selectable (and re-restorable) like any other version.
    let after_backup = store_snapshot(backups_dir, &target)?;

    let (safety_path, safety_hash, safety_size) = match &safety_backup {
        Some((path, hash, size)) => (
            Some(path.to_string_lossy().into_owned()),
            Some(hash.clone()),
            Some(*size),
        ),
        None => (None, None, None),
    };
    // Content this restore is about to overwrite gets its own content version,
    // not just a backup file: otherwise bytes the registry never knew about (a
    // user edit made outside any recorded task, another program's write) could
    // never be brought back through the product, because every version row
    // resolves its *own* content. The row is `replaced` — the content this
    // restore replaced — and it is registered before the restored row so the
    // per-path version order stays chronological. The append-only registry is
    // untouched: this only appends.
    if let (Some(path), Some(hash), Some(size)) = (&safety_path, &safety_hash, &safety_size) {
        if let Err(error) = database.register_managed_file_version(
            &ManagedFileVersionInput {
                conversation_id,
                run_id: None,
                tool_call_id: None,
                tool: "restore",
                storage_path: &version.storage_path,
                display_name: &version.display_name,
                change_kind: "replaced",
                before_hash: None,
                before_size: None,
                after_hash: Some(hash.as_str()),
                after_size: Some(*size),
                backup_path: None,
                after_backup_path: Some(path.as_str()),
                restored_from_id: None,
                source: ManagedFileSource::Restore,
            },
            now_ms(),
        ) {
            eprintln!(
                "managed-file registration of the replaced content failed for {}: {error}",
                version.display_name
            );
        }
    }
    let new_id = database.register_managed_file_version(
        &ManagedFileVersionInput {
            conversation_id,
            run_id: None,
            tool_call_id: None,
            tool: "restore",
            storage_path: &version.storage_path,
            display_name: &version.display_name,
            change_kind: "restored",
            before_hash: safety_hash.as_deref(),
            before_size: safety_size,
            after_hash: Some(&after_hash),
            after_size: Some(after_size),
            backup_path: safety_path.as_deref(),
            after_backup_path: after_backup.to_str(),
            restored_from_id: Some(&version.id),
            source: ManagedFileSource::Restore,
        },
        now_ms(),
    )?;
    database
        .managed_file_version(&new_id)?
        .ok_or_else(|| "restored version row vanished".to_string())
}

/// The bytes a Host-verified write can be registered from: everything the Host
/// derived itself, never a value taken from a tool result at face value.
/// Which bytes a Host-verified dispatch will write, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VerifiedWriteTarget {
    /// Canonical path of the file the Host admitted for this dispatch.
    pub storage_path: String,
    pub display_name: String,
}

impl VerifiedWriteTarget {
    pub(crate) fn new(root: &Path, storage_path: &Path) -> VerifiedWriteTarget {
        let storage_path = storage_path.to_string_lossy().into_owned();
        let display_name = storage_path
            .parse::<PathBuf>()
            .ok()
            .and_then(|path| path.strip_prefix(root).map(|value| value.to_path_buf()).ok())
            .map(|relative| relative.to_string_lossy().into_owned())
            .unwrap_or_else(|| storage_path.clone());
        VerifiedWriteTarget {
            storage_path,
            display_name,
        }
    }
}

/// Accept or reject a connector-declared `details.foxManagedFile` block.
///
/// A connector result is *data*, even when it comes from the built-in Office
/// connector: the Host registers a version only when its own verified facts
/// agree with the declaration. Rejections are returned (never silently
/// accepted) so the caller can log exactly which invariant failed.
pub(crate) struct VerifiedOfficeWrite {
    pub tool: String,
    pub storage_path: String,
    pub display_name: String,
    pub change_kind: String,
    pub before_hash: Option<String>,
    pub before_size: Option<i64>,
    pub after_hash: String,
    pub after_size: i64,
    pub backup_path: Option<String>,
    pub after_backup_path: String,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn verify_office_write(
    backups_dir: &Path,
    target: &VerifiedWriteTarget,
    tool: &str,
    declared: &serde_json::Map<String, serde_json::Value>,
) -> Result<VerifiedOfficeWrite, String> {
    let string = |key: &str| declared.get(key).and_then(|value| value.as_str());
    // 1) The Host's own facts: the actual tool, the prepared target path and
    //    the bytes on disk. Nothing here is read from the tool result.
    let declared_tool = string("tool").unwrap_or_default();
    if declared_tool != tool {
        return Err(format!(
            "connector declared tool {declared_tool:?} but the Host dispatched {tool:?}"
        ));
    }
    if !matches!(tool, "office_create" | "office_edit") {
        return Err("only office_create/office_edit produce managed versions".into());
    }
    let declared_path = string("storagePath").unwrap_or_default();
    let declared_path = PathBuf::from(declared_path);
    let declared_path = declared_path.canonicalize().unwrap_or(declared_path);
    let target_path = PathBuf::from(&target.storage_path);
    let target_canonical = target_path.canonicalize().unwrap_or(target_path);
    if declared_path != target_canonical {
        return Err(format!(
            "connector declared target {} but the Host verified {}",
            declared_path.display(),
            target_canonical.display()
        ));
    }
    let (actual_hash, actual_size) = hash_file(&target_canonical)
        .ok_or_else(|| "the Office target is missing after the write".to_string())?;
    let declared_after = string("afterHash").unwrap_or_default();
    let declared_after_size = declared.get("afterSize").and_then(|value| value.as_i64());
    if declared_after != actual_hash || declared_after_size != Some(actual_size) {
        return Err(
            "connector-declared content hash/size does not match the file the Host verified"
                .into(),
        );
    }
    // 2) A declared pre-write backup must itself hold the declared before
    //    bytes, otherwise the undo of this write would restore unknown content.
    let backup_path = string("backupPath").map(str::to_owned);
    let before_hash = string("beforeHash").map(str::to_owned);
    let before_size = declared.get("beforeSize").and_then(|value| value.as_i64());
    match (&backup_path, &before_hash) {
        (Some(path), Some(expected)) => {
            let (hash, size) = hash_file(Path::new(path))
                .ok_or_else(|| "the declared Office backup is unreadable".to_string())?;
            if &hash != expected || Some(size) != before_size {
                return Err("the declared Office backup does not match its recorded hash".into());
            }
        }
        (None, None) => {}
        _ => {
            return Err(
                "the connector declared a partial before-write state (hash or backup missing)"
                    .into(),
            )
        }
    }
    // 3) Host-owned snapshot of the content this version records. The display
    //    name comes from the Host's own root-relative path, never the result.
    let after_backup = store_snapshot(backups_dir, &target_canonical)?;
    Ok(VerifiedOfficeWrite {
        tool: tool.to_owned(),
        storage_path: target.storage_path.clone(),
        display_name: target.display_name.clone(),
        change_kind: if before_hash.is_some() {
            "modified".to_owned()
        } else {
            "created".to_owned()
        },
        before_hash,
        before_size,
        after_hash: actual_hash,
        after_size: actual_size,
        backup_path,
        after_backup_path: after_backup.to_string_lossy().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database, conversation and project folder for one version test. The
    /// `runs` rows required by the registry FK are seeded over a dedicated
    /// SQLite connection (Database keeps its pool private).
    struct Harness {
        db: Database,
        db_path: PathBuf,
        root: PathBuf,
        backups: PathBuf,
        conversation: String,
    }

    impl Harness {
        fn new() -> Self {
            let db_path =
                std::env::temp_dir().join(format!("fox-managed-test-{}.db", uuid::Uuid::new_v4()));
            let db = Database::open(db_path.clone()).expect("open test database");
            let conversation = db
                .create_conversation("fox-general", None, None, None)
                .expect("create conversation")
                .id;
            let root = std::env::temp_dir().join(format!(
                "fox-managed-root-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&root).unwrap();
            let backups = root.join("_backups");
            Self {
                db,
                db_path,
                root,
                backups,
                conversation,
            }
        }

        fn seed_run(&self, run: &str) {
            let conn = rusqlite::Connection::open(&self.db_path).expect("seed connection");
            conn.execute(
                "INSERT OR IGNORE INTO runs(id, conversation_id, status, model, created_at)
                 VALUES (?1, ?2, 'completed', 'test-model', ?3)",
                rusqlite::params![run, self.conversation, now_ms()],
            )
            .expect("seed run");
        }

        fn target(&self, relative: &str) -> PathBuf {
            self.root.join(relative)
        }

        fn write_file(&self, relative: &str, contents: &str) -> PathBuf {
            let path = self.target(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&path, contents).unwrap();
            path
        }

        /// Capture before, replace contents, record after; returns version id.
        fn recorded_write(&self, run: &str, relative: &str, contents: &str) -> String {
            self.seed_run(run);
            let target = self.target(relative);
            let capture =
                capture_before(&self.backups, &self.root, &target).expect("capture before");
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(&target, contents).unwrap();
            record_after(
                &self.db,
                &self.backups,
                &self.conversation,
                run,
                Some(&format!("call-{}", uuid::Uuid::new_v4())),
                "write_file",
                capture,
            )
            .expect("record after")
        }

        fn versions(&self, path: Option<&Path>) -> Vec<ManagedFileVersion> {
            self.db
                .managed_file_versions(
                    &self.conversation,
                    path.map(|value| value.to_string_lossy().into_owned()).as_deref(),
                )
                .expect("list versions")
        }

        fn restore(&self, version_id: &str, force: bool) -> Result<ManagedFileVersion, String> {
            restore_version(
                &self.db,
                &self.backups,
                &self.conversation,
                version_id,
                force,
            )
        }
    }

    fn contents_of(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    /// F2: the shared execution seam is what the desktop Host *and* the real-task
    /// evaluation drive, so an evaluation that writes a real document registers a
    /// restorable version exactly like the app does. A tool the seam does not
    /// recognize as a managed write stays unregistered even when its result
    /// carries the same declaration fields.
    #[test]
    fn the_shared_execution_seam_registers_exactly_what_it_executes() {
        let harness = Harness::new();
        harness.seed_run("run-seam");
        let scope = crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: Default::default(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: Default::default(),
            lifecycle_hooks: Vec::new(),
        };
        let root = harness.root.to_string_lossy().into_owned();
        let context = ManagedExecutionContext {
            database: &harness.db,
            backups_dir: &harness.backups,
            conversation_id: harness.conversation.as_str(),
            run_id: "run-seam",
            project_root: Some(root.as_str()),
            permission_mode: "allow",
            scope: &scope,
        };

        // A managed Host write goes through the seam: the callback runs and a
        // verified version row exists for the bytes it wrote.
        let mut ran = false;
        let result = execute_with_managed_versions(
            context,
            "write_file",
            &serde_json::json!({"path": "seam.txt", "content": "seam-content"}),
            Some("call-seam"),
            || {
                ran = true;
                fs::write(harness.target("seam.txt"), "seam-content").unwrap();
                Ok(serde_json::json!({"content":[{"type":"text","text":"written"}]}))
            },
        )
        .expect("the seam returns the dispatch result");
        assert!(ran, "the seam must execute the dispatch it was given");
        assert_eq!(result["content"][0]["text"], "written");
        let rows = harness.versions(None);
        assert_eq!(rows.len(), 1, "the executed write must be registered: {rows:?}");
        let row = &rows[0];
        assert_eq!(row.tool, "write_file");
        assert_eq!(row.tool_call_id.as_deref(), Some("call-seam"));
        assert!(row.source_verified);
        assert_eq!(
            row.after_hash.as_deref(),
            Some(hash_file(&harness.target("seam.txt")).unwrap().0.as_str())
        );
        // And it is restorable to exactly those bytes.
        let restored = harness.restore(&row.id, false).expect("restore");
        assert_eq!(restored.change_kind, "restored");
        assert_eq!(contents_of(&harness.target("seam.txt")), "seam-content");

        // A tool the seam does not treat as a managed write registers nothing,
        // even though it also wrote a file and announced it the same way.
        let before = harness.versions(None).len();
        let mut ran = false;
        execute_with_managed_versions(
            context,
            "mcp__other__write_file",
            &serde_json::json!({"path": "other.txt"}),
            Some("call-other"),
            || {
                ran = true;
                fs::write(harness.target("other.txt"), "other-content").unwrap();
                Ok(serde_json::json!({"details":{"foxManagedFile":{"path":"other.txt"}}}))
            },
        )
        .expect("the seam still runs the dispatch");
        assert!(ran);
        assert_eq!(
            harness.versions(None).len(),
            before,
            "a non-managed tool must not become a managed file version"
        );
    }

    #[test]
    fn created_file_registers_a_version_without_backup() {
        let harness = Harness::new();
        let target = harness.target("notes.txt");

        let capture = capture_before(&harness.backups, &harness.root, &target).expect("capture");
        assert_eq!(capture.change_kind, "created");
        assert!(capture.backup_path.is_none());
        fs::write(&target, "hello").unwrap();
        harness.seed_run("run-1");
        let id = record_after(
            &harness.db,
            &harness.backups,
            &harness.conversation,
            "run-1",
            Some("call-1"),
            "write_file",
            capture,
        )
        .expect("register");

        let rows = harness.versions(None);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.id, id);
        assert_eq!(row.version_no, 1);
        assert_eq!(row.change_kind, "created");
        assert_eq!(row.run_id.as_deref(), Some("run-1"));
        assert_eq!(row.tool_call_id.as_deref(), Some("call-1"));
        assert_eq!(row.source, ManagedFileSource::HostCapture);
        assert!(row.source_verified);
        assert!(row.before_hash.is_none());
        assert!(row.before_size.is_none());
        let (hash, size) = hash_file(&target).expect("hash target");
        assert_eq!(row.after_hash.as_deref(), Some(hash.as_str()));
        assert_eq!(row.after_size, Some(size));
        assert!(row.backup_path.is_none());
        // The created content itself is snapshotted and therefore restorable.
        let after_backup = row.after_backup_path.as_ref().expect("content snapshot");
        assert_eq!(contents_of(Path::new(after_backup)), "hello");
        let restored = harness.restore(&id, false).expect("created version restores");
        assert_eq!(restored.change_kind, "restored");
        assert_eq!(contents_of(&target), "hello");
    }

    #[test]
    fn modified_file_is_backed_up_before_the_write() {
        let harness = Harness::new();
        let target = harness.write_file("doc.txt", "original bytes");

        let capture = capture_before(&harness.backups, &harness.root, &target).expect("capture");
        assert_eq!(capture.change_kind, "modified");
        let backup_path = capture.backup_path.clone().expect("backup created");
        assert_eq!(
            fs::read_to_string(&backup_path).unwrap(),
            "original bytes",
            "backup holds the pre-write bytes"
        );
        assert_eq!(
            capture.before_hash.as_deref(),
            hash_file(&target).map(|(hash, _)| hash).as_deref()
        );

        fs::write(&target, "changed bytes").unwrap();
        harness.seed_run("run-1");
        record_after(
            &harness.db,
            &harness.backups,
            &harness.conversation,
            "run-1",
            Some("call-1"),
            "edit_file",
            capture,
        )
        .expect("register");

        let rows = harness.versions(Some(&target));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.change_kind, "modified");
        assert_eq!(
            fs::read_to_string(row.backup_path.as_ref().unwrap()).unwrap(),
            "original bytes"
        );
        assert_ne!(row.before_hash, row.after_hash);
    }

    /// R1: `version_no`, the recorded size and hash, and the bytes a restore
    /// writes must describe the SAME content, for every kind of version.
    #[test]
    fn selecting_a_version_restores_that_versions_exact_bytes() {
        let harness = Harness::new();
        let target = harness.write_file("chain.txt", "content A");

        let created_a = harness.recorded_write("run-1", "chain.txt", "content A");
        let version_b = harness.recorded_write("run-1", "chain.txt", "content B");
        let version_c = harness.recorded_write("run-1", "chain.txt", "content C");
        assert_eq!(contents_of(&target), "content C");

        let rows = harness.versions(Some(&target));
        assert_eq!(rows.len(), 3);
        let size_of = |contents: &str| contents.len() as i64;

        // Each row's displayed size equals the size of its own content, not of
        // the content it replaced.
        assert_eq!(rows[0].after_size, Some(size_of("content A")));
        assert_eq!(rows[1].after_size, Some(size_of("content B")));
        assert_eq!(rows[2].after_size, Some(size_of("content C")));

        // Restoring B yields B, not A (the old off-by-one regression).
        let restored_b = harness.restore(&version_b, false).expect("restore B");
        assert_eq!(contents_of(&target), "content B");
        assert_eq!(restored_b.after_size, Some(size_of("content B")));

        // Restoring A yields A, and C is still retrievable afterwards.
        let restored_a = harness.restore(&created_a, false).expect("restore A");
        assert_eq!(contents_of(&target), "content A");
        assert_eq!(restored_a.after_size, Some(size_of("content A")));
        assert_eq!(
            restored_a.restored_from_id.as_deref(),
            Some(created_a.as_str())
        );

        // The bytes that the A-restore overwrote (C) were safety-backed up and
        // that backup is registered as its own restorable version.
        let overwritten = restored_a
            .backup_path
            .as_ref()
            .expect("safety backup of the overwritten bytes");
        assert_eq!(contents_of(Path::new(overwritten)), "content B");
        let rows_after = harness.versions(Some(&target));
        let safety_row = rows_after
            .iter()
            .find(|row| row.id == restored_a.id)
            .expect("restored row");
        assert_eq!(
            safety_row.before_hash.as_deref(),
            hash_file(Path::new(overwritten)).map(|(hash, _)| hash).as_deref()
        );

        // And every row of the chain still resolves to its own content.
        for (id, expected) in [
            (&created_a, "content A"),
            (&version_b, "content B"),
            (&version_c, "content C"),
        ] {
            let row = harness
                .db
                .managed_file_version(id)
                .unwrap()
                .expect("version row");
            let source = resolve_restore_source(&harness.db, &row).expect("content resolvable");
            assert_eq!(
                contents_of(&source.path),
                expected,
                "version {} must resolve to its own content",
                row.version_no
            );
        }
    }

    #[test]
    fn restore_copies_backup_without_replaying_and_registers_restored_row() {
        let harness = Harness::new();
        let target = harness.write_file("doc.txt", "original bytes");

        let first = harness.recorded_write("run-1", "doc.txt", "first edit");
        let _second = harness.recorded_write("run-1", "doc.txt", "second edit");
        assert_eq!(fs::read_to_string(&target).unwrap(), "second edit");

        let restored = harness.restore(&first, false).expect("restore first version");

        assert_eq!(fs::read_to_string(&target).unwrap(), "first edit");
        assert_eq!(restored.change_kind, "restored");
        assert_eq!(restored.restored_from_id.as_deref(), Some(first.as_str()));
        // Restore never replays tools or creates a Run: the row carries no
        // run/tool-call identity.
        assert_eq!(restored.tool, "restore");
        assert_eq!(restored.source, ManagedFileSource::Restore);
        assert!(restored.run_id.is_none());
        assert!(restored.tool_call_id.is_none());
        // Bytes about to be overwritten were safety-backed-up first.
        let safety = restored.backup_path.as_ref().expect("safety backup");
        assert_eq!(fs::read_to_string(safety).unwrap(), "second edit");
        assert_eq!(
            restored.before_hash.as_deref(),
            hash_file(Path::new(safety)).map(|(h, _)| h).as_deref()
        );

        let rows = harness.versions(Some(&target));
        // Append-only: the restore preserves the content it replaced as its own
        // version row, then appends the restored row. Nothing is rewritten.
        assert_eq!(rows.len(), 4, "append-only: replaced + restored are appended");
        assert_eq!(rows[2].change_kind, "replaced");
        assert_eq!(rows[3].id, restored.id);
        assert_eq!(rows[3].version_no, 4);
    }

    /// N6: content that a forced restore overwrites must be recoverable through
    /// the product, not only by reading a backup file by hand.
    ///
    /// A → B (recorded) → X (edited outside any recorded task) → force-restore A.
    /// X was never a registered version, so before this fix no version row could
    /// bring it back: the registry only ever resolves a row's *own* content.
    #[test]
    fn the_content_replaced_by_a_force_restore_is_itself_selectable() {
        let harness = Harness::new();
        let target = harness.write_file("doc.txt", "content A");
        let version_a = harness.recorded_write("run-1", "doc.txt", "content A");
        let _version_b = harness.recorded_write("run-1", "doc.txt", "content B");
        // Another program writes X; the registry knows nothing about it.
        fs::write(&target, "content X from outside").unwrap();

        // Selecting A without force is refused (drift), with force it succeeds.
        assert!(harness.restore(&version_a, false).is_err());
        let restored = harness.restore(&version_a, true).expect("force restore A");
        assert_eq!(contents_of(&target), "content A");

        // The overwritten X is now a first-class content version of this file.
        let rows = harness.versions(Some(&target));
        let replaced = rows
            .iter()
            .find(|row| row.change_kind == "replaced")
            .expect("the replaced content must be registered as a version");
        assert_eq!(
            replaced.after_size,
            Some("content X from outside".len() as i64)
        );
        assert!(
            replaced.source_verified,
            "the Host snapshotted these bytes itself, so the row is verified"
        );
        // …and selecting it through the public restore entry brings X back.
        let back = harness
            .restore(&replaced.id, false)
            .expect("X is selectable and restorable");
        assert_eq!(contents_of(&target), "content X from outside");
        assert_eq!(back.change_kind, "restored");
        assert_eq!(
            back.restored_from_id.as_deref(),
            Some(replaced.id.as_str()),
            "the restored row records which version it came from"
        );
        // A is still retrievable afterwards: nothing was consumed.
        harness.restore(&version_a, true).expect("A again");
        assert_eq!(contents_of(&target), "content A");
        let _ = restored;
    }

    #[test]
    fn external_change_blocks_restore_until_force() {
        let harness = Harness::new();
        let target = harness.write_file("doc.txt", "original bytes");
        let first = harness.recorded_write("run-1", "doc.txt", "first edit");
        harness.recorded_write("run-1", "doc.txt", "second edit");

        fs::write(&target, "touched by another program").unwrap();
        let error = harness
            .restore(&first, false)
            .expect_err("drift must block restore");
        assert!(error.contains("changed outside"), "unexpected error: {error}");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "touched by another program",
            "blocked restore leaves the file untouched"
        );

        let restored = harness.restore(&first, true).expect("force overrides drift");
        assert_eq!(fs::read_to_string(&target).unwrap(), "first edit");
        let safety = restored.backup_path.expect("safety backup path");
        assert_eq!(
            fs::read_to_string(Path::new(&safety)).unwrap(),
            "touched by another program",
            "the drifted bytes remain recoverable via the safety backup"
        );
    }

    #[test]
    fn deletion_blocks_restore_and_force_recreates_the_file() {
        let harness = Harness::new();
        let target = harness.write_file("nested/doc.txt", "original bytes");
        let first = harness.recorded_write("run-1", "nested/doc.txt", "first edit");

        fs::remove_file(&target).unwrap();
        let error = harness
            .restore(&first, false)
            .expect_err("deletion must block restore");
        assert!(error.contains("deleted"), "unexpected error: {error}");

        let restored = harness
            .restore(&first, true)
            .expect("force recreates the file");
        assert_eq!(fs::read_to_string(&target).unwrap(), "first edit");
        assert_eq!(restored.change_kind, "restored");
        assert!(
            restored.before_hash.is_none(),
            "nothing to safety-back up when the file was gone"
        );
    }

    #[test]
    fn restore_refuses_other_conversations_and_missing_backups() {
        let harness = Harness::new();
        harness.write_file("doc.txt", "original bytes");
        let first = harness.recorded_write("run-1", "doc.txt", "first edit");
        let other = harness
            .db
            .create_conversation("fox-general", None, None, None)
            .expect("second conversation")
            .id;

        let error = restore_version(
            &harness.db,
            &harness.backups,
            &other,
            &first,
            false,
        )
        .expect_err("cross-conversation restore must be refused");
        assert!(error.contains("another conversation"), "unexpected error: {error}");

        // Tamper with the registered content snapshot: integrity check must fire
        // even with force.
        let row = harness.db.managed_file_version(&first).unwrap().unwrap();
        let snapshot = row.after_backup_path.clone().unwrap();
        fs::write(&snapshot, "tampered snapshot").unwrap();
        let error = harness
            .restore(&first, true)
            .expect_err("tampered snapshot must be rejected even with force");
        assert!(
            error.contains("不一致") || error.contains("丢失"),
            "unexpected error: {error}"
        );

        // A row whose provenance was never verified is never restorable.
        fs::write(&snapshot, "first edit").unwrap();
        let untrusted = harness.recorded_write("run-1", "doc.txt", "second edit");
        let conn = rusqlite::Connection::open(&harness.db_path).unwrap();
        conn.execute(
            "UPDATE managed_file_versions SET source_kind='legacy_unknown', source_verified=0
             WHERE id=?1",
            rusqlite::params![untrusted],
        )
        .unwrap();
        drop(conn);
        let error = harness
            .restore(&untrusted, false)
            .expect_err("unverified provenance must be refused");
        assert!(error.contains("未经 Host 验证"), "unexpected error: {error}");
    }

    /// A snapshot that holds different bytes than the row records must never be
    /// written over the live file, with or without force.
    #[test]
    fn a_snapshot_that_disagrees_with_its_registered_hash_is_refused() {
        let harness = Harness::new();
        let target = harness.write_file("doc.txt", "original bytes");
        let first = harness.recorded_write("run-1", "doc.txt", "first edit");
        let row = harness.db.managed_file_version(&first).unwrap().unwrap();
        let snapshot = PathBuf::from(row.after_backup_path.clone().unwrap());
        fs::write(&snapshot, "totally different bytes").unwrap();

        for force in [false, true] {
            let error = harness
                .restore(&first, force)
                .expect_err("mismatched snapshot must be refused");
            assert!(error.contains("不一致"), "unexpected error: {error}");
            assert_eq!(
                contents_of(&target),
                "first edit",
                "the live file is untouched by a refused restore"
            );
        }
    }

    // -----------------------------------------------------------------------
    // N1: the real Office dispatch is the frozen MCP wrapper
    // -----------------------------------------------------------------------

    /// A frozen scope that authorizes the built-in Office connector and one
    /// document-mutating operation on it.
    fn frozen_office_scope(operations: &[&str]) -> crate::database::KernelHostScope {
        crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: ["call_mcp_tool".to_owned()].into_iter().collect(),
            mcp_server_hashes: [(crate::office::SERVER_ID.to_owned(), "hash".to_owned())]
                .into_iter()
                .collect(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: operations.iter().map(|value| (*value).to_owned()).collect(),
            lifecycle_hooks: Vec::new(),
        }
    }

    fn office_wrapper_input(server: &str, tool: &str, output: &str) -> serde_json::Value {
        serde_json::json!({
            "serverId": server,
            "tool": tool,
            "arguments": {"output": output},
        })
    }

    /// The production Office dispatch is `call_mcp_tool → fox-office`, so that
    /// is what a managed Office write must be recognized from. Reading only the
    /// outer tool name (or only `office_create`/`office_edit`) misses it, which
    /// is exactly how legitimate Office saves lost their version record.
    #[test]
    fn the_frozen_office_wrapper_is_recognized_as_a_managed_write() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_create", "office_edit", "office_read"]);

        // office_create: no source document.
        let create = office_wrapper_input(crate::office::SERVER_ID, "office_create", "报告-新建.docx");
        match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &create)
            .expect("a frozen office_create is a managed write")
        {
            ManagedWrite::Office { inner_tool, verified } => {
                assert_eq!(inner_tool, "office_create");
                assert!(verified.storage_path.ends_with("报告-新建.docx"));
            }
            other => panic!("expected an Office write, got {other:?}"),
        }

        // office_edit: an existing document plus an atomic operation batch.
        harness.write_file("existing.docx", "placeholder document");
        let edit = serde_json::json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_edit",
            "arguments": {
                "file": "existing.docx",
                "output": "报告-编辑.docx",
                "operations": [{"command": "set", "path": "/body/p[1]", "props": {"text": "新"}}],
            },
        });
        match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &edit) {
            Some(ManagedWrite::Office { inner_tool, verified }) => {
                assert_eq!(inner_tool, "office_edit");
                assert!(verified.storage_path.ends_with("报告-编辑.docx"));
            }
            Some(other) => panic!("expected an Office write, got {other:?}"),
            None => panic!("a frozen office_edit must classify as a managed write"),
        }
    }

    /// Nothing but a frozen built-in Office mutation may become a managed write,
    /// and a generic MCP result is never one — however its JSON is shaped.
    #[test]
    fn a_generic_or_unfrozen_connector_is_never_a_managed_write() {
        let root = std::env::temp_dir().join(format!("fox-managed-office-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let scope = frozen_office_scope(&["office_create", "office_edit"]);

        // An ordinary MCP server with a document-shaped tool.
        let generic = office_wrapper_input("some-remote-mcp", "office_create", "报告.docx");
        assert!(frozen_office_operation(&scope, "call_mcp_tool", &generic).is_none());
        assert!(verified_managed_write(&root, "allow", &scope, "call_mcp_tool", &generic).is_none());

        // The built-in connector id, but not part of the Run's frozen scope.
        let unfrozen = crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: ["call_mcp_tool".to_owned()].into_iter().collect(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: ["office_create".to_owned()].into_iter().collect(),
            lifecycle_hooks: Vec::new(),
        };
        let frozen_id = office_wrapper_input(crate::office::SERVER_ID, "office_create", "报告.docx");
        assert!(frozen_office_operation(&unfrozen, "call_mcp_tool", &frozen_id).is_none());

        // An operation the frozen scope does not authorize.
        let unauthorized = office_wrapper_input(crate::office::SERVER_ID, "office_merge", "报告.docx");
        assert!(frozen_office_operation(&scope, "call_mcp_tool", &unauthorized).is_none());

        // A non-mutating Office operation never produces a version.
        let reader = office_wrapper_input(crate::office::SERVER_ID, "office_read", "报告.docx");
        let reader_scope = frozen_office_scope(&["office_read"]);
        assert!(frozen_office_operation(&reader_scope, "call_mcp_tool", &reader).is_none());

        // The wrapper is required: a bare inner name is not a managed write
        // either, because no production dispatch uses that shape.
        let bare = serde_json::json!({"output": "报告.docx"});
        assert!(verified_managed_write(&root, "allow", &scope, "office_create", &bare).is_none());
        let _ = fs::remove_dir_all(root);
    }

    /// A host-intercepted file write is still recognized from Host facts.
    #[test]
    fn a_host_file_write_is_still_a_managed_write() {
        let root = std::env::temp_dir().join(format!("fox-managed-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let scope = frozen_office_scope(&[]);
        let input = serde_json::json!({"path": "notes.txt", "content": "hello"});
        match verified_managed_write(&root, "allow", &scope, "write_file", &input)
            .expect("write_file remains a managed write")
        {
            ManagedWrite::HostFile { verified, .. } => {
                assert!(verified.storage_path.ends_with("notes.txt"))
            }
            other => panic!("expected a host file write, got {other:?}"),
        }
        let _ = fs::remove_dir_all(root);
    }

    /// The positive registration path: a frozen Office create whose declaration
    /// agrees with the verified target and the bytes on disk is registered as a
    /// restorable content version whose `tool` is the real Office operation.
    #[test]
    fn a_verified_office_write_is_registered_with_the_office_operation_name() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_create", "office_edit"]);
        let output = "报告.docx";
        let input = office_wrapper_input(crate::office::SERVER_ID, "office_create", output);
        let managed = verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input)
            .expect("frozen office create");
        let (inner_tool, verified) = match managed {
            ManagedWrite::Office { inner_tool, verified } => (inner_tool, verified),
            other => panic!("expected an Office write, got {other:?}"),
        };
        // The connector then saves the document.
        let target = PathBuf::from(&verified.storage_path);
        fs::write(&target, "saved document bytes").unwrap();
        let (after_hash, after_size) = hash_file(&target).unwrap();
        harness.seed_run("run-1");
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "saved"}],
            "isError": false,
            "details": {"foxManagedFile": {
                "tool": inner_tool,
                "storagePath": PathBuf::from(&verified.storage_path).canonicalize().unwrap()
                    .to_string_lossy(),
                "displayName": "报告.docx",
                "changeKind": "created",
                "beforeHash": serde_json::Value::Null,
                "beforeSize": serde_json::Value::Null,
                "afterHash": after_hash,
                "afterSize": after_size,
                "backupPath": serde_json::Value::Null,
            }},
        });
        record_office_write_from_result(
            &harness.db,
            &harness.backups,
            &harness.conversation,
            "run-1",
            Some("call-office"),
            &verified,
            &inner_tool,
            &result,
        )
        .expect("a fully consistent Office declaration registers");

        let rows = harness.versions(Some(&target));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool, "office_create");
        assert_eq!(rows[0].source, ManagedFileSource::OfficeConnector);
        assert!(rows[0].source_verified);
        assert_eq!(rows[0].after_hash.as_deref(), Some(after_hash.as_str()));
        // …and the registered content is selectable for restore.
        fs::write(&target, "later edit by the user").unwrap();
        let restored = harness.restore(&rows[0].id, true).expect("restore the saved document");
        assert_eq!(contents_of(&target), "saved document bytes");
        assert_eq!(restored.after_hash.as_deref(), Some(after_hash.as_str()));
    }

    /// A declaration that disagrees with the verified target or with the bytes
    /// on disk is refused, so an Office-shaped result can never register a
    /// document the Host did not actually admit and verify.
    #[test]
    fn an_office_declaration_that_disagrees_with_host_facts_is_refused() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_create"]);
        let input = office_wrapper_input(crate::office::SERVER_ID, "office_create", "报告.docx");
        let verified = match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input)
            .expect("frozen office create")
        {
            ManagedWrite::Office { verified, .. } => verified,
            other => panic!("expected an Office write, got {other:?}"),
        };
        let target = PathBuf::from(&verified.storage_path);
        fs::write(&target, "saved document bytes").unwrap();
        let (after_hash, after_size) = hash_file(&target).unwrap();
        harness.seed_run("run-1");

        let declaration = |over: serde_json::Value| {
            let mut meta = serde_json::json!({
                "tool": "office_create",
                "storagePath": target.canonicalize().unwrap().to_string_lossy(),
                "displayName": "报告.docx",
                "changeKind": "created",
                "beforeHash": serde_json::Value::Null,
                "beforeSize": serde_json::Value::Null,
                "afterHash": after_hash,
                "afterSize": after_size,
                "backupPath": serde_json::Value::Null,
            });
            for (key, value) in over.as_object().unwrap() {
                meta[key] = value.clone();
            }
            serde_json::json!({"content": [], "isError": false, "details": {"foxManagedFile": meta}})
        };
        let cases: Vec<(&str, serde_json::Value)> = vec![
            ("another target", serde_json::json!({"storagePath": harness.root.join("其他.docx").canonicalize().unwrap_or_else(|_| harness.root.join("其他.docx")).to_string_lossy()})),
            ("another content hash", serde_json::json!({"afterHash": "0".repeat(64)})),
            ("another operation", serde_json::json!({"tool": "office_edit"})),
            ("a backup with no before hash", serde_json::json!({"backupPath": "/tmp/x.foxbak"})),
        ];
        for (label, over) in cases {
            let error = record_office_write_from_result(
                &harness.db,
                &harness.backups,
                &harness.conversation,
                "run-1",
                Some("call-office"),
                &verified,
                "office_create",
                &declaration(over),
            )
            .expect_err(label);
            assert!(!error.is_empty(), "{label} must be refused with a reason");
        }
        assert!(
            harness.versions(Some(&target)).is_empty(),
            "no refused declaration may leave a registered version"
        );
        // A result without any declaration is refused too, rather than guessed.
        assert!(record_office_write_from_result(
            &harness.db,
            &harness.backups,
            &harness.conversation,
            "run-1",
            Some("call-office"),
            &verified,
            "office_create",
            &serde_json::json!({"content": [], "isError": false}),
        )
        .is_err());
    }
}
