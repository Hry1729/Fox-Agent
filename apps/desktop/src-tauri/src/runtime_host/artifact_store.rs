//! Host-owned placement, classification and commit rules for user files.
//!
//! Rationale: before this module every transient artefact of an Office write
//! (`.fox-office-<uuid>.<ext>` working copies, `<file>.fox-backup-<uuid>` undo
//! copies, rendered HTML previews) was created *next to the target document*,
//! i.e. inside the user's project folder. That made the project unreadable,
//! and the briefly locked files killed Vite's file watcher with EBUSY.
//!
//! Responsibilities split three ways:
//! * **Placement** — where a new artifact belongs (project deliverable folder,
//!   Host-private preview folder, Host-private work/staging area).
//! * **Classification** — which lifecycle bucket a *Host-verified* artifact
//!   belongs to, so the UI never has to guess from a file extension.
//! * **Commit** — how a prepared payload becomes the target file without ever
//!   describing a plain `fs::copy` over an existing file as "atomic".
//!
//! Everything here is a Host fact: the private roots come from the Host's own
//! runtime state, never from tool input, so a model cannot point the work or
//! preview area at arbitrary application data.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Project-relative folder that holds conversation deliverables.
pub const PROJECT_DELIVERABLE_DIR: &str = "fox";
/// Upper bound for either side of a safe directory prefix. Long enough for a
/// readable Chinese topic name, far below the Windows path budget.
const MAX_PREFIX_CHARS: usize = 24;
/// Host-owned artifacts root, relative to the application data directory.
const ARTIFACTS_DIR: &str = "artifacts";

/// Lifecycle bucket of one artifact. Stored on the `artifacts` row so the
/// "deliverable vs process file" decision comes from a Host-verified fact
/// instead of a filename extension guess.
///
/// The classification is deliberately **independent of the extension**: a CSV
/// produced by `attachment_compute` is an intermediate process file, while a
/// CSV that the user asked to be delivered and that the Host placed in the
/// conversation deliverable folder is a deliverable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactClass {
    /// A final result the user receives (`交付物`).
    Deliverable,
    /// A rendered preview of a result, reached through the result's own
    /// preview entry rather than listed as a separate deliverable.
    Preview,
    /// Intermediate data or a transient working copy (`过程文件`).
    Process,
}

impl ArtifactClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ArtifactClass::Deliverable => "deliverable",
            ArtifactClass::Preview => "preview",
            ArtifactClass::Process => "process",
        }
    }

    pub(crate) fn parse(value: &str) -> ArtifactClass {
        match value {
            "deliverable" => ArtifactClass::Deliverable,
            "preview" => ArtifactClass::Preview,
            _ => ArtifactClass::Process,
        }
    }
}

/// Whether the bytes live in the user's project or in a Host-private area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ArtifactOrigin {
    /// Inside the conversation's authorized project folder.
    Project,
    /// Inside an application-private area (compute, preview, work, versions).
    HostPrivate,
}

impl ArtifactOrigin {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ArtifactOrigin::Project => "project",
            ArtifactOrigin::HostPrivate => "host_private",
        }
    }
}

/// The Host's private artifact roots for one application data directory.
#[derive(Debug, Clone)]
pub(crate) struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    /// Root of every Host-private artifact area. Created lazily by the
    /// individual accessors so a read-only conversation never creates folders.
    pub(crate) fn new(data_dir: &Path) -> ArtifactStore {
        ArtifactStore {
            root: data_dir.join(ARTIFACTS_DIR),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Deterministic deliverable-folder **name** for one conversation.
    ///
    /// `slug` is a readable, already-sanitized topic token (empty is allowed).
    /// The name is `<slug>-<yyyyMMdd>-<12 hex>`; the hex suffix is derived from
    /// the conversation id, so two conversations can never collide even with
    /// the same topic and day, and the model can never redirect the folder to
    /// another conversation. The date is the day the conversation first asked
    /// for a deliverable, and re-running the same task therefore reuses the
    /// folder instead of creating one per Run.
    pub(crate) fn deliverable_directory_name(&self, slug: &str, conversation_id: &str) -> String {
        let prefix = sanitize_prefix(slug);
        let date = today_utc();
        let suffix = &short_conversation_id(conversation_id)[..12];
        if prefix.is_empty() {
            format!("{date}-{suffix}")
        } else {
            format!("{prefix}-{date}-{suffix}")
        }
    }

    /// Project-relative deliverable folder path (`fox/<name>`).
    pub(crate) fn deliverable_relative_path(&self, slug: &str, conversation_id: &str) -> PathBuf {
        Path::new(PROJECT_DELIVERABLE_DIR)
            .join(self.deliverable_directory_name(slug, conversation_id))
    }

    /// Create the deliverable folder inside an already canonicalized project
    /// root and return the absolute path. Idempotent: an existing folder for
    /// this conversation is reused, so continuation and retry stay in one
    /// user-visible result folder.
    pub(crate) fn ensure_deliverable_root(
        &self,
        project_root: &Path,
        slug: &str,
        conversation_id: &str,
    ) -> Result<PathBuf, String> {
        let fox_dir = project_root.join(PROJECT_DELIVERABLE_DIR);
        create_dir_checked(&fox_dir)?;
        // Reuse the conversation's existing folder when one is already there,
        // regardless of the date, so a task spanning midnight keeps one folder.
        let suffix = format!("-{}", &short_conversation_id(conversation_id)[..12]);
        if let Ok(entries) = fs::read_dir(&fox_dir) {
            let mut reused: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(&suffix))
                })
                .collect();
            reused.sort();
            if let Some(existing) = reused.into_iter().next() {
                return Ok(existing);
            }
        }
        let directory = fox_dir.join(self.deliverable_directory_name(slug, conversation_id));
        create_dir_checked(&directory)?;
        Ok(directory)
    }

    /// Host-private area for rendered previews of one conversation.
    pub(crate) fn preview_root(&self, conversation_id: &str) -> Result<PathBuf, String> {
        let root = self
            .root
            .join("previews")
            .join(short_conversation_id(conversation_id));
        create_dir_checked(&root)?;
        Ok(root)
    }

    /// Absolute Host-private path for one rendered preview. Deterministic per
    /// (conversation, source document, mode) so a re-render replaces the stale
    /// preview in place and no dangling link survives.
    pub(crate) fn preview_path(
        &self,
        conversation_id: &str,
        source: &Path,
        extension: &str,
        fingerprint: &str,
    ) -> Result<PathBuf, String> {
        let root = self.preview_root(conversation_id)?;
        let stem = source
            .file_stem()
            .and_then(|value| value.to_str())
            .map(sanitize_file_stem)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "preview".to_owned());
        let mut hasher = Sha256::new();
        hasher.update(source.to_string_lossy().as_bytes());
        hasher.update(b"|");
        hasher.update(fingerprint.as_bytes());
        let digest = hex::encode(hasher.finalize());
        Ok(root.join(format!("{stem}-{}.{extension}", &digest[..16])))
    }

    /// Fresh Host-private working directory for long-running document edits and
    /// render passes. Outside the project, so nothing is ever watched, indexed
    /// or left behind in the user's folder.
    pub(crate) fn work_directory(&self, conversation_id: &str) -> Result<PathBuf, String> {
        let root = self
            .root
            .join("work")
            .join(short_conversation_id(conversation_id))
            .join(uuid::Uuid::new_v4().to_string());
        create_dir_checked(&root)?;
        Ok(root)
    }

    /// Controlled staging directory on the **target volume**.
    ///
    /// A build's working copy lives on the application data volume, which is
    /// often a different volume than the project. A cross-volume `rename`
    /// cannot work, so the verified payload is first copied to this short-lived
    /// staging file next to the destination and only then swapped in. The
    /// staging name is derived from the relative target so all staging files
    /// for one folder sit in one place and are removed by [`commit`] or by
    /// [`sweep_staging`].
    pub(crate) fn staging_path(
        root: &Path,
        relative: &Path,
        token: &str,
    ) -> Result<PathBuf, String> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::RootDir))
        {
            return Err("staging target must be a relative in-project path".into());
        }
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        if parent.as_os_str().is_empty() {
            return Ok(root.join(format!(".fox-stage-{token}.tmp")));
        }
        Ok(root.join(parent).join(format!(".fox-stage-{token}.tmp")))
    }
}

/// Create a directory (and parents) and refuse a reparse point on the final
/// component, mirroring the compute-workspace rule.
fn create_dir_checked(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("无法创建目录 {}: {error}", path.display()))?;
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(format!("{} 不是普通目录", path.display()));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!("{} 不能是重解析点", path.display()));
        }
    }
    Ok(())
}

pub(crate) fn short_conversation_id(conversation_id: &str) -> String {
    hex::encode(Sha256::digest(conversation_id.as_bytes()))
}

/// Reduce an arbitrary topic string to a filesystem-safe, readable prefix.
///
/// Only letters/digits (any script) survive; everything else collapses to a
/// single dash. Reserved Windows device names are neutralized, and the result
/// is capped so one folder name cannot blow the path budget.
pub(crate) fn sanitize_prefix(raw: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(ch);
            if out.chars().count() >= MAX_PREFIX_CHARS {
                break;
            }
        } else {
            pending_dash = true;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if is_reserved_name(&out) {
        return format!("{out}-dir");
    }
    out
}

fn sanitize_file_stem(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_alphanumeric() || matches!(ch, '-' | '_') {
            out.push(ch);
        } else {
            out.push('-');
        }
        if out.chars().count() >= MAX_PREFIX_CHARS {
            break;
        }
    }
    let out = out.trim_matches('-').to_owned();
    if is_reserved_name(&out) {
        format!("{out}-file")
    } else {
        out
    }
}

fn is_reserved_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

/// `yyyyMMdd` from the current UTC date without a chrono dependency.
fn today_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0);
    // Deterministic clock seam for tests: a "cross-midnight continuation" must
    // be exercised by moving the clock, not by sleeping until tomorrow.
    #[cfg(test)]
    let seconds = seconds + DAY_OFFSET_DAYS.load(std::sync::atomic::Ordering::SeqCst) * 86_400;
    let days = seconds.div_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}{month:02}{day:02}")
}

/// Test-only day offset applied by [`today_utc`].
#[cfg(test)]
static DAY_OFFSET_DAYS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Move the process clock used for deliverable folder naming.
///
/// Tests only: production always reads the real clock.
#[cfg(test)]
pub(crate) fn set_test_day_offset(days: i64) {
    DAY_OFFSET_DAYS.store(days, std::sync::atomic::Ordering::SeqCst);
}

/// Whether a folder name read back from the placement ledger is still a valid
/// single-component deliverable folder name.
///
/// Re-validated on every read rather than trusted. The ledger is durable state
/// that a later version (or a hand-edited database) could hold a stale or
/// hostile value in, and a name containing a separator or `..` would redirect a
/// write out of `fox/`.
pub(crate) fn is_valid_deliverable_folder_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
        && !name.ends_with(['.', ' '])
        && !is_reserved_name(name)
}

/// Stable ledger key for one authorized project root.
///
/// Windows paths are case-insensitive, so the key is folded on Windows; the
/// trailing separator is dropped so `C:\p` and `C:\p\` are one project. A
/// different project therefore never inherits another project's folder.
pub(crate) fn project_key(root: &str) -> String {
    let trimmed = root.trim().trim_end_matches(['/', '\\']);
    let text = trimmed
        .strip_prefix(r"\\?\")
        .or_else(|| trimmed.strip_prefix(r"\\.\"))
        .unwrap_or(trimmed);
    #[cfg(windows)]
    {
        text.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        text.to_owned()
    }
}

/// Howard Hinnant's days-from-civil algorithm, inverted.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

/// Where a committed payload ended up and whether it replaced existing bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitOutcome {
    pub target: PathBuf,
    pub replaced: bool,
}

/// Move a fully prepared payload onto its target path.
///
/// This is the **single** commit primitive for Host-managed file writes, so
/// "how the bytes land" is not re-implemented per tool. Contract:
///
/// * `prepared` is the Host's private working copy, which normally lives on the
///   application data volume. It is deliberately treated as read-only input:
///   the bytes are re-hashed at the target after the swap, so a truncated copy
///   fails loudly instead of being reported as success.
/// * The payload is **always** copied to a controlled staging file on the
///   target's own volume first (see [`ArtifactStore::staging_path`]), then
///   swapped in. Two cases make this mandatory rather than an optimisation:
///   a cross-volume `rename` from the application data directory cannot work
///   (Windows `ERROR_NOT_SAME_DEVICE`), and for an existing target the final
///   placement must be a same-volume [`platform_replace`] rather than a
///   `fs::copy` straight over the live file.
/// * A brand-new target still goes through staging: `MOVEFILE_REPLACE_EXISTING`
///   on a not-yet-existing path is a plain rename, so the same code path covers
///   both cases without assuming the caller's volume layout.
/// * The staging file is **registered in the ownership ledger** before it is
///   created and released on every outcome, including failure, so a crash
///   leaves an attributable record rather than anonymous litter. A staging name
///   that already exists is refused, never overwritten: it is either another
///   execution's file or a user's, and neither may be replaced on a guess.
pub(crate) fn commit(
    prepared: &Path,
    target: &Path,
    staging_root: &Path,
    relative: &Path,
    token: &str,
    ledger: Option<&StagingLedger>,
) -> Result<CommitOutcome, String> {
    let metadata = fs::metadata(prepared)
        .map_err(|error| format!("待提交的工作副本不可读: {error}"))?;
    if !metadata.is_file() {
        return Err("待提交的工作副本不是普通文件".into());
    }
    let expected = hash_and_size(prepared)?;
    if let Some(parent) = target.parent() {
        create_dir_checked(parent)?;
    }
    let replaced = target.exists();
    let staging = ArtifactStore::staging_path(staging_root, relative, token)?;
    if let Some(parent) = staging.parent() {
        // Refuses a symlink or reparse point on the staging directory, so a
        // linked target folder cannot redirect the write out of the project.
        create_dir_checked(parent)?;
    }
    if staging.exists() {
        return Err(format!(
            "暂存路径已被占用，拒绝覆盖陌生文件: {}",
            staging.display()
        ));
    }
    if let Some(ledger) = ledger {
        ledger.register(token, &staging, target)?;
    }
    // Copy the verified bytes onto the target volume first. The live file is
    // untouched while this copy runs, so an interrupted copy leaves the
    // original intact.
    if let Err(error) = fs::copy(prepared, &staging) {
        let _ = fs::remove_file(&staging);
        if let Some(ledger) = ledger {
            ledger.release(token);
        }
        return Err(format!("无法在目标卷创建暂存文件: {error}"));
    }
    if let Err(error) = verify_target(&staging, &expected) {
        let _ = fs::remove_file(&staging);
        if let Some(ledger) = ledger {
            ledger.release(token);
        }
        return Err(error);
    }
    if let Some(ledger) = ledger {
        // Optional bookkeeping: a failure here must not fail an authorized
        // write, it only costs the post-mortem detail.
        let _ = ledger.mark_ready(token, &staging, target, &expected.0);
    }
    match platform_replace(&staging, target) {
        Ok(()) => {
            // The staging file has been consumed. Release the ledger entry on
            // both outcomes: a verification failure must not leave the entry
            // behind, even though a later reclaim could retire it.
            let verified = verify_target(target, &expected);
            if let Some(ledger) = ledger {
                ledger.release(token);
            }
            verified?;
            Ok(CommitOutcome {
                target: target.to_path_buf(),
                replaced,
            })
        }
        Err(error) => {
            let _ = fs::remove_file(&staging);
            if let Some(ledger) = ledger {
                ledger.release(token);
            }
            Err(error)
        }
    }
}

/// Replace `target` with `staging` as one platform-supported operation.
///
/// Windows: `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`
/// performs an in-place metadata swap on the same volume, so a reader never
/// observes a half-written document. Other platforms use `rename`, which has
/// the same guarantee within a filesystem.
fn platform_replace(staging: &Path, target: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Foundation::GetLastError;
        use windows::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from: Vec<u16> = staging
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let to: Vec<u16> = target
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        return unsafe {
            MoveFileExW(
                PCWSTR(from.as_ptr()),
                PCWSTR(to.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|_| {
            format!(
                "无法替换目标文件 {}：Windows 错误 {:?}",
                target.display(),
                unsafe { GetLastError() }
            )
        });
    }
    #[cfg(not(windows))]
    {
        fs::rename(staging, target)
            .map_err(|error| format!("无法替换目标文件 {}: {error}", target.display()))
    }
}

fn hash_and_size(path: &Path) -> Result<(String, u64), String> {
    let bytes = fs::read(path).map_err(|error| format!("无法读取文件: {error}"))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok((hex::encode(hasher.finalize()), bytes.len() as u64))
}

fn verify_target(path: &Path, expected: &(String, u64)) -> Result<(), String> {
    let actual = hash_and_size(path)?;
    if &actual != expected {
        return Err(format!(
            "提交后的文件与工作副本不一致（期望 {} 字节，实际 {} 字节）",
            expected.1, actual.1
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Staging ownership ledger
// ---------------------------------------------------------------------------
//
// A staging file is the only Host-managed file that ever appears *inside* the
// user's project, so it is also the only one that has to be reclaimed after a
// crash. Reclaiming it can never be decided from the file name: the name prefix
// proves nothing about who created the file, an mtime says nothing about
// whether a commit is still in flight, a missing PID is not a portable fact,
// and `EXECUTION_LOCK` is an in-process mutex that says nothing about a second
// Fox process working in the same project.
//
// Instead every staging file is *registered* before it is created, and the
// registration carries a cross-process ownership lease: an OS handle opened
// with sharing denied. Another Fox process can therefore prove, without
// guessing, whether the owner is still alive.

/// `artifacts/staging` — the ledger root.
const STAGING_LEDGER_DIR: &str = "staging";
const STAGING_ENTRY_DIR: &str = "entries";
const STAGING_LEASE_DIR: &str = "leases";

/// Token of a Host staging file name, or `None` when the name is not one.
///
/// This is a *shape* check and nothing more. A file whose name happens to match
/// but which has no ledger entry was not created by this Host and is left
/// strictly alone.
pub(crate) fn staging_token_of_name(name: &str) -> Option<&str> {
    let token = name.strip_prefix(".fox-stage-")?.strip_suffix(".tmp")?;
    if token.is_empty()
        || token.len() > 64
        || !token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        return None;
    }
    Some(token)
}

/// One Host-registered staging file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StagingRecord {
    pub token: String,
    /// Absolute path of the staging file itself.
    pub staging_path: String,
    /// Absolute path of the final target this commit is producing.
    pub target_path: String,
    pub conversation_id: String,
    pub execution_id: String,
    /// `staging` while the payload is being copied onto the target volume,
    /// `ready` once the staged bytes are verified and only the swap is left.
    /// Kept so a diagnostic can report how far an interrupted attempt got.
    pub state: String,
    /// SHA-256 of the payload as staged, for post-mortem diagnostics.
    pub payload_sha256: String,
    pub created_at_ms: i64,
}

/// What one reclaim pass did. Every entry that was *not* removed carries the
/// reason, so a leftover that could not be proven abandoned produces an
/// actionable diagnostic instead of silence.
#[derive(Debug, Default)]
pub(crate) struct ReclaimReport {
    /// Staging files proven abandoned and removed.
    pub removed: Vec<PathBuf>,
    /// Ledger entries dropped because their staging file is already gone (the
    /// swap completed and only the bookkeeping survived the crash).
    pub retired: Vec<PathBuf>,
    /// Entries deliberately left in place, with the reason.
    pub kept: Vec<(PathBuf, String)>,
    /// Ledger entries that could not be read; left in place, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
}

impl ReclaimReport {
    /// One line per leftover that was kept, for the Host log.
    pub(crate) fn diagnostics(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .kept
            .iter()
            .map(|(path, reason)| format!("保留暂存残留 {}：{reason}", path.display()))
            .collect();
        out.extend(
            self.unreadable
                .iter()
                .map(|(path, reason)| format!("暂存台账条目不可读 {}：{reason}", path.display())),
        );
        out
    }
}

/// Cross-process ownership of one Office execution's staging files.
pub(crate) struct StagingLedger {
    root: PathBuf,
    execution_id: String,
    conversation_id: String,
    lease_path: PathBuf,
    /// Held open, with sharing denied, for the whole execution. This handle is
    /// the ownership proof another process reads.
    lease: Option<fs::File>,
}

impl StagingLedger {
    /// Open a ledger and take this execution's exclusive lease.
    pub(crate) fn begin(
        artifacts_dir: &Path,
        conversation_id: &str,
    ) -> Result<StagingLedger, String> {
        let root = artifacts_dir.join(STAGING_LEDGER_DIR);
        create_dir_checked(&root.join(STAGING_LEASE_DIR))?;
        create_dir_checked(&root.join(STAGING_ENTRY_DIR))?;
        let execution_id = uuid::Uuid::new_v4().to_string();
        let lease_path = root
            .join(STAGING_LEASE_DIR)
            .join(format!("{execution_id}.lease"));
        let lease = open_exclusive(&lease_path, true).map_err(|error| {
            format!(
                "无法建立 Office 暂存归属租约 {}: {error}",
                lease_path.display()
            )
        })?;
        Ok(StagingLedger {
            root,
            execution_id,
            conversation_id: conversation_id.to_owned(),
            lease_path,
            lease: Some(lease),
        })
    }

    pub(crate) fn execution_id(&self) -> &str {
        &self.execution_id
    }

    fn entries_dir(&self) -> PathBuf {
        self.root.join(STAGING_ENTRY_DIR)
    }

    fn leases_dir(&self) -> PathBuf {
        self.root.join(STAGING_LEASE_DIR)
    }

    /// Register a staging file *before* any byte is written to it, so a crash
    /// at any later point leaves a record naming the exact file this execution
    /// owned and the target it was about to produce.
    pub(crate) fn register(
        &self,
        token: &str,
        staging: &Path,
        target: &Path,
    ) -> Result<(), String> {
        self.write_record(&StagingRecord {
            token: token.to_owned(),
            staging_path: staging.to_string_lossy().into_owned(),
            target_path: target.to_string_lossy().into_owned(),
            conversation_id: self.conversation_id.clone(),
            execution_id: self.execution_id.clone(),
            state: "staging".to_owned(),
            payload_sha256: String::new(),
            created_at_ms: now_ms(),
        })
    }

    /// Record that the staged bytes are verified and only the swap remains.
    pub(crate) fn mark_ready(
        &self,
        token: &str,
        staging: &Path,
        target: &Path,
        payload_sha256: &str,
    ) -> Result<(), String> {
        self.write_record(&StagingRecord {
            token: token.to_owned(),
            staging_path: staging.to_string_lossy().into_owned(),
            target_path: target.to_string_lossy().into_owned(),
            conversation_id: self.conversation_id.clone(),
            execution_id: self.execution_id.clone(),
            state: "ready".to_owned(),
            payload_sha256: payload_sha256.to_owned(),
            created_at_ms: now_ms(),
        })
    }

    /// Drop this execution's entry for one staging file after the commit
    /// finished either way. Never a directory-wide sweep.
    pub(crate) fn release(&self, token: &str) {
        let path = self.entries_dir().join(format!("{token}.json"));
        // Only ever delete an entry this execution owns; a token collision with
        // another execution's record must not let this process erase it.
        match fs::read(&path)
            .ok()
            .and_then(|body| serde_json::from_slice::<StagingRecord>(&body).ok())
        {
            Some(record) if record.execution_id == self.execution_id => {
                let _ = fs::remove_file(&path);
            }
            Some(_) => {}
            // An unreadable entry is still this execution's own file name, and
            // the caller is releasing the token it just created, so remove it.
            None => {
                let _ = fs::remove_file(&path);
            }
        }
    }

    fn write_record(&self, record: &StagingRecord) -> Result<(), String> {
        let dir = self.entries_dir();
        create_dir_checked(&dir)?;
        let body = serde_json::to_vec(record).map_err(|error| error.to_string())?;
        let path = dir.join(format!("{}.json", record.token));
        // Written through a sibling temp name and renamed, so a crash can never
        // leave a half-written record that a later pass would misread.
        let staging = dir.join(format!(".{}.writing", record.token));
        fs::write(&staging, &body).map_err(|error| {
            format!("无法写入暂存台账 {}: {error}", path.display())
        })?;
        fs::rename(&staging, &path).map_err(|error| {
            let _ = fs::remove_file(&staging);
            format!("无法提交暂存台账 {}: {error}", path.display())
        })?;
        Ok(())
    }

    /// Reclaim staging files this Host can *prove* are abandoned.
    ///
    /// `scope_roots` are the controlled locations a staging file may live in
    /// (the project root and, for an out-of-project target, that target's own
    /// folder). `protect` lets the caller veto a removal with a reason when a
    /// Host ledger still references the bytes. An entry is removed only when
    /// every one of these holds:
    ///
    /// 1. it is registered here and the registered name matches its token;
    /// 2. it sits inside one of `scope_roots` with no traversal component;
    /// 3. it is a regular, non-link, non-reparse-point file;
    /// 4. its owning execution's lease is not held by any live process;
    /// 5. `protect` does not claim it.
    ///
    /// Anything that cannot be proven is kept and reported.
    pub(crate) fn reclaim(
        &self,
        scope_roots: &[&Path],
        protect: &dyn Fn(&Path) -> Option<String>,
    ) -> ReclaimReport {
        let mut report = ReclaimReport::default();
        let Ok(entries) = fs::read_dir(self.entries_dir()) else {
            return report;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let record = match fs::read(&path)
                .ok()
                .and_then(|body| serde_json::from_slice::<StagingRecord>(&body).ok())
            {
                Some(record) => record,
                None => {
                    // Never delete on an unreadable record: the Host cannot
                    // prove what the file is, so the file stays and the reason
                    // is reported.
                    report
                        .unreadable
                        .push((path, "台账条目无法解析，无法证明归属".to_owned()));
                    continue;
                }
            };
            match self.reclaim_one(&record, scope_roots, protect) {
                Outcome::Removed(staging) => report.removed.push(staging),
                Outcome::RemovedButEntryKept(staging, reason) => {
                    report.removed.push(staging);
                    report.kept.push((path, reason));
                }
                Outcome::Retired { entry } => report.retired.push(entry),
                Outcome::Kept(reason) => report.kept.push((path, reason)),
            }
        }
        report
    }

    fn reclaim_one(
        &self,
        record: &StagingRecord,
        scope_roots: &[&Path],
        protect: &dyn Fn(&Path) -> Option<String>,
    ) -> Outcome {
        let staging = PathBuf::from(&record.staging_path);
        let entry_path = self.entries_dir().join(format!("{}.json", record.token));

        // (1) The ledger must name the file it claims to own.
        let name = staging
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if staging_token_of_name(name) != Some(record.token.as_str()) {
            return Outcome::Kept("登记的文件名与令牌不一致".to_owned());
        }

        // (2) Only a controlled location may be swept.
        let Some(relative) = scope_roots
            .iter()
            .find_map(|root| relative_to(root, &staging))
        else {
            return Outcome::Kept("暂存文件不在允许的受控位置".to_owned());
        };
        if relative.is_absolute()
            || relative.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Outcome::Kept("暂存路径包含越权分量".to_owned());
        }

        // (3) Ownership **before** anything else that could remove state.
        //
        // This ordering is the whole point of this check. An execution registers
        // its staging file *before* it copies any bytes, so "the staging file is
        // not on disk" is exactly what a live, just-registered execution looks
        // like. Treating that state as "the commit already finished" let a
        // second ledger delete the record of a live execution, after which the
        // interrupted writer's leftover had no record to be reclaimed by.
        // Neither retiring the record nor deleting the file may therefore happen
        // until the owning execution is *proven* gone.
        if record.execution_id == self.execution_id {
            return Outcome::Kept("属于当前活跃执行".to_owned());
        }
        let lease = self
            .leases_dir()
            .join(format!("{}.lease", record.execution_id));
        match lease_state(&lease) {
            LeaseState::Held => {
                return Outcome::Kept(format!(
                    "暂存记录仍归活跃执行 {} 所有",
                    record.execution_id
                ));
            }
            // Permissions, I/O errors and malformed lease state are *not*
            // evidence that the owner is gone.
            LeaseState::Unknown(reason) => {
                return Outcome::Kept(format!("租约状态无法判定，保留：{reason}"));
            }
            // No lease file at all, or a lease file no live process holds: the
            // execution that registered this record has ended.
            LeaseState::Released | LeaseState::Abandoned => {}
        }

        // (4) Shape of the file on disk. Only now is a missing file meaningful:
        //     the owner is gone, so the swap either consumed the bytes or never
        //     got to write them, and only the bookkeeping survived.
        let metadata = match fs::symlink_metadata(&staging) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return match self.retire_entry(&entry_path) {
                    Ok(()) => Outcome::Retired { entry: entry_path },
                    Err(reason) => Outcome::Kept(reason),
                };
            }
            Err(error) => {
                return Outcome::Kept(format!("无法读取暂存文件元数据: {error}"));
            }
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Outcome::Kept("暂存路径不是普通文件或为链接".to_owned());
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if metadata.file_attributes() & 0x400 != 0 {
                return Outcome::Kept("暂存路径是重解析点".to_owned());
            }
        }

        // (5) Host ledgers (artifacts, version/recovery rows) still naming it.
        if let Some(reason) = protect(&staging) {
            return Outcome::Kept(reason);
        }

        match fs::remove_file(&staging) {
            Ok(()) => match self.retire_entry(&entry_path) {
                // The bytes are gone and the record is gone: fully cleaned.
                Ok(()) => Outcome::Removed(staging),
                // The bytes are gone but the record could not be removed.
                // Report both facts rather than claiming a complete cleanup.
                Err(reason) => Outcome::RemovedButEntryKept(staging, reason),
            },
            Err(error) => Outcome::Kept(format!("删除暂存文件失败: {error}")),
        }
    }

    /// Drop one ledger entry, reporting whether it really happened.
    ///
    /// A cleanup that could not delete the record must never be reported as a
    /// successful cleanup: the caller turns the failure into a visible
    /// diagnostic. An entry that is already gone counts as retired.
    fn retire_entry(&self, entry_path: &Path) -> Result<(), String> {
        match fs::remove_file(entry_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!(
                "暂存台账条目删除失败，残留记录仍可见: {error}"
            )),
        }
    }
}

impl Drop for StagingLedger {
    fn drop(&mut self) {
        // The share-denied handle must be closed *before* the lease file can be
        // removed; on Windows deleting a share-denied open file fails.
        self.lease = None;
        let _ = fs::remove_file(&self.lease_path);
    }
}

/// What one reclaim decision did to one registered staging file.
enum Outcome {
    /// The staging file was deleted and its ledger entry was retired.
    Removed(PathBuf),
    /// The staging file was deleted but its ledger entry could not be removed.
    /// Kept separate from [`Outcome::Removed`] so a partial cleanup is never
    /// reported as a complete one.
    RemovedButEntryKept(PathBuf, String),
    /// Only the ledger entry was retired (the staging file was already gone).
    Retired { entry: PathBuf },
    /// Nothing was touched, with the reason.
    Kept(String),
}

/// What a lease probe established.
///
/// Only the two "no live owner" variants allow a reclaim to touch anything; the
/// other two keep the record. This is deliberately an enum rather than a `bool`:
/// collapsing "nobody holds it" and "I could not tell" into one value is exactly
/// how a lease check turns into a licence to delete.
#[derive(Debug, PartialEq, Eq)]
enum LeaseState {
    /// No lease file exists: the owning execution never started, or it finished
    /// and its ledger object was dropped.
    Released,
    /// The lease file exists but no live process holds it open: the owner died.
    Abandoned,
    /// A live process still holds the lease.
    Held,
    /// The probe could not be answered (permissions, I/O error, malformed state).
    Unknown(String),
}

/// Probe one execution's lease.
///
/// `NotFound` is genuine evidence of absence. A sharing violation is genuine
/// evidence of a live owner. Everything else — including an ACL denial and a
/// lease path that is not a regular file — is unknown, and unknown never
/// authorises deletion.
fn lease_state(path: &Path) -> LeaseState {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LeaseState::Released,
        Err(error) => LeaseState::Unknown(format!("读取租约文件元数据失败: {error}")),
        Ok(metadata) if !metadata.is_file() => {
            LeaseState::Unknown("租约路径不是普通文件".to_owned())
        }
        Ok(_) => match open_exclusive(path, false) {
            Ok(file) => {
                drop(file);
                LeaseState::Abandoned
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => LeaseState::Released,
            Err(error) if is_lease_held_error(&error) => LeaseState::Held,
            Err(error) => LeaseState::Unknown(format!("无法打开租约文件: {error}")),
        },
    }
}

/// Whether a failed exclusive open proves that another handle still holds it.
///
/// A Windows sharing violation is `ERROR_SHARING_VIOLATION` (32); the raw code
/// is used rather than the `ErrorKind` because Rust maps 32 and
/// `ERROR_ACCESS_DENIED` (5) to the same `PermissionDenied` — and those two mean
/// opposite things here. `ERROR_LOCK_VIOLATION` (33) is treated the same way.
/// On other platforms a non-blocking lock contention surfaces as `WouldBlock`.
fn is_lease_held_error(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32) | Some(33))
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Open `path` with every other handle denied.
///
/// `create` distinguishes "take the lease" from "probe the lease": the probe
/// must never create the file it is testing.
fn open_exclusive(path: &Path, create: bool) -> std::io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    if create {
        options.create(true);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    #[cfg(not(windows))]
    if create {
        // Without share modes, an exclusive create is the portable equivalent
        // of "only one owner may hold this lease".
        options.create_new(true);
    }
    options.open(path)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or(0)
}

/// Whether a project-relative path is a Host-managed transient name that must
/// never be registered as a user artifact.
///
/// Recognition only; it never authorises a deletion.
pub(crate) fn is_host_transient_name(name: &str) -> bool {
    name.starts_with(".fox-office-")
        || name.contains(".fox-backup-")
        || staging_token_of_name(name).is_some()
}

/// Test-only: create a lease file that no process holds, i.e. the on-disk state
/// a crashed execution leaves behind.
#[cfg(test)]
pub(crate) fn write_abandoned_lease_for_test(
    artifacts_dir: &Path,
    execution_id: &str,
) -> Result<(), String> {
    let dir = artifacts_dir
        .join(STAGING_LEDGER_DIR)
        .join(STAGING_LEASE_DIR);
    create_dir_checked(&dir)?;
    fs::write(dir.join(format!("{execution_id}.lease")), b"")
        .map_err(|error| format!("无法写入测试租约文件: {error}"))
}

/// Test-only: make the lease path unprobeable in a way that is *not* `NotFound`,
/// so the "unknown probe state must keep the record" branch can be exercised
/// deterministically. A directory in the lease's place fails an exclusive
/// read+write open with an access error rather than a sharing violation.
#[cfg(test)]
pub(crate) fn write_unprobeable_lease_for_test(
    artifacts_dir: &Path,
    execution_id: &str,
) -> Result<(), String> {
    let dir = artifacts_dir
        .join(STAGING_LEDGER_DIR)
        .join(STAGING_LEASE_DIR)
        .join(format!("{execution_id}.lease"));
    create_dir_checked(&dir)?;
    Ok(())
}

/// Record a staging file on behalf of an execution that is **not** this
/// process — i.e. simulate one that crashed before it could clean up.
///
/// Only used by integration tests that need a genuine "earlier execution left
/// this behind" state; there is no production caller.
#[cfg(test)]
pub(crate) fn write_staging_record_for_test(
    artifacts_dir: &Path,
    record: &StagingRecord,
) -> Result<(), String> {
    let dir = artifacts_dir
        .join(STAGING_LEDGER_DIR)
        .join(STAGING_ENTRY_DIR);
    create_dir_checked(&dir)?;
    let body = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    fs::write(dir.join(format!("{}.json", record.token)), body)
        .map_err(|error| format!("无法写入测试暂存台账: {error}"))
}

/// Path of the ledger entry for one token, so a test can assert that the
/// ledger converged after a reclaim.
#[cfg(test)]
pub(crate) fn staging_entry_path_for_test(artifacts_dir: &Path, token: &str) -> PathBuf {
    artifacts_dir
        .join(STAGING_LEDGER_DIR)
        .join(STAGING_ENTRY_DIR)
        .join(format!("{token}.json"))
}

/// The conversation's existing private preview root, canonicalized, or `None`
/// when it does not exist yet.
///
/// Used by the artifact gateway so a rendered preview can be opened through the
/// saved result's own preview entry. Deliberately returns nothing for a
/// non-existent folder: opening an artifact must never create application
/// directories as a side effect, and a conversation without previews simply
/// contributes no extra root.
pub(crate) fn existing_preview_root(data_dir: &Path, conversation_id: &str) -> Option<PathBuf> {
    let root = ArtifactStore::new(data_dir).root.join("previews").join(short_conversation_id(conversation_id));
    let metadata = fs::symlink_metadata(&root).ok()?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return None;
    }
    fs::canonicalize(&root).ok()
}

/// Whether an absolute path sits inside the conversation's deliverable folder
/// (`<project>/fox/...`), i.e. a path the model deliberately targeted as a
/// user-facing result rather than the project root.
///
/// The check is structural (path components), so it cannot be fooled by a file
/// that merely contains the word "fox".
///
/// Both sides are compared after stripping the Windows extended-length prefix:
/// `fs::canonicalize` returns `\\?\C:\...` while the Host's stored project root
/// is the plain `C:\...`, and a raw `starts_with` on the two forms silently
/// fails and would misclassify every real deliverable as a process file.
pub(crate) fn is_deliverable_path(project_root: &Path, path: &Path) -> bool {
    let Some(relative) = relative_to(project_root, path) else {
        return false;
    };
    matches!(
        relative.components().next(),
        Some(Component::Normal(first)) if first.eq_ignore_ascii_case(PROJECT_DELIVERABLE_DIR)
    )
}

/// `path` relative to `root`, tolerating the Windows verbatim-path prefix.
pub(crate) fn relative_to(root: &Path, path: &Path) -> Option<PathBuf> {
    if let Ok(relative) = path.strip_prefix(root) {
        return Some(relative.to_path_buf());
    }
    let normalize = |value: &Path| -> PathBuf {
        let text = value.to_string_lossy();
        let trimmed = text
            .strip_prefix(r"\\?\")
            .or_else(|| text.strip_prefix(r"\\.\"))
            .unwrap_or(&text);
        // `\\?\UNC\server\share` is the verbatim form of `\\server\share`.
        match trimmed.strip_prefix("UNC\\") {
            Some(rest) => PathBuf::from(format!(r"\\{rest}")),
            None => PathBuf::from(trimmed),
        }
    };
    let normalized_root = normalize(root);
    let normalized_path = normalize(path);
    normalized_path
        .strip_prefix(&normalized_root)
        .ok()
        .map(Path::to_path_buf)
}

/// Whether `path` lies inside `root` with no traversal component.
pub(crate) fn is_inside(root: &Path, path: &Path) -> bool {
    relative_to(root, path).is_some_and(|relative| {
        !relative.is_absolute()
            && !relative.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
    })
}

/// Operations that produce a document the user receives.
///
/// Intermediates are not produced here at all: they come from the
/// `attachment_compute` channel, which keeps its bytes in a Host-private
/// workspace and hands them on by `artifactId`. That separation is what makes
/// "a document tool wrote this file, so it is a result" a fact rather than a
/// guess.
pub(crate) fn produces_user_document(tool: &str) -> bool {
    matches!(
        tool,
        "office_create" | "office_edit" | "office_import_data" | "office_merge"
    )
}

/// Host-side classification of a produced file.
///
/// Three independent facts are deliberately kept apart, because collapsing them
/// is what made the old rule wrong:
///
/// * **purpose** — `deliverable` / `preview` / `process`, derived from the
///   Host-verified operation, where the bytes ended up and whether the caller
///   explicitly exported them. Never from the file extension, and never from
///   anything a connector result claims.
/// * **origin** — where the bytes are: inside an authorized project folder or in
///   an application-private Host area. Decided structurally from the path
///   against the Host's own root, never from a declaration.
/// * **verification state** — carried by the version registry, not here.
///
/// `host_private` must be the Host's own placement decision (its private preview
/// cache or working area), not a connector's `artifactOrigin` claim. Every input
/// is therefore recomputable by the projection from Host facts alone, so a
/// forged `artifactClass` in a tool result cannot influence the verdict.
pub(crate) fn classify(
    project_root: Option<&Path>,
    path: &Path,
    tool: &str,
    host_private: bool,
) -> (ArtifactClass, ArtifactOrigin) {
    if host_private {
        // The Host's private areas hold rendered *views* and intermediates.
        // A private file is never a deliverable: the user reaches it through the
        // result it belongs to rather than as a path of their own.
        let class = if tool == "office_render" {
            ArtifactClass::Preview
        } else {
            ArtifactClass::Process
        };
        return (class, ArtifactOrigin::HostPrivate);
    }
    let Some(root) = project_root else {
        // No authorized project root means the Host cannot vouch for the
        // location at all, so nothing is claimed as a deliverable.
        return (ArtifactClass::Process, ArtifactOrigin::HostPrivate);
    };
    if !is_inside(root, path) {
        // Outside the authorized project. This only happens for a path the Host
        // never admitted, so the conservative answer is "not in the project".
        return (ArtifactClass::Process, ArtifactOrigin::HostPrivate);
    }
    let origin = ArtifactOrigin::Project;
    if tool == "office_render" {
        // A rendered preview is normally redirected into the Host's private
        // cache, so reaching the project at all means the caller explicitly
        // exported the HTML/PNG as a result.
        return (ArtifactClass::Deliverable, origin);
    }
    // The conversation result folder is the Host's own result area: a file
    // deliberately written there is a result even for a general file tool.
    if is_deliverable_path(root, path) {
        return (ArtifactClass::Deliverable, origin);
    }
    // A document operation whose target the caller named explicitly is the
    // user's result as well — including a CSV or HTML the user asked for at
    // another authorized path.
    if produces_user_document(tool) {
        return (ArtifactClass::Deliverable, origin);
    }
    (ArtifactClass::Process, origin)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fox-artifact-store-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn deliverable_directory_name_is_safe_readable_and_conversation_bound() {
        let store = ArtifactStore::new(&temp_root("name"));
        let name = store.deliverable_directory_name("AGV 长时间等待问题研究", "conv-a");
        assert!(name.starts_with("AGV-长时间等待问题研究-"), "{name}");
        // The name always ends in the date plus the conversation-derived id, so
        // the topic part may contain dashes without breaking the structure. A
        // task folder therefore never collides with another conversation's.
        assert!(
            regex_like(&name),
            "expected <topic>-<yyyyMMdd>-<12 hex>, got {name}"
        );
        assert!(name.split('-').all(|part| !part.is_empty()), "{name}");
        // Reserved characters never survive.
        let win = store.deliverable_directory_name("a<b>c:d/e\\f|g?h*i", "conv-a");
        assert!(!win.contains(['<', '>', ':', '/', '\\', '|', '?', '*']), "{win}");
        assert!(regex_like(&win), "{win}");
        // Two conversations never share a folder, even with one topic and day.
        assert_ne!(
            store.deliverable_directory_name("报告", "conv-a"),
            store.deliverable_directory_name("报告", "conv-b")
        );
        // An empty topic still yields a valid, conflict-resistant name.
        let bare = store.deliverable_directory_name("", "conv-a");
        assert!(regex_like(&bare), "{bare}");
        assert!(bare.starts_with("20"), "a bare name starts with the date: {bare}");
        // Reserved device names are neutralized.
        assert!(store.deliverable_directory_name("CON", "conv-a").starts_with("CON-dir"));
        assert!(store.deliverable_directory_name("nul", "conv-a").starts_with("nul-dir"));
        // The suffix is derived from the conversation, so a task cannot be
        // redirected into another conversation's folder by naming a topic.
        let suffix = |value: &str| value.rsplit('-').next().unwrap_or_default().to_owned();
        assert_eq!(suffix(&name), suffix(&store.deliverable_directory_name("other topic", "conv-a")));
        assert_ne!(suffix(&name), suffix(&store.deliverable_directory_name("other topic", "conv-b")));
        assert_eq!(suffix(&name).len(), 12);
    }

    /// `<topic>-<8 digits>-<12 lowercase hex>`, or `<8 digits>-<12 hex>` when the
    /// task has no readable topic; no segment may be empty.
    fn regex_like(value: &str) -> bool {
        let parts: Vec<&str> = value.split('-').collect();
        if parts.len() < 2 || parts.iter().any(|part| part.is_empty()) {
            return false;
        }
        let id = parts[parts.len() - 1];
        let date = parts[parts.len() - 2];
        date.len() == 8
            && date.bytes().all(|b| b.is_ascii_digit())
            && id.len() == 12
            && id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }

    #[test]
    fn deliverable_root_is_stable_across_runs_and_isolated_across_conversations() {
        let store = ArtifactStore::new(&temp_root("reuse"));
        let project = temp_root("project");
        let first = store
            .ensure_deliverable_root(&project, "AGV", "conv-a")
            .expect("create deliverable root");
        // A second Run of the same task reuses the same user-visible folder.
        let second = store
            .ensure_deliverable_root(&project, "AGV", "conv-a")
            .expect("reuse deliverable root");
        assert_eq!(first, second);
        assert!(first.starts_with(project.join(PROJECT_DELIVERABLE_DIR)));
        // A different conversation never lands in the same folder.
        let other = store
            .ensure_deliverable_root(&project, "AGV", "conv-b")
            .expect("other conversation root");
        assert_ne!(first, other);
        // Exactly two conversation folders exist.
        let count = fs::read_dir(project.join(PROJECT_DELIVERABLE_DIR))
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .count();
        assert_eq!(count, 2);
        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn preview_and_work_areas_are_private_and_outside_the_project() {
        let data = temp_root("data");
        let project = temp_root("project");
        let store = ArtifactStore::new(&data);
        let preview = store
            .preview_path("conv-a", &project.join("报告.docx"), "html", "mode=html")
            .unwrap();
        assert!(preview.starts_with(data.join(ARTIFACTS_DIR)));
        assert!(!preview.starts_with(&project));
        // Deterministic per (conversation, source, fingerprint): a re-render
        // overwrites the same preview instead of leaving a dead link.
        let again = store
            .preview_path("conv-a", &project.join("报告.docx"), "html", "mode=html")
            .unwrap();
        assert_eq!(preview, again);
        assert_ne!(
            preview,
            store
                .preview_path("conv-b", &project.join("报告.docx"), "html", "mode=html")
                .unwrap()
        );
        let work = store.work_directory("conv-a").unwrap();
        assert!(work.starts_with(data.join(ARTIFACTS_DIR)));
        assert_ne!(work, store.work_directory("conv-a").unwrap());
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn commit_creates_a_new_target_and_replaces_an_existing_one_without_losing_bytes() {        let data = temp_root("commit-data");
        let project = temp_root("commit-project");
        let store = ArtifactStore::new(&data);
        let work = store.work_directory("conv-a").unwrap();

        // New file: staged on the target volume, then swapped in.
        let prepared = work.join("new.bin");
        fs::write(&prepared, b"first").unwrap();
        let target = project.join("fox").join("case").join("new.bin");
        let outcome = commit(
            &prepared,
            &target,
            &project,
            Path::new("fox/case/new.bin"),
            "t1",
            None,
        )
        .expect("create");
        assert!(!outcome.replaced);
        assert_eq!(fs::read(&target).unwrap(), b"first");
        // The working copy is *read-only input*: `commit` stages and swaps, it
        // never consumes the private working copy. The caller owns that
        // directory and removes it, so a failed verification can still be
        // diagnosed after the attempt.
        assert!(prepared.is_file());
        assert_eq!(fs::read(&prepared).unwrap(), b"first");

        // Existing file: staged on the target volume, then replaced.
        let prepared = work.join("new2.bin");
        fs::write(&prepared, b"second").unwrap();
        let outcome = commit(
            &prepared,
            &target,
            &project,
            Path::new("fox/case/new.bin"),
            "t2",
            None,
        )
        .expect("replace");
        assert!(outcome.replaced);
        assert_eq!(fs::read(&target).unwrap(), b"second");
        // No staging litter remains anywhere in the project.
        let litter = fs::read_dir(project.join("fox").join("case"))
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".fox-stage-")
            })
            .count();
        assert_eq!(litter, 0);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn commit_refuses_a_payload_that_changed_underneath_it() {
        let data = temp_root("commit-bad");
        let project = temp_root("commit-bad-project");
        let store = ArtifactStore::new(&data);
        let work = store.work_directory("conv-a").unwrap();
        let prepared = work.join("x.bin");
        fs::write(&prepared, b"ok").unwrap();
        // A directory in place of a file is rejected before anything is copied.
        let target = project.join("dir-target");
        fs::create_dir_all(&target).unwrap();
        let error = commit(
            &prepared,
            &target,
            &project,
            Path::new("dir-target"),
            "t",
            None,
        )
        .unwrap_err();
        assert!(!error.is_empty());
        // The original directory is untouched.
        assert!(target.is_dir());
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    // -----------------------------------------------------------------------
    // Purpose vs location vs verification state
    // -----------------------------------------------------------------------

    /// Purpose is not a location and not an extension: the same folder can hold
    /// results and intermediates, and a result can sit outside the result
    /// folder when the caller named it explicitly.
    #[test]
    fn purpose_is_decided_by_host_facts_not_by_location_or_extension() {
        let root = Path::new(r"C:\proj");
        let in_deliverable = r"C:\proj\fox\case\明细.csv";
        let in_project = r"C:\proj\报表.docx";
        let private = r"C:\Users\x\AppData\Roaming\com.fox.agent\artifacts\previews\a\p.html";

        // The document channel produces results wherever the caller put them,
        // including a CSV or an explicitly exported HTML.
        for path in [in_deliverable, in_project] {
            assert_eq!(
                classify(Some(root), Path::new(path), "office_create", false),
                (ArtifactClass::Deliverable, ArtifactOrigin::Project),
                "{path}"
            );
            assert_eq!(
                classify(Some(root), Path::new(path), "office_import_data", false),
                (ArtifactClass::Deliverable, ArtifactOrigin::Project),
                "{path}"
            );
        }
        assert_eq!(
            classify(Some(root), Path::new(r"C:\proj\fox\case\预览.html"), "office_render", false),
            (ArtifactClass::Deliverable, ArtifactOrigin::Project),
            "a render that reached the project was explicitly exported"
        );

        // The Host's private areas hold views and intermediates — never results.
        assert_eq!(
            classify(Some(root), Path::new(private), "office_render", true),
            (ArtifactClass::Preview, ArtifactOrigin::HostPrivate)
        );
        assert_eq!(
            classify(Some(root), Path::new(private), "attachment_compute", true),
            (ArtifactClass::Process, ArtifactOrigin::HostPrivate)
        );

        // A general file tool is a result only inside the Host's own result
        // folder; elsewhere in the project it stays an intermediate.
        assert_eq!(
            classify(Some(root), Path::new(in_deliverable), "write_file", false),
            (ArtifactClass::Deliverable, ArtifactOrigin::Project)
        );
        assert_eq!(
            classify(Some(root), Path::new(r"C:\proj\notes.txt"), "write_file", false),
            (ArtifactClass::Process, ArtifactOrigin::Project)
        );

        // No authorized root, or a path outside it: never claimed as a result.
        assert_eq!(
            classify(None, Path::new(in_project), "office_create", false),
            (ArtifactClass::Process, ArtifactOrigin::HostPrivate)
        );
        assert_eq!(
            classify(Some(root), Path::new(r"C:\elsewhere\report.docx"), "office_create", false),
            (ArtifactClass::Process, ArtifactOrigin::HostPrivate)
        );
    }

    #[test]
    fn traversal_in_a_relative_path_never_counts_as_inside_the_project() {
        let root = Path::new(r"C:\proj");
        assert!(is_inside(root, Path::new(r"C:\proj\fox\a.xlsx")));
        assert!(is_inside(root, Path::new(r"\\?\C:\proj\fox\a.xlsx")));
        assert!(!is_inside(root, Path::new(r"C:\proj\..\secret\a.xlsx")));
        assert!(!is_inside(root, Path::new(r"C:\other\a.xlsx")));
    }

    #[test]
    fn transient_names_are_recognized() {
        assert!(is_host_transient_name(".fox-office-abc.xlsx"));
        assert!(is_host_transient_name("报告.xlsx.fox-backup-abc"));
        assert!(is_host_transient_name(".fox-stage-t.tmp"));
        assert!(!is_host_transient_name("报告.xlsx"));
    }

    /// `fs::canonicalize` returns the Windows extended-length form while the
    /// Host's stored project root is the plain form. Comparing them naively
    /// would classify every real deliverable as a process file, so both sides
    /// are normalized.
    #[test]
    fn deliverable_detection_survives_the_windows_verbatim_prefix() {
        let root = Path::new(r"C:\proj");
        assert!(is_deliverable_path(root, Path::new(r"C:\proj\fox\case\a.xlsx")));
        assert!(is_deliverable_path(
            root,
            Path::new(r"\\?\C:\proj\fox\case\a.xlsx")
        ));
        assert!(is_deliverable_path(
            Path::new(r"\\?\C:\proj"),
            Path::new(r"C:\proj\fox\a.xlsx")
        ));
        assert!(!is_deliverable_path(root, Path::new(r"C:\proj\a.xlsx")));
        assert!(!is_deliverable_path(root, Path::new(r"C:\other\fox\a.xlsx")));
        // A sibling folder whose name merely starts with the same letters is
        // not the deliverable folder.
        assert!(!is_deliverable_path(root, Path::new(r"C:\proj\foxes\a.xlsx")));
        // UNC shares keep working through the verbatim form.
        assert!(is_deliverable_path(
            Path::new(r"\\server\share\proj"),
            Path::new(r"\\?\UNC\server\share\proj\fox\a.xlsx")
        ));
    }

    // -----------------------------------------------------------------------
    // Staging ownership ledger
    // -----------------------------------------------------------------------

    fn ledger_dir(data: &Path) -> PathBuf {
        data.join(ARTIFACTS_DIR).join(STAGING_LEDGER_DIR)
    }

    /// The ledger always lives under the Host artifact root, exactly as
    /// production passes it (`ArtifactStore::root`).
    fn open_ledger(data: &Path, conversation_id: &str) -> Result<StagingLedger, String> {
        StagingLedger::begin(&data.join(ARTIFACTS_DIR), conversation_id)
    }

    fn write_entry(data: &Path, record: &StagingRecord) {
        let dir = ledger_dir(data).join(STAGING_ENTRY_DIR);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(format!("{}.json", record.token)),
            serde_json::to_vec(record).unwrap(),
        )
        .unwrap();
    }

    fn record(token: &str, staging: &Path, execution_id: &str) -> StagingRecord {
        StagingRecord {
            token: token.to_owned(),
            staging_path: staging.to_string_lossy().into_owned(),
            target_path: staging
                .with_file_name("target.bin")
                .to_string_lossy()
                .into_owned(),
            conversation_id: "conv".to_owned(),
            execution_id: execution_id.to_owned(),
            state: "staging".to_owned(),
            payload_sha256: String::new(),
            created_at_ms: 0,
        }
    }

    // -----------------------------------------------------------------------
    // V2 regression: a record may only be retired once its owner is proven gone
    // -----------------------------------------------------------------------

    /// The window the review reproduced: a live execution has registered its
    /// staging file but has not copied a single byte yet, so the file is not on
    /// disk. "The file is not there" must not be read as "the commit finished".
    ///
    /// Deterministic: no sleeps, no timing guesses. The reclaim runs between
    /// `register` and the simulated `fs::copy`, which is exactly the production
    /// order at that point.
    #[test]
    fn a_registered_but_not_yet_copied_staging_file_keeps_its_ledger_entry() {
        let data = temp_root("precopy-data");
        let project = temp_root("precopy-project");
        let root = data.join(ARTIFACTS_DIR);

        // Execution A takes its lease and registers, then is pre-empted before
        // the copy.
        let owner = open_ledger(&data, "exec-a").unwrap();
        let staging = project.join(".fox-stage-precopy.tmp");
        let target = project.join("report.xlsx");
        owner.register("precopy", &staging, &target).unwrap();
        assert!(!staging.exists(), "the copy has not started in this window");
        let entry = staging_entry_path_for_test(&root, "precopy");
        assert!(entry.is_file());

        // Execution B reclaims concurrently.
        let collector = open_ledger(&data, "exec-b").unwrap();
        let first = collector.reclaim(&[project.as_path()], &|_| None);
        assert_eq!(
            first.retired.len(),
            0,
            "a live execution's record must never be retired: {first:?}"
        );
        assert_eq!(first.removed.len(), 0, "{first:?}");
        assert_eq!(first.kept.len(), 1, "{first:?}");
        assert!(
            entry.is_file(),
            "the record of a live execution must survive a concurrent reclaim"
        );

        // A reaches the copy, then dies before mark_ready. `drop` releases the
        // lease the way an orderly end of the execution does.
        fs::write(&staging, b"verified payload").unwrap();
        drop(owner);

        // Now, and only now, the leftover is reclaimable and the ledger
        // converges instead of leaving an unrecorded orphan behind.
        let second = collector.reclaim(&[project.as_path()], &|_| None);
        assert_eq!(second.removed.len(), 1, "{second:?}");
        assert!(!staging.exists(), "the orphaned staging file is reclaimed");
        assert!(!entry.exists(), "the ledger must converge");

        drop(collector);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// The same window, but with the entry belonging to *this* execution: a
    /// ledger must never sweep its own live registrations either.
    #[test]
    fn a_ledger_never_retires_its_own_live_registration() {
        let data = temp_root("self-data");
        let project = temp_root("self-project");
        let root = data.join(ARTIFACTS_DIR);
        let ledger = open_ledger(&data, "exec-self").unwrap();
        let staging = project.join(".fox-stage-selfown.tmp");
        ledger
            .register("selfown", &staging, &project.join("t.xlsx"))
            .unwrap();
        let report = ledger.reclaim(&[project.as_path()], &|_| None);
        assert_eq!(report.retired.len(), 0, "{report:?}");
        assert_eq!(report.removed.len(), 0, "{report:?}");
        assert!(staging_entry_path_for_test(&root, "selfown").is_file());
        // Even after the bytes appear, this execution's own record is its own.
        fs::write(&staging, b"payload").unwrap();
        let again = ledger.reclaim(&[project.as_path()], &|_| None);
        assert_eq!(again.removed.len(), 0, "{again:?}");
        assert!(staging.is_file());
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// A crashed owner leaves both the entry and an unheld lease file behind.
    /// Both the interrupted-before-copy and the interrupted-after-copy states
    /// must converge once, and only once, the owner is proven gone.
    #[test]
    fn a_crashed_owner_converges_after_copy_and_before_copy() {
        let data = temp_root("crash-data");
        let project = temp_root("crash-project");
        let root = data.join(ARTIFACTS_DIR);

        // (a) crashed before the copy: entry present, lease file present but
        //     unheld, staging file never created.
        let before = project.join(".fox-stage-crashbefore.tmp");
        write_entry(&data, &record("crashbefore", &before, "exec-crash-a"));
        write_abandoned_lease_for_test(&root, "exec-crash-a").unwrap();
        // (b) crashed after the copy, before mark_ready: same, plus bytes.
        let after = project.join(".fox-stage-crashafter.tmp");
        fs::write(&after, b"half-committed payload").unwrap();
        write_entry(&data, &record("crashafter", &after, "exec-crash-b"));
        write_abandoned_lease_for_test(&root, "exec-crash-b").unwrap();

        let collector = open_ledger(&data, "collector").unwrap();
        let report = collector.reclaim(&[project.as_path()], &|_| None);
        assert_eq!(
            report.retired.len(),
            1,
            "the before-copy record is retired (nothing on disk to delete): {report:?}"
        );
        assert_eq!(
            report.removed.len(),
            1,
            "the after-copy leftover is deleted: {report:?}"
        );
        assert!(report.kept.is_empty(), "{report:?}");
        assert!(!before.exists());
        assert!(!after.exists());
        assert!(!staging_entry_path_for_test(&root, "crashbefore").exists());
        assert!(!staging_entry_path_for_test(&root, "crashafter").exists());

        // A second pass is a no-op: the ledger really did converge.
        let again = collector.reclaim(&[project.as_path()], &|_| None);
        assert!(again.removed.is_empty() && again.retired.is_empty() && again.kept.is_empty(), "{again:?}");
        drop(collector);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// A lease that cannot be probed is not evidence of absence. The record and
    /// the bytes stay, and the reason is reported.
    #[test]
    fn an_unprobeable_lease_keeps_the_record_and_reports_why() {
        let data = temp_root("unknown-data");
        let project = temp_root("unknown-project");
        let root = data.join(ARTIFACTS_DIR);
        let staging = project.join(".fox-stage-unknowable.tmp");
        fs::write(&staging, b"payload").unwrap();
        write_entry(&data, &record("unknowable", &staging, "exec-unknown"));
        write_unprobeable_lease_for_test(&root, "exec-unknown").unwrap();

        let collector = open_ledger(&data, "collector").unwrap();
        let report = collector.reclaim(&[project.as_path()], &|_| None);
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.kept.len(), 1, "{report:?}");
        assert!(staging.is_file(), "bytes must survive an unanswerable probe");
        assert!(staging_entry_path_for_test(&root, "unknowable").is_file());
        let diagnostics = report.diagnostics();
        assert!(
            diagnostics.iter().any(|line| line.contains("租约")),
            "the diagnostic must name the lease, got {diagnostics:?}"
        );
        drop(collector);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// The exact confusion the review found: on Windows a *sharing violation*
    /// (32) and an *access denial* (5) both surface as
    /// `ErrorKind::PermissionDenied`. Only the former proves a live owner.
    #[test]
    fn only_a_sharing_violation_counts_as_a_live_lease() {
        #[cfg(windows)]
        {
            assert!(
                is_lease_held_error(&std::io::Error::from_raw_os_error(32)),
                "ERROR_SHARING_VIOLATION proves a live owner"
            );
            assert!(
                is_lease_held_error(&std::io::Error::from_raw_os_error(33)),
                "ERROR_LOCK_VIOLATION proves a live owner"
            );
            assert!(
                !is_lease_held_error(&std::io::Error::from_raw_os_error(5)),
                "ERROR_ACCESS_DENIED must stay unknown, not become `held`"
            );
        }
        assert!(is_lease_held_error(&std::io::Error::from(
            std::io::ErrorKind::WouldBlock
        )));
        assert!(!is_lease_held_error(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        assert!(!is_lease_held_error(&std::io::Error::from(
            std::io::ErrorKind::InvalidData
        )));
    }

    /// The four probe outcomes, read directly off the production function.
    #[test]
    fn the_lease_probe_separates_absent_abandoned_held_and_unknown() {
        let data = temp_root("lease-state-data");
        let root = data.join(ARTIFACTS_DIR);
        let leases = root.join(STAGING_LEDGER_DIR).join(STAGING_LEASE_DIR);
        fs::create_dir_all(&leases).unwrap();

        assert_eq!(
            lease_state(&leases.join("nobody.lease")),
            LeaseState::Released,
            "a missing lease file is genuine evidence of absence"
        );
        fs::write(leases.join("dead.lease"), b"").unwrap();
        assert_eq!(
            lease_state(&leases.join("dead.lease")),
            LeaseState::Abandoned,
            "an unheld lease file is a crashed owner"
        );

        // A live ledger in *this* process still holds the share-denied handle;
        // share modes are per handle, so the probe must see a conflict.
        let holder = StagingLedger::begin(&root, "conv").unwrap();
        let holder_lease = holder.lease_path.clone();
        assert_eq!(
            lease_state(&holder_lease),
            LeaseState::Held,
            "a held lease must be reported as held"
        );

        fs::create_dir_all(leases.join("weird.lease")).unwrap();
        assert!(
            matches!(
                lease_state(&leases.join("weird.lease")),
                LeaseState::Unknown(_)
            ),
            "malformed lease state must not be treated as absence"
        );

        drop(holder);
        assert_eq!(
            lease_state(&holder_lease),
            LeaseState::Released,
            "dropping the ledger releases the lease"
        );
        fs::remove_dir_all(&data).ok();
    }

    /// The rule the old filename sweep violated: a file the Host never
    /// registered belongs to the user and is byte-for-byte untouchable, however
    /// much its name looks like Host litter.
    #[test]
    fn a_user_file_that_merely_looks_like_staging_is_never_deleted() {        let data = temp_root("manual-data");
        let project = temp_root("manual-project");
        let manual = project.join(".fox-stage-notours.tmp");
        fs::write(&manual, b"user bytes").unwrap();
        let nested = project.join("fox").join("case");
        fs::create_dir_all(&nested).unwrap();
        let nested_manual = nested.join(".fox-stage-also-not-ours.tmp");
        fs::write(&nested_manual, b"more user bytes").unwrap();

        let ledger = open_ledger(&data, "conv").unwrap();
        let report = ledger.reclaim(&[&project], &|_| None);
        assert!(report.removed.is_empty(), "{report:?}");
        // Not even *considered*: no ledger entry claims either file.
        assert!(report.kept.is_empty(), "{report:?}");
        assert_eq!(fs::read(&manual).unwrap(), b"user bytes");
        assert_eq!(fs::read(&nested_manual).unwrap(), b"more user bytes");
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// A registered leftover whose owning execution no longer exists (crashed
    /// process, lease handle gone) is reclaimable — and only then.
    #[test]
    fn a_registered_leftover_is_reclaimed_only_without_a_live_owner() {
        let data = temp_root("reclaim-data");
        let project = temp_root("reclaim-project");
        let abandoned = project.join(".fox-stage-abandoned.tmp");
        fs::write(&abandoned, b"half copied").unwrap();
        write_entry(&data, &record("abandoned", &abandoned, "exec-gone"));

        // An owner that never took a lease at all is not an owner.
        let lease_dir = ledger_dir(&data).join(STAGING_LEASE_DIR);
        fs::create_dir_all(&lease_dir).unwrap();

        let ledger = open_ledger(&data, "conv-observer").unwrap();
        let report = ledger.reclaim(&[&project], &|_| None);
        assert_eq!(report.removed.len(), 1, "{report:?}");
        assert!(!abandoned.exists());
        assert!(report.kept.is_empty(), "{report:?}");
        // The ledger converges: the entry is gone too.
        assert!(!ledger_dir(&data)
            .join(STAGING_ENTRY_DIR)
            .join("abandoned.json")
            .exists());
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// Everything the Host cannot *prove* stays on disk with a reason.
    #[test]
    fn nothing_is_reclaimed_without_a_registered_in_scope_unprotected_owner_free_file() {
        let data = temp_root("guard-data");
        let project = temp_root("guard-project");
        let outside = temp_root("guard-outside");

        // (a) registered but outside every allowed root
        let stray = outside.join(".fox-stage-stray.tmp");
        fs::write(&stray, b"x").unwrap();
        write_entry(&data, &record("stray", &stray, "exec-gone"));
        // (b) registered, in scope, but a Host ledger still references it
        let protected = project.join(".fox-stage-protected.tmp");
        fs::write(&protected, b"y").unwrap();
        write_entry(&data, &record("protected", &protected, "exec-gone"));
        // (c) an unreadable ledger entry
        let entries = ledger_dir(&data).join(STAGING_ENTRY_DIR);
        fs::create_dir_all(&entries).unwrap();
        fs::write(entries.join("broken.json"), b"{not json").unwrap();

        let ledger = open_ledger(&data, "conv-observer").unwrap();
        let report = ledger.reclaim(&[&project], &|path| {
            (path.file_name().and_then(|n| n.to_str()) == Some(".fox-stage-protected.tmp"))
                .then(|| "产物台账仍引用该路径".to_owned())
        });
        assert!(report.removed.is_empty(), "{report:?}");
        assert_eq!(report.kept.len(), 2, "{report:?}");
        assert_eq!(report.unreadable.len(), 1, "{report:?}");
        assert!(stray.is_file());
        assert!(protected.is_file());
        // Kept leftovers always carry an actionable reason.
        assert!(report
            .diagnostics()
            .iter()
            .all(|line| line.contains("暂存")));
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
        fs::remove_dir_all(&outside).ok();
    }

    /// A commit that uses the ledger leaves neither a staging file nor a ledger
    /// entry behind, on the success path and on a refused path.
    #[test]
    fn a_ledgered_commit_leaves_no_entry_and_refuses_a_taken_staging_name() {
        let data = temp_root("ledger-commit-data");
        let project = temp_root("ledger-commit-project");
        let store = ArtifactStore::new(&data);
        let work = store.work_directory("conv").unwrap();
        let prepared = work.join("p.bin");
        fs::write(&prepared, b"payload").unwrap();
        let target = project.join("fox").join("case").join("out.bin");
        let relative = Path::new("fox/case/out.bin");

        let ledger = open_ledger(&data, "conv").unwrap();
        commit(&prepared, &target, &project, relative, "tok-a", Some(&ledger)).expect("commit");
        assert_eq!(fs::read(&target).unwrap(), b"payload");
        assert!(!ledger_dir(&data)
            .join(STAGING_ENTRY_DIR)
            .join("tok-a.json")
            .exists());
        assert_eq!(
            fs::read_dir(ledger_dir(&data).join(STAGING_ENTRY_DIR))
                .unwrap()
                .count(),
            0
        );

        // A staging name that already exists is refused, never overwritten, so
        // a foreign file cannot be clobbered by a token collision.
        let squatter = project.join("fox").join("case").join(".fox-stage-tok-b.tmp");
        fs::write(&squatter, b"someone else's bytes").unwrap();
        let error = commit(&prepared, &target, &project, relative, "tok-b", Some(&ledger))
            .unwrap_err();
        assert!(error.contains("拒绝覆盖"), "{error}");
        assert_eq!(fs::read(&squatter).unwrap(), b"someone else's bytes");
        // The refused attempt registered nothing.
        assert!(!ledger_dir(&data)
            .join(STAGING_ENTRY_DIR)
            .join("tok-b.json")
            .exists());
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }

    /// Child half of the cross-process test below. A no-op in an ordinary run.
    #[test]
    fn staging_lease_child_holds_the_lease() {
        let Ok(lease) = std::env::var("FOX_STAGING_LEASE_CHILD") else {
            return;
        };
        let Ok(ready) = std::env::var("FOX_STAGING_LEASE_CHILD_READY") else {
            return;
        };
        let file = open_exclusive(Path::new(&lease), true).expect("child takes the lease");
        fs::write(&ready, b"ready").expect("child signals readiness");
        // Hold until the parent kills this process; the bounded wait only keeps
        // a failed parent from leaving a process behind forever.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        drop(file);
    }

    /// The check a single-process mutex cannot make: while a *different process*
    /// owns the staging file, a reclaim pass in this process must leave it
    /// alone — and once that process dies, the same leftover becomes
    /// reclaimable. Synchronisation is a readiness marker written by the owner,
    /// not a sleep.
    #[test]
    fn a_live_owner_in_another_process_blocks_reclaim_until_it_dies() {
        if std::env::var_os("FOX_STAGING_LEASE_CHILD").is_some() {
            // Running as the child helper.
            return;
        }
        let data = temp_root("cross-data");
        let project = temp_root("cross-project");
        let staging = project.join("fox").join("case").join(".fox-stage-crosstoken.tmp");
        fs::create_dir_all(staging.parent().unwrap()).unwrap();
        fs::write(&staging, b"payload").unwrap();

        let lease_dir = ledger_dir(&data).join(STAGING_LEASE_DIR);
        fs::create_dir_all(&lease_dir).unwrap();
        let lease_path = lease_dir.join("owner-execution.lease");
        write_entry(&data, &record("crosstoken", &staging, "owner-execution"));

        let ready = data.join("child-ready.marker");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime_host::artifact_store::tests::staging_lease_child_holds_the_lease",
                "--nocapture",
            ])
            .env("FOX_STAGING_LEASE_CHILD", &lease_path)
            .env("FOX_STAGING_LEASE_CHILD_READY", &ready)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the owning process");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !ready.is_file() {
            assert!(
                std::time::Instant::now() < deadline,
                "the owning process never signalled readiness"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let ledger = open_ledger(&data, "conv-observer").unwrap();
        let held = ledger.reclaim(&[&project], &|_| None);
        assert!(
            held.removed.is_empty(),
            "a live owner's staging file must survive a concurrent reclaim: {held:?}"
        );
        assert_eq!(held.retired.len(), 0, "and so must its record: {held:?}");
        assert_eq!(held.kept.len(), 1, "{held:?}");
        assert!(staging.is_file());
        assert!(
            staging_entry_path_for_test(&data.join(ARTIFACTS_DIR), "crosstoken").is_file(),
            "the live owner's ledger entry must survive"
        );

        // The owner dies. Its lease *file* stays on disk; only the handle is
        // gone, which is exactly the proof this design needs.
        let _ = child.kill();
        let _ = child.wait();
        assert!(lease_path.is_file());

        let orphaned = ledger.reclaim(&[&project], &|_| None);
        assert_eq!(
            orphaned.removed.len(),
            1,
            "an orphaned registered staging file is reclaimable: {orphaned:?}"
        );
        assert!(!staging.exists());
        assert!(
            !staging_entry_path_for_test(&data.join(ARTIFACTS_DIR), "crosstoken").exists(),
            "the ledger converges once the owner is gone"
        );
        drop(ledger);
        fs::remove_dir_all(&data).ok();
        fs::remove_dir_all(&project).ok();
    }
}
