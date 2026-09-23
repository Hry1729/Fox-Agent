//! Capture and restore of user-file versions produced by Host-intercepted
//! writes (`write_file`, `edit_file`, the fox-office connector).
//!
//! Invariants:
//! * Registration happens around a write that already passed every frozen
//!   Host admission check. Admitted file writes register under the file lock;
//!   a registration failure is visible and never causes automatic replay.
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
    fs::OpenOptions::new().write(true).open(&backup)
        .and_then(|file| file.sync_all())
        .map_err(|error| format!("failed to flush managed snapshot: {error}"))?;
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
    pub expected_after_version: Option<String>,
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
        if hash_file(&backup) != Some((hash.clone(), size)) {
            return Err("managed pre-image changed during capture; snapshot retained".into());
        }
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
        expected_after_version: None,
    })
}

/// Hash the post-write file, snapshot its content as this version's own bytes,
/// and append the version row.
///
/// The admitted file executor invokes this while holding the file lock.
/// The content snapshot is what makes "restore version *n*" mean *this*
/// content: `version n`'s bytes are kept next to `version n`'s row instead of
/// being inferred from a neighbouring row's undo copy. Registration errors are
/// returned with the committed side-effect evidence; never replay the write.
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
    if capture.expected_after_version.as_deref().is_some_and(|expected| expected != format!("sha256:{after_hash}")) {
        return Err("managed target changed after commit; refusing to register external bytes".into());
    }
    // A missing content snapshot must not silently degrade the row into
    // "unrestorable": fail the registration so the caller logs an honest gap.
    let after_backup = store_snapshot(backups_dir, &target)?;
    if hash_file(&after_backup) != Some((after_hash.clone(), after_size)) {
        return Err("managed target changed during post-write snapshot; materials retained".into());
    }
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

/// The artifact lifecycle verdict for one verified Office write.
///
/// Derived from Host facts only — the frozen operation name and the verified
/// target path relative to the authorized project root — so a connector result
/// cannot promote an intermediate file to a deliverable, and the renderer never
/// has to guess from a file extension.
pub(crate) fn office_write_artifact_class(
    project_root: &Path,
    target: &Path,
    tool: &str,
) -> &'static str {
    crate::runtime_host::artifact_store::classify(Some(project_root), target, tool, false)
        .0
        .as_str()
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
/// * only the document-mutating operations produce versions.
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
    if !matches!(inner, "office_create" | "office_edit" | "office_import_data") {
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
    context: Option<&ManagedExecutionContext<'_>>,
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
            // The same artifact context is used so an artifactId import is
            // admitted here exactly as the executor will admit it.
            let office_context = context.and_then(|context| {
                Some(crate::office::OfficeCallContext {
                    database: context.database,
                    sessions_dir: context.sessions_dir?,
                    conversation_id: context.conversation_id,
                    // Derived from the Host's own session store, exactly like
                    // `RuntimeHostState::managed_files_dir`, so the pre-flight
                    // resolves the same private roots as the execution.
                    artifacts_dir: context
                        .sessions_dir
                        .and_then(std::path::Path::parent),
                })
            });
            let target = crate::office::prepare_with_context(
                &inner_tool,
                arguments,
                Some(&root_string),
                permission_mode,
                office_context.as_ref(),
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

/// Whether two absolute paths denote the same location, tolerating the Windows
/// extended-length (`\\?\`) prefix difference between a canonicalized path and
/// a path the Host stored before canonicalization.
fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (
        crate::runtime_host::artifact_store::relative_to(left, right),
        crate::runtime_host::artifact_store::relative_to(right, left),
    ) {
        (Some(forward), Some(backward)) => forward.as_os_str().is_empty() && backward.as_os_str().is_empty(),
        _ => false,
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
    /// The Host's session store, used to resolve compute-artifact references in
    /// the managed-write pre-flight. `None` in unit tests.
    pub sessions_dir: Option<&'a Path>,
}

/// Canonical target resolver used by the credential issuance transaction.
pub(crate) struct HostFileTargetResolver<'a> { pub project_root: &'a Path }
impl crate::database::kernel_execution_admission::FileTargetResolver for HostFileTargetResolver<'_> {
    fn resolve(&self, class: fox_engine_protocol::ActionClass,
        canonical_input_json: &str) -> Option<String> {
        use fox_engine_protocol::ActionClass;
        if !matches!(class, ActionClass::Write | ActionClass::Destructive) { return None; }
        let input: serde_json::Value = serde_json::from_str(canonical_input_json).ok()?;
        crate::tool_host::canonical_file_identity(self.project_root, input.get("path")?.as_str()?).ok()
    }
}

/// Construct an observation only from bytes actually read by the Host, never
/// by reopening the file after the read (which could bind a different version).
pub(crate) fn observation_from_read(root: &Path, path: &str, bytes: &[u8], tool_call_id: &str)
    -> Result<fox_engine_protocol::HostObservation, String> {
    let target = crate::tool_host::canonical_file_identity(root, path)?;
    let total_units = String::from_utf8_lossy(bytes).encode_utf16().count() as u64;
    Ok(fox_engine_protocol::HostObservation {
        target_identity: target,
        version: crate::tool_host::file_version(bytes),
        observed_by_tool_call_id: tool_call_id.into(),
        // A read that returns the whole file establishes full-content coverage of
        // that version. Callers that deliver only a page pass their own range.
        view_kind: fox_engine_protocol::ObservationView::FullFile,
        covered_whole_file: true,
        range_start: Some(0),
        range_end: Some(total_units),
        total_units: Some(total_units),
        truncated: false,
    })
}

/// Execute a real reader and persist only its private opened-file evidence.
/// Both desktop and Kernel call this seam. A missing read keeps its original
/// failure while a positively verified absent target establishes the baseline.
pub(crate) fn execute_observed_reader(
    database: &Database, binding: &fox_engine_protocol::RunControlBinding,
    tool: &str, input: &serde_json::Value, tool_call_id: &str,
    cancellation: &crate::kernel::CancellationToken, budget: std::time::Duration,
) -> Result<serde_json::Value, String> {
    let started = std::time::Instant::now();
    let outcome = crate::resource_gateway::execute_with_observation_budget(
        binding, tool, input, cancellation, budget);
    match outcome {
        Ok((result, observation)) => {
            cancellation.check()?;
            if let Some(observation) = observation {
                database.record_host_observation(&binding.run_id, &binding.conversation_id,
                    &observation.host_observation(tool_call_id))?;
            }
            Ok(result)
        }
        Err(error) => {
            if tool == "read" && started.elapsed() < budget && cancellation.check().is_ok() {
                if let Some(path) = input.get("path").and_then(serde_json::Value::as_str) {
                    if let Ok(observation) = crate::resource_gateway::observe_missing_file(binding, path, cancellation) {
                        database.record_host_observation(&binding.run_id, &binding.conversation_id,
                            &observation.host_observation(tool_call_id))?;
                    }
                }
            }
            Err(error)
        }
    }
}

/// Called exclusively by the Host after its durable one-shot claim. All
/// pre-image capture, snapshot verification, final revalidation, replacement
/// and version registration execute while the file lock remains held.
pub(crate) fn execute_admitted_file(
    context: ManagedExecutionContext<'_>, tool: &str, input: &serde_json::Value,
    tool_call_id: &str, credential: &fox_engine_protocol::ExecutionCredential,
    cancellation: Option<&crate::kernel::CancellationToken>,
    revalidate: &mut dyn FnMut() -> Result<(), String>,
) -> (Result<serde_json::Value, String>, fox_engine_protocol::ExecutionEvidence) {
    use fox_engine_protocol::{ExecutionCredential, ExecutionEvidence};
    if credential.intent_digest != crate::database::kernel_execution_admission::launch_params_hash(tool, &input.to_string()) {
        return (Err("credential_mismatch: file payload differs from the admitted intent".into()), ExecutionEvidence::NotStarted);
    }
    struct Hooks<'a, 'b> {
        context: ManagedExecutionContext<'a>, tool: &'b str, tool_call_id: &'b str,
        credential: &'b ExecutionCredential, before: Option<BeforeCapture>,
        evidence: ExecutionEvidence, directory_side_effect: bool, expected_after_version: String,
        revalidate: &'b mut dyn FnMut() -> Result<(), String>,
    }
    impl crate::tool_host::FileCommitContext for Hooks<'_, '_> {
        fn baseline(&self) -> &str { &self.credential.file_baseline.as_ref().expect("checked baseline").version }
        fn dispatch_id(&self) -> &str { &self.credential.dispatch_id }
        fn validate(&mut self, target: &Path) -> Result<(), String> {
            self.context.database.verify_execution_credential(self.context.run_id, self.credential)?;
            if self.credential.run_id != self.context.run_id
                || self.credential.conversation_id != self.context.conversation_id {
                return Err("credential_mismatch: file execution scope differs".into());
            }
            let baseline = self.credential.file_baseline.as_ref().ok_or("observation_incomplete")?;
            let identity = crate::tool_host::canonical_file_identity(
                Path::new(self.context.project_root.ok_or("file project root missing")?),
                &target.to_string_lossy())?;
            if baseline.target_identity != identity { return Err("credential_mismatch: file target differs".into()); }
            (self.revalidate)()
        }
        fn capture_before(&mut self, target: &Path) -> Result<(), String> {
            let mut capture = capture_before(self.context.backups_dir,
                Path::new(self.context.project_root.ok_or("file project root missing")?), target)?;
            capture.expected_after_version = Some(self.expected_after_version.clone());
            self.before = Some(capture);
            Ok(())
        }
        fn record_committed(&mut self, _target: &Path) -> Result<(), String> {
            record_after(self.context.database, self.context.backups_dir,
                self.context.conversation_id, self.context.run_id, Some(self.tool_call_id),
                self.tool, self.before.take().ok_or("missing pre-write capture")?).map(|_| ())
        }
        fn mark_applying(&mut self) { self.evidence = ExecutionEvidence::Unknown; }
        fn mark_committed(&mut self) { self.evidence = ExecutionEvidence::Started; }
        fn mark_directory_creation(&mut self) {
            self.directory_side_effect = true;
            self.evidence = ExecutionEvidence::Unknown;
        }
        fn mark_not_applied(&mut self) {
            self.evidence = if self.directory_side_effect { ExecutionEvidence::Unknown } else { ExecutionEvidence::NotStarted };
        }
    }
    let Some(baseline) = credential.file_baseline.as_ref() else {
        return (Err("observation_incomplete".into()), ExecutionEvidence::NotStarted);
    };
    let prepared = match crate::tool_host::prepare_admitted_file(tool, input,
        context.project_root.unwrap_or_default(), &baseline.version) {
        Ok(prepared) => prepared,
        Err(error) => return (Err(error), ExecutionEvidence::NotStarted),
    };
    let expected_after_version = match &prepared {
        crate::tool_host::PreparedToolAction::WriteFile { content, .. }
        | crate::tool_host::PreparedToolAction::EditFile { content, .. } => crate::tool_host::file_version(content.as_bytes()),
        _ => return (Err("file admission cannot execute a non-file action".into()), ExecutionEvidence::NotStarted),
    };
    let mut hooks = Hooks { context, tool, tool_call_id, credential, before: None,
        evidence: ExecutionEvidence::NotStarted, directory_side_effect: false, expected_after_version, revalidate };
    let result = crate::tool_host::execute_file_with_context(prepared, cancellation, &mut hooks);
    (result, hooks.evidence)
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
    let managed = context.project_root.and_then(|_| {
        verified_managed_write(
            Path::new(context.project_root.unwrap_or_default()),
            context.permission_mode,
            context.scope,
            tool,
            input,
            Some(&context),
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
///
/// REV-02: a restore is a file mutation with a precise baseline and a precise
/// candidate, so it goes through the SAME commit critical section a tool write
/// uses (`tool_host::atomic_write_with_context`): the same write lock, the same
/// version precondition re-checked inside the lock, the same replace journal and
/// the same verified-layout settlement. It never re-executes the original tool
/// and never bypasses the lock with a plain `fs::copy`.
///
/// The request carries its own identity (`restore:<version_id>:<nonce>`), so a
/// restore is auditable as itself rather than as the dispatch that produced the
/// version. `force` decides how *known drift* is handled; it never skips target
/// identity, the commit critical section, the in-lock baseline re-check or the
/// failure classification.
pub(crate) fn restore_version(
    database: &Database,
    backups_dir: &Path,
    conversation_id: &str,
    version_id: &str,
    force: bool,
) -> Result<ManagedFileVersion, String> {
    // A caller that supplies no request identity gets a fresh one; a retry that
    // repeats the SAME identity is deduplicated against the recorded result.
    let request_id = format!("restore-{}", uuid::Uuid::new_v4().simple());
    restore_version_with_seam(
        database,
        backups_dir,
        conversation_id,
        version_id,
        force,
        &request_id,
        &NoRestoreFault,
    )
}

/// Deterministic fault injection point for the restore path. Production passes
/// [`NoRestoreFault`]; tests inject a fault at an exact stage. Like
/// `BackendCapabilityProof`, the proof is an argument, so no environment
/// variable, Cargo feature or global switch can turn a test behaviour into a
/// shipped one.
pub(crate) trait RestoreFaultSeam: Send + Sync {
    fn before_capture(&self) -> Result<(), String> {
        Ok(())
    }
    /// Runs INSIDE the commit critical section, after the baseline was captured
    /// and before the replacement. A concurrent write landing here is exactly
    /// the case the in-lock re-check must refuse.
    fn after_capture(&self) -> Result<(), String> {
        Ok(())
    }
    fn after_replace(&self) -> Result<(), String> {
        Ok(())
    }
}

pub(crate) struct NoRestoreFault;
impl RestoreFaultSeam for NoRestoreFault {}

pub(crate) fn restore_version_with_seam(
    database: &Database,
    backups_dir: &Path,
    conversation_id: &str,
    version_id: &str,
    force: bool,
    request_id: &str,
    seam: &dyn RestoreFaultSeam,
) -> Result<ManagedFileVersion, String> {
    if request_id.trim().is_empty() || request_id.len() > 200 {
        return Err("restore request identity must be a non-empty short string".into());
    }
    // Stable dedup: a repeated delivery of the SAME request reads the recorded
    // result instead of performing the file mutation a second time.
    if let Some(row) = database.restore_request(conversation_id, request_id)? {
        if let Some(result) = row.result_version_id {
            return database
                .managed_file_version(&result)?
                .ok_or_else(|| "recorded restore result vanished".to_string());
        }
        return Err(match row.state.as_str() {
            "recovery_required" => format!(
                "{} was left in an uncertain state during restore; recovery materials are retained",
                row.display_name
            ),
            "indeterminate" => format!(
                "{} could not be verified as restored; the result is unknown and must not be retried \
                 with the same request identity",
                row.display_name
            ),
            _ => format!(
                "{} was not restored; the target was left untouched{}",
                row.display_name,
                row.error
                    .as_deref()
                    .map(|error| format!(" ({error})"))
                    .unwrap_or_default()
            ),
        });
    }

    let version = database
        .managed_file_version(version_id)?
        .ok_or_else(|| "managed file version not found".to_string())?;
    if version.conversation_id != conversation_id {
        return Err("managed file version belongs to another conversation".into());
    }
    // The exact bytes this version records, verified against its own hash. A
    // version is never restored by copying some other version's backup.
    let source = resolve_restore_source(database, &version)?;
    let candidate = fs::read(&source.path)
        .map_err(|error| format!("cannot read the selected version snapshot: {error}"))?;
    // `RestoreSource.sha256` is the raw hex digest; `file_version` is the
    // `sha256:<hex>` form used as a version string. Compare like with like.
    if hash_bytes(&candidate).0 != source.sha256 {
        return Err("selected version snapshot does not match its recorded hash".into());
    }

    let target = PathBuf::from(&version.storage_path);
    let latest = database.latest_managed_file_version(conversation_id, &version.storage_path)?;
    // The baseline is the Host's OWN read of the target, taken at confirmation
    // time and re-checked inside the commit critical section. It is never the
    // registry's `after_hash` and never a value supplied by the request.
    let current_raw = if target.exists() {
        Some(hash_bytes(&fs::read(&target).map_err(|error| error.to_string())?).0)
    } else {
        None
    };
    let baseline = current_raw.as_ref().map(|hash| format!("sha256:{hash}"));
    if !force {
        match (&latest, &current_raw) {
            (Some(latest), Some(hash)) if Some(hash.as_str()) == latest.after_hash.as_deref() => {}
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
    seam.before_capture()?;

    // The request's own identity. It is stable for a given `request_id`, so a
    // restore is auditable as itself rather than as the dispatch that produced
    // the version, and a repeat delivery is recognisable.
    let dispatch_id = format!("restore:{request_id}");
    database.record_restore_request(
        conversation_id,
        request_id,
        &version.id,
        &version.storage_path,
        baseline.as_deref(),
        &dispatch_id,
    )?;

    let mut hooks = RestoreCommitHooks {
        database,
        backups_dir,
        conversation_id,
        run_id: None,
        tool_call_id: None,
        tool: "restore",
        display_name: &version.display_name,
        dispatch_id: &dispatch_id,
        restored_from_id: &version.id,
        source_sha256: &source.sha256,
        source_size: source.size,
        before: None,
        directory_side_effect: false,
        evidence: fox_engine_protocol::ExecutionEvidence::NotStarted,
        outcome: RestoreOutcome::NotApplied,
        registered_version_id: None,
        seam,
    };
    let root = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot recreate the target folder: {error}"))?;
    }
    let expected = baseline.clone().unwrap_or_else(|| "missing".to_owned());
    let commit = crate::tool_host::atomic_write_with_context_mode(
        &root,
        &target,
        &candidate,
        &expected,
        None,
        Some(&mut hooks),
        true,
    );
    // A fault injected after the replacement must NOT return early: the write
    // already happened, so its outcome still has to be observed, classified,
    // settled and registered. Skipping that would leave a real side effect with
    // no recovery material and no honest receipt.
    let after_replace_fault = seam.after_replace().err();

    // Classify from the verified state of the target, not from "the result hash
    // differs" (REV-02). A restore that reported an error is only `not_applied`
    // when the target still holds exactly the bytes the Host read as its
    // baseline; anything else is uncertain and keeps its recovery materials.
    let observed = if target.exists() {
        hash_file(&target).map(|(hash, size)| (hash, size))
    } else {
        None
    };
    let observed_raw = observed.as_ref().map(|(hash, _)| hash.clone());
    let committed = commit.is_ok() && observed_raw.as_deref() == Some(source.sha256.as_str());
    let baseline_intact = observed_raw.is_none() && baseline.is_none()
        || observed_raw.as_deref() == current_raw.as_deref();
    if !committed {
        hooks.outcome = if commit.is_ok() {
            // The commit reported success but the bytes are not the selected
            // version's: the outcome is genuinely unknown.
            RestoreOutcome::Indeterminate
        } else if baseline_intact {
            RestoreOutcome::NotApplied
        } else {
            RestoreOutcome::RecoveryRequired
        };
    }
    match hooks.outcome {
        RestoreOutcome::Committed => {
            let new_id = hooks
                .registered_version_id
                .clone()
                .ok_or_else(|| "restore committed but no version was registered".to_string())?;
            let row = database
                .managed_file_version(&new_id)?
                .ok_or_else(|| "restored version row vanished".to_string())?;
            database.settle_restore_request(
                conversation_id,
                request_id,
                "committed",
                Some(&new_id),
                None,
            )?;
            return Ok(row);
        }
        RestoreOutcome::RecoveryRequired
        | RestoreOutcome::NotApplied
        | RestoreOutcome::Indeterminate => {}
    }
    let state = match hooks.outcome {
        RestoreOutcome::RecoveryRequired => "recovery_required",
        RestoreOutcome::Indeterminate => "indeterminate",
        _ => "not_applied",
    };
    let mut error = commit.err();
    if error.is_none() {
        error = after_replace_fault;
    }
    let message = match hooks.outcome {
        RestoreOutcome::RecoveryRequired => format!(
            "{} was left in an uncertain state during restore; recovery materials are retained \
             next to the target",
            version.display_name
        ),
        RestoreOutcome::Indeterminate => format!(
            "{} could not be verified as restored; the result is unknown and must not be retried \
             with the same request identity",
            version.display_name
        ),
        _ => format!(
            "{} was not restored; the target was left untouched{}",
            version.display_name,
            error
                .as_ref()
                .map(|error| format!(" ({error})"))
                .unwrap_or_default()
        ),
    };
    database.settle_restore_request(conversation_id, request_id, state, None, error.as_deref())?;
    Err(message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RestoreOutcome {
    NotApplied,
    RecoveryRequired,
    Indeterminate,
    Committed,
}

/// The restore's own commit context. It reuses the same capture/registration
/// helpers an admitted file dispatch uses, so a restored version is a first-class
/// version rather than a bare backup file.
struct RestoreCommitHooks<'a> {
    database: &'a Database,
    backups_dir: &'a Path,
    conversation_id: &'a str,
    run_id: Option<&'a str>,
    tool_call_id: Option<&'a str>,
    tool: &'a str,
    display_name: &'a str,
    dispatch_id: &'a str,
    restored_from_id: &'a str,
    /// The bytes this restore writes, already hash-verified against the
    /// selected version's own record.
    source_sha256: &'a str,
    source_size: i64,
    before: Option<BeforeCapture>,
    directory_side_effect: bool,
    evidence: fox_engine_protocol::ExecutionEvidence,
    outcome: RestoreOutcome,
    /// Set inside the commit critical section, so the registry can never
    /// describe content the file does not hold.
    registered_version_id: Option<String>,
    seam: &'a dyn RestoreFaultSeam,
}

impl crate::tool_host::FileCommitContext for RestoreCommitHooks<'_> {
    fn baseline(&self) -> &str {
        // The Host read the target itself before entering the critical section;
        // `atomic_write_with_context` re-checks it inside the lock.
        self.before
            .as_ref()
            .map(|capture| capture.before_hash.as_deref().unwrap_or("missing"))
            .unwrap_or("missing")
    }

    fn dispatch_id(&self) -> &str {
        self.dispatch_id
    }

    fn validate(&mut self, target: &Path) -> Result<(), String> {
        if !target.is_file() && target.exists() {
            return Err("restore target is not a regular file".to_owned());
        }
        Ok(())
    }

    fn capture_before(&mut self, target: &Path) -> Result<(), String> {
        self.seam.before_capture()?;
        self.before = Some(capture_before(
            self.backups_dir,
            target.parent().unwrap_or(target),
            target,
        )?);
        self.seam.after_capture()?;
        Ok(())
    }

    fn record_committed(&mut self, target: &Path) -> Result<(), String> {
        // Registration happens inside the commit critical section: the version
        // row is written while the write lock is still held and the target is
        // known to hold exactly the selected bytes, so the registry can never
        // describe content the file does not hold.
        let (after_hash, after_size) = hash_file(target)
            .ok_or_else(|| "restored target is unreadable after commit".to_string())?;
        if after_hash != self.source_sha256 || after_size != self.source_size {
            // Do not register bytes that are not the selected version's.
            self.outcome = RestoreOutcome::Indeterminate;
            return Ok(());
        }
        let after_backup = store_snapshot(self.backups_dir, target)?;
        let (safety_path, safety_hash, safety_size) = match &self.before {
            Some(capture) => (
                capture.backup_path.as_ref().map(|p| p.to_string_lossy().into_owned()),
                capture.before_hash.clone(),
                capture.before_size,
            ),
            None => (None, None, None),
        };
        // The content this restore replaced gets its own version row, so bytes
        // the registry never knew about stay recoverable through the product.
        if let (Some(path), Some(hash), Some(size)) = (&safety_path, &safety_hash, safety_size) {
            let _ = self.database.register_managed_file_version(
                &ManagedFileVersionInput {
                    conversation_id: self.conversation_id,
                    run_id: self.run_id,
                    tool_call_id: self.tool_call_id,
                    tool: self.tool,
                    storage_path: &target.to_string_lossy().to_owned(),
                    display_name: self.display_name,
                    change_kind: "replaced",
                    before_hash: None,
                    before_size: None,
                    after_hash: Some(hash.as_str()),
                    after_size: Some(size),
                    backup_path: None,
                    after_backup_path: Some(path.as_str()),
                    restored_from_id: None,
                    source: ManagedFileSource::Restore,
                },
                now_ms(),
            );
        }
        let new_id = self
            .database
            .register_managed_file_version(
                &ManagedFileVersionInput {
                    conversation_id: self.conversation_id,
                    run_id: self.run_id,
                    tool_call_id: self.tool_call_id,
                    tool: self.tool,
                    storage_path: &target.to_string_lossy().to_owned(),
                    display_name: self.display_name,
                    change_kind: "restored",
                    before_hash: safety_hash.as_deref(),
                    before_size: safety_size,
                    after_hash: Some(&after_hash),
                    after_size: Some(after_size),
                    backup_path: safety_path.as_deref(),
                    after_backup_path: after_backup.to_str(),
                    restored_from_id: Some(self.restored_from_id),
                    source: ManagedFileSource::Restore,
                },
                now_ms(),
            )
            .map_err(|error| {
                // A registration failure must not be reported as "nothing
                // happened": the write is committed and the materials exist.
                self.outcome = RestoreOutcome::Indeterminate;
                error
            })?;
        self.registered_version_id = Some(new_id);
        Ok(())
    }

    fn mark_applying(&mut self) {
        self.evidence = fox_engine_protocol::ExecutionEvidence::Unknown;
    }

    fn mark_directory_creation(&mut self) {
        self.directory_side_effect = true;
        self.evidence = fox_engine_protocol::ExecutionEvidence::Unknown;
    }

    fn mark_committed(&mut self) {
        self.evidence = fox_engine_protocol::ExecutionEvidence::Started;
        self.outcome = RestoreOutcome::Committed;
    }

    fn mark_not_applied(&mut self) {
        self.evidence = if self.directory_side_effect {
            fox_engine_protocol::ExecutionEvidence::Unknown
        } else {
            fox_engine_protocol::ExecutionEvidence::NotStarted
        };
        self.outcome = if self.directory_side_effect {
            RestoreOutcome::Indeterminate
        } else {
            RestoreOutcome::NotApplied
        };
    }
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
        // `storage_path` may be canonicalized (`\\?\C:\...` on Windows) while
        // `root` is the Host's stored form, so the comparison tolerates the
        // verbatim prefix; otherwise every deliverable would be displayed as an
        // absolute path instead of a project-relative name.
        let display_name = crate::runtime_host::artifact_store::relative_to(
            root,
            Path::new(&storage_path),
        )
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
    if !matches!(tool, "office_create" | "office_edit" | "office_import_data") {
        return Err("only office_create/office_edit/office_import_data produce managed versions".into());
    }
    let declared_path = string("storagePath").unwrap_or_default();
    let declared_path = PathBuf::from(declared_path);
    let declared_path = declared_path.canonicalize().unwrap_or(declared_path);
    let target_path = PathBuf::from(&target.storage_path);
    let target_canonical = target_path.canonicalize().unwrap_or(target_path);
    // Both sides are canonicalized above, but on Windows the two call sites can
    // still differ by the extended-length prefix, so the comparison normalizes
    // it instead of rejecting a legitimate write.
    if !same_path(&declared_path, &target_canonical) {
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

    fn file_scope() -> crate::database::KernelHostScope {
        crate::database::KernelHostScope {
            schema_version: 1,
            tool_names: ["write_file".to_owned()].into_iter().collect(),
            mcp_server_hashes: Default::default(),
            knowledge_reference_hashes: Default::default(),
            knowledge_connection_hashes: Default::default(),
            office_tools: Default::default(),
            lifecycle_hooks: Vec::new(),
        }
    }

    /// Build the A-side authority used by `execute_admitted_file`: a real
    /// frozen binding supplies the budget and permission snapshot, while the
    /// baseline comes from bytes the Host actually read and persisted.
    fn admitted_file_authority(
        harness: &Harness,
        run_id: &str,
        tool_call_id: &str,
        relative: &str,
        input: &serde_json::Value,
    ) -> (
        fox_engine_protocol::RunControlBinding,
        fox_engine_protocol::ExecutionCredential,
    ) {
        harness.seed_run(run_id);
        let root = harness.root.to_string_lossy().into_owned();
        harness
            .db
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversations SET project_root=?2 WHERE id=?1",
                    rusqlite::params![harness.conversation.as_str(), root.as_str()],
                )?;
                Ok(())
            })
            .unwrap();
        let initial_policy = harness
            .db
            .execution_policy(&harness.conversation)
            .expect("read initial live policy");
        if initial_policy.mode != "allow" {
            harness
                .db
                .change_execution_policy(
                    &harness.conversation,
                    &format!("managed-file-authority-{run_id}"),
                    initial_policy.version,
                    "allow",
                )
                .expect("authorize the managed write through the live policy");
        }
        let mut budgets = fox_engine_protocol::TimeBudgets::default();
        budgets.tool_execution_ms = 30_000;
        let binding = harness
            .db
            .freeze_kernel_run_control(run_id, "durable", budgets)
            .expect("freeze real run control");
        assert_eq!(binding.permission.project_root.as_deref(), Some(root.as_str()));

        let observed_bytes = fs::read(harness.target(relative)).expect("read Host baseline");
        let observation = observation_from_read(
            &harness.root,
            relative,
            &observed_bytes,
            &format!("read-{tool_call_id}"),
        )
        .expect("construct Host observation from opened bytes");
        harness
            .db
            .record_host_observation(run_id, &harness.conversation, &observation)
            .expect("persist Host observation");

        let canonical_input = serde_json::to_string(input).unwrap();
        let policy = harness
            .db
            .execution_policy(&harness.conversation)
            .expect("read live policy");
        let credential = fox_engine_protocol::ExecutionCredential::new(
            fox_engine_protocol::encode_dispatch_id(run_id, tool_call_id).unwrap(),
            run_id.to_owned(),
            harness.conversation.clone(),
            crate::database::kernel_execution_admission::launch_params_hash(
                "write_file",
                &canonical_input,
            ),
            fox_engine_protocol::ActionClass::Write,
            Some(observation),
            binding.execution_profile_id.clone(),
            binding.permission_snapshot_id.clone(),
            Some(policy.version),
            binding.budgets.tool_execution_ms,
            None,
            crate::database::kernel_execution_admission::unverified_backend_requirement(),
        )
        .expect("build write credential");
        harness
            .db
            .issue_execution_credential(&credential)
            .expect("persist authoritative credential");
        harness
            .db
            .verify_execution_credential(run_id, &credential)
            .expect("presented credential matches DB authority");
        harness
            .db
            .revalidate_execution_credential(&credential)
            .expect("credential initially passes live DB gates");
        (binding, credential)
    }

    fn replace_journals(root: &Path) -> Vec<(PathBuf, serde_json::Value)> {
        let mut records = fs::read_dir(root)
            .unwrap()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                (name.starts_with(".fox-replace-journal-") && name.ends_with(".json"))
                    .then(|| {
                        let path = entry.path();
                        let value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                        (path, value)
                    })
            })
            .collect::<Vec<_>>();
        records.sort_by(|left, right| left.0.cmp(&right.0));
        records
    }

    /// A-side integration only: B's durable claim is tested at the Host seam.
    /// This drives the real prepare, locked atomic replace and version registry
    /// with a DB-authoritative credential and live revalidation.
    #[test]
    fn admitted_file_executes_atomic_replace_and_registers_exact_bytes() {
        let harness = Harness::new();
        let target = harness.write_file("admitted.txt", "old bytes");
        let input = serde_json::json!({"path":"admitted.txt","content":"new bytes"});
        let (binding, credential) = admitted_file_authority(
            &harness,
            "run-admitted",
            "call-admitted",
            "admitted.txt",
            &input,
        );
        let scope = file_scope();
        let root = harness.root.to_string_lossy().into_owned();
        let context = ManagedExecutionContext {
            database: &harness.db,
            backups_dir: &harness.backups,
            conversation_id: &harness.conversation,
            run_id: &binding.run_id,
            project_root: Some(root.as_str()),
            permission_mode: binding.permission.mode.as_str(),
            scope: &scope,
            sessions_dir: None,
        };
        let revalidation_count = std::cell::Cell::new(0usize);
        let mut revalidate = || {
            revalidation_count.set(revalidation_count.get() + 1);
            harness.db.revalidate_execution_credential(&credential)
        };
        let forged = serde_json::json!({"path":"admitted.txt","content":"forged same-target bytes"});
        let (refusal, evidence) = execute_admitted_file(context, "write_file", &forged,
            "call-admitted", &credential, None, &mut revalidate);
        assert!(refusal.unwrap_err().contains("credential_mismatch"));
        assert_eq!(evidence, fox_engine_protocol::ExecutionEvidence::NotStarted);
        assert_eq!(contents_of(&target), "old bytes");
        assert!(harness.versions(None).is_empty());
        let (result, evidence) = execute_admitted_file(
            context,
            "write_file",
            &input,
            "call-admitted",
            &credential,
            None,
            &mut revalidate,
        );
        let result = result.expect("admitted write commits");
        assert_eq!(
            evidence,
            fox_engine_protocol::ExecutionEvidence::Started,
            "only the committed atomic replacement may confirm execution"
        );
        assert_eq!(revalidation_count.get(), 3, "every real pre-commit gate must run");
        assert_eq!(contents_of(&target), "new bytes");
        assert_eq!(
            result["details"]["readVersion"],
            crate::tool_host::file_version(b"new bytes")
        );

        // The executor records the Host's canonical storage identity. Query the
        // actual conversation row first, then compare that identity exactly;
        // the fixture's joined `PathBuf` may omit Windows' canonical prefix.
        let rows = harness.versions(None);
        assert_eq!(rows.len(), 1, "the committed bytes get one version row");
        let row = &rows[0];
        assert_eq!(
            row.storage_path,
            crate::tool_host::canonical_file_identity(
                &harness.root,
                &target.to_string_lossy(),
            )
            .expect("canonical committed target identity")
        );
        assert_eq!(row.tool, "write_file");
        assert_eq!(row.tool_call_id.as_deref(), Some("call-admitted"));
        assert_eq!(row.change_kind, "modified");
        assert!(row.source_verified);
        assert_eq!(
            row.before_hash.as_deref(),
            Some(crate::tool_host::file_version(b"old bytes").trim_start_matches("sha256:"))
        );
        assert_eq!(
            row.after_hash.as_deref(),
            Some(crate::tool_host::file_version(b"new bytes").trim_start_matches("sha256:"))
        );
        assert_eq!(
            contents_of(Path::new(row.backup_path.as_deref().expect("pre-image backup"))),
            "old bytes"
        );
        assert_eq!(
            contents_of(Path::new(
                row.after_backup_path.as_deref().expect("committed snapshot")
            )),
            "new bytes"
        );
        let journals = replace_journals(&harness.root);
        let settled = journals
            .iter()
            .find(|(path, _)| path.to_string_lossy().ends_with(".settled.json"))
            .expect("atomic replace settlement journal");
        assert_eq!(settled.1["status"], "committed");
        assert!(
            settled.1["note"]
                .as_str()
                .is_some_and(|note| note.contains(&credential.dispatch_id)),
            "the real journal is bound to the admitted dispatch"
        );
    }

    #[test]
    fn admitted_file_second_execution_rejects_an_externally_changed_old_baseline() {
        let harness = Harness::new();
        let target = harness.write_file("stale.txt", "observed bytes");
        let input = serde_json::json!({"path":"stale.txt","content":"proposed bytes"});
        let (binding, credential) = admitted_file_authority(
            &harness,
            "run-stale",
            "call-stale",
            "stale.txt",
            &input,
        );
        let scope = file_scope();
        let root = harness.root.to_string_lossy().into_owned();
        let context = ManagedExecutionContext {
            database: &harness.db,
            backups_dir: &harness.backups,
            conversation_id: &harness.conversation,
            run_id: &binding.run_id,
            project_root: Some(root.as_str()),
            permission_mode: binding.permission.mode.as_str(),
            scope: &scope,
            sessions_dir: None,
        };
        let revalidation_count = std::cell::Cell::new(0usize);
        let mut revalidate = || {
            revalidation_count.set(revalidation_count.get() + 1);
            harness.db.revalidate_execution_credential(&credential)
        };
        let (first, first_evidence) = execute_admitted_file(
            context,
            "write_file",
            &input,
            "call-stale",
            &credential,
            None,
            &mut revalidate,
        );
        first.expect("the first admitted write commits");
        assert_eq!(first_evidence, fox_engine_protocol::ExecutionEvidence::Started);
        assert_eq!(revalidation_count.get(), 3);
        assert_eq!(contents_of(&target), "proposed bytes");
        let journals_before_refusal = replace_journals(&harness.root);

        // Reusing this A-side authority after an external edit exercises the
        // stale baseline check. Preparation sees the drift and refuses before
        // entering the file lock, revalidating again, or creating a journal.
        // B's one-shot claim prevents this in the full Host; this test
        // intentionally isolates A's pre-commit baseline gate.
        fs::write(&target, "external bytes").unwrap();
        let (result, evidence) = execute_admitted_file(
            context,
            "write_file",
            &input,
            "call-stale",
            &credential,
            None,
            &mut revalidate,
        );
        let error = result.expect_err("the observed baseline is stale");
        assert!(error.contains("tool.file_conflict"), "unexpected error: {error}");
        assert_eq!(evidence, fox_engine_protocol::ExecutionEvidence::NotStarted);
        assert_eq!(
            revalidation_count.get(), 3,
            "prepare rejects the stale baseline before any locked revalidation"
        );
        assert_eq!(contents_of(&target), "external bytes");
        assert_eq!(
            replace_journals(&harness.root),
            journals_before_refusal,
            "a prepare-time refusal creates no commit journal"
        );
        let rows = harness.versions(None);
        assert_eq!(rows.len(), 1, "the refused second write adds no version");
        assert_eq!(
            rows[0].storage_path,
            crate::tool_host::canonical_file_identity(
                &harness.root,
                &target.to_string_lossy(),
            )
            .expect("canonical committed target identity")
        );
        assert_eq!(
            rows[0].after_hash.as_deref(),
            Some(crate::tool_host::file_version(b"proposed bytes").trim_start_matches("sha256:"))
        );
    }

    #[test]
    fn final_revalidation_refusal_keeps_a_not_applied_journal() {
        let harness = Harness::new();
        let target = harness.write_file("revoked.txt", "original bytes");
        let input = serde_json::json!({"path":"revoked.txt","content":"blocked bytes"});
        let (binding, credential) = admitted_file_authority(
            &harness,
            "run-revoked",
            "call-revoked",
            "revoked.txt",
            &input,
        );
        let scope = file_scope();
        let root = harness.root.to_string_lossy().into_owned();
        let context = ManagedExecutionContext {
            database: &harness.db,
            backups_dir: &harness.backups,
            conversation_id: &harness.conversation,
            run_id: &binding.run_id,
            project_root: Some(root.as_str()),
            permission_mode: binding.permission.mode.as_str(),
            scope: &scope,
            sessions_dir: None,
        };
        let revalidation_count = std::cell::Cell::new(0usize);
        let mut revalidate = || {
            revalidation_count.set(revalidation_count.get() + 1);
            if revalidation_count.get() == 3 {
                Err("policy revoked at final revalidation".to_owned())
            } else {
                harness.db.revalidate_execution_credential(&credential)
            }
        };
        let (result, evidence) = execute_admitted_file(
            context,
            "write_file",
            &input,
            "call-revoked",
            &credential,
            None,
            &mut revalidate,
        );
        let error = result.expect_err("the final live gate must refuse the write");
        assert!(
            error.contains("policy revoked at final revalidation"),
            "unexpected error: {error}"
        );
        assert_eq!(revalidation_count.get(), 3);
        assert_eq!(evidence, fox_engine_protocol::ExecutionEvidence::NotStarted);
        assert_eq!(contents_of(&target), "original bytes");
        assert!(harness.versions(Some(&target)).is_empty());

        let journals = replace_journals(&harness.root);
        let applying = journals
            .iter()
            .find(|(path, _)| !path.to_string_lossy().ends_with(".settled.json"))
            .expect("the applying journal remains for audit");
        let settled = journals
            .iter()
            .find(|(path, _)| path.to_string_lossy().ends_with(".settled.json"))
            .expect("final refusal has a durable settlement");
        assert_eq!(applying.1["status"], "applying");
        assert_eq!(settled.1["status"], "not_applied");
        assert!(
            settled.1["note"]
                .as_str()
                .is_some_and(|note| note.contains(&credential.dispatch_id)),
            "the refusal journal remains bound to the admitted dispatch"
        );
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
            sessions_dir: None,
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
        match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &create, None)
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
        match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &edit, None) {
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
        assert!(verified_managed_write(&root, "allow", &scope, "call_mcp_tool", &generic, None).is_none());

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
        assert!(verified_managed_write(&root, "allow", &scope, "office_create", &bare, None).is_none());
        let _ = fs::remove_dir_all(root);
    }

    /// A host-intercepted file write is still recognized from Host facts.
    #[test]
    fn a_host_file_write_is_still_a_managed_write() {
        let root = std::env::temp_dir().join(format!("fox-managed-file-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let scope = frozen_office_scope(&[]);
        let input = serde_json::json!({"path": "notes.txt", "content": "hello"});
        match verified_managed_write(&root, "allow", &scope, "write_file", &input, None)
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
        let managed = verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input, None)
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
        let verified = match verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input, None)
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

    /// The data-block import is a first-class managed write: recognized from
    /// the same frozen wrapper, registered under its own operation name, and
    /// restorable byte-for-byte through the shared version chain.
    #[test]
    fn a_verified_import_data_write_registers_and_restores() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_import_data"]);
        let input = serde_json::json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_import_data",
            "arguments": {"output": "汇总.xlsx", "sheet": "数据", "data": "name,value\nalpha,1"},
        });
        let managed = verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input, None)
            .expect("frozen office import");
        let (inner_tool, verified) = match managed {
            ManagedWrite::Office { inner_tool, verified } => (inner_tool, verified),
            other => panic!("expected an Office write, got {other:?}"),
        };
        assert_eq!(inner_tool, "office_import_data");
        let target = PathBuf::from(&verified.storage_path);
        fs::write(&target, "workbook after the import").unwrap();
        let (after_hash, after_size) = hash_file(&target).unwrap();
        harness.seed_run("run-1");
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "imported"}],
            "isError": false,
            "details": {"foxManagedFile": {
                "tool": "office_import_data",
                "storagePath": target.canonicalize().unwrap().to_string_lossy(),
                "displayName": "汇总.xlsx",
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
            Some("call-import"),
            &verified,
            &inner_tool,
            &result,
        )
        .expect("a consistent import declaration registers");
        let rows = harness.versions(Some(&target));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool, "office_import_data");
        fs::write(&target, "overwritten later").unwrap();
        harness.restore(&rows[0].id, true).expect("restore the imported workbook");
        assert_eq!(contents_of(&target), "workbook after the import");
    }

    /// The Legacy transport shares the same verify/registration seam: an
    /// import-by-artifact reference is admitted (artifact resolved against the
    /// frozen conversation) and the completed import registers a restorable
    /// version. This mirrors `runtime_host::mod.rs`'s precheck +
    /// `record_office_write_from_result` for `office_import_data`.
    #[test]
    fn legacy_artifact_import_is_verified_and_registered_for_restore() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_import_data"]);
        let sessions_dir = harness.root.join("sessions");
        fs::create_dir_all(&sessions_dir).unwrap();
        // Seed a saved compute CSV artifact for this exact conversation.
        let outputs = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions_dir,
            &harness.conversation,
            "run-legacy-artifact",
        )
        .unwrap()
        .join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        let csv = "name,value\nalpha,1\nbeta,2\n";
        let artifact_path = outputs.join("saved.csv");
        fs::write(&artifact_path, csv.as_bytes()).unwrap();
        let artifact_path_string = artifact_path.to_string_lossy().into_owned();
        let artifact_id =
            crate::database::Database::computed_artifact_id(&artifact_path_string);
        harness
            .db
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                     VALUES(?1,?2,NULL,'saved.csv','created_file',?3,'text/csv',?4,?5,'ready',?6,?6)",
                    rusqlite::params![
                        artifact_id,
                        harness.conversation,
                        artifact_path_string,
                        csv.len() as i64,
                        hex::encode(sha2::Sha256::digest(csv.as_bytes())),
                        now_ms(),
                    ],
                )
            })
            .unwrap();

        let root_string = harness.root.to_string_lossy().into_owned();
        let context = ManagedExecutionContext {
            database: &harness.db,
            backups_dir: &harness.backups,
            conversation_id: &harness.conversation,
            run_id: "run-legacy",
            project_root: Some(root_string.as_str()),
            permission_mode: "allow",
            scope: &scope,
            sessions_dir: Some(&sessions_dir),
        };
        let input = serde_json::json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_import_data",
            "arguments": {"output": "汇总.xlsx", "sheet": "Sheet1", "artifactId": artifact_id},
        });
        let managed = verified_managed_write(
            &harness.root,
            "allow",
            &scope,
            "call_mcp_tool",
            &input,
            Some(&context),
        )
        .expect("an artifactId import is admitted through the shared seam");
        let (inner_tool, verified) = match managed {
            ManagedWrite::Office { inner_tool, verified } => (inner_tool, verified),
            other => panic!("expected an Office write, got {other:?}"),
        };
        assert_eq!(inner_tool, "office_import_data");
        let target = PathBuf::from(&verified.storage_path);
        fs::write(&target, "workbook written from the saved artifact").unwrap();
        let (after_hash, after_size) = hash_file(&target).unwrap();
        harness.seed_run("run-legacy");
        let result = serde_json::json!({
            "content": [{"type": "text", "text": "imported by artifact"}],
            "isError": false,
            "details": {"foxManagedFile": {
                "tool": "office_import_data",
                "storagePath": target.canonicalize().unwrap().to_string_lossy(),
                "displayName": "汇总.xlsx",
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
            "run-legacy",
            Some("call-artifact-import"),
            &verified,
            &inner_tool,
            &result,
        )
        .expect("a Legacy artifact import registers a version");
        let rows = harness.versions(Some(&target));
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool, "office_import_data");
        fs::write(&target, "overwritten after import").unwrap();
        harness
            .restore(&rows[0].id, true)
            .expect("the imported workbook version is restorable");
        assert_eq!(contents_of(&target), "workbook written from the saved artifact");
    }

    /// An import dispatch outside the frozen operation set is not even
    /// recognized as a managed write: new capabilities need a new frozen scope.
    #[test]
    fn an_import_outside_the_frozen_scope_is_not_a_managed_write() {
        let harness = Harness::new();
        let scope = frozen_office_scope(&["office_create", "office_edit"]);
        let input = serde_json::json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_import_data",
            "arguments": {"output": "汇总.xlsx", "sheet": "数据", "data": "a\n1"},
        });
        assert!(verified_managed_write(&harness.root, "allow", &scope, "call_mcp_tool", &input, None).is_none());
    }

    /// FULL Legacy production chain against the REAL pinned OfficeCLI — no mock
    /// workbook, no hand-built success envelope. The workbook on disk and the
    /// `foxManagedFile` declaration are both produced by the real CLI; the Host
    /// independently re-hashes the bytes and registers a restorable version.
    ///
    /// Sequence, each step the exact production call `runtime_host/mod.rs`
    /// (`execute_mcp_tool_request`) makes for the Legacy transport:
    /// 1. pre-check `office::prepare_with_context` resolves the saved artifact
    ///    (the same Host authorization runs before a human is asked);
    /// 2. under `ask` permission a mutating import requires approval; a recorded
    ///    conversation grant (the exact row a resolved approval writes) clears
    ///    it without another prompt; read-only is refused;
    /// 3. `office::execute_with_cancellation` runs the pinned CLI by reference
    ///    (the full CSV never enters the arguments);
    /// 4. `record_office_write_from_result` verifies the genuine CLI declaration
    ///    against the bytes on disk and registers a source-verified version;
    /// 5. after an out-of-band change, `restore_version` brings back the exact
    ///    CLI-produced workbook byte-for-byte.
    /// Cross-session, tampered, dual-payload and foreign-conversation-restore
    /// requests are all refused without writing.
    #[test]
    #[ignore = "requires the pinned OfficeCLI binary; run explicitly for integration verification"]
    fn real_legacy_chain_precheck_approval_execute_register_restore() {
        use crate::office::OfficeCallContext;

        let root = std::env::temp_dir()
            .join(format!("fox-legacy-chain-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root_string = root.to_string_lossy().into_owned();
        let db = Database::open(root.join("legacy-chain.db")).unwrap();
        crate::office::setup(
            &db,
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources"),
        )
        .unwrap();
        let server = db.get_mcp_server(crate::office::SERVER_ID).unwrap().unwrap();
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        // A real project conversation in `ask` mode: mutations need approval.
        let conversation = db
            .create_conversation(db.default_agent_id(), None, Some(&root_string), Some("ask"))
            .unwrap()
            .id;
        let run = db.create_run(&conversation, "legacy-chain", None).unwrap().run.id;

        // A genuinely saved AGV-scale compute artifact (~100 KB, 927 rows).
        let outputs = crate::runtime_host::attachment_compute::safe_workspace(
            &sessions,
            &conversation,
            &run,
        )
        .unwrap()
        .join("outputs");
        fs::create_dir_all(&outputs).unwrap();
        const ROW_COUNT: usize = 927;
        let mut csv = String::from("id,tag,v1,v2,v3,v4,v5,v6,v7,v8,v9,v10\n");
        for index in 0..ROW_COUNT {
            csv.push_str(&format!("{index},agv-task-{index:05}-zone-{:02}", index % 24));
            for _ in 0..10 {
                csv.push_str(&format!(",{}", 10_000 + index));
            }
            csv.push('\n');
        }
        assert!(csv.len() > 64 * 1024, "fixture must exceed the inline result limit");
        let artifact_path = outputs.join("saved.csv");
        fs::write(&artifact_path, csv.as_bytes()).unwrap();
        let artifact_path_string = artifact_path.to_string_lossy().into_owned();
        let artifact_id = Database::computed_artifact_id(&artifact_path_string);
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,?3,'saved.csv','created_file',?4,'text/csv',?5,?6,'ready',?7,?7)",
                rusqlite::params![
                    &artifact_id,
                    &conversation,
                    &run,
                    &artifact_path_string,
                    csv.len() as i64,
                    hex::encode(sha2::Sha256::digest(csv.as_bytes())),
                    now_ms(),
                ],
            )
        })
        .unwrap();

        let context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &conversation,
            artifacts_dir: None,
        };
        let arguments = serde_json::json!({
            "output": "legacy-chain.xlsx",
            "sheet": "Sheet1",
            "createSheet": false,
            "artifactId": artifact_id,
        });
        let wrapper = serde_json::json!({
            "serverId": crate::office::SERVER_ID,
            "tool": "office_import_data",
            "arguments": arguments,
        });

        // --- 1) Pre-check resolves the artifact BEFORE anyone is asked ------
        let (access_root, access_mode) = db
            .conversation_project_access(&conversation)
            .unwrap()
            .expect("conversation carries a project root and permission mode");
        assert_eq!(access_mode, "ask");
        let prepared = crate::office::prepare_with_context(
            "office_import_data",
            &arguments,
            Some(&access_root),
            &access_mode,
            Some(&context),
        )
        .expect("the Legacy pre-check resolves this conversation's artifact");
        assert!(prepared.mutates, "an import into a new file is a mutation");
        // This is the admitted write target, captured BEFORE execution exactly
        // as the Legacy host does: after a create the output already exists.
        let pre_execution_target = prepared
            .target_path()
            .expect("the pre-execution prepare admits a write target")
            .to_path_buf();
        // The Host streams the saved bytes to stdin; the model's arguments
        // carry only the id — no body, no source path on the command line.
        assert_eq!(prepared.stdin_payload().expect("resolved payload"), csv);
        assert!(wrapper.to_string().contains(artifact_id.as_str()));
        assert!(!wrapper.to_string().contains("agv-task-00500"), "the data body must not be re-copied through the request");

        // read-only is a hard refusal in pre-check, before any write.
        let read_only_error = crate::office::prepare_with_context(
            "office_import_data",
            &serde_json::json!({"output":"ro.xlsx","sheet":"Sheet1","createSheet":false,"artifactId":artifact_id}),
            Some(&access_root),
            "read_only",
            Some(&context),
        )
        .expect_err("a mutating import is refused under read-only permission");
        assert!(!root.join("ro.xlsx").exists(), "read-only refusal writes nothing: {read_only_error}");

        // --- 2) Permission / human approval -------------------------------
        let office_access = db.conversation_project_access(&conversation).unwrap();
        let scope = super::super::permission_scope_hash(
            "office-project-request",
            serde_json::json!({"access": office_access, "input": wrapper})
                .to_string()
                .as_bytes(),
        );
        let granted_before = db
            .conversation_tool_permission_granted(&conversation, "call_mcp_tool", &scope)
            .unwrap();
        assert!(!granted_before, "under ask mode a fresh mutation is not pre-authorised");
        // The exact write a resolved "allow for this conversation" approval
        // performs (database/repositories.rs resolve_tool_approval).
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO conversation_tool_permissions(conversation_id, tool_name, scope_key, granted_at)
                 VALUES (?1, 'call_mcp_tool', ?2, ?3)
                 ON CONFLICT(conversation_id, tool_name, scope_key)
                 DO UPDATE SET granted_at = excluded.granted_at",
                rusqlite::params![&conversation, &scope, now_ms()],
            )
        })
        .unwrap();
        let granted_after = db
            .conversation_tool_permission_granted(&conversation, "call_mcp_tool", &scope)
            .unwrap();
        assert!(granted_after, "the recorded conversation grant authorises this dispatch");

        // --- 3) REAL pinned-CLI execution by reference --------------------
        let result = crate::office::execute_with_cancellation(
            &server,
            "office_import_data",
            &arguments,
            Some(&access_root),
            &access_mode,
            None,
            std::time::Duration::from_secs(180),
            Some(&context),
        )
        .expect("the pinned OfficeCLI imports the saved artifact");
        // The declaration is the genuine CLI envelope, not a test construct.
        let declaration = &result["details"]["foxManagedFile"];
        assert_eq!(declaration["tool"], "office_import_data");
        let target_path = root.join("legacy-chain.xlsx");
        assert!(target_path.is_file(), "the real CLI produced the workbook");
        let genuine_bytes = fs::read(&target_path).unwrap();
        let (genuine_hash, _genuine_size) =
            hash_file(&target_path).expect("the CLI-written workbook is readable");

        // --- 4) Host registration verifies genuine bytes on disk ----------
        // The Host registers against the target captured before execution
        // (the file now exists, so a post-execution prepare would refuse it).
        // `record_office_write_from_result` independently re-hashes the bytes
        // the real CLI actually left on disk and checks the genuine envelope.
        let verified_target = VerifiedWriteTarget::new(
            std::path::Path::new(&access_root),
            &pre_execution_target,
        );
        let backups = root.join("managed-versions");
        record_office_write_from_result(
            &db,
            &backups,
            &conversation,
            &run,
            Some("call-legacy-chain"),
            &verified_target,
            "office_import_data",
            &result,
        )
        .expect("the Host registers the genuine CLI write after re-verifying disk");
        let target_string = verified_target.storage_path.clone();
        let mut versions = db
            .managed_file_versions(&conversation, Some(&target_string))
            .expect("versions are listed for the workbook");
        assert!(!versions.is_empty(), "a restorable version was registered");
        let first = versions.remove(0);
        assert_eq!(first.tool, "office_import_data");
        assert!(first.source_verified, "the version's bytes were Host-verified");
        assert_eq!(first.after_hash.as_deref(), Some(genuine_hash.as_str()));

        // --- 5) Version recovery returns the exact CLI-produced bytes -----
        fs::write(&target_path, b"changed outside the recorded task history").unwrap();
        restore_version(&db, &backups, &conversation, &first.id, true)
            .expect("the imported workbook version is restorable");
        let restored_bytes = fs::read(&target_path).unwrap();
        assert_eq!(restored_bytes, genuine_bytes, "restore is byte-for-byte");
        assert_eq!(
            hash_file(&target_path).unwrap().0,
            genuine_hash,
            "restored content hash matches the registered version"
        );

        // A foreign conversation may not restore this conversation's version.
        let other_conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let foreign_restore = restore_version(&db, &backups, &other_conversation, &first.id, true);
        assert!(foreign_restore.is_err(), "cross-conversation restore must be refused: {foreign_restore:?}");
        assert!(
            foreign_restore.unwrap_err().contains("another conversation"),
            "cross-conversation restore must name the ownership boundary"
        );

        // --- 5b) Overwrite import registers a second restorable version ---
        // The real AGV flow creates a workbook then re-imports a revised table
        // with overwrite=true. A SECOND, smaller CSV artifact makes the two
        // versions genuinely different, so each registered version restores
        // distinct CLI-produced bytes through the production restore path.
        let csv2_path = outputs.join("revised.csv");
        let mut csv2 = String::from("id,name,v\n");
        for index in 0..300u32 {
            csv2.push_str(&format!("{index},task-{index},{}\n", 20_000 + index));
        }
        fs::write(&csv2_path, csv2.as_bytes()).unwrap();
        let id2 = Database::computed_artifact_id(csv2_path.to_string_lossy().as_ref());
        db.with_connection(|connection| {
            connection.execute(
                "INSERT INTO artifacts(id,conversation_id,run_id,display_name,artifact_type,storage_path,media_type,byte_size,sha256,status,created_at,updated_at)
                 VALUES(?1,?2,?3,'revised.csv','created_file',?4,'text/csv',?5,?6,'ready',?7,?7)",
                rusqlite::params![
                    &id2,
                    &conversation,
                    &run,
                    csv2_path.to_string_lossy().as_ref(),
                    csv2.len() as i64,
                    hex::encode(Sha256::digest(csv2.as_bytes())),
                    now_ms(),
                ],
            )
        })
        .unwrap();
        let args2 = serde_json::json!({
            "file": "legacy-chain.xlsx",
            "output": "legacy-chain.xlsx",
            "overwrite": true,
            "sheet": "Sheet1",
            "createSheet": false,
            "startCell": "A1",
            "artifactId": id2,
        });
        let prepared2 = crate::office::prepare_with_context(
            "office_import_data",
            &args2,
            Some(&access_root),
            &access_mode,
            Some(&context),
        )
        .expect("an overwrite of the existing workbook is admitted pre-execution");
        let target2 = prepared2
            .target_path()
            .expect("overwrite admits the existing target")
            .to_path_buf();
        assert_eq!(target2, pre_execution_target, "create and overwrite share one target");
        let result2 = crate::office::execute_with_cancellation(
            &server,
            "office_import_data",
            &args2,
            Some(&access_root),
            &access_mode,
            None,
            std::time::Duration::from_secs(180),
            Some(&context),
        )
        .expect("the pinned CLI overwrites the workbook from the second artifact");
        let genuine2_bytes = fs::read(&target_path).unwrap();
        assert_ne!(genuine2_bytes, genuine_bytes, "the revised import changes the file");
        let verified2 = VerifiedWriteTarget::new(std::path::Path::new(&access_root), &target2);
        record_office_write_from_result(
            &db, &backups, &conversation, &run, Some("call-legacy-chain-v2"),
            &verified2, "office_import_data", &result2,
        )
        .expect("the overwrite is registered as a second managed version");
        let mut import_versions: Vec<ManagedFileVersion> = db
            .managed_file_versions(&conversation, Some(&verified2.storage_path))
            .unwrap()
            .into_iter()
            .filter(|v| v.tool == "office_import_data")
            .collect();
        import_versions.sort_by_key(|v| v.version_no);
        assert_eq!(import_versions.len(), 2, "created + modified versions are both registered");
        assert!(import_versions.iter().all(|v| v.source_verified), "both versions Host-verified");
        let (v1, v2) = (&import_versions[0], &import_versions[1]);
        assert_ne!(v1.after_hash, v2.after_hash, "the two versions hold different content");
        // Restore the older created version, then the revised one: each returns
        // exactly the bytes its own CLI import wrote.
        restore_version(&db, &backups, &conversation, &v1.id, true)
            .expect("the created version restores");
        assert_eq!(fs::read(&target_path).unwrap(), genuine_bytes, "v1 restore");
        restore_version(&db, &backups, &conversation, &v2.id, true)
            .expect("the modified version restores");
        assert_eq!(fs::read(&target_path).unwrap(), genuine2_bytes, "v2 restore");

        // --- Rejections never write --------------------------------------
        // Tampered saved bytes (checksum mismatch) are refused at pre-check.
        fs::write(&artifact_path, b"id,tag\n0,tampered\n").unwrap();
        let tampered = crate::office::prepare_with_context(
            "office_import_data",
            &serde_json::json!({"output":"tamper.xlsx","sheet":"Sheet1","createSheet":false,"artifactId":artifact_id}),
            Some(&access_root),
            &access_mode,
            Some(&context),
        )
        .expect_err("a tampered artifact is refused");
        assert!(tampered.contains("校验失败"), "{tampered}");
        assert!(!root.join("tamper.xlsx").exists());
        // A reference from a different conversation never resolves.
        let foreign_context = OfficeCallContext {
            database: &db,
            sessions_dir: &sessions,
            conversation_id: &other_conversation,
            artifacts_dir: None,
        };
        let cross = crate::office::prepare_with_context(
            "office_import_data",
            &serde_json::json!({"output":"cross.xlsx","sheet":"Sheet1","createSheet":false,"artifactId":artifact_id}),
            Some(&access_root),
            &access_mode,
            Some(&foreign_context),
        )
        .expect_err("a cross-conversation artifact id is refused");
        assert!(cross.contains("不存在"), "{cross}");
        assert!(!root.join("cross.xlsx").exists());
        // data + artifactId together are mutually exclusive.
        let dual = crate::office::prepare_with_context(
            "office_import_data",
            &serde_json::json!({"output":"dual.xlsx","sheet":"Sheet1","createSheet":false,"artifactId":artifact_id,"data":"a,b\n1,2"}),
            Some(&access_root),
            &access_mode,
            Some(&context),
        )
        .expect_err("dual payloads are refused");
        assert!(dual.contains("只能使用"), "{dual}");
        assert!(!root.join("dual.xlsx").exists());

        println!("real Legacy chain artifacts: {}", root.display());
    }
}
