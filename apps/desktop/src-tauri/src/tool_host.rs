use serde::Serialize;
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[path = "command_environment.rs"]
mod command_environment;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;
/// Per-stream hard retention for command output. Everything up to this cap is
/// kept and handed to the Host result pipeline (which spills large results to
/// the blob store and pages them back); beyond it the stream keeps draining
/// so the child cannot deadlock, and the result reports the dropped bytes
/// explicitly with a remedy instead of silently losing them.
const MAX_COMMAND_OUTPUT_BYTES: usize = 16 * 1024 * 1024;
const MAX_COMMAND_SECONDS: u64 = 120;
/// How long an already-terminated child's output threads are waited for. The
/// pipes reach end-of-file once the process tree is gone, so this only has to
/// cover the OS teardown; without a bound a wedged grandchild could hold the
/// tool call open past its own budget and drop the diagnostics it produced.
const STREAM_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// The execution outcomes CONTRACTS v1.2 §1 (EX-v1) distinguishes. The tag is part
/// of the Host-to-Runtime failure text contract (`[tool.<code>] …`), so a
/// failure never has to be re-guessed from prose by the Host mapping, the
/// persisted receipt or the model view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolErrorCode {
    /// The child ran and reported a non-zero exit code.
    NonZeroExit,
    /// The child could not be created at all (missing binary, bad cwd).
    StartFailed,
    /// The call is malformed: missing field, wrong type, empty required text,
    /// size overflow, unreadable or non-UTF-8 target.
    InvalidInput,
    /// The file differs from the version returned by read.
    Conflict,
    /// The Host refused the call: frozen scope escape, read-only mode.
    PermissionDenied,
    /// The task itself introduced this path as input material to read, so a
    /// managed write to it is outside what the Run was authorized to change.
    /// Distinct from [`Self::PermissionDenied`]: an approval for one ordinary
    /// write cannot widen the task's own authorization.
    ReadOnlyInput,
    /// The command ran past its own timeout with its output kept.
    TimedOut,
    /// Run or tool cancellation was requested.
    Cancelled,
    /// The operation failed for a reason the executor cannot classify, or the
    /// effect of a side-effecting operation is genuinely unknown.
    Unknown,
}

impl ToolErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NonZeroExit => "tool.nonzero_exit",
            Self::StartFailed => "tool.start_failed",
            Self::InvalidInput => "tool.invalid_input",
            Self::Conflict => "tool.file_conflict",
            Self::PermissionDenied => "tool.permission_denied",
            Self::ReadOnlyInput => "tool.read_only_input",
            Self::TimedOut => "tool.timed_out",
            Self::Cancelled => "tool.cancelled",
            Self::Unknown => "tool.unknown",
        }
    }

    /// Tag one failure message. Already-tagged messages are kept verbatim so a
    /// specific site can override the default classification of its helper.
    pub fn error(self, message: impl Into<String>) -> String {
        let message = message.into();
        if ToolErrorCode::classify(&message).is_some() {
            message
        } else {
            format!("[{}] {message}", self.as_str())
        }
    }

    /// The tag at the start of a failure message, if there is one.
    pub fn classify(message: &str) -> Option<Self> {
        let rest = message.strip_prefix('[')?;
        let (tag, _) = rest.split_once(']')?;
        Self::from_tag(tag.trim())
    }

    fn from_tag(tag: &str) -> Option<Self> {
        [
            (Self::NonZeroExit.as_str(), Self::NonZeroExit),
            (Self::StartFailed.as_str(), Self::StartFailed),
            (Self::InvalidInput.as_str(), Self::InvalidInput),
            (Self::Conflict.as_str(), Self::Conflict),
            (Self::PermissionDenied.as_str(), Self::PermissionDenied),
            (Self::ReadOnlyInput.as_str(), Self::ReadOnlyInput),
            (Self::TimedOut.as_str(), Self::TimedOut),
            (Self::Cancelled.as_str(), Self::Cancelled),
            (Self::Unknown.as_str(), Self::Unknown),
        ]
        .into_iter()
        .find(|(candidate, _)| *candidate == tag)
        .map(|(_, code)| code)
    }

    /// `true` when the text describes a completed business failure rather than
    /// a refusal to run. A non-zero exit produced a real process and real
    /// output; the other categories did not start or were not allowed to.
    pub const fn is_completed_failure(self) -> bool {
        matches!(self, Self::NonZeroExit | Self::TimedOut)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPreview {
    pub tool: String,
    pub title: String,
    pub target: String,
    pub summary: String,
    pub diff: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PreparedToolAction {
    CommandJob {root:PathBuf,input:Value,preview:ToolPreview},
    WriteFile {
        root: PathBuf,
        path: PathBuf,
        content: String,
        expected_version: String,
        create_directories: bool,
        preview: ToolPreview,
    },
    EditFile {
        root: PathBuf,
        path: PathBuf,
        content: String,
        expected_version: String,
        preview: ToolPreview,
    },
    RunCommand {
        root: PathBuf,
        command: String,
        cwd: PathBuf,
        timeout: Duration,
        preview: ToolPreview,
    },
}

impl PreparedToolAction {
    pub fn preview(&self) -> &ToolPreview {
        match self {
            Self::CommandJob { preview, .. }
            | Self::WriteFile { preview, .. }
            | Self::EditFile { preview, .. }
            | Self::RunCommand { preview, .. } => preview,
        }
    }

    /// The file a write/edit will replace, used for Host-side version
    /// capture. Commands have no single managed target.
    pub fn target_path(&self) -> Option<&Path> {
        match self {
            Self::WriteFile { path, .. } | Self::EditFile { path, .. } => Some(path),
            Self::RunCommand { .. } | Self::CommandJob { .. } => None,
        }
    }
}

pub fn prepare(
    tool: &str,
    input: &Value,
    project_root: &str,
) -> Result<PreparedToolAction, String> {
    let root = canonical_directory(Path::new(project_root), "project folder")
        .map_err(|error| ToolErrorCode::InvalidInput.error(error))?;
    match tool {
        "write_file" => prepare_write(input, &root),
        "edit_file" => prepare_edit(input, &root),
        "run_command" if input.get("action").is_some_and(|a|a!="sync") => crate::runtime_host::command_jobs::prepare(input,&root),
        "run_command" => prepare_command(input, &root),
        _ => Err(ToolErrorCode::InvalidInput.error(format!("unsupported host tool: {tool}"))),
    }
}

/// Host-only hooks. The caller must have won the durable dispatch claim; this
/// interface never obtains authority from tool input or an expectedVersion field.
pub(crate) trait FileCommitContext {
    fn baseline(&self) -> &str;
    fn dispatch_id(&self) -> &str;
    fn validate(&mut self, target: &Path) -> Result<(), String>;
    fn capture_before(&mut self, target: &Path) -> Result<(), String>;
    fn record_committed(&mut self, target: &Path) -> Result<(), String>;
    fn mark_applying(&mut self) {}
    fn mark_directory_creation(&mut self) { self.mark_applying(); }
    fn mark_committed(&mut self) {}
    fn mark_not_applied(&mut self) {}
}

/// The same canonical target identity is used by read observations and issuance.
pub(crate) fn canonical_file_identity(root: &Path, value: &str) -> Result<String, String> {
    let root = canonical_directory(root, "project folder")?;
    let target = resolve_write_target(&root, value)?;
    Ok(target.to_string_lossy().into_owned())
}

pub(crate) fn prepare_admitted_file(tool: &str, input: &Value, root: &str,
    baseline: &str) -> Result<PreparedToolAction, String> {
    let mut trusted = input.clone();
    let object = trusted.as_object_mut().ok_or("file input must be an object")?;
    object.insert("expectedVersion".into(), Value::String(baseline.into()));
    prepare(tool, &trusted, root)
}

/// The version a managed write may commit against, or the refusal that says why.
///
/// The model's declared `expectedVersion` is a precondition claim; the caller
/// resolves it against observations this Run really delivered and passes the
/// match here. A missing target is not a conflict: the caller establishes that
/// baseline itself (see `resource_gateway::observe_missing_file`) and passes the
/// resulting version in `observed`, so an ordinary first write never has to be
/// preceded by a doomed read. Only a *proven* absence is read as "new file":
/// every other filesystem error, and every existing file without an observation,
/// fails closed instead of being mistaken for an empty slot.
pub(crate) fn write_baseline(
    target: &Path,
    declared: Option<&str>,
    observed: Option<&str>,
) -> Result<String, String> {
    if let Some(version) = observed {
        return Ok(version.to_owned());
    }
    match fs::metadata(target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok("missing".to_owned()),
        Err(error) => Err(ToolErrorCode::Conflict.error(format!(
            "cannot inspect the write target {}: {error}",
            target.display()
        ))),
        Ok(metadata) if metadata.is_dir() => Err(ToolErrorCode::InvalidInput.error(format!(
            "{} is a directory, not a file",
            target.display()
        ))),
        Ok(_) => Err(match declared {
            Some(version) => ToolErrorCode::Conflict.error(format!(
                "No Host observation of {version} exists in this Run; read the target again and retry with its readVersion"
            )),
            None => ToolErrorCode::Conflict
                .error("Read the target before writing; no Host observation exists"),
        }),
    }
}

/// Refuse a managed write whose target the frozen task introduced as read-only
/// input material.
///
/// The constraint comes from the task itself, not from an approval click, so
/// approving one ordinary write cannot widen it; and a task that also asks to
/// save the same path keeps that path writable (see `read_only_path_roles`).
/// Path identity is the project-relative path with its separators preserved, so
/// the rule holds for any user folder layout rather than for names like `in/`,
/// and `source/a.csv` can never be confused with `source_a.csv`.
pub(crate) fn ensure_writable_target(
    task_text: Option<&str>,
    project_root: &str,
    target: &Path,
) -> Result<(), String> {
    let Some(text) = task_text else {
        return Ok(());
    };
    let roles =
        crate::runtime_host::delivery::read_only_path_roles(text, Some(project_root));
    if roles.is_empty() {
        return Ok(());
    }
    let root = canonical_directory(Path::new(project_root), "project folder")?;
    // Resolve the target the same way a write does, so a file that does not
    // exist yet still yields the project-relative identity it will be created
    // under. A target outside the project is not this gate's business; the
    // write path itself refuses it.
    let Ok(canonical_target) = resolve_write_target(&root, &target.to_string_lossy()) else {
        return Ok(());
    };
    let Ok(relative) = canonical_target.strip_prefix(&root) else {
        return Ok(());
    };
    let display = relative.to_string_lossy().replace('\\', "/");
    if roles.contains_relative(&display) {
        return Err(ToolErrorCode::ReadOnlyInput.error(format!(
            "「{display}」是任务给出的只读输入材料（任务只要求读取或处理它，没有要求修改或保存它）：\
             请把结果写到任务指定的输出路径，不要改动输入文件"
        )));
    }
    Ok(())
}

pub(crate) fn execute_file_with_context(
    action: PreparedToolAction,
    cancellation: Option<&crate::kernel::CancellationToken>,
    context: &mut dyn FileCommitContext,
) -> Result<Value, String> {
    let (root, path, content, create_directories) = match action {
        PreparedToolAction::WriteFile { root, path, content, create_directories, .. } =>
            (root, path, content, create_directories),
        PreparedToolAction::EditFile { root, path, content, .. } => (root, path, content, false),
        _ => return Err("file admission cannot execute a non-file action".into()),
    };
    check_cancellation(cancellation)?;
    let expected = context.baseline().to_owned();
    // Directory creation is a side effect too: validate before it, while keeping
    // the final file validation inside the common replacement lock below.
    context.validate(&path)?;
    if create_directories {
        if let Some(parent) = path.parent() {
            if !parent.is_dir() {
                // Directory creation can partially succeed. Preserve uncertain
                // evidence if a later gate or file operation refuses.
                context.mark_directory_creation();
                fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
        }
    }
    atomic_write_with_context(&root, &path, content.as_bytes(), &expected,
        cancellation, Some(context))?;
    Ok(text_result(format!("Wrote {} bytes to {}", content.len(), path.display()),
        json!({"path":path,"bytes":content.len(),"operation":if expected == "missing" { "created" } else { "modified" },"readVersion":file_version(content.as_bytes())})))
}

pub fn execute(action: PreparedToolAction) -> Result<Value, String> {
    execute_with_cancellation(action, None)
}

pub fn execute_with_cancellation(
    action: PreparedToolAction,
    cancellation: Option<&crate::kernel::CancellationToken>,
) -> Result<Value, String> {
    check_cancellation(cancellation)?;
    #[cfg(not(test))]
    if matches!(&action, PreparedToolAction::WriteFile { .. } | PreparedToolAction::EditFile { .. }) {
        return Err(ToolErrorCode::PermissionDenied.error("File writes require durable Host admission"));
    }
    match action {
        PreparedToolAction::CommandJob { .. } => Err(ToolErrorCode::PermissionDenied.error("Command jobs require the durable Host adapter")),
        PreparedToolAction::WriteFile {
            root,
            path,
            content,
            expected_version,
            create_directories,
            ..
        } => {
            revalidate_target(&root, &path, None)?;
            let operation = if path.exists() { "modified" } else { "created" };
            if create_directories {
                if let Some(parent) = path.parent() {
                    check_cancellation(cancellation)?;
                    fs::create_dir_all(parent).map_err(|error| {
                        ToolErrorCode::Unknown.error(format!(
                            "failed to create parent directory: {error}"
                        ))
                    })?;
                }
            }
            check_cancellation(cancellation)?;
            atomic_write(&root, &path, content.as_bytes(), &expected_version, cancellation)?;
            Ok(text_result(
                format!("Wrote {} bytes to {}", content.len(), path.display()),
                json!({ "path": path, "bytes": content.len(), "operation": operation, "readVersion": file_version(content.as_bytes()) }),
            ))
        }
        PreparedToolAction::EditFile {
            root,
            path,
            content,
            expected_version,
            ..
        } => {
            revalidate_target(&root, &path, Some(false))?;
            check_cancellation(cancellation)?;
            atomic_write(&root, &path, content.as_bytes(), &expected_version, cancellation)?;
            Ok(text_result(
                format!("Updated {}", path.display()),
                json!({ "path": path, "bytes": content.len(), "operation": "modified", "readVersion": file_version(content.as_bytes()) }),
            ))
        }
        PreparedToolAction::RunCommand {
            root,
            command,
            cwd,
            timeout,
            ..
        } => {
            crate::process_jobs::authorize_execution().map_err(|error|
                ToolErrorCode::PermissionDenied.error(format!("sandbox_unavailable: {error:?}")))?;
            revalidate_target(&root, &cwd, Some(true))?;
            execute_command(&command, &cwd, timeout, cancellation)
        }
    }
}

/// Hash the exact bytes returned by the reader, including line endings.
pub(crate) fn file_version(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn write_precondition(input: &Value, current: Option<&str>) -> Result<String, String> {
    let version = current.map(|s| file_version(s.as_bytes())).unwrap_or_else(|| "missing".into());
    let expected = match input.get("expectedVersion") {
        None if current.is_none() => "missing",
        Some(Value::String(value)) => value.as_str(),
        _ => return Err(ToolErrorCode::Conflict.error(
            "Existing files require expectedVersion from read. Read the file, revise the change against that content, and retry; no additional authorization is needed.")),
    };
    if expected != version {
        return Err(ToolErrorCode::Conflict.error(
            "File changed since read (or was created/deleted). Read it again, reconcile your change, and retry with its readVersion; no additional authorization is needed."));
    }
    Ok(version)
}

// Serialize this Host's compare-and-replace operations. External writers are
// checked immediately before replacement; this is optimistic concurrency, not
// an OS transaction with arbitrary editors or shell processes.
static FILE_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Prefix of the Host-owned `ReplaceFileW` backup (`apiBackupPath`) and of the
/// applying record. Both live in the target's own directory: that is the only
/// place this module can prove is on the same volume as the target, which
/// `ReplaceFileW` requires of its backup name ([v1.1] 6.5 rule 2).
const REPLACE_BACKUP_PREFIX: &str = ".fox-replace-backup-";
const REPLACE_JOURNAL_PREFIX: &str = ".fox-replace-journal-";

/// What a probe of one recovery path found. A probe error is never flattened
/// into "absent": an unreadable target or backup is an *unknown* state, and
/// unknown means the materials are pinned, never cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Probe {
    /// The file exists; its exact bytes hash to this version.
    Present(String),
    /// The file is genuinely not there.
    Absent,
    /// The probe could not answer: permission denied, sharing violation or
    /// another I/O error. Treated as "cannot prove anything".
    Unknown(String),
}

impl Probe {
    fn present_version(&self) -> Option<&str> {
        match self {
            Probe::Present(version) => Some(version),
            _ => None,
        }
    }

    fn is_absent(&self) -> bool {
        matches!(self, Probe::Absent)
    }
}

/// Read one recovery path, telling a real absence from a probe error.
fn probe_version(path: &Path) -> Probe {
    match fs::read(path) {
        Ok(bytes) => Probe::Present(file_version(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Probe::Absent,
        Err(error) => Probe::Unknown(error.to_string()),
    }
}

/// The on-disk facts a finished in-place replace is judged from. Every probe
/// keeps its error state, so an unreadable file can never be mistaken for a
/// missing one and used as grounds for cleanup.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReplaceLayout {
    target: Probe,
    backup: Probe,
    candidate: Probe,
}

/// Read the real post-replace layout of target, candidate and backup.
fn inspect_layout(target: &Path, candidate: &Path, backup: &Path) -> ReplaceLayout {
    ReplaceLayout {
        target: probe_version(target),
        backup: probe_version(backup),
        candidate: probe_version(candidate),
    }
}

/// What a finished in-place replace means, and which materials may be cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplaceVerdict {
    /// The replace provably never reached its side-effecting stage: the target
    /// still holds the base bytes, no backup was created and the candidate is
    /// still staged — all three confirmed, with no probe errors. There is
    /// nothing to recover, so the candidate and the in-flight record are
    /// settled cleanup ([v1.1] 6.6 `not_applied`).
    NotApplied,
    /// The target holds the intended new bytes, the candidate was consumed into
    /// it, **and the original survives in a verified backup** whose bytes hash
    /// to the pre-replace version. The backup is the replace's own recovery
    /// material and is retained: without a durable registration receipt from
    /// the version-history layer, deleting it would rely on a best-effort outer
    /// snapshot ([v1.1] 6.5 / 6.6 `committed`).
    Committed,
    /// Partial progress, a moved original, an unverifiable backup, a probe
    /// error, or simply not enough evidence. Everything is pinned: no replay,
    /// no overwrite, no generic cleanup ([v1.1] 6.5 rule 5 / 6.6
    /// `recovery_required`).
    RecoveryRequired { reason: String },
}

/// Judge a finished replace from its error code and its verified layout. Both
/// settled verdicts require *all three* paths to be accounted for; any probe
/// error, any third content or any mismatch pins the materials instead of
/// deleting them.
fn classify_replace_outcome(
    os_error: Option<u32>,
    layout: &ReplaceLayout,
    base_version: &str,
    new_version: &str,
) -> ReplaceVerdict {
    // Committed: the candidate became the target, and the original is provably
    // in the backup. The verified backup is what makes the replace's recovery
    // material real, independent of any best-effort outer snapshot.
    if layout.target.present_version() == Some(new_version)
        && layout.candidate.is_absent()
        && layout.backup.present_version() == Some(base_version)
    {
        return ReplaceVerdict::Committed;
    }
    // Not applied: nothing moved. The target is still the original, no backup
    // was created and the candidate is still the staged content.
    if layout.target.present_version() == Some(base_version)
        && layout.backup.is_absent()
        && layout.candidate.present_version() == Some(new_version)
    {
        return ReplaceVerdict::NotApplied;
    }
    ReplaceVerdict::RecoveryRequired {
        reason: format!(
            "in-place replace left an indeterminate state (OS error {error}); verified layout: \
             target={target}, backup={backup}, candidate={candidate}",
            error = os_error
                .map(|code| code.to_string())
                .unwrap_or_else(|| "not reported".into()),
            target = probe_label(&layout.target),
            backup = probe_label(&layout.backup),
            candidate = probe_label(&layout.candidate),
        ),
    }
}

/// A short, human-readable label for one probe, including the error text so an
/// unknown state is visible in the failure message instead of being hidden.
fn probe_label(probe: &Probe) -> String {
    match probe {
        Probe::Present(version) => format!("present ({version})"),
        Probe::Absent => "absent".to_string(),
        Probe::Unknown(error) => format!("unknown ({error})"),
    }
}

fn epoch_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

/// The small, self-contained applying record. It is deliberately not a shared
/// protocol or a database table: [v1.1] 6.5 only asks for a reliable record of
/// what was in flight, so a flushed sidecar file next to the target is enough.
#[derive(Serialize)]
struct ReplaceJournal {
    operation_id: String,
    status: String,
    target: String,
    candidate: String,
    backup: String,
    base_version: String,
    new_version: String,
    started_at_ms: u128,
    settled_at_ms: Option<u128>,
    os_error: Option<u32>,
    note: Option<String>,
}

/// Deterministic fault injection for the settlement record. The recovery tests
/// must be able to prove the durable applying record survives a settlement
/// write that fails after open, mid-body or at flush, so the fault points are
/// explicit rather than simulated by a wrapper. Production always passes NONE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JournalFault(u8);

impl JournalFault {
    const NONE: JournalFault = JournalFault(0);
    #[cfg(test)]
    const FAIL_OPEN: JournalFault = JournalFault(1);
    #[cfg(test)]
    const FAIL_BODY: JournalFault = JournalFault(2);
    #[cfg(test)]
    const FAIL_FLUSH: JournalFault = JournalFault(3);
}

/// Create the applying record: a new, unique file, written and flushed *before*
/// the replace touches the original. It is never truncated in place — the
/// settlement writes a separate record — so a failed settlement cannot destroy
/// the only durable trace of what was in flight. If the target's directory
/// cannot hold it, the replace is refused rather than attempted without a
/// durable record.
fn store_applying(journal: &Path, record: &ReplaceJournal) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::File::create_new(journal)
        .map_err(|error| format!("cannot create the applying record: {error}"))?;
    let body = serde_json::to_string(record)
        .map_err(|error| format!("cannot encode the applying record: {error}"))?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot flush the applying record: {error}"))
}

/// The settlement record lives beside the applying record and never replaces
/// it, so an interrupted or failed settlement cannot truncate the applying
/// record and leave an empty or half-written trace at the worst possible
/// moment.
fn settled_record_path(journal: &Path) -> PathBuf {
    let name = journal
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let stem = name.strip_suffix(".json").unwrap_or(name);
    journal.with_file_name(format!("{stem}.settled.json"))
}

/// Write the settlement outcome next to the untouched applying record. Every
/// failure is returned, never swallowed: the caller must keep all recovery
/// material when it happens. `fault` injects a failure after open, mid-body or
/// at flush for the recovery tests.
fn store_settled(
    settled: &Path,
    record: &ReplaceJournal,
    fault: JournalFault,
) -> Result<(), String> {
    use std::io::Write;
    match fault.0 {
        1 => {
            return Err(
                "injected settlement failure: could not open the settlement record".to_string()
            )
        }
        2 => {
            let _ = fs::File::create_new(settled);
            return Err(
                "injected settlement failure: the write was interrupted mid-body".to_string(),
            )
        }
        3 => {
            let mut file = fs::File::create_new(settled)
                .map_err(|error| format!("cannot create the settlement record: {error}"))?;
            let body = serde_json::to_string(record)
                .map_err(|error| format!("cannot encode the settlement record: {error}"))?;
            file.write_all(body.as_bytes())
                .map_err(|error| format!("cannot write the settlement record: {error}"))?;
            return Err(
                "injected settlement failure: the flush failed after the body was written"
                    .to_string(),
            );
        }
        _ => {}
    }
    let mut file = fs::File::create_new(settled)
        .map_err(|error| format!("cannot create the settlement record: {error}"))?;
    let body = serde_json::to_string(record)
        .map_err(|error| format!("cannot encode the settlement record: {error}"))?;
    file.write_all(body.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("cannot flush the settlement record: {error}"))
}

/// Decide a finished in-place replace from the verified layout, keep every
/// recovery material the verdict requires, and clean only what is provably
/// settled. `Ok` means the target holds the intended bytes.
fn settle_replace(
    target: &Path,
    candidate: &Path,
    backup: &Path,
    journal: &Path,
    record: &mut ReplaceJournal,
    os_error: Option<u32>,
    fault: JournalFault,
) -> Result<(), String> {
    record.settled_at_ms = Some(epoch_ms());
    record.os_error = os_error;
    let layout = inspect_layout(target, candidate, backup);
    let settled = settled_record_path(journal);
    match classify_replace_outcome(os_error, &layout, &record.base_version, &record.new_version) {
        ReplaceVerdict::Committed => {
            record.status = "committed".to_string();
            // The replace committed and the original is *verified* in the
            // backup. The backup, the applying record and the settlement record
            // are all retained: tool_host has no durable registration receipt
            // from the version-history layer, so deleting the only independent
            // copy of the original here would rest on a best-effort outer
            // snapshot. A registration failure above must not turn into a lost
            // original ([v1.1] 6.5 / O-A01).
            if let Err(error) = store_settled(&settled, record, fault) {
                // The commit is a fact — the target holds the new bytes — so it
                // is not replayed or reported as a failure. But the settlement
                // record is missing, so every material stays and the gap is
                // reported rather than swallowed.
                return Err(format!("file committed but settlement record could not be persisted: {error}; recovery materials retained at {}", journal.display()));
            }
            Ok(())
        }
        ReplaceVerdict::NotApplied => {
            // Verified no effect: the target is still the original, nothing was
            // moved and the candidate is still staged — all three confirmed.
            // There is nothing to recover, so the candidate and the in-flight
            // record are settled cleanup.
            record.status = "not_applied".into();
            store_settled(&settled, record, fault)?;
            let _ = fs::remove_file(candidate);
            Err(ToolErrorCode::Unknown.error(format!(
                "atomic file replacement failed (OS error {error}) and was verified to have had \
                 no effect: the target still holds its pre-replace content",
                error = os_error
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "not reported".into()),
            )))
        }
        ReplaceVerdict::RecoveryRequired { reason } => {
            record.status = "recovery_required".to_string();
            record.note = Some(format!("{}; materials pinned: reconcile before any new attempt; do not replay",
                record.note.as_deref().unwrap_or_default()));
            // The applying record is never truncated; the settlement is a
            // separate file. A failed settlement write must not be swallowed:
            // the materials stay, the applying record is still intact and the
            // failure becomes part of the reported error.
            let settled_gap = store_settled(&settled, record, fault)
                .err()
                .map(|error| format!(
                    "; the settlement record could not be persisted ({error}), the applying \
                     record is still intact"
                ));
            Err(ToolErrorCode::Unknown.error(format!(
                "{reason}. Recovery materials are retained next to the target — candidate \
                 {candidate}, backup {backup}, applying record {journal}, settlement {settled}{settled_gap}; \
                 the write was NOT verified as never executed, so treat the target as suspect",
                candidate = candidate.display(),
                backup = backup.display(),
                journal = journal.display(),
                settled = settled.display(),
                settled_gap = settled_gap.unwrap_or_default(),
            )))
        }
    }
}

/// Build the applying record paths for one replace. Both are unique (fresh id)
/// and sit in the target's directory, which is on the target's volume.
fn replace_material_paths(target: &Path) -> Result<(PathBuf, PathBuf, String), String> {
    let directory = target
        .parent()
        .ok_or("the replace target has no parent directory")?;
    let id = uuid::Uuid::new_v4();
    let backup = directory.join(format!("{REPLACE_BACKUP_PREFIX}{id}"));
    let journal = directory.join(format!("{REPLACE_JOURNAL_PREFIX}{id}.json"));
    Ok((backup, journal, id.to_string()))
}

fn atomic_write(root: &Path, path: &Path, bytes: &[u8], expected: &str,
    cancellation: Option<&crate::kernel::CancellationToken>) -> Result<(), String> {
    atomic_write_with_context(root, path, bytes, expected, cancellation, None)
}

/// The one file-commit critical section every mutating path shares: tool writes,
/// admitted file dispatches and user restores alike. `binary` selects how the
/// current version is computed — from the raw bytes (restore of a snapshot that
/// may not be UTF-8) or from the decoded text (`write_file` / `edit_file`).
pub(crate) fn atomic_write_with_context(root: &Path, path: &Path, bytes: &[u8], expected: &str,
    cancellation: Option<&crate::kernel::CancellationToken>,
    context: Option<&mut dyn FileCommitContext>) -> Result<(), String> {
    atomic_write_with_context_mode(root, path, bytes, expected, cancellation, context, false)
}

pub(crate) fn atomic_write_with_context_mode(root: &Path, path: &Path, bytes: &[u8], expected: &str,
    cancellation: Option<&crate::kernel::CancellationToken>,
    mut context: Option<&mut dyn FileCommitContext>, binary: bool) -> Result<(), String> {
    use std::io::Write;
    let _guard = FILE_WRITE_LOCK.lock().map_err(|_| ToolErrorCode::Unknown.error("file write lock poisoned"))?;
    revalidate_target(root, path, None)?;
    if let Some(context) = context.as_deref_mut() {
        context.validate(path)?;
        let current = current_version_of(path, binary)?;
        ensure_unchanged(expected, current.as_deref())?;
        context.capture_before(path)?;
    }
    let staging = path.parent().ok_or("missing write parent")?
        .join(format!(".fox-write-{}.tmp", uuid::Uuid::new_v4()));
    // The guard cleans the candidate only while the write is still before its
    // side-effecting stage. Once the replace has been attempted, its own
    // settlement decides the candidate's fate from the verified layout, so the
    // guard hands the candidate off before the API call ([v1.1] 6.5 rule 5).
    struct StagingGuard(Option<PathBuf>);
    impl StagingGuard {
        fn path(&self) -> &Path {
            self.0.as_deref().expect("staging path used after hand-off")
        }
        /// Own the candidate from here on; `drop` will not touch it again.
        fn hand_off(&mut self) -> PathBuf {
            self.0.take().expect("staging path handed off twice")
        }
    }
    impl Drop for StagingGuard {
        fn drop(&mut self) {
            if let Some(path) = self.0.take() {
                let _ = fs::remove_file(path);
            }
        }
    }
    let mut staging = StagingGuard(Some(staging));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(staging.path())
        .map_err(|e| ToolErrorCode::Unknown.error(format!("cannot stage file: {e}")))?;
    file.write_all(bytes).and_then(|_| file.sync_all())
        .map_err(|e| ToolErrorCode::Unknown.error(format!("cannot flush staged file: {e}")))?;
    drop(file);
    check_cancellation(cancellation)?;
    revalidate_target(root, path, None)?;
    let current = current_version_of(path, binary)?;
    ensure_unchanged(expected, current.as_deref())?;
    if current.is_none() {
        // Atomic create without replacing a file created since the check.
        let staging_path = staging.path().to_path_buf();
        let (backup, journal, operation_id) = replace_material_paths(path)?;
        let mut record = ReplaceJournal {
            operation_id, status: "applying".into(), target: path.to_string_lossy().into_owned(),
            candidate: staging_path.to_string_lossy().into_owned(), backup: backup.to_string_lossy().into_owned(),
            base_version: "missing".into(), new_version: file_version(bytes), started_at_ms: epoch_ms(),
            settled_at_ms: None, os_error: None,
            note: context.as_deref().map(|context| format!("dispatch_id={}", context.dispatch_id())),
        };
        store_applying(&journal, &record)?;
        let final_check = check_cancellation(cancellation).and_then(|_| {
            if let Some(context) = context.as_deref_mut() { context.validate(path) } else { Ok(()) }
        });
        if let Err(error) = final_check {
            record.status = "not_applied".into();
            record.settled_at_ms = Some(epoch_ms());
            if let Some(context) = context.as_deref_mut() { context.mark_not_applied(); }
            store_settled(&settled_record_path(&journal), &record, JournalFault::NONE)?;
            return Err(error);
        }
        if let Some(context) = context.as_deref_mut() { context.mark_applying(); }
        match fs::hard_link(&staging_path, path) {
            Ok(()) => {
                // Preserve committed evidence even if the settlement disk fails.
                if let Some(context) = context.as_deref_mut() { context.mark_committed(); }
                staging.hand_off();
                record.status = "committed".into();
                record.settled_at_ms = Some(epoch_ms());
                store_settled(&settled_record_path(&journal), &record, JournalFault::NONE)
                    .map_err(|error| format!("file created but settlement failed; materials retained: {error}"))?;
                let _ = fs::remove_file(&staging_path);
            }
            Err(e) => {
                if let Some(context) = context.as_deref_mut() { context.mark_not_applied(); }
                record.status = "not_applied".into();
                record.settled_at_ms = Some(epoch_ms());
                store_settled(&settled_record_path(&journal), &record, JournalFault::NONE)?;
                return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                    ToolErrorCode::Conflict.error("File was created concurrently; read it before retrying")
                } else { ToolErrorCode::Unknown.error(format!("cannot publish new file: {e}")) })
            }
        }
    } else {
        let permissions = fs::metadata(path).map_err(|e| e.to_string())?.permissions();
        if permissions.readonly() {
            return Err(ToolErrorCode::PermissionDenied.error("Target file is read-only"));
        }
        #[cfg(not(windows))]
        fs::set_permissions(staging.path(), permissions).map_err(|e| e.to_string())?;
        // From here the replace owns the candidate's lifecycle: only its
        // settlement, judged from the verified layout, may remove it.
        let candidate = staging.hand_off();
        replace_existing_file_with_context(&candidate, path, expected, &file_version(bytes),
            cancellation, &mut context)?;
    }
    if let Some(context) = context.as_deref_mut() {
        context.mark_committed();
        context.record_committed(path).map_err(|error| format!(
            "file committed but version registration failed; recovery materials retained: {error}"))?;
    }
    Ok(())
}

/// Replace `target` with the prepared `candidate` in place. The original is
/// renamed to a Host-owned same-volume backup instead of being deleted, an
/// applying record is flushed before the API call, and the outcome is judged
/// from the real file layout afterwards. The caller owns `candidate`; this
/// function settles it (retain or clean) before returning.
fn replace_existing_file(
    candidate: &Path,
    target: &Path,
    base_version: &str,
    new_version: &str,
) -> Result<(), String> {
    replace_existing_file_with_context(candidate, target, base_version, new_version, None, &mut None)
}

fn replace_existing_file_with_context(
    candidate: &Path, target: &Path, base_version: &str, new_version: &str,
    cancellation: Option<&crate::kernel::CancellationToken>,
    context: &mut Option<&mut dyn FileCommitContext>,
) -> Result<(), String> {
    let (backup, journal, operation_id) = replace_material_paths(target)?;
    // Preserve a verified pre-image BEFORE admission is revalidated or the OS
    // replacement is invoked on either platform. Windows' API backup remains a
    // second independent copy so it cannot overwrite the verified pre-image.
    let preimage = backup.with_extension("verified");
    if let Err(error) = prepare_verified_preimage(target, &preimage, base_version) {
        let _ = fs::remove_file(candidate);
        return Err(error);
    }
    let dispatch_note = context.as_deref().map(|context| format!(
        "dispatch_id={}; verified_preimage={}", context.dispatch_id(), preimage.display()));
    let mut record = ReplaceJournal {
        operation_id,
        status: "applying".to_string(),
        target: target.to_string_lossy().into_owned(),
        candidate: candidate.to_string_lossy().into_owned(),
        backup: backup.to_string_lossy().into_owned(),
        base_version: base_version.to_string(),
        new_version: new_version.to_string(),
        started_at_ms: epoch_ms(),
        settled_at_ms: None,
        os_error: None,
        note: dispatch_note,
    };
    // The record is created and flushed before the API is called. If the
    // directory cannot hold it, the replace is refused rather than attempted
    // without a durable trace — no best-effort proceed — and the candidate is
    // still pre-applying at that point, so it is cleaned up here.
    if let Err(error) = store_applying(&journal, &record) {
        let _ = fs::remove_file(candidate);
        return Err(error);
    }
    let final_check = (|| {
        check_cancellation(cancellation)?;
        if let Some(context) = context.as_deref_mut() { context.validate(target)?; }
        match probe_version(target) {
            Probe::Present(version) if version == base_version => Ok(()),
            _ => Err(ToolErrorCode::Conflict.error("File changed before atomic replacement; verified pre-image retained")),
        }
    })();
    if let Err(error) = final_check {
        record.status = "not_applied".into();
        record.settled_at_ms = Some(epoch_ms());
        if let Some(context) = context.as_deref_mut() { context.mark_not_applied(); }
        store_settled(&settled_record_path(&journal), &record, JournalFault::NONE)
            .map_err(|settle| format!("{error}; refusal settlement failed; materials retained: {settle}"))?;
        let _ = fs::remove_file(candidate);
        return Err(error);
    }
    if let Some(context) = context.as_deref_mut() { context.mark_applying(); }
    let outcome = replace_with_backup(target, candidate, &backup, &journal, &mut record, JournalFault::NONE);
    if let Some(context) = context.as_deref_mut() {
        match classify_replace_outcome(record.os_error, &inspect_layout(target, candidate, &backup), base_version, new_version) {
            ReplaceVerdict::Committed => context.mark_committed(),
            _ if record.status == "not_applied" => context.mark_not_applied(),
            _ => {},
        }
    }
    outcome
}

#[cfg(windows)]
fn win32_code(error: &windows::core::Error) -> u32 {
    // The `windows` binding freezes `GetLastError` into the error at the
    // instant of failure; `HRESULT_FROM_WIN32` keeps the original code in the
    // low 16 bits. Extract it before any later syscall can overwrite it.
    (error.code().0 as u32) & 0xFFFF
}

#[cfg(windows)]
fn replace_with_backup(
    target: &Path,
    candidate: &Path,
    backup: &Path,
    journal: &Path,
    record: &mut ReplaceJournal,
    fault: JournalFault,
) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::{core::PCWSTR, Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS}};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    let replaced = wide(target);
    let replacement = wide(candidate);
    let backup_name = wide(backup);
    // ReplaceFile preserves the destination's ACL and metadata, and renames the
    // original to `backup` instead of deleting it. There is no delete-then-
    // rename fallback: a failed replacement keeps both the original and the
    // candidate for reconciliation ([v1.1] 6.5 rule 7).
    let result = unsafe {
        ReplaceFileW(PCWSTR(replaced.as_ptr()), PCWSTR(replacement.as_ptr()),
            PCWSTR(backup_name.as_ptr()), REPLACE_FILE_FLAGS(0), None, None)
    };
    // Freeze the OS error code before any additional file check can overwrite
    // the thread-local last-error value ([v1.1] 6.5 rule 4).
    let os_error = result.as_ref().err().map(win32_code);
    settle_replace(target, candidate, backup, journal, record, os_error, fault)
}

/// Where the pre-image is staged before it is verified and published, next to
/// the backup it becomes.
fn staged_preimage_path(backup: &Path) -> PathBuf {
    let name = backup
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    backup.with_file_name(format!("{name}.staging"))
}

/// Build the Host-owned verified pre-image that `ReplaceFileW` would otherwise
/// create by renaming the original: copy the target to a staging name, flush
/// the copy, check its bytes against the pre-replace version, and only then
/// publish it with an atomic rename. Nothing about the target is touched while
/// this runs; any failure cleans the staging name and leaves the original in
/// place, so the replace that follows can still be refused *before* its side
/// effect ([v1.1] 6.5 rule 1).
fn prepare_verified_preimage(
    target: &Path,
    backup: &Path,
    base_version: &str,
) -> Result<(), String> {
    let staged = staged_preimage_path(backup);
    if let Err(error) = fs::copy(target, &staged) {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot stage the pre-image backup: {error}"));
    }
    // The staged copy must be on disk before it is verified or published. A
    // write handle is needed: on Windows, flushing a read-only handle is
    // refused.
    if let Err(error) = fs::OpenOptions::new()
        .write(true)
        .open(&staged)
        .and_then(|file| file.sync_all())
    {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot flush the pre-image backup: {error}"));
    }
    match probe_version(&staged) {
        Probe::Present(version) if version == base_version => {}
        Probe::Present(other) => {
            let _ = fs::remove_file(&staged);
            return Err(format!(
                "the staged pre-image does not match the pre-replace version (staged {other}, \
                 expected {base_version})"
            ));
        }
        Probe::Absent => {
            return Err(
                "the staged pre-image vanished before it could be verified".to_string()
            )
        }
        Probe::Unknown(error) => {
            let _ = fs::remove_file(&staged);
            return Err(format!("cannot verify the staged pre-image: {error}"));
        }
    }
    if let Err(error) = fs::rename(&staged, backup) {
        let _ = fs::remove_file(&staged);
        return Err(format!("cannot publish the pre-image backup: {error}"));
    }
    Ok(())
}

/// The in-place replace on platforms without `ReplaceFileW`: the Host builds
/// and verifies the pre-image itself, then performs the single atomic rename.
/// The verified backup is what makes `Committed` reachable, so a successful
/// replace is reported as a success with its recovery material in place —
/// never as an indeterminate state reported after the side effect has already
/// happened (R1-A01).
fn replace_with_verified_preimage(
    target: &Path,
    candidate: &Path,
    backup: &Path,
    journal: &Path,
    record: &mut ReplaceJournal,
    fault: JournalFault,
) -> Result<(), String> {
    match prepare_verified_preimage(target, backup, &record.base_version) {
        Ok(()) => {
            // POSIX rename is all-or-nothing, so it cannot leave the partial
            // states `ReplaceFileW` can; the record and the layout check are
            // kept so the same recovery discipline applies on every platform.
            if probe_version(target).present_version() != Some(record.base_version.as_str()) {
                return Err(ToolErrorCode::Conflict.error("File changed before rename; recovery materials retained"));
            }
            let os_error = match fs::rename(candidate, target) {
                Ok(()) => None,
                Err(error) => error.raw_os_error().map(|code| code as u32),
            };
            settle_replace(target, candidate, backup, journal, record, os_error, fault)
        }
        // The pre-image could not be built, so nothing has been renamed yet.
        // Settling from the real layout verifies that the target is untouched
        // and cleans the staged candidate, while the real reason the replace
        // was refused stays in the reported error.
        Err(preparation_error) => settle_replace(
            target,
            candidate,
            backup,
            journal,
            record,
            None,
            fault,
        )
        .map_err(|settled| format!("{preparation_error}; {settled}")),
    }
}

#[cfg(not(windows))]
fn replace_with_backup(
    target: &Path,
    candidate: &Path,
    backup: &Path,
    journal: &Path,
    record: &mut ReplaceJournal,
    fault: JournalFault,
) -> Result<(), String> {
    // Without `ReplaceFileW` the Host builds the verified pre-image itself
    // before the atomic rename; see `replace_with_verified_preimage`.
    replace_with_verified_preimage(target, candidate, backup, journal, record, fault)
}

fn prepare_write(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let path = required_string(input, "path")?;
    let content = content_field(input, "content")?;
    let create_directories = input
        .get("createDirectories")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    enforce_content_size(&content)?;
    let target = resolve_write_target(root, &path)?;
    let old = read_optional_text(&target)?;
    let expected_version = write_precondition(input, old.as_deref())?;
    let diff = line_diff(old.as_deref().unwrap_or_default(), &content, &target);
    let summary = if old.is_some() {
        format!("Replace the contents of {}", target.display())
    } else {
        format!("Create {}", target.display())
    };
    let preview = ToolPreview {
        tool: "write_file".to_owned(),
        title: if old.is_some() {
            "修改文件".to_owned()
        } else {
            "创建文件".to_owned()
        },
        target: target.to_string_lossy().into_owned(),
        summary,
        diff: Some(diff),
        command: None,
        cwd: None,
    };
    Ok(PreparedToolAction::WriteFile {
        root: root.to_path_buf(),
        path: target,
        content,
        expected_version,
        create_directories,
        preview,
    })
}

fn prepare_edit(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let path = required_string(input, "path")?;
    let old_text = required_string(input, "oldText")?;
    // Deleting a matched block is a legitimate edit, so the replacement may be
    // empty or whitespace only. It is stored exactly as given.
    let new_text = content_field(input, "newText")?;
    let replace_all = input
        .get("replaceAll")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let target = resolve_existing_target(root, &path, false)?;
    let current = read_text(&target)?;
    if old_text.is_empty() {
        return Err(ToolErrorCode::InvalidInput.error("oldText cannot be empty"));
    }
    let occurrences = current.matches(&old_text).count();
    if occurrences == 0 {
        return Err(ToolErrorCode::InvalidInput
            .error("oldText was not found in the target file; read the file again and use an exact match"));
    }
    if occurrences > 1 && !replace_all {
        return Err(ToolErrorCode::InvalidInput.error(format!(
            "oldText occurs {occurrences} times; set replaceAll to true or provide a more specific match"
        )));
    }
    let expected_version = write_precondition(input, Some(&current))?;
    let content = if replace_all {
        current.replace(&old_text, &new_text)
    } else {
        current.replacen(&old_text, &new_text, 1)
    };
    enforce_content_size(&content)?;
    let preview = ToolPreview {
        tool: "edit_file".to_owned(),
        title: "编辑文件".to_owned(),
        target: target.to_string_lossy().into_owned(),
        summary: format!(
            "Replace {occurrences} matching section(s) in {}",
            target.display()
        ),
        diff: Some(line_diff(&current, &content, &target)),
        command: None,
        cwd: None,
    };
    Ok(PreparedToolAction::EditFile {
        root: root.to_path_buf(),
        path: target,
        content,
        expected_version,
        preview,
    })
}

fn prepare_command(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let command = required_string(input, "command")?;
    if command.len() > 8_000 {
        return Err(ToolErrorCode::InvalidInput.error("command is too long"));
    }
    // An absent, null or blank `cwd` keeps its documented meaning: run in the
    // frozen project root. A present value of the wrong type is a malformed
    // call, not something to ignore.
    if let Some(cwd) = input.get("cwd") {
        if !cwd.is_null() && cwd.as_str().is_none() {
            return Err(ToolErrorCode::InvalidInput
                .error("tool input must contain a string cwd"));
        }
    }
    let cwd = input
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| resolve_existing_target(root, value, true))
        .transpose()?
        .unwrap_or_else(|| root.to_path_buf());
    let seconds = input
        .get("timeoutSeconds")
        .and_then(Value::as_u64)
        .unwrap_or(60)
        .clamp(1, MAX_COMMAND_SECONDS);
    let preview = ToolPreview {
        tool: "run_command".to_owned(),
        title: "运行命令".to_owned(),
        target: cwd.to_string_lossy().into_owned(),
        summary: format!("Run a command in {}", cwd.display()),
        diff: None,
        command: Some(command.clone()),
        cwd: Some(cwd.to_string_lossy().into_owned()),
    };
    Ok(PreparedToolAction::RunCommand {
        root: root.to_path_buf(),
        command,
        cwd,
        timeout: Duration::from_secs(seconds),
        preview,
    })
}

fn check_cancellation(cancellation: Option<&crate::kernel::CancellationToken>) -> Result<(), String> {
    cancellation.map_or(Ok(()), |token| {
        token
            .check()
            .map_err(|message| ToolErrorCode::Cancelled.error(message))
    })
}

/// How one command attempt ended. `Exited(None)` means the child produced no
/// exit code at all (signalled or otherwise unreported); that is deliberately
/// not the same as exit code 0. The payload type is `std::process::ExitStatus`'s
/// own `Option<i32>`; it is not widened to 64 bits anywhere in this path.
enum CommandOutcome {
    Exited(Option<i32>),
    TimedOut,
    Cancelled(String),
    /// `Child::try_wait` itself failed, so neither the exit code nor whether the
    /// child is still alive is knowable. The attempt ends as an error; output
    /// already produced is still collected, because skipping that drain would
    /// strand two reader slots and their threads.
    WaitFailed,
}

struct CommandContainment {
    #[cfg(windows)]
    job: Option<crate::process_jobs::WindowsJob>,
}

impl CommandContainment {
    fn attach(child: &std::process::Child) -> std::io::Result<Self> {
        #[cfg(windows)]
        {
            return crate::process_jobs::WindowsJob::assign_and_resume(child)
                .map(|job| Self { job: Some(job) });
        }
        #[cfg(not(windows))]
        {
            let _ = child;
            Ok(Self {})
        }
    }

    fn terminate(&mut self, child: &mut std::process::Child) {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            let _ = job.terminate();
            drop(job);
        }
        terminate_process_tree(child);
    }

    fn close_after_parent_exit(&mut self) -> std::io::Result<()> {
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            let terminated = job.terminate();
            drop(job);
            terminated?;
        }
        Ok(())
    }
}

pub(crate) fn command_spawn_spec(
    command: &str,
    cwd: &Path,
) -> crate::process_jobs::SpawnSpec {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe".to_owned(),
        vec![
            "/D".to_owned(),
            "/S".to_owned(),
            "/C".to_owned(),
            command.to_owned(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = ("sh".to_owned(), vec!["-lc".to_owned(), command.to_owned()]);

    crate::process_jobs::SpawnSpec {
        program,
        args,
        cwd: cwd.to_string_lossy().into_owned(),
        env: crate::process_jobs::EnvMode::Inherit,
    }
}

/// Explicit test injection for low-level command/output tests. This entry point
/// and its caller-supplied proof do not exist in the production build.
#[cfg(test)]
pub(crate) fn execute_command_with_test_backend(
    action: PreparedToolAction,
    proof: &dyn crate::process_jobs::BackendCapabilityProof,
) -> Result<Value, String> {
    proof.availability().authorize().map_err(|error| error.reason().to_owned())?;
    match action {
        PreparedToolAction::RunCommand { command, cwd, timeout, .. } => execute_command(&command, &cwd, timeout, None),
        _ => Err("test backend only accepts commands".into()),
    }
}

fn execute_command(command: &str, cwd: &Path, timeout: Duration, cancellation: Option<&crate::kernel::CancellationToken>) -> Result<Value, String> {
    // The reader budget is process-wide, so the tests that measure it cannot run
    // against each other's reservations. Compiles out of the product build; in
    // tests it serializes command execution on a re-entrant guard (a test that
    // already holds it - the ceiling tests - simply nests).
    #[cfg(test)]
    let _budget = tests::lock_reader_budget();
    check_cancellation(cancellation)?;
    // Reap output readers left behind by earlier commands whose pipes only now
    // reached EOF, before this call adds any of its own.
    reap_finished_readers();
    let spawn_spec = command_spawn_spec(command, cwd);
    let mut process = Command::new(&spawn_spec.program);
    crate::process_jobs::apply_spawn_arguments(&mut process, &spawn_spec);
    #[cfg(windows)]
    {
        process.creation_flags(
            crate::process_jobs::CREATE_NO_WINDOW_FLAG
                | crate::process_jobs::CREATE_SUSPENDED_FLAG,
        );
    }
    match &spawn_spec.env {
        crate::process_jobs::EnvMode::Inherit => {
            command_environment::apply_inherited(&mut process)
        }
        crate::process_jobs::EnvMode::Replace(entries) => {
            command_environment::apply_explicit(&mut process, entries)
                .map_err(|error| ToolErrorCode::InvalidInput.error(error.to_string()))?;
        }
    }
    // A command costs two drain slots, and both of its pipes exist from here on.
    // Reserve them while there is still no child: after spawning, the only ways
    // out of a full ceiling are leaking a thread or hanging on a read that no
    // user-space code can cancel, so refusal has to happen at this seam.
    if !reserve_reader_slots(READERS_PER_COMMAND) {
        return Err(ToolErrorCode::StartFailed.error(format!(
            "Fox 的输出读取线程已达上限（{used}/{MAX_LIVE_OUTPUT_READERS}）：有前序命令的后代仍占着管道，其读取线程无法从用户态取消。**本次命令未启动，未产生任何副作用**。等待片刻后重试即可（新命令会先回收已结束的读取线程），或让命令把输出重定向到文件（command > out.txt 2>&1）后用读取工具分页查看",
            used = live_output_readers()
        )));
    }
    let mut child = match process
        .current_dir(&spawn_spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            // No reader was started, so the whole reservation goes back.
            for _ in 0..READERS_PER_COMMAND {
                release_reader_slot();
            }
            return Err(ToolErrorCode::StartFailed.error(format!(
                "failed to start command: {error}"
            )));
        }
    };
    let mut containment = match CommandContainment::attach(&child) {
        Ok(containment) => containment,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            for _ in 0..READERS_PER_COMMAND {
                release_reader_slot();
            }
            return Err(ToolErrorCode::StartFailed.error(format!(
                "failed to confine command before execution: {error}"
            )));
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    // Each call consumes exactly one of the two reserved slots, releasing it
    // again if that pipe turned out not to exist.
    let mut stdout = attach_output(stdout, MAX_COMMAND_OUTPUT_BYTES);
    let mut stderr = attach_output(stderr, MAX_COMMAND_OUTPUT_BYTES);
    let started = Instant::now();
    let mut wait_failure = None;
    let outcome = loop {
        if let Err(error) = check_cancellation(cancellation) {
            containment.terminate(&mut child);
            break CommandOutcome::Cancelled(error);
        }
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(error) => {
                // An OS-level wait failure used to return straight out of the
                // loop, which skipped the drain and abandoned both reader slots
                // and threads. Record it, tear the tree down and fall through to
                // the common collection below instead.
                wait_failure = Some(ToolErrorCode::Unknown.error(format!(
                    "failed to wait for command: {error}"
                )));
                containment.terminate(&mut child);
                break CommandOutcome::WaitFailed;
            }
        };
        if let Some(status) = status {
            break CommandOutcome::Exited(status.code());
        }
        if started.elapsed() >= timeout {
            containment.terminate(&mut child);
            break CommandOutcome::TimedOut;
        }
        thread::sleep(Duration::from_millis(40));
    };
    if matches!(outcome, CommandOutcome::Exited(_)) {
        if let Err(error) = containment.close_after_parent_exit() {
            wait_failure = Some(ToolErrorCode::Unknown.error(format!(
                "failed to terminate command descendants after shell exit: {error}"
            )));
        }
    }
    // The tree is gone (or was never going to be killed), so both pipes normally
    // reach end of file; whatever the child printed before it stopped is what
    // the model needs in order to act. The snapshot is taken even when a
    // surviving descendant keeps a pipe open past the grace window, and that
    // case is reported instead of being presented as an empty result.
    let (stdout, stderr) = collect_streams(stdout.take(), stderr.take(), STREAM_DRAIN_GRACE);
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let undrained = stdout.undrained || stderr.undrained;
    let mut text = if stderr.text.trim().is_empty() {
        stdout.text.clone()
    } else if stdout.text.trim().is_empty() {
        stderr.text.clone()
    } else {
        format!("{}\n\n[stderr]\n{}", stdout.text, stderr.text)
    };
    // Every byte the child produced is accounted for. Output within the
    // retention cap reaches the Host result pipeline intact (and from there
    // the blob store); only bytes beyond the per-stream cap were dropped,
    // and that fact is part of the result, with the way to get the rest.
    let dropped = stdout.dropped_bytes + stderr.dropped_bytes;
    if dropped > 0 {
        text.push_str(&format!(
            "\n\n[fox: 输出超过每路 {} 字节保留上限，丢弃 {dropped} 字节；可将输出重定向到文件后用读取工具分页查看]",
            MAX_COMMAND_OUTPUT_BYTES,
        ));
    }
    let (code, error_code, outcome_text) = match &outcome {
        CommandOutcome::Exited(Some(0)) => (Some(0), None, String::new()),
        CommandOutcome::Exited(code) => (
            *code,
            Some(ToolErrorCode::NonZeroExit),
            match code {
                Some(code) => format!("[fox: 命令以退出码 {code} 结束（非零退出，工具失败）]\n"),
                None => "[fox: 命令未报告退出码（进程被终止或异常结束，工具失败）]\n".to_owned(),
            },
        ),
        CommandOutcome::TimedOut => (
            None,
            Some(ToolErrorCode::TimedOut),
            format!(
                "[fox: 命令超过 {timeout:?} 超时上限被终止，以下是终止前已产生的输出]\n"
            ),
        ),
        CommandOutcome::Cancelled(_) => (
            None,
            Some(ToolErrorCode::Cancelled),
            "[fox: 命令被取消，以下是取消前已产生的输出]\n".to_owned(),
        ),
        CommandOutcome::WaitFailed => (
            None,
            Some(ToolErrorCode::Unknown),
            "[fox: 等待子进程结束的调用本身失败，退出码与该进程是否仍在运行不可确定，以下是已产生的输出]\n".to_owned(),
        ),
    };
    let mut text = if outcome_text.is_empty() {
        text
    } else {
        format!("{outcome_text}{text}").trim_end().to_owned()
    };
    if undrained {
        text.push_str("\n\n[fox: 子进程仍有输出流未被读完，上面的输出不完整；退出码与该流的剩余部分不可确定]");
    }
    let encoding = if stdout.encoding == stderr.encoding {
        stdout.encoding.to_owned()
    } else {
        format!("{}+{}", stdout.encoding, stderr.encoding)
    };
    let mut details = json!({
        "cwd": cwd,
        "exitCode": code,
        "timedOut": matches!(outcome, CommandOutcome::TimedOut),
        "cancelled": matches!(outcome, CommandOutcome::Cancelled(_)),
        "undrainedOutput": undrained,
        "stdout": stdout.text,
        "stderr": stderr.text,
        "stdoutBytes": stdout.total_bytes,
        "stderrBytes": stderr.total_bytes,
        "outputDroppedBytes": dropped,
        "outputRetentionBytes": MAX_COMMAND_OUTPUT_BYTES,
        "elapsedMs": elapsed_ms,
        "timeoutMs": timeout.as_millis() as u64,
        "shell": if cfg!(windows) { "cmd.exe /D /S /C" } else { "sh -lc" },
        "platform": if cfg!(windows) { "windows" } else { "posix" },
        "outputEncoding": encoding,
    });
    if let Some(error_code) = error_code {
        if let Some(object) = details.as_object_mut() {
            object.insert("errorCode".to_owned(), json!(error_code.as_str()));
        }
        if encoding.contains("lossy") || encoding.contains("undecodable") || encoding.contains("truncated") {
            if let Some(object) = details.as_object_mut() {
                object.insert(
                    "outputEncodingNote".to_owned(),
                    json!("子进程输出不是 UTF-8，已按控制台代码页尽力解码（见 outputEncoding）；需要逐字精确的文本时，让命令把输出重定向到文件（command > out.txt 2>&1）后用读取工具查看。"),
                );
            }
        }
    }
    if let Some(message) = wait_failure {
        // The attempt cannot be described as a business result: whether the
        // child ran to completion is unknowable. It fails the same way an
        // uncertain effect must, and the output Fox did manage to read travels
        // with the message rather than being thrown away with the reader.
        let retained = text.chars().take(4_000).collect::<String>();
        return Err(format!("{message}; 失败前已产生的输出：\n{retained}"));
    }
    if let CommandOutcome::Cancelled(error) = outcome {
        // Cancellation is not a business result: it must keep failing the Run
        // terminal path. The retained output still travels with it.
        let retained = text.chars().take(4_000).collect::<String>();
        return Err(ToolErrorCode::Cancelled.error(format!(
            "{error}; 取消前已产生的输出：\n{retained}"
        )));
    }
    let result = failure_text_result(text, details, error_code.is_some());
    Ok(result)
}

#[cfg(windows)]
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
    // `Child::kill` only terminates cmd.exe. Kill its descendants as well so
    // a timed-out compiler, shell script, or package manager cannot survive
    // invisibly after Fox reports the tool failure.
    let pid = child.id().to_string();
    let _ = Command::new("taskkill")
        .args(["/PID", &pid, "/T", "/F"])
        .creation_flags(0x0800_0000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(windows))]
pub(crate) fn terminate_process_tree(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// What one output reader has absorbed so far, published under the lock after
/// every chunk. The waiting side can take a consistent snapshot even while the
/// reader is still blocked on a pipe that a surviving descendant holds open -
/// which is exactly the moment when the already-produced output must not be
/// thrown away together with the thread.
#[derive(Debug, Default)]
struct StreamProgress {
    kept: Vec<u8>,
    total_bytes: u64,
    finished: bool,
    /// Set once the owning command has taken its snapshot. A reader that is
    /// still blocked then keeps draining (the child must never deadlock on a
    /// full pipe) but stops retaining bytes: parking a wedged thread would
    /// otherwise also park up to `MAX_COMMAND_OUTPUT_BYTES` per stream for as
    /// long as the descendant lives.
    detached: bool,
}

/// One attached output reader: shared progress plus the thread feeding it. The
/// handle is always either joined or parked for a later join by
/// [`reap_finished_readers`]; it is never silently dropped.
struct OutputStream {
    progress: std::sync::Arc<Mutex<StreamProgress>>,
    reader: Option<thread::JoinHandle<()>>,
}

fn stream_finished(stream: &OutputStream) -> bool {
    stream
        .progress
        .lock()
        .map(|progress| progress.finished)
        .unwrap_or(true)
}

/// Readers that outlived their command's drain grace. A blocked `read` cannot
/// be interrupted from outside, so the thread is parked here and joined by a
/// later command once its pipe reached EOF. Parking (rather than forgetting) is
/// what makes a wedged reader visible and reclaimable; keeping the shared
/// progress next to the handle is what makes its retained bytes observable, so
/// the ceiling can be proven in bytes as well as in threads.
struct ParkedReader {
    handle: thread::JoinHandle<()>,
    progress: std::sync::Arc<Mutex<StreamProgress>>,
}

static ABANDONED_READERS: Mutex<Vec<ParkedReader>> = Mutex::new(Vec::new());

/// Ceiling on output reader threads alive at once, i.e. draining plus parked but
/// still blocked. Parking alone bounds nothing: a descendant that never closes
/// the write end means its reader never returns from `read`, so every such
/// command would permanently add threads (and their retained buffers) for the
/// rest of the process. A blocked read cannot be cancelled in user space, so the
/// bound is enforced where it still has a choice - before the child exists - and
/// exceeding it is a reported refusal rather than a silent leak or a hang.
pub(crate) const MAX_LIVE_OUTPUT_READERS: usize = 64;

/// Both of a command's streams are piped, so every command costs exactly this
/// many drain slots, whether or not it later produces output on them.
const READERS_PER_COMMAND: usize = 2;

/// Slots currently held by live readers. Reserved by
/// [`reserve_reader_slots`] and released when a reader is joined, so it counts
/// what is actually still running rather than what has been parked.
static LIVE_OUTPUT_READERS: AtomicUsize = AtomicUsize::new(0);

fn live_output_readers() -> usize {
    LIVE_OUTPUT_READERS.load(Ordering::Acquire)
}

/// Take `wanted` slots atomically, or none. Called before the child is spawned
/// so a refusal cannot leave a side effect behind.
fn reserve_reader_slots(wanted: usize) -> bool {
    let mut current = LIVE_OUTPUT_READERS.load(Ordering::Acquire);
    loop {
        if current.saturating_add(wanted) > MAX_LIVE_OUTPUT_READERS {
            return false;
        }
        match LIVE_OUTPUT_READERS.compare_exchange_weak(
            current,
            current + wanted,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return true,
            Err(observed) => current = observed,
        }
    }
}

/// One reader is done being a thread: joined, or never started because the pipe
/// was absent. `attach_output` consumes exactly one reserved slot per call, so
/// the two callers below together account for both streams of a command.
fn release_reader_slot() {
    let previous = LIVE_OUTPUT_READERS.fetch_sub(1, Ordering::AcqRel);
    debug_assert!(
        previous > 0,
        "an output reader slot was released more often than it was reserved"
    );
}

/// How many readers are still owed a join. A test-only observation point: the
/// count is what proves a wedged reader was parked rather than forgotten, that a
/// later command reaped it, and that the ceiling actually holds.
#[cfg(test)]
pub(crate) fn abandoned_reader_count() -> usize {
    ABANDONED_READERS
        .lock()
        .map(|handles| handles.len())
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) fn live_reader_slots() -> usize {
    live_output_readers()
}

/// Bytes still retained by parked (still-blocked) readers. A test-only probe for
/// the other half of the ceiling: wedged threads must keep draining so their
/// child never deadlocks, but they must not keep holding output nobody will
/// read again.
#[cfg(test)]
pub(crate) fn parked_retained_bytes() -> usize {
    let Ok(readers) = ABANDONED_READERS.lock() else { return usize::MAX };
    readers
        .iter()
        .map(|parked| {
            parked
                .progress
                .lock()
                .map(|state| state.kept.len())
                .unwrap_or(usize::MAX)
        })
        .sum()
}

/// Join parked readers that have since finished, without ever blocking. Runs
/// before each command so reaping happens on a path that is actually used, and
/// every join gives its slot back.
pub(crate) fn reap_finished_readers() {
    let Ok(mut handles) = ABANDONED_READERS.lock() else { return };
    if handles.is_empty() {
        return;
    }
    // `JoinHandle::join` consumes the handle, so the parked set is rebuilt from
    // the still-blocked readers instead of retaining it in place.
    let pending = std::mem::take(&mut *handles);
    let mut still_running = Vec::with_capacity(pending.len());
    for parked in pending {
        if parked.handle.is_finished() {
            let _ = parked.handle.join();
            release_reader_slot();
        } else {
            still_running.push(parked);
        }
    }
    *handles = still_running;
}

fn retire_reader(
    handle: thread::JoinHandle<()>,
    progress: std::sync::Arc<Mutex<StreamProgress>>,
) {
    if handle.is_finished() {
        let _ = handle.join();
        release_reader_slot();
        return;
    }
    if let Ok(mut handles) = ABANDONED_READERS.lock() {
        handles.push(ParkedReader { handle, progress });
        return;
    }
    // A poisoned registry leaves the thread detached: it still belongs to the
    // process and exits as soon as the pipe closes, but nothing can join it any
    // more, so its slot stays taken. Under-reserving would let the real thread
    // count drift past [`MAX_LIVE_OUTPUT_READERS`] unnoticed.
}

/// Attach a draining reader to one child pipe, consuming one reserved slot. It
/// keeps draining past the retention cap so a verbose child can never deadlock
/// on a full pipe, while the accounting makes any dropped bytes explicit in the
/// result. `None` means the child had no such pipe; the reservation is handed
/// back in that case, so the caller never has to know which stream was absent.
fn attach_output<R>(reader: Option<R>, retain: usize) -> Option<OutputStream>
where
    R: Read + Send + 'static,
{
    let mut reader = match reader {
        Some(reader) => reader,
        None => {
            release_reader_slot();
            return None;
        }
    };
    let progress = std::sync::Arc::new(Mutex::new(StreamProgress::default()));
    let shared = std::sync::Arc::clone(&progress);
    let handle = thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    let Ok(mut state) = shared.lock() else { break };
                    state.total_bytes = state.total_bytes.saturating_add(count as u64);
                    if state.detached {
                        // Already reported: keep the pipe moving, hold nothing.
                        if !state.kept.is_empty() {
                            state.kept = Vec::new();
                        }
                        continue;
                    }
                    let keep = count.min(retain.saturating_sub(state.kept.len()));
                    state.kept.extend_from_slice(&buffer[..keep]);
                }
            }
        }
        if let Ok(mut state) = shared.lock() {
            state.finished = true;
        }
    });
    Some(OutputStream {
        progress,
        reader: Some(handle),
    })
}

/// Wait for both pipes against one shared deadline, then take both snapshots and
/// retire both readers. Collecting the streams together is what keeps the grace
/// period from being paid twice, and taking the snapshot before the join is what
/// preserves output from a stream that never reached EOF.
fn collect_streams(
    stdout: Option<OutputStream>,
    stderr: Option<OutputStream>,
    grace: Duration,
) -> (CapturedStream, CapturedStream) {
    let deadline = Instant::now() + grace;
    loop {
        let done = stdout.as_ref().map_or(true, stream_finished)
            && stderr.as_ref().map_or(true, stream_finished);
        if done || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let past_deadline = Instant::now() >= deadline;
    (
        collect_stream(stdout, past_deadline),
        collect_stream(stderr, past_deadline),
    )
}

/// Take the snapshot first and only then retire the reader: a stream that never
/// reached EOF still contributes everything it had already produced.
fn collect_stream(stream: Option<OutputStream>, undrained_expected: bool) -> CapturedStream {
    let Some(OutputStream { progress, mut reader }) = stream else {
        return CapturedStream {
            encoding: "none".to_owned(),
            ..Default::default()
        };
    };
    let (kept, total_bytes, finished) = match progress.lock() {
        Ok(mut state) => {
            let (total_bytes, finished) = (state.total_bytes, state.finished);
            // Hand the bytes over instead of cloning them, and tell the reader
            // to stop retaining: a stream that never reached EOF is about to be
            // parked, and a parked thread must not also park its whole retention
            // cap for as long as the descendant holds the pipe open.
            let kept = std::mem::take(&mut state.kept);
            if !finished {
                state.detached = true;
            }
            (kept, total_bytes, finished)
        }
        Err(_) => (Vec::new(), 0, false),
    };
    if let Some(handle) = reader.take() {
        retire_reader(handle, std::sync::Arc::clone(&progress));
    }
    let undrained = !finished;
    debug_assert!(!undrained || undrained_expected);
    let (text, mut encoding) = decode_child_bytes(&kept);
    if undrained {
        encoding.push_str("+truncated");
    }
    CapturedStream {
        text,
        total_bytes,
        dropped_bytes: total_bytes.saturating_sub(kept.len() as u64),
        encoding,
        undrained,
    }
}

/// A decoded view of one output stream: everything the reader had absorbed when
/// the command stopped or the drain window closed.
#[derive(Debug, Default)]
struct CapturedStream {
    text: String,
    total_bytes: u64,
    dropped_bytes: u64,
    /// How `text` was obtained from the child's bytes, e.g. `utf-8`, `cp936` or
    /// `cp936+truncated`. `none` means there was no stream at all.
    encoding: String,
    /// `true` when the reader had not reached EOF, so the text is what was
    /// produced *so far* rather than the whole stream.
    undrained: bool,
}

/// Turn a child's raw bytes into text plus the name of what was actually
/// applied. UTF-8 output is taken verbatim; a stream cut mid-sequence keeps its
/// decodable prefix and says so; anything else is decoded with the codepage the
/// console is actually using instead of being silently replaced.
fn decode_child_bytes(bytes: &[u8]) -> (String, String) {
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_owned(), "utf-8".to_owned()),
        Err(error) if error.error_len().is_none() => {
            let mut text = String::from_utf8_lossy(&bytes[..error.valid_up_to()]).into_owned();
            text.push('\u{FFFD}');
            (text, "utf-8+truncated".to_owned())
        }
        Err(_) => decode_console_codepage(bytes),
    }
}

/// The code page a child writes its redirected output with. A GUI host owns no
/// console, so the per-process console code page can be unset; the machine OEM
/// code page is what `cmd.exe` falls back to (CP936 on a Chinese Windows).
#[cfg(windows)]
fn console_output_codepage() -> u32 {
    use windows::Win32::Globalization::GetOEMCP;
    use windows::Win32::System::Console::GetConsoleOutputCP;
    let (console_codepage, oem_codepage) = unsafe { (GetConsoleOutputCP(), GetOEMCP()) };
    if console_codepage == 0 {
        oem_codepage
    } else {
        console_codepage
    }
}

#[cfg(windows)]
fn decode_console_codepage(bytes: &[u8]) -> (String, String) {
    use windows::Win32::Globalization::{MultiByteToWideChar, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
    let codepage = console_output_codepage();
    let lossy = || String::from_utf8_lossy(bytes).into_owned();
    if codepage == 0 || codepage == 65001 || bytes.is_empty() {
        return (lossy(), format!("cp{codepage}-lossy"));
    }
    // Two calls, as WinAPI requires: ask for the buffer size, then convert.
    // Flags are left at 0 so an undecodable byte produces a replacement rather
    // than failing the whole stream; the label below still reports which case hit.
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let wide = unsafe {
        let count = MultiByteToWideChar(codepage, flags, bytes, None);
        if count <= 0 {
            return (lossy(), format!("cp{codepage}-undecodable"));
        }
        let mut buffer = vec![0u16; count as usize];
        let written = MultiByteToWideChar(codepage, flags, bytes, Some(&mut buffer));
        if written <= 0 {
            return (lossy(), format!("cp{codepage}-undecodable"));
        }
        buffer.truncate(written as usize);
        buffer
    };
    match String::from_utf16(&wide) {
        Ok(text) => (text, format!("cp{codepage}")),
        Err(_) => (lossy(), format!("cp{codepage}-undecodable")),
    }
}

#[cfg(not(windows))]
fn decode_console_codepage(bytes: &[u8]) -> (String, String) {
    (
        String::from_utf8_lossy(bytes).into_owned(),
        "non-utf8-lossy".to_owned(),
    )
}

fn required_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            ToolErrorCode::InvalidInput
                .error(format!("tool input must contain a non-empty {key}"))
        })
}

/// One of the two file payloads (`content`, `newText`). Unlike a path or a
/// command, an empty or whitespace-only file body is a valid request: clearing
/// a file and deleting a matched block are both ordinary edits. The field must
/// still be present and be a string, and the size limit is enforced separately.
fn content_field(input: &Value, key: &str) -> Result<String, String> {
    match input.get(key) {
        None => Err(ToolErrorCode::InvalidInput.error(format!(
            "tool input must contain a string {key}"
        ))),
        Some(Value::String(value)) => Ok(value.clone()),
        Some(_) => Err(ToolErrorCode::InvalidInput.error(format!(
            "tool input must contain a string {key}"
        ))),
    }
}

fn enforce_content_size(content: &str) -> Result<(), String> {
    if content.len() as u64 > MAX_FILE_BYTES {
        Err(ToolErrorCode::InvalidInput.error(format!(
            "file content exceeds the {} byte limit",
            MAX_FILE_BYTES
        )))
    } else {
        Ok(())
    }
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf, String> {
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("{label} cannot be resolved: {error}"))?;
    if !canonical.is_dir() {
        return Err(format!("{label} is not a directory"));
    }
    Ok(canonical)
}

fn resolve_existing_target(
    root: &Path,
    value: &str,
    require_directory: bool,
) -> Result<PathBuf, String> {
    validate_path_text(value)?;
    let requested = Path::new(value);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let target = joined
        .canonicalize()
        .map_err(|error| format!("tool path cannot be resolved: {error}"))?;
    ensure_inside(root, &target)?;
    if require_directory && !target.is_dir() {
        return Err("tool path must be a directory".to_owned());
    }
    if !require_directory && !target.is_file() {
        return Err("tool path must be a file".to_owned());
    }
    Ok(target)
}

fn resolve_write_target(root: &Path, value: &str) -> Result<PathBuf, String> {
    validate_path_text(value)?;
    let requested = Path::new(value);
    let joined = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    if joined.exists() {
        let target = joined
            .canonicalize()
            .map_err(|error| format!("tool path cannot be resolved: {error}"))?;
        ensure_inside(root, &target)?;
        if target.is_dir() {
            return Err("write target cannot be a directory".to_owned());
        }
        return Ok(target);
    }

    let mut ancestor = joined.as_path();
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| "write target has no resolvable parent".to_owned())?;
        missing.push(name.to_os_string());
        ancestor = ancestor
            .parent()
            .ok_or_else(|| "write target has no resolvable parent".to_owned())?;
    }
    let canonical_ancestor = ancestor
        .canonicalize()
        .map_err(|error| format!("write parent cannot be resolved: {error}"))?;
    ensure_inside(root, &canonical_ancestor)?;
    let mut target = canonical_ancestor;
    for component in missing.into_iter().rev() {
        target.push(component);
    }
    ensure_inside(root, &target)?;
    Ok(target)
}

fn revalidate_target(
    root: &Path,
    target: &Path,
    require_directory: Option<bool>,
) -> Result<(), String> {
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("project folder changed or is unavailable: {error}"))?;
    let canonical_target = if target.exists() {
        target
            .canonicalize()
            .map_err(|error| format!("tool target changed or is unavailable: {error}"))?
    } else {
        let mut ancestor = target;
        let mut missing = Vec::new();
        while !ancestor.exists() {
            let name = ancestor
                .file_name()
                .ok_or_else(|| "tool target has no resolvable parent".to_owned())?;
            missing.push(name.to_os_string());
            ancestor = ancestor
                .parent()
                .ok_or_else(|| "tool target has no resolvable parent".to_owned())?;
        }
        let mut canonical = ancestor
            .canonicalize()
            .map_err(|error| format!("tool target parent changed or is unavailable: {error}"))?;
        for component in missing.into_iter().rev() {
            canonical.push(component);
        }
        canonical
    };
    ensure_inside(&canonical_root, &canonical_target)?;
    if require_directory == Some(false) && !canonical_target.is_file() {
        return Err("tool target is no longer a file".to_owned());
    }
    if require_directory == Some(true) && !canonical_target.is_dir() {
        return Err("tool target is no longer a directory".to_owned());
    }
    if require_directory.is_none() && canonical_target.exists() && canonical_target.is_dir() {
        return Err("tool target cannot be a directory".to_owned());
    }
    Ok(())
}

fn validate_path_text(value: &str) -> Result<(), String> {
    if value.contains('\0') {
        return Err(ToolErrorCode::InvalidInput.error("tool path contains an invalid null byte"));
    }
    if Path::new(value)
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ToolErrorCode::PermissionDenied
            .error("tool path cannot contain parent traversal"));
    }
    Ok(())
}

fn ensure_inside(root: &Path, target: &Path) -> Result<(), String> {
    if target.starts_with(root) {
        Ok(())
    } else {
        Err(ToolErrorCode::PermissionDenied
            .error("tool path is outside the authorized project folder"))
    }
}

fn read_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| ToolErrorCode::InvalidInput.error(format!("failed to inspect file: {error}")))?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err(ToolErrorCode::InvalidInput.error(format!(
            "file exceeds the {} byte limit",
            MAX_FILE_BYTES
        )));
    }
    fs::read_to_string(path).map_err(|error| {
        ToolErrorCode::InvalidInput.error(format!("file is not valid UTF-8 text: {error}"))
    })
}

fn read_optional_text(path: &Path) -> Result<Option<String>, String> {
    if path.exists() {
        read_text(path).map(Some)
    } else {
        Ok(None)
    }
}

/// The version the commit critical section compares against. `binary` hashes the
/// raw bytes so a snapshot that is not valid UTF-8 still gets an exact,
/// non-lossy precondition instead of a decode failure. Both modes return the
/// `sha256:<hex>` version string, never raw content.
fn current_version_of(path: &Path, binary: bool) -> Result<Option<String>, String> {
    if !path.exists() {
        return Ok(None);
    }
    if binary {
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        Ok(Some(file_version(&bytes)))
    } else {
        Ok(read_optional_text(path)?.map(|text| file_version(text.as_bytes())))
    }
}

/// The in-lock precondition: the target must still be exactly the version the
/// caller read. A mismatch is a recoverable conflict, never a silent overwrite.
fn ensure_unchanged(expected: &str, current_version: Option<&str>) -> Result<(), String> {
    let version = current_version.unwrap_or("missing");
    if expected != version {
        return Err(ToolErrorCode::Conflict.error(
            "File changed since read (or was created/deleted). Read it again, reconcile your change,              and retry with its readVersion; no additional authorization is needed.",
        ));
    }
    Ok(())
}

fn line_diff(old: &str, new: &str, path: &Path) -> String {
    if old == new {
        return "No textual changes.".to_owned();
    }
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let mut prefix = 0;
    while prefix < old_lines.len()
        && prefix < new_lines.len()
        && old_lines[prefix] == new_lines[prefix]
    {
        prefix += 1;
    }
    let mut old_suffix = old_lines.len();
    let mut new_suffix = new_lines.len();
    while old_suffix > prefix
        && new_suffix > prefix
        && old_lines[old_suffix - 1] == new_lines[new_suffix - 1]
    {
        old_suffix -= 1;
        new_suffix -= 1;
    }
    let mut output = format!("--- {}\n+++ {}\n", path.display(), path.display());
    for line in old_lines[prefix..old_suffix].iter().take(240) {
        output.push_str(&format!("-{line}\n"));
    }
    for line in new_lines[prefix..new_suffix].iter().take(240) {
        output.push_str(&format!("+{line}\n"));
    }
    if old_suffix - prefix > 240 || new_suffix - prefix > 240 {
        output.push_str("... diff truncated ...\n");
    }
    output
}

fn text_result(text: String, details: Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

/// One completed-but-unsuccessful execution. The diagnostics stay with the
/// result (that is what makes the failure actionable for the model), while the
/// `isError` flag is the single fact every consumer uses to keep the outcome
/// failed: the Host persistence mapping, the receipt, the managed-file version
/// registration and the Kernel settled state all read it. This is not a
/// success value carrying an error note — `finalize_host_tool_execution` turns
/// it into a `failed` ToolCall with an error message.
fn failure_text_result(text: String, details: Value, failed: bool) -> Value {
    if !failed {
        return text_result(text, details);
    }
    json!({
        "isError": true,
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use uuid::Uuid;

    fn project() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-tool-host-{}", Uuid::new_v4()));
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src").join("note.txt"), "hello\nworld\n").unwrap();
        root
    }

    /// The reader ceiling is one process-wide budget, so tests that consume it
    /// must not observe each other's bookkeeping or be starved by the test that
    /// deliberately fills it. Every budget-touching test takes this guard, at
    /// test level and inside the helpers; it is re-entrant per thread, so a
    /// guarded test can call a guarded helper without deadlocking, and the
    /// underlying lock is released when the outermost guard goes away.
    static READER_BUDGET_TEST_LOCK: Mutex<()> = Mutex::new(());

    thread_local! {
        static BUDGET_GUARD: RefCell<Option<std::sync::MutexGuard<'static, ()>>> =
            const { RefCell::new(None) };
        static BUDGET_DEPTH: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) struct BudgetGuard;

    impl Drop for BudgetGuard {
        fn drop(&mut self) {
            let remaining = BUDGET_DEPTH.with(|depth| {
                let value = depth.get().saturating_sub(1);
                depth.set(value);
                value
            });
            if remaining == 0 {
                BUDGET_GUARD.with(|slot| *slot.borrow_mut() = None);
            }
        }
    }

    pub(super) fn lock_reader_budget() -> BudgetGuard {
        BUDGET_DEPTH.with(|depth| depth.set(depth.get() + 1));
        if !BUDGET_GUARD.with(|slot| slot.borrow().is_some()) {
            // Do not hold the RefCell borrow across a blocking lock.
            let guard = READER_BUDGET_TEST_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            BUDGET_GUARD.with(|slot| *slot.borrow_mut() = Some(guard));
        }
        BudgetGuard
    }

    /// `attach_output` consumes one reserved reader slot per call - that is the
    /// invariant `execute_command` upholds. Direct callers have to reserve the
    /// same way, or the ceiling would be measured against a broken baseline.
    fn attach_reserved<R>(reader: Option<R>, retain: usize) -> Option<OutputStream>
    where
        R: Read + Send + 'static,
    {
        let _budget = lock_reader_budget();
        assert!(
            reserve_reader_slots(1),
            "no reader slot was free while the test held the budget lock"
        );
        attach_output(reader, retain)
    }

    #[test]
    fn cancelled_tool_does_not_write_or_create_parent_directories() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "write").unwrap();
        let action = prepare("write_file", &json!({"path":"cancelled/note.txt","content":"must not be written"}), root.to_str().unwrap()).unwrap();
        registry.request_tool_cancel("r", "write");
        assert!(execute_with_cancellation(action, Some(&token)).unwrap_err().contains("tool.cancelled"));
        assert!(!root.join("cancelled").exists());
    }

    #[test]
    fn cancellation_stops_an_in_flight_command() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "command").unwrap();
        let command_root = root.clone();
        #[cfg(windows)]
        let command = "echo ready>started.txt & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "printf ready > started.txt; exec sleep 30";
        let worker = thread::spawn(move || execute_command(command, &command_root, Duration::from_secs(60), Some(&token)));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("started.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let started = root.join("started.txt").exists();
        let cancelled_at = Instant::now();
        registry.request_tool_cancel("r", "command");
        let result = worker.join().unwrap();
        assert!(started, "test command must actually start before cancellation");
        assert!(result.unwrap_err().contains("tool.cancelled"));
        assert!(cancelled_at.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn prepares_and_executes_file_edits_inside_the_project() {
        let root = project();
        let action = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "world", "newText": "Fox", "expectedVersion": file_version(b"hello\nworld\n") }),
            root.to_str().unwrap(),
        )
        .unwrap();
        assert!(action.preview().diff.as_deref().unwrap().contains("+Fox"));
        let result = execute(action).unwrap();
        assert_eq!(result["details"]["operation"], "modified");
        assert_eq!(
            fs::read_to_string(root.join("src/note.txt")).unwrap(),
            "hello\nFox\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepares_new_files_but_blocks_parent_traversal() {
        let root = project();
        let action = prepare(
            "write_file",
            &json!({ "path": "generated/report.md", "content": "# Report" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let result = execute(action).unwrap();
        assert_eq!(result["details"]["operation"], "created");
        assert_eq!(
            fs::read_to_string(root.join("generated/report.md")).unwrap(),
            "# Report"
        );
        assert!(prepare(
            "write_file",
            &json!({ "path": "../outside.txt", "content": "blocked" }),
            root.to_str().unwrap(),
        )
        .is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn command_output_beyond_the_old_cap_is_fully_retained_and_accounted() {
        let root = project();
        // ~1 MiB of output used to be cut at 512 KiB (and could wedge the
        // pipe); it must now reach the result intact with exact accounting.
        let script = root.join("emit.js");
        fs::write(
            &script,
            "const s='abcdefgh'.repeat(2000); for(let i=0;i<70;i++) process.stdout.write(s+'\\n');",
        )
        .unwrap();
        let command = "node emit.js";
        let result = execute_command(command, &root, Duration::from_secs(60), None).unwrap();
        let stdout = result["details"]["stdout"].as_str().unwrap();
        let expected = ("abcdefgh".repeat(2000) + "\n").repeat(70).len();
        assert_eq!(result["details"]["stdoutBytes"].as_u64().unwrap() as usize, expected);
        assert_eq!(stdout.len(), expected, "every byte is retained");
        assert_eq!(result["details"]["outputDroppedBytes"], 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn attached_readers_drain_past_retention_and_report_dropped_bytes() {
        let data = vec![b'x'; MAX_COMMAND_OUTPUT_BYTES + 10_000];
        let attached = attach_reserved(Some(std::io::Cursor::new(data)), MAX_COMMAND_OUTPUT_BYTES);
        let (captured, _) = collect_streams(attached, None, Duration::from_secs(5));
        assert_eq!(captured.text.len(), MAX_COMMAND_OUTPUT_BYTES);
        assert_eq!(captured.total_bytes, (MAX_COMMAND_OUTPUT_BYTES + 10_000) as u64);
        assert_eq!(captured.dropped_bytes, 10_000);
        assert!(!captured.undrained, "a finished stream must not claim truncation");
        let (empty, _) = collect_streams(None, None, Duration::from_millis(1));
        assert_eq!(empty.total_bytes, 0);
        assert_eq!(empty.encoding, "none");
    }

    #[test]
    fn synchronous_host_commands_require_a_verified_backend() {
        let root = project();
        let action = prepare("run_command", &json!({"command":"echo unsafe > should-not-exist.txt"}), root.to_str().unwrap()).unwrap();
        let error = execute(action).unwrap_err();
        assert!(error.contains("sandbox_unavailable"));
        assert!(!root.join("should-not-exist.txt").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn commands_must_use_a_working_directory_inside_the_project() {
        let root = project();
        let outside = root.parent().unwrap().to_path_buf();
        assert!(prepare(
            "run_command",
            &json!({ "command": "echo hello", "cwd": outside }),
            root.to_str().unwrap(),
        )
        .is_err());
        let action = prepare(
            "run_command",
            &json!({ "command": "echo hello", "cwd": ".", "timeoutSeconds": 5 }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let PreparedToolAction::RunCommand { command, cwd, timeout, .. } = action else { panic!("expected command"); };
        let result = execute_command(&command, &cwd, timeout, None).unwrap();
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("hello"));
        let _ = fs::remove_dir_all(root);
    }

    // ---------------------------------------------------------------- A01/A02

    fn command_text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().unwrap_or_default()
    }

    #[test]
    fn a_non_zero_exit_that_only_wrote_to_stdout_still_reports_that_output() {
        // The regression this guards: the failure branch used to build its
        // message from stderr only, so a compiler that prints its real reason
        // on stdout left the model with "exited with code 3: ".
        let root = project();
        #[cfg(windows)]
        let command = "echo failure-explained-on-stdout & exit 3";
        #[cfg(not(windows))]
        let command = "echo failure-explained-on-stdout; exit 3";
        let result = execute_command(command, &root, Duration::from_secs(30), None).unwrap();
        assert_eq!(result["isError"], Value::Bool(true));
        assert_eq!(result["details"]["exitCode"], 3);
        assert_eq!(result["details"]["errorCode"], "tool.nonzero_exit");
        assert!(
            command_text(&result).contains("failure-explained-on-stdout"),
            "retained stdout must reach the model: {result}"
        );
        assert_eq!(result["details"]["timedOut"], Value::Bool(false));
        assert_eq!(result["details"]["cancelled"], Value::Bool(false));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_non_zero_exit_keeps_both_streams_and_labels_them() {
        let root = project();
        #[cfg(windows)]
        let command = "echo built-stdout-marker & echo broken-stderr-marker 1>&2 & exit 9";
        #[cfg(not(windows))]
        let command = "echo built-stdout-marker; echo broken-stderr-marker 1>&2; exit 9";
        let result = execute_command(command, &root, Duration::from_secs(30), None).unwrap();
        let text = command_text(&result);
        assert!(text.contains("built-stdout-marker"), "{text}");
        assert!(text.contains("broken-stderr-marker"), "{text}");
        assert!(text.contains("[stderr]"), "streams must stay distinguishable: {text}");
        assert!(text.contains("退出码 9"), "the outcome line explains the classification: {text}");
        assert!(result["details"]["stdoutBytes"].as_u64().unwrap() >= 19);
        assert!(result["details"]["stderr"].as_str().unwrap().contains("broken-stderr-marker"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_timeout_reports_the_output_produced_before_the_tree_was_killed() {
        let root = project();
        #[cfg(windows)]
        let command = "echo partial-before-timeout & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo partial-before-timeout; sleep 30";
        let started = Instant::now();
        let result = execute_command(command, &root, Duration::from_secs(2), None).unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the timeout must actually bound the call"
        );
        assert_eq!(result["isError"], Value::Bool(true));
        assert_eq!(result["details"]["timedOut"], Value::Bool(true));
        assert_eq!(result["details"]["errorCode"], "tool.timed_out");
        assert!(
            result["details"].get("exitCode").unwrap().is_null(),
            "a killed child has no exit code; it must not be reported as 0"
        );
        assert!(command_text(&result).contains("partial-before-timeout"), "{result}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cancellation_keeps_failed_semantics_and_carries_the_partial_output() {
        use crate::kernel::CancellationPort;
        let root = project();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.tool_token("r", "cancel-me").unwrap();
        #[cfg(windows)]
        let command = "echo marker-before-cancel & echo started>started.txt & ping -n 30 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo marker-before-cancel; echo started > started.txt; sleep 30";
        let command_root = root.clone();
        let worker = thread::spawn(move || {
            execute_command(command, &command_root, Duration::from_secs(60), Some(&token))
        });
        // Wait until the child really ran instead of sleeping a fixed time: the
        // reader budget is process-wide, so this call may legitimately queue
        // before the child exists, and cancelling something that never started
        // would throw away the very output under test.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("started.txt").exists() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        let started = root.join("started.txt").exists();
        registry.request_tool_cancel("r", "cancel-me");
        let error = worker.join().unwrap().unwrap_err();
        assert!(started, "the test command must start before cancellation");
        // Cancellation is a Run-level outcome, not a business result: it still
        // travels through the error channel so the Run terminal state governs.
        assert!(error.contains("tool.cancelled"), "{error}");
        assert!(error.contains("marker-before-cancel"), "retained output must not be lost: {error}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_output_is_counted_and_a_remedy_is_offered() {
        let data = vec![b'x'; 100_000];
        let stream = attach_reserved(Some(std::io::Cursor::new(data)), 1_024);
        let (captured, _) = collect_streams(stream, None, Duration::from_secs(5));
        assert_eq!(captured.total_bytes, 100_000);
        assert_eq!(captured.dropped_bytes, 100_000 - 1_024);
        assert_eq!(captured.text.len(), 1_024);
        assert!(!captured.undrained);
        assert_eq!(captured.encoding, "utf-8");
        // `undrained` above belongs to this reader. Global parking counts may
        // change concurrently as unrelated command tests finish.
    }

    /// O-A-01 item 3: a descendant that keeps the write end open must not cost
    /// the already-produced output, and the reader thread must end up joined
    /// rather than abandoned. A real OS pipe makes this deterministic.
    #[test]
    fn a_pipe_that_never_closes_keeps_its_partial_output_and_is_reaped_later() {
        let _budget = lock_reader_budget();
        let baseline = abandoned_reader_count();
        let (receiver, sender) = std::io::pipe().expect("OS pipe");
        {
            use std::io::Write;
            let mut sender = sender.try_clone().expect("clone pipe writer");
            sender.write_all(b"first-part-of-the-log\n").unwrap();
        }
        let stream = attach_reserved(Some(receiver), MAX_COMMAND_OUTPUT_BYTES);
        let (captured, _) = collect_streams(stream, None, Duration::from_millis(150));
        assert!(
            captured.undrained,
            "the writer is still open, so the stream cannot be called complete"
        );
        assert_eq!(captured.encoding, "utf-8+truncated");
        assert!(
            captured.text.contains("first-part-of-the-log"),
            "output produced before the wedge must survive: {:?}",
            captured.text
        );
        assert!(
            abandoned_reader_count() >= baseline + 1,
            "the unfinished reader is parked for a later join: {} vs baseline {}",
            abandoned_reader_count(),
            baseline
        );
        let parked = abandoned_reader_count();
        drop(sender);
        // The next command's reaping pass joins it once the pipe reaches EOF.
        // Global counts are shared with other tests, so the assertion is that the
        // parked set shrank - this reader was joined - not an absolute number.
        for _ in 0..50 {
            reap_finished_readers();
            if abandoned_reader_count() < parked {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            abandoned_reader_count() < parked,
            "reader thread was not reaped after its pipe closed"
        );
    }

    /// O-A-01 follow-up: parking a wedged reader is not the same as bounding it.
    /// A descendant that never closes the write end never returns from `read`, so
    /// repeating such a command would add a thread every time. The bound has to
    /// be enforced where a choice still exists - before the child is created -
    /// and the refusal has to be visible instead of turning into a hang or a
    /// degraded read. Real OS pipes, no timing luck.
    #[test]
    fn wedged_readers_hit_a_ceiling_and_the_refusal_is_reported() {
        let _budget = lock_reader_budget();
        let parked_before = abandoned_reader_count();
        let mut senders = Vec::new();
        let mut held = 0usize;
        let deadline = Instant::now() + Duration::from_secs(20);
        while held < MAX_LIVE_OUTPUT_READERS {
            if Instant::now() >= deadline {
                break;
            }
            if !reserve_reader_slots(1) {
                // Something else is still holding a slot; it is released when its
                // reader is joined, so waiting is the only correct action.
                reap_finished_readers();
                thread::sleep(Duration::from_millis(5));
                continue;
            }
            let (receiver, sender) = std::io::pipe().expect("OS pipe");
            let stream = attach_output(Some(receiver), 4_096);
            // The snapshot detaches the reader and the still-open writer parks it.
            let (captured, _) = collect_streams(stream, None, Duration::from_millis(2));
            assert!(captured.undrained, "an open writer cannot read as drained");
            senders.push(sender);
            held += 1;
        }
        assert_eq!(
            held, MAX_LIVE_OUTPUT_READERS,
            "the ceiling was not reached within the deadline; live slots were {}",
            live_reader_slots()
        );
        // The invariant that matters: repetition stops adding readers.
        assert!(
            live_reader_slots() <= MAX_LIVE_OUTPUT_READERS,
            "the ceiling was exceeded: {}",
            live_reader_slots()
        );
        assert!(
            abandoned_reader_count() >= parked_before + held,
            "the wedged readers were parked, not forgotten"
        );
        assert_eq!(
            parked_retained_bytes(),
            0,
            "64 parked readers must not hold 64 retention caps"
        );

        // At the ceiling a new command is refused before a child exists: nothing
        // runs, no further thread is created, and the message says what happened
        // and what to do instead.
        let root = project();
        let marker = root.join("must-not-exist.txt");
        let error = execute_command(
            &format!("echo refused > {}", marker.display()),
            &root,
            Duration::from_secs(5),
            None,
        )
        .expect_err("a full reader ceiling has to refuse, not run degraded");
        assert!(error.starts_with("[tool.start_failed]"), "{error}");
        assert!(error.contains("未启动"), "{error}");
        assert!(error.contains("重定向到文件"), "{error}");
        assert!(!marker.exists(), "a refused command must not have run: {error}");

        // The pressure is transient: once the wedged pipes close, the parked
        // readers are joined, their slots come back and the same command runs.
        drop(senders);
        let mut recovered = false;
        for _ in 0..200 {
            reap_finished_readers();
            if execute_command("echo after-pressure", &root, Duration::from_secs(20), None).is_ok()
            {
                recovered = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            recovered,
            "reader capacity never came back; live slots were {}",
            live_reader_slots()
        );
        let _ = fs::remove_dir_all(root);
    }

    /// The other half of the same bound: a parked reader must not also park its
    /// retained output. The producer keeps writing after the command gave up on
    /// the stream; a reader that kept retaining would climb back to its cap and
    /// turn a thread ceiling into a memory leak instead.
    #[test]
    fn a_parked_reader_keeps_draining_but_holds_no_bytes() {
        use std::io::Write;
        let _budget = lock_reader_budget();
        let (receiver, mut sender) = std::io::pipe().expect("OS pipe");
        sender.write_all(&[b'a'; 512]).unwrap();
        let stream = attach_reserved(Some(receiver), 1_024);
        let (captured, _) = collect_streams(stream, None, Duration::from_millis(200));
        assert!(captured.undrained);
        assert_eq!(
            captured.text.len(),
            512,
            "the snapshot must hand over what was produced before the wedge"
        );
        assert_eq!(
            parked_retained_bytes(),
            0,
            "the reported bytes must not stay parked"
        );
        for _ in 0..3 {
            // 2 KiB at a time, under the OS pipe capacity: the writes only
            // succeed while the parked reader keeps draining.
            sender.write_all(&[b'b'; 2_048]).unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            parked_retained_bytes(),
            0,
            "a detached reader counts bytes, it does not accumulate them"
        );
        drop(sender);
        let parked = abandoned_reader_count();
        for _ in 0..100 {
            reap_finished_readers();
            if abandoned_reader_count() < parked {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            abandoned_reader_count() < parked,
            "the reader was not joined after its pipe closed"
        );
        assert!(
            live_reader_slots() <= MAX_LIVE_OUTPUT_READERS,
            "the ceiling held while this reader lived: {}",
            live_reader_slots()
        );
    }

    #[test]
    fn cancelling_a_command_that_outruns_its_grace_window_still_reaps_readers() {
        let baseline = abandoned_reader_count();
        let root = project();
        // The child itself is short-lived; the pipe holder is its descendant, so
        // the drain window can close while a reader is still blocked.
        #[cfg(windows)]
        let command = "echo before-detach & start /b cmd /c \"ping -n 6 127.0.0.1 > NUL\" & ping -n 6 127.0.0.1 > NUL";
        #[cfg(not(windows))]
        let command = "echo before-detach; (sleep 6 &) ; sleep 6";
        let result = execute_command(command, &root, Duration::from_millis(400), None).unwrap();
        assert_eq!(result["details"]["errorCode"], "tool.timed_out");
        assert!(
            result["content"][0]["text"].as_str().unwrap().contains("before-detach"),
            "{result}"
        );
        let _ = fs::remove_dir_all(root);
        // Whatever this call parked, a following call reaps it.
        execute_command("echo next", &std::env::temp_dir(), Duration::from_secs(20), None).ok();
        for _ in 0..50 {
            reap_finished_readers();
            if abandoned_reader_count() <= baseline {
                break;
            }
            thread::sleep(Duration::from_millis(40));
        }
        assert!(
            abandoned_reader_count() <= baseline,
            "readers from the timed-out call were not reaped: {}",
            abandoned_reader_count()
        );
    }

    #[test]
    fn error_codes_round_trip_and_never_double_tag() {
        for code in [
            ToolErrorCode::NonZeroExit,
            ToolErrorCode::StartFailed,
            ToolErrorCode::InvalidInput,
            ToolErrorCode::PermissionDenied,
            ToolErrorCode::TimedOut,
            ToolErrorCode::Cancelled,
            ToolErrorCode::Unknown,
        ] {
            let message = code.error("a reason");
            assert_eq!(message, format!("[{}] a reason", code.as_str()));
            assert_eq!(ToolErrorCode::classify(&message), Some(code));
            assert_eq!(
                ToolErrorCode::PermissionDenied.error(message.as_str()),
                message,
                "an existing classification must survive re-wrapping"
            );
        }
        assert_eq!(ToolErrorCode::classify("plain prose"), None);
        assert_eq!(ToolErrorCode::classify("[tool.made_up] prose"), None);
        assert!(ToolErrorCode::NonZeroExit.is_completed_failure());
        assert!(!ToolErrorCode::PermissionDenied.is_completed_failure());
    }

    // ------------------------------------------------------------------- A03

    #[test]
    fn an_empty_and_a_whitespace_only_file_body_are_written_exactly_as_given() {
        let root = project();
        let empty = prepare(
            "write_file",
            &json!({ "path": "cleared.txt", "content": "" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(execute(empty).unwrap()["details"]["bytes"], 0);
        assert_eq!(fs::read(root.join("cleared.txt")).unwrap().len(), 0);

        let blank = prepare(
            "write_file",
            &json!({ "path": "indent.txt", "content": "   \n\t  \n" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(blank).unwrap();
        assert_eq!(fs::read_to_string(root.join("indent.txt")).unwrap(), "   \n\t  \n");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_edit_with_an_empty_replacement_deletes_the_matched_text() {
        let root = project();
        let action = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "world\n", "newText": "", "expectedVersion": file_version(b"hello\nworld\n") }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(action).unwrap();
        assert_eq!(fs::read_to_string(root.join("src/note.txt")).unwrap(), "hello\n");

        let blanked = prepare(
            "edit_file",
            &json!({ "path": "src/note.txt", "oldText": "hello\n", "newText": "  ", "expectedVersion": file_version(b"hello\n") }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(blanked).unwrap();
        assert_eq!(fs::read_to_string(root.join("src/note.txt")).unwrap(), "  ");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unicode_and_crlf_bodies_survive_a_write_and_an_edit() {
        let root = project();
        let action = prepare(
            "write_file",
            &json!({ "path": "中文 名称.txt", "content": "第一行\r\n第二行 😀\r\n" }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(action).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("中文 名称.txt")).unwrap(),
            "第一行\r\n第二行 😀\r\n"
        );
        let edit = prepare(
            "edit_file",
            &json!({ "path": "中文 名称.txt", "oldText": "第二行 😀", "newText": "第二行", "expectedVersion": file_version("第一行\r\n第二行 😀\r\n".as_bytes()) }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(edit).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("中文 名称.txt")).unwrap(),
            "第一行\r\n第二行\r\n"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn malformed_write_and_edit_calls_stay_rejected_with_a_classification() {
        let root = project();
        for (tool, input) in [
            ("write_file", json!({ "path": "a.txt" })),
            ("write_file", json!({ "path": "a.txt", "content": Value::Null })),
            ("write_file", json!({ "path": "a.txt", "content": 7 })),
            ("write_file", json!({ "content": "" })),
            ("write_file", json!({ "path": "  ", "content": "" })),
            ("write_file", json!({ "path": "../escape.txt", "content": "" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "hello" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "hello", "newText": 1 })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "", "newText": "x" })),
            ("edit_file", json!({ "path": "src/note.txt", "oldText": "absent", "newText": "" })),
            ("run_command", json!({ "command": "" })),
            ("run_command", json!({ "command": "echo x", "cwd": 5 })),
        ] {
            let error = prepare(tool, &input, root.to_str().unwrap()).unwrap_err();
            let code = if input
                .get("path")
                .and_then(Value::as_str)
                .is_some_and(|value| value.contains(".."))
            {
                ToolErrorCode::PermissionDenied
            } else {
                ToolErrorCode::InvalidInput
            };
            assert_eq!(
                ToolErrorCode::classify(&error),
                Some(code),
                "{tool} {input} produced {error}"
            );
        }
        let huge = "x".repeat(MAX_FILE_BYTES as usize + 1);
        assert_eq!(
            ToolErrorCode::classify(
                &prepare("write_file", &json!({ "path": "big.txt", "content": huge }), root.to_str().unwrap())
                    .unwrap_err()
            ),
            Some(ToolErrorCode::InvalidInput)
        );
        let _ = fs::remove_dir_all(root);
    }

    // ------------------------------------------------------------------- A05

    #[test]
    fn the_result_names_the_shell_and_platform_that_actually_ran() {
        let root = project();
        let action = prepare(
            "run_command",
            &json!({ "command": "echo reported-shell", "timeoutSeconds": 10_000 }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let PreparedToolAction::RunCommand { command, cwd, timeout, .. } = action else { panic!("expected command"); };
        let result = execute_command(&command, &cwd, timeout, None).unwrap();
        let expected_shell = if cfg!(windows) { "cmd.exe /D /S /C" } else { "sh -lc" };
        let expected_platform = if cfg!(windows) { "windows" } else { "posix" };
        assert_eq!(result["details"]["shell"], expected_shell);
        assert_eq!(result["details"]["platform"], expected_platform);
        // The advertised clamp is the clamp that was applied.
        assert_eq!(result["details"]["timeoutMs"], 120_000);
        assert_eq!(result["details"]["outputEncoding"], "utf-8");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_command_runs_in_a_directory_with_chinese_and_spaces_and_propagates_its_code() {
        let root = std::env::temp_dir().join(format!("fox-项目 {}", Uuid::new_v4()));
        let nested = root.join("新建 文件夹");
        fs::create_dir_all(&nested).unwrap();
        let root_text = root.to_str().unwrap().to_owned();

        let action = prepare(
            "run_command",
            // The command writes into its own working directory, which is the
            // only way to prove the non-ASCII cwd actually reached the child.
            &json!({ "command": "echo ran-here > env-marker.txt" , "cwd": "新建 文件夹" }),
            &root_text,
        )
        .unwrap();
        let PreparedToolAction::RunCommand { command, cwd, timeout, .. } = action else { panic!("expected command"); };
        assert_eq!(execute_command(&command, &cwd, timeout, None).unwrap()["details"]["exitCode"], 0);
        let marker = nested.join("env-marker.txt");
        assert!(marker.exists(), "the cwd must resolve through a non-ASCII path");
        assert!(fs::read_to_string(&marker).unwrap().contains("ran-here"));

        #[cfg(windows)]
        let failing = "if defined COMSPEC (echo env-seen) else (exit 5)";
        #[cfg(not(windows))]
        let failing = "test -n \"$HOME\" && echo env-seen || exit 5";
        let action = prepare("run_command", &json!({ "command": failing }), &root_text).unwrap();
        let PreparedToolAction::RunCommand { command, cwd, timeout, .. } = action else { panic!("expected command"); };
        let result = execute_command(&command, &cwd, timeout, None).unwrap();
        // A success keeps the baseline result shape unchanged: no `isError` key
        // is invented for it, so existing consumers of the payload cannot
        // regress. Only a failed call gains `isError: true`.
        assert!(
            result.get("isError").is_none() || result["isError"] == Value::Bool(false),
            "success must not be reported as a failure: {result}"
        );
        assert!(command_text(&result).contains("env-seen"));
        assert_eq!(result["details"]["exitCode"], 0);
        assert!(result["details"].get("errorCode").is_none());

        let nonzero = "exit 7";
        let action = prepare("run_command", &json!({ "command": nonzero }), &root_text).unwrap();
        let PreparedToolAction::RunCommand { command, cwd, timeout, .. } = action else { panic!("expected command"); };
        let result = execute_command(&command, &cwd, timeout, None).unwrap();
        assert_eq!(result["details"]["exitCode"], 7);
        assert_eq!(result["details"]["errorCode"], "tool.nonzero_exit");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn utf8_child_bytes_are_kept_and_other_bytes_use_the_console_codepage() {
        let chinese = "编译失败：3 errors".as_bytes().to_vec();
        let (text, encoding) = decode_child_bytes(&chinese);
        assert_eq!(encoding, "utf-8");
        assert_eq!(text, "编译失败：3 errors");

        // A stream cut in the middle of a multi-byte character keeps the part
        // that is real and marks the boundary, instead of re-decoding the tail
        // as some other codepage.
        let truncated = "失败".as_bytes()[..5].to_vec();
        let (_, encoding) = decode_child_bytes(&truncated);
        assert_eq!(encoding, "utf-8+truncated");

        // "编译失败" in GBK/CP936: valid console bytes, invalid UTF-8. On a
        // Chinese Windows the executor must give the original characters back;
        // elsewhere it says which code page it applied rather than hiding it.
        let gbk: Vec<u8> = vec![0xB1, 0xE0, 0xD2, 0xEB, 0xCA, 0xA7, 0xB0, 0xDC];
        let (text, encoding) = decode_child_bytes(&gbk);
        if cfg!(windows) {
            let codepage = console_output_codepage();
            if codepage == 65001 {
                // These deliberately invalid UTF-8 bytes cannot round-trip in
                // a UTF-8 console; the result must disclose the lossy fallback.
                assert_eq!(encoding, "cp65001-lossy");
                assert!(text.contains('\u{fffd}'));
                return;
            }
            assert!(
                encoding == format!("cp{codepage}")
                    || encoding == format!("cp{codepage}-undecodable"),
                "the label must name the code page that was applied, got {encoding}"
            );
            if codepage == 936 && encoding == "cp936" {
                assert_eq!(text, "编译失败", "CP936 bytes must round-trip");
            }
        } else {
            assert_eq!(encoding, "non-utf8-lossy");
        }
    }
}

#[cfg(test)]
mod revision_regression_tests {
    use super::*;
    #[test]
    fn r4_stale_preparation_and_stale_read_cannot_overwrite_new_content() {
        let root = std::env::temp_dir().join(format!("fox-r4-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("file.txt");
        fs::write(&path, "old").unwrap();
        assert!(prepare("write_file", &json!({"path":"file.txt","content":"bad"}),root.to_str().unwrap()).unwrap_err().contains("tool.file_conflict"));
        for tool in ["write_file", "edit_file"] {
            fs::write(&path, "old").unwrap();
            let input=json!({"path":"file.txt","content":"replacement","oldText":"old","newText":"replacement","expectedVersion":file_version(b"old")});
            let action=prepare(tool,&input,root.to_str().unwrap()).unwrap();
            fs::write(&path,"other writer").unwrap();
            assert!(execute(action).unwrap_err().contains("tool.file_conflict"));
            assert_eq!(fs::read_to_string(&path).unwrap(),"other writer");
            assert!(prepare(tool,&input,root.to_str().unwrap()).is_err());
        }
        let first=prepare("write_file",&json!({"path":"new.txt","content":"first"}),root.to_str().unwrap()).unwrap();
        let second=prepare("write_file",&json!({"path":"new.txt","content":"second"}),root.to_str().unwrap()).unwrap();
        execute(first).unwrap();
        assert!(execute(second).unwrap_err().contains("tool.file_conflict"));
        assert_eq!(fs::read_to_string(root.join("new.txt")).unwrap(),"first");
        assert!(!fs::read_dir(&root).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with(".fox-write-")), "uncommitted candidates are cleaned; journal history is retained");
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(windows)]
    #[test]
    fn r4_failed_atomic_replace_preserves_original_and_cleans_staging() {
        use std::os::windows::fs::OpenOptionsExt;
        let root=std::env::temp_dir().join(format!("fox-r4-locked-{}",uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path=root.join("file.txt"); fs::write(&path,"original").unwrap();
        let action=prepare("write_file",&json!({"path":"file.txt","content":"new","expectedVersion":file_version(b"original")}),root.to_str().unwrap()).unwrap();
        let guard=fs::OpenOptions::new().read(true).share_mode(1).open(&path).unwrap();
        assert!(execute(action).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(),"original");
        let settled = fs::read_dir(&root).unwrap().flatten().find(|e| e.file_name().to_string_lossy().ends_with(".settled.json")).expect("durable refusal settlement");
        let record: Value = serde_json::from_slice(&fs::read(settled.path()).unwrap()).unwrap();
        assert_eq!(record["status"], "not_applied");
        drop(guard); fs::remove_dir_all(root).unwrap();
    }
}

/// A0: the ReplaceFileW failure-recovery guarantees. The tests come in three
/// kinds, and the difference matters for the evidence record:
/// * the classification tests are pure and deterministic;
/// * the settlement tests drive the real settlement code against an on-disk
///   layout that was arranged to look like a partial ReplaceFileW — that is
///   fault injection, not a real Windows failure;
/// * the `ReplaceFileW` tests call the real API on this machine.
#[cfg(test)]
mod a0_replace_recovery_tests {
    use super::*;

    fn fresh_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-a0-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    /// Host-owned artifacts left behind in the target's directory.
    fn leftovers(root: &Path) -> Vec<String> {
        fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".fox-"))
            .collect()
    }

    fn make_record(
        target: &Path,
        candidate: &Path,
        backup: &Path,
        base: &str,
        new: &str,
    ) -> ReplaceJournal {
        ReplaceJournal {
            operation_id: uuid::Uuid::new_v4().to_string(),
            status: "applying".to_string(),
            target: target.to_string_lossy().into_owned(),
            candidate: candidate.to_string_lossy().into_owned(),
            backup: backup.to_string_lossy().into_owned(),
            base_version: base.to_string(),
            new_version: new.to_string(),
            started_at_ms: 0,
            settled_at_ms: None,
            os_error: None,
            note: None,
        }
    }

    // ---- classification: deterministic, no Windows API involved ----

    fn versions() -> (String, String) {
        (file_version(b"original"), file_version(b"new content"))
    }

    #[test]
    fn not_applied_requires_an_untouched_original_no_backup_and_a_staged_candidate() {
        let (base, new) = versions();
        // What a sharing violation looks like afterwards: the original is
        // intact, nothing was renamed, the candidate is still staged.
        let layout = ReplaceLayout {
            target: Probe::Present(base.clone()),
            backup: Probe::Absent,
            candidate: Probe::Present(new.clone()),
        };
        assert_eq!(
            classify_replace_outcome(Some(32), &layout, &base, &new),
            ReplaceVerdict::NotApplied
        );
    }

    #[test]
    fn committed_requires_a_verified_backup_holding_the_original() {
        let (base, new) = versions();
        let layout = ReplaceLayout {
            target: Probe::Present(new.clone()),
            backup: Probe::Present(base.clone()),
            candidate: Probe::Absent,
        };
        assert_eq!(
            classify_replace_outcome(None, &layout, &base, &new),
            ReplaceVerdict::Committed
        );
    }

    #[test]
    fn committed_still_holds_when_the_new_content_equals_the_original() {
        // Writing identical bytes makes base and new the same hash; the
        // consumed candidate plus the verified backup prove the replace.
        let (base, _) = versions();
        let layout = ReplaceLayout {
            target: Probe::Present(base.clone()),
            backup: Probe::Present(base.clone()),
            candidate: Probe::Absent,
        };
        assert_eq!(
            classify_replace_outcome(None, &layout, &base, &base),
            ReplaceVerdict::Committed
        );
    }

    #[test]
    fn a_committed_target_without_a_verified_backup_is_not_committed() {
        // The target holds the new bytes, but the original is nowhere to be
        // verified: this is recovery-required, never a settled commit whose
        // materials could be cleaned.
        let (base, new) = versions();
        let layout = ReplaceLayout {
            target: Probe::Present(new.clone()),
            backup: Probe::Absent,
            candidate: Probe::Absent,
        };
        assert!(matches!(
            classify_replace_outcome(None, &layout, &base, &new),
            ReplaceVerdict::RecoveryRequired { .. }
        ));
    }

    #[test]
    fn replace_errors_1175_1176_1177_pin_the_materials() {
        let (base, new) = versions();
        for code in [1175u32, 1176, 1177] {
            // The shape reported for 1176 when no backup name was supplied: the
            // original is gone and only the candidate holds the new content.
            let layout = ReplaceLayout {
                target: Probe::Absent,
                backup: Probe::Absent,
                candidate: Probe::Present(new.clone()),
            };
            match classify_replace_outcome(Some(code), &layout, &base, &new) {
                ReplaceVerdict::RecoveryRequired { reason } => {
                    assert!(
                        reason.contains(&code.to_string()),
                        "the reason must name OS error {code}: {reason}"
                    );
                }
                other => panic!("OS error {code} must pin the materials, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_original_renamed_to_the_backup_is_recovery_required() {
        let (base, new) = versions();
        // The 1177 shape: the original was renamed to the backup before the
        // candidate could be moved.
        let layout = ReplaceLayout {
            target: Probe::Absent,
            backup: Probe::Present(base.clone()),
            candidate: Probe::Present(new.clone()),
        };
        assert!(matches!(
            classify_replace_outcome(Some(1177), &layout, &base, &new),
            ReplaceVerdict::RecoveryRequired { .. }
        ));
    }

    #[test]
    fn a_target_that_became_a_third_content_is_recovery_required() {
        let (base, new) = versions();
        let layout = ReplaceLayout {
            target: Probe::Present(file_version(b"someone else wrote this")),
            backup: Probe::Absent,
            candidate: Probe::Present(new.clone()),
        };
        assert!(matches!(
            classify_replace_outcome(Some(1175), &layout, &base, &new),
            ReplaceVerdict::RecoveryRequired { .. }
        ));
    }

    #[test]
    fn an_unreadable_path_is_unknown_not_absent_and_pins_the_materials() {
        // A probe error must never be flattened into "absent": reading a
        // directory fails, and that failure is an unknown state.
        let dir = fresh_dir();
        let (base, new) = versions();
        assert!(matches!(probe_version(&dir), Probe::Unknown(_)));
        assert!(matches!(probe_version(&dir.join("nope.txt")), Probe::Absent));
        let unknown_target = probe_version(&dir);
        let layout = ReplaceLayout {
            target: unknown_target,
            backup: Probe::Absent,
            candidate: Probe::Present(new.clone()),
        };
        assert!(matches!(
            classify_replace_outcome(Some(1177), &layout, &base, &new),
            ReplaceVerdict::RecoveryRequired { .. }
        ));
        let _ = fs::remove_dir_all(dir);
    }

    // ---- settlement against an arranged on-disk layout (fault injection) ----

    /// Simulates what `ReplaceFileW` has already done when it fails with 1177:
    /// the original is now at the backup name and the candidate is still staged.
    #[test]
    fn settle_pins_every_material_when_the_original_was_already_moved() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        let backup = root.join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        fs::write(&target, "original").unwrap();
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        // Inject the mid-state: the original moved to the backup. The applying
        // record was flushed before the API call, as in the real order.
        fs::rename(&target, &backup).unwrap();
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        let error = settle_replace(
            &target, &candidate, &backup, &journal, &mut record, Some(1177), JournalFault::NONE,
        )
        .unwrap_err();
        assert!(error.contains("1177"), "the failure text must keep the OS error: {error}");
        assert!(
            !error.contains("no effect"),
            "a moved original must not be reported as a no-effect failure: {error}"
        );
        assert!(candidate.exists(), "the candidate must not be deleted");
        assert!(backup.exists(), "the backup must not be deleted");
        assert!(journal.exists(), "the applying record must be retained");
        assert!(settled_record_path(&journal).exists(), "the settlement record is written");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
        assert_eq!(fs::read_to_string(&candidate).unwrap(), "new content");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn settle_cleans_the_candidate_when_the_original_is_verifiably_intact() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        let backup = root.join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        fs::write(&target, "original").unwrap();
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        // Nothing was moved: the target still holds the original. The applying
        // record was flushed before the API call, as in the real order.
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        let error = settle_replace(
            &target, &candidate, &backup, &journal, &mut record, Some(32), JournalFault::NONE,
        )
        .unwrap_err();
        assert!(error.contains("no effect"), "a no-effect failure must say so: {error}");
        assert!(error.contains("OS error 32"), "the OS error code must survive: {error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert!(!candidate.exists(), "a verified no-effect candidate is settled cleanup");
        assert!(journal.exists(), "not-applied preserves applying history");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn settle_commits_and_keeps_the_verified_backup_and_both_records() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        let backup = root.join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        let settled = settled_record_path(&journal);
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        // Injected commit: the candidate became the target and the original was
        // renamed into the verified backup. The applying record was flushed
        // before the API call, as in the real order.
        fs::rename(&candidate, &target).unwrap();
        fs::write(&backup, "original").unwrap();
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        settle_replace(
            &target, &candidate, &backup, &journal, &mut record, None, JournalFault::NONE,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
        // The verified backup and both records stay: without a durable
        // registration receipt they are the only recovery material.
        assert!(backup.exists(), "the verified backup is retained");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
        assert!(journal.exists(), "the applying record is retained");
        assert!(settled.exists(), "the settlement record is written beside it");
        let body = fs::read_to_string(&settled).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"] == "committed",
            "the settlement record must carry the committed status: {body}"
        );
        let _ = fs::remove_dir_all(root);
    }

    // ---- the replace on platforms without ReplaceFileW: the Host builds the
    // verified pre-image, then renames. Real files, real entry points. ----

    /// A successful replace keeps the verified pre-image and both records, so
    /// the original stays recoverable after the commit.
    #[test]
    fn a_preimage_replace_commits_and_keeps_the_verified_backup() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        let backup = root.join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        fs::write(&target, "original").unwrap();
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        replace_with_verified_preimage(
            &target, &candidate, &backup, &journal, &mut record, JournalFault::NONE,
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
        assert!(!candidate.exists(), "the candidate was consumed into the target");
        // The verified pre-image is what makes this a commit, not just a
        // rename: it is kept, and it still holds the original bytes.
        assert!(backup.exists(), "the verified pre-image is kept");
        assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
        let settled = settled_record_path(&journal);
        assert!(journal.exists(), "the applying record is kept");
        assert!(settled.exists(), "the settlement record is kept");
        let body = fs::read_to_string(&settled).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"] == "committed",
            "the settlement record must carry the committed status: {body}"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// When the pre-image cannot be staged, the replace is refused before any
    /// rename: the original is untouched and the staged candidate is cleaned.
    #[test]
    fn a_preimage_replace_is_refused_when_the_backup_cannot_be_staged() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        // The backup would live in a directory that does not exist, so the
        // pre-image cannot be staged at all.
        let backup = root.join("no-such-dir").join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        fs::write(&target, "original").unwrap();
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        let error = replace_with_verified_preimage(
            &target, &candidate, &backup, &journal, &mut record, JournalFault::NONE,
        )
        .unwrap_err();
        assert!(
            error.contains("cannot stage the pre-image backup"),
            "the refusal must name the missing storage condition: {error}"
        );
        assert!(error.contains("no effect"), "the refusal must be verified as no-effect: {error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        assert!(!candidate.exists(), "a refused replace cleans its staged candidate");
        assert!(journal.exists(), "refusal preserves applying history");
        assert!(!backup.exists(), "no backup is published for a refused replace");
        let _ = fs::remove_dir_all(root);
    }

    /// The pre-image is hashed before the rename, so an original that changed
    /// since the precondition check is detected and the replace is refused
    /// *before* its side effect. The conflict is reported, not forced into a
    /// commit, and nothing is cleaned behind it.
    #[test]
    fn a_preimage_replace_detects_a_changed_original_before_renaming() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        let candidate = root.join(".fox-write-candidate.tmp");
        let backup = root.join(".fox-replace-backup-injected");
        let journal = root.join(".fox-replace-journal-injected.json");
        fs::write(&target, "original").unwrap();
        fs::write(&candidate, "new content").unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        store_applying(&journal, &record).unwrap();
        // The target changed after the precondition was checked.
        fs::write(&target, "someone else wrote this").unwrap();
        let error = replace_with_verified_preimage(
            &target, &candidate, &backup, &journal, &mut record, JournalFault::NONE,
        )
        .unwrap_err();
        assert!(
            error.contains("does not match the pre-replace version"),
            "the mismatch must be reported: {error}"
        );
        // No rename happened: the candidate is still staged and the target was
        // not touched by this replace.
        assert!(candidate.exists(), "no rename may happen on a refused replace");
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "someone else wrote this",
            "the target is left as the concurrent writer left it"
        );
        assert!(!backup.exists(), "no backup is published for a refused replace");
        // The conflict is recovery-required, not a forced commit and not a
        // silent cleanup.
        assert!(
            error.contains("indeterminate state"),
            "a changed original must be reported as the conflict it is: {error}"
        );
        assert!(
            !error.contains("no effect"),
            "a changed original must not be reported as a clean no-effect: {error}"
        );
        let settled = settled_record_path(&journal);
        assert!(settled.exists(), "the settlement record pins the conflict");
        let body = fs::read_to_string(&settled).unwrap();
        assert!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["status"]
                == "recovery_required",
            "a changed original must be pinned, not committed: {body}"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// End-to-end on a non-Windows host: the real entry commits through the
    /// verified pre-image. Compiled and run only outside Windows; this Windows
    /// review cannot execute it (see the A0-R2 report's evidence boundary).
    #[cfg(not(windows))]
    #[test]
    fn a_posix_replace_commits_with_a_verified_preimage_through_the_real_entry() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        fs::write(&target, "original").unwrap();
        let candidate = root.join(".fox-write-candidate.tmp");
        fs::write(&candidate, "new content").unwrap();
        replace_existing_file(
            &candidate,
            &target,
            &file_version(b"original"),
            &file_version(b"new content"),
        )
        .unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "new content");
        assert!(!candidate.exists(), "the candidate was consumed");
        let mut backups = Vec::new();
        let mut journals = Vec::new();
        for name in leftovers(&root) {
            if name.starts_with(REPLACE_BACKUP_PREFIX) && !name.ends_with(".staging") {
                backups.push(name);
            } else if name.starts_with(REPLACE_JOURNAL_PREFIX) {
                journals.push(name);
            } else {
                panic!("unexpected Host artifact left behind: {name}");
            }
        }
        assert_eq!(backups.len(), 2, "verified pre-image and replacement backup: {backups:?}");
        assert_eq!(
            fs::read_to_string(root.join(&backups[0])).unwrap(),
            "original"
        );
        assert_eq!(journals.len(), 2, "the applying and settlement records: {journals:?}");
        let _ = fs::remove_dir_all(root);
    }

    // ---- journal fault injection: the applying record must survive a failed
    // settlement write, at every point it can fail ----

    #[test]
    fn a_failed_settlement_never_destroys_the_applying_record() {
        let (base, new) = versions();
        for fault in [
            JournalFault::FAIL_OPEN,
            JournalFault::FAIL_BODY,
            JournalFault::FAIL_FLUSH,
        ] {
            let root = fresh_dir();
            let target = root.join("note.txt");
            let candidate = root.join(".fox-write-candidate.tmp");
            let backup = root.join(".fox-replace-backup-injected");
            let journal = root.join(".fox-replace-journal-injected.json");
            // The settlement record is a separate file, never the applying one.
            assert_ne!(settled_record_path(&journal), journal);
            fs::write(&target, "original").unwrap();
            fs::write(&candidate, "new content").unwrap();
            // The applying record exists and is complete before settlement.
            let mut record = make_record(&target, &candidate, &backup, &base, &new);
            store_applying(&journal, &record).unwrap();
            // Inject the 1177 mid-state: the original moved to the backup and
            // the target is gone, leaving a recovery-required layout. The
            // settlement write then fails at the injected point.
            fs::rename(&target, &backup).unwrap();
            let error = settle_replace(
                &target, &candidate, &backup, &journal, &mut record, Some(1177), fault,
            )
            .unwrap_err();
            assert!(
                error.contains("1177"),
                "the recovery verdict must be reported: {error}"
            );
            assert!(
                error.contains("the settlement record could not be persisted")
                    || error.contains("applying record is still intact"),
                "a failed settlement write must be reported, not swallowed: {error}"
            );
            // The original applying record is still there, complete and intact.
            assert!(journal.exists(), "the applying record must survive a failed settlement");
            let applying = fs::read_to_string(&journal).unwrap();
            let parsed = serde_json::from_str::<serde_json::Value>(&applying)
                .expect("the applying record must still be complete, valid JSON");
            assert_eq!(parsed["status"], "applying", "the applying record is unchanged");
            // The recovery materials are all retained, untouched.
            assert_eq!(fs::read_to_string(&candidate).unwrap(), "new content");
            assert_eq!(fs::read_to_string(&backup).unwrap(), "original");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    /// The replace is refused when the target's directory cannot durably hold
    /// the applying record: no best-effort proceed, target untouched.
    #[test]
    fn a_replace_is_refused_when_the_record_cannot_be_stored() {
        let root = fresh_dir();
        // The target's "directory" is a file, so no record can be created in
        // it — a stand-in for any permission or storage condition that makes
        // the durable record impossible.
        let blocker = root.join("blocker");
        fs::write(&blocker, "not a directory").unwrap();
        let target = blocker.join("note.txt");
        let candidate = root.join("candidate.tmp");
        fs::write(&candidate, "new content").unwrap();
        let error = replace_existing_file(
            &candidate,
            &target,
            &file_version(b"original"),
            &file_version(b"new content"),
        )
        .unwrap_err();
        assert!(
            error.contains("pre-image backup"),
            "the refusal must identify the failed pre-image preparation: {error}"
        );
        // The API was never called: the target path was never reached, and the
        // candidate was cleaned up as pre-applying material.
        assert!(!target.exists(), "a refused replace never reaches the target");
        assert!(!candidate.exists(), "a refused replace cleans its pre-applying candidate");
        let _ = fs::remove_dir_all(root);
    }

    // ---- the real tool path ----

    #[test]
    fn an_edit_commits_and_keeps_the_verified_recovery_materials() {
        let root = fresh_dir();
        let target = root.join("note.txt");
        fs::write(&target, "hello\nworld\n").unwrap();
        let base = file_version(b"hello\nworld\n");
        let action = prepare(
            "edit_file",
            &json!({
                "path": "note.txt",
                "oldText": "world",
                "newText": "Fox",
                "expectedVersion": base.clone()
            }),
            root.to_str().unwrap(),
        )
        .unwrap();
        execute(action).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello\nFox\n");
        // The commit keeps its verified recovery material: exactly one backup
        // holding the original bytes, plus the applying and settlement records.
        let mut backups = Vec::new();
        let mut journals = Vec::new();
        for name in leftovers(&root) {
            if name.starts_with(REPLACE_BACKUP_PREFIX) {
                backups.push(name);
            } else if name.starts_with(REPLACE_JOURNAL_PREFIX) {
                journals.push(name);
            } else {
                panic!("unexpected Host artifact left behind: {name}");
            }
        }
        assert_eq!(backups.len(), 2, "verified pre-image and API backup: {backups:?}");
        let backup_path = root.join(&backups[0]);
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), "hello\nworld\n");
        assert_eq!(
            file_version(b"hello\nworld\n"),
            base,
            "the backup must hash to the pre-replace version"
        );
        assert_eq!(journals.len(), 2, "the applying and settlement records: {journals:?}");
        let _ = fs::remove_dir_all(root);
    }

    /// A replace that fails after the original was moved must not take the
    /// candidate with it, and the original must stay recoverable. This calls
    /// the real `ReplaceFileW`; the injected part is only the held handle.
    #[cfg(windows)]
    /// A replace that fails on the real `ReplaceFileW` must keep the original
    /// recoverable and carry the OS error. Depending on where the call gives up
    /// it either renames the original to the backup first (the 1176/1177 shape,
    /// in which case the candidate must survive too) or refuses up front (a
    /// verified no-effect, whose redundant candidate is settled cleanup). The
    /// injected part is only the held handle.
    #[cfg(windows)]
    #[test]
    fn failed_replace_keeps_the_original_and_the_new_content_recoverable() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = fresh_dir();
        let target = root.join("note.txt");
        fs::write(&target, "original").unwrap();
        let candidate = root.join(".fox-write-candidate.tmp");
        fs::write(&candidate, "new content").unwrap();
        let (backup, journal, _id) = replace_material_paths(&target).unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        // Hold the candidate without delete sharing, so `ReplaceFileW` cannot
        // move it. Depending on where the call gives up it either renames the
        // original to the backup first (the 1176/1177 shape) or refuses up
        // front; both must leave every material recoverable.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // FILE_SHARE_READ only: deny delete sharing
            .open(&candidate)
            .unwrap();
        let error = replace_with_backup(
            &target, &candidate, &backup, &journal, &mut record, JournalFault::NONE,
        )
        .unwrap_err();
        assert!(
            error.contains("OS error"),
            "the failure text must carry the real OS error: {error}"
        );
        let original_in_target = fs::read(&target).ok().as_deref() == Some(b"original");
        let original_in_backup = fs::read(&backup).ok().as_deref() == Some(b"original");
        assert!(
            original_in_target || original_in_backup,
            "the original bytes must survive a failed replace"
        );
        if original_in_backup {
            // The partial-progress case this batch exists for: the original was
            // moved out of the target path, so the candidate is the only copy
            // of the new content there. It must not be cleaned up.
            assert!(
                candidate.exists(),
                "a moved original must not take the candidate with it"
            );
            assert!(journal.exists(), "the applying record must be pinned for reconciliation");
            assert!(
                !error.contains("no effect"),
                "a partial replace must not be reported as a no-effect failure: {error}"
            );
        }
        drop(held);
        let _ = fs::remove_dir_all(root);
    }

    /// A replace refused before anything is renamed is reported as a verified
    /// no-effect failure and leaves nothing behind. Real `ReplaceFileW`.
    #[cfg(windows)]
    #[test]
    fn failed_replace_on_a_locked_target_is_a_verified_no_effect_failure() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = fresh_dir();
        let target = root.join("note.txt");
        fs::write(&target, "original").unwrap();
        // Hold the target without delete sharing: the replace is refused before
        // anything is renamed.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&target)
            .unwrap();
        let action = prepare(
            "write_file",
            &json!({
                "path": "note.txt",
                "content": "new",
                "expectedVersion": file_version(b"original")
            }),
            root.to_str().unwrap(),
        )
        .unwrap();
        let error = execute(action).unwrap_err();
        assert!(
            error.contains("no effect"),
            "a refusal before any rename must be reported as a verified no-effect failure: {error}"
        );
        assert!(error.contains("OS error"), "the real OS error code must be preserved: {error}");
        assert_eq!(fs::read_to_string(&target).unwrap(), "original");
        let materials = leftovers(&root);
        assert!(materials.iter().any(|p| p.ends_with(".settled.json")), "refusal history must survive");
        assert!(!materials.iter().any(|p| p.starts_with(".fox-write-")), "candidate must be cleaned");
        drop(held);
        let _ = fs::remove_dir_all(root);
    }

    /// Diagnostic probe, not part of the default suite: records the real error
    /// code `ReplaceFileW` returns on this machine when the candidate cannot be
    /// moved, and the layout that follows. Run with `--nocapture` to capture
    /// the numbers for the evidence record.
    #[cfg(windows)]
    #[test]
    #[ignore = "diagnostic probe: run with --nocapture to record the real ReplaceFileW failure shape"]
    fn a0_probe_records_the_real_replace_failure_shape() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = fresh_dir();
        let target = root.join("note.txt");
        fs::write(&target, "original").unwrap();
        let candidate = root.join(".fox-write-candidate.tmp");
        fs::write(&candidate, "new content").unwrap();
        let (backup, journal, _id) = replace_material_paths(&target).unwrap();
        let base = file_version(b"original");
        let new = file_version(b"new content");
        let mut record = make_record(&target, &candidate, &backup, &base, &new);
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&candidate)
            .unwrap();
        let result = replace_with_backup(
            &target, &candidate, &backup, &journal, &mut record, JournalFault::NONE,
        );
        let layout = inspect_layout(&target, &candidate, &backup);
        eprintln!(
            "A0_PROBE replace_with_backup -> {result:?}; os_error={os_error:?}; layout={layout:?}; \
             record_status={status:?}",
            os_error = record.os_error,
            status = record.status
        );
        let original_recoverable =
            fs::read(&target).ok().as_deref() == Some(b"original")
                || fs::read(&backup).ok().as_deref() == Some(b"original");
        assert!(original_recoverable, "the original must stay recoverable");
        if fs::read(&backup).ok().as_deref() == Some(b"original") {
            assert_eq!(fs::read(&candidate).unwrap(), b"new content");
        }
        drop(held);
        let _ = fs::remove_dir_all(root);
    }
}

#[cfg(test)]
mod admitted_file_tests {
    use super::*;
    struct Hooks {
        baseline: String, validations: usize, reject_at: Option<usize>,
        fail_registration: bool, committed: bool, captured: bool,
    }
    impl FileCommitContext for Hooks {
        fn baseline(&self) -> &str { &self.baseline }
        fn dispatch_id(&self) -> &str { "tool-dispatch:1:r:write" }
        fn validate(&mut self, _: &Path) -> Result<(), String> {
            self.validations += 1;
            if self.reject_at == Some(self.validations) { Err("credential_mismatch".into()) } else { Ok(()) }
        }
        fn capture_before(&mut self, _: &Path) -> Result<(), String> { self.captured = true; Ok(()) }
        fn mark_committed(&mut self) { self.committed = true; }
        fn record_committed(&mut self, target: &Path) -> Result<(), String> {
            assert!(self.captured);
            assert!(self.committed);
            assert!(FILE_WRITE_LOCK.try_lock().is_err(), "registration must remain inside file lock");
            assert_eq!(fs::read(target).unwrap(), b"new");
            if self.fail_registration { Err("injected registry failure".into()) } else { Ok(()) }
        }
    }
    fn fixture() -> (PathBuf, Hooks) {
        let root = std::env::temp_dir().join(format!("fox-admission-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"old").unwrap();
        (root, Hooks { baseline: file_version(b"old"), validations: 0, reject_at: None,
            fail_registration: false, committed: false, captured: false })
    }
    fn action(root: &Path, hooks: &Hooks) -> PreparedToolAction {
        prepare_admitted_file("write_file", &json!({"path":"a.txt", "content":"new",
            "expectedVersion":"model-forged-version"}), root.to_str().unwrap(), &hooks.baseline).unwrap()
    }
    #[test]
    fn final_gate_refuses_after_verified_preimage_before_any_replace() {
        let (root, mut hooks) = fixture();
        hooks.reject_at = Some(3); // outer preflight, lock entry, then final commit gate
        let result = execute_file_with_context(action(&root, &hooks), None, &mut hooks);
        assert!(result.unwrap_err().contains("credential_mismatch"));
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"old");
        assert!(!hooks.committed);
        let preimages: Vec<_> = fs::read_dir(&root).unwrap().flatten()
            .filter(|entry| entry.path().extension().is_some_and(|x| x == "verified")).collect();
        assert_eq!(preimages.len(), 1);
        assert_eq!(fs::read(preimages[0].path()).unwrap(), b"old");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn authority_ignores_model_baseline_and_registration_failure_is_visible() {
        let (root, mut hooks) = fixture();
        hooks.fail_registration = true;
        let error = execute_file_with_context(action(&root, &hooks), None, &mut hooks).unwrap_err();
        assert!(error.contains("file committed but version registration failed"));
        assert!(hooks.committed);
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"new");
        assert!(fs::read_dir(&root).unwrap().flatten().any(|entry|
            entry.file_name().to_string_lossy().ends_with(".verified")));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stale_authoritative_baseline_refuses_before_capture() {
        let (root, mut hooks) = fixture();
        let prepared = action(&root, &hooks);
        fs::write(root.join("a.txt"), b"external").unwrap();
        assert!(execute_file_with_context(prepared, None, &mut hooks).is_err());
        assert!(!hooks.captured);
        assert_eq!(fs::read(root.join("a.txt")).unwrap(), b"external");
        fs::remove_dir_all(root).unwrap();
    }
}

/// Regression, 2026-10-01 capability run: the write gate must create new files
/// without a read-first dance, keep refusing blind overwrites, and honour the
/// task's own read-only inputs.
#[cfg(test)]
mod write_gate_tests {
    use super::*;

    fn fresh_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("fox-write-gate-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_missing_target_is_a_new_file_not_a_conflict() {
        let root = fresh_dir();
        let target = root.join("out").join("summary.json");
        // The parent directory does not exist either: creation is allowed, and
        // the write itself creates it inside the project root.
        assert_eq!(write_baseline(&target, None, None).unwrap(), "missing");
        // An observation always wins, so a read-backed write keeps its claim.
        assert_eq!(
            write_baseline(&target, Some("sha256:x"), Some("sha256:x")).unwrap(),
            "sha256:x"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_existing_target_still_requires_a_read_and_fails_closed_otherwise() {
        let root = fresh_dir();
        let target = root.join("report.docx");
        fs::write(&target, b"existing").unwrap();
        let blind = write_baseline(&target, None, None).unwrap_err();
        assert!(blind.contains("tool.file_conflict"), "{blind}");
        let declared = write_baseline(&target, Some("sha256:stale"), None).unwrap_err();
        assert!(declared.contains("sha256:stale"), "{declared}");
        // A directory is a malformed target, never a new file.
        let directory = root.join("folder");
        fs::create_dir_all(&directory).unwrap();
        let error = write_baseline(&directory, None, None).unwrap_err();
        assert!(error.contains("tool.invalid_input"), "{error}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_target_created_after_admission_is_not_overwritten() {
        let root = fresh_dir();
        let action = prepare(
            "write_file",
            &json!({"path": "out/new.csv", "content": "mine"}),
            root.to_str().unwrap(),
        )
        .unwrap();
        // Another process creates the same path between admission and commit.
        fs::create_dir_all(root.join("out")).unwrap();
        fs::write(root.join("out").join("new.csv"), b"theirs").unwrap();
        let error = execute(action).unwrap_err();
        assert!(error.contains("tool.file_conflict"), "{error}");
        assert_eq!(
            fs::read_to_string(root.join("out").join("new.csv")).unwrap(),
            "theirs"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn task_inputs_are_read_only_while_outputs_and_unmentioned_paths_stay_writable() {
        let root = fresh_dir();
        fs::create_dir_all(root.join("in")).unwrap();
        let task = "读取 policy.pdf、sales.csv，清洗 in/messy_sales.csv，生成 out/summary.json。";
        for blocked in ["policy.pdf", "sales.csv", "in/messy_sales.csv"] {
            let error = ensure_writable_target(
                Some(task),
                root.to_str().unwrap(),
                &root.join(blocked.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )
            .unwrap_err();
            assert!(error.contains("tool.read_only_input"), "{blocked}: {error}");
        }
        for allowed in ["out/summary.json", "notes.txt"] {
            ensure_writable_target(
                Some(task),
                root.to_str().unwrap(),
                &root.join(allowed.replace('/', std::path::MAIN_SEPARATOR_STR)),
            )
            .expect("a declared output or an unmentioned new file is writable");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_explicit_save_keeps_its_path_writable() {
        let root = fresh_dir();
        let task = "读取 台账.xlsx，更新内容后保存 台账.xlsx。";
        ensure_writable_target(Some(task), root.to_str().unwrap(), &root.join("台账.xlsx"))
            .expect("an explicit in-place save is authorized by the task");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_task_without_file_mentions_leaves_every_path_writable() {
        let root = fresh_dir();
        ensure_writable_target(Some("你好，帮我想个名字。"), root.to_str().unwrap(), &root.join("a.txt"))
            .expect("no mentions means no read-only set");
        ensure_writable_target(None, root.to_str().unwrap(), &root.join("a.txt"))
            .expect("no task text means no restriction");
        fs::remove_dir_all(root).unwrap();
    }
}
