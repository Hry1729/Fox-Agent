//! Windows sandbox layout and lifecycle (docs/sandbox.md sections 7.5-7.9):
//!
//! * the managed runtime directory `%ProgramFiles%\Fox\sandbox-runtime\<version>`
//!   (Node, srt, the backends; administrator-owned, Users read+execute) and
//!   its check ([`check_windows_runtime_root`]);
//! * the per-user private root `%ProgramData%\Fox\sandbox\<user SID>` with
//!   `homes\<project uuid>\<backend>` and `tmp\<session>`
//!   ([`WindowsPrivateLayout`], [`prepare_windows_private_root`]);
//! * the srt installation as srt records it in `HKLM\SOFTWARE\sandbox-runtime`
//!   ([`read_srt_registry`]), the ledger of every sandbox SID Fox has seen
//!   (`srt-sids.json`) and Fox's install record (`srt-install.json`);
//! * the uninstall boundary ([`decide_srt_uninstall`]).
//!
//! Fox state stays in `%LOCALAPPDATA%\Fox\state`.

use crate::sandbox::{normalized, within, HostOs, SandboxError, SandboxErrorCode, SystemProbe};
use crate::statefile::{lock_path_for, read_json, unix_now, write_json, StateLock};
use crate::winacl::{
    is_string_sid, WindowsAclProbe, WindowsAclWriter, ACE_FLAG_INHERITED, SID_BUILTIN_ADMINISTRATORS, SID_LOCAL_SYSTEM, SID_TRUSTED_INSTALLER,
    WRITE_CLASS_MASK,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Test / developer override of the managed runtime directory. The value is
/// held to the same rules as the default (administrator-owned, no write for
/// Users / Authenticated Users / Everyone / INTERACTIVE, not in a user
/// profile).
pub const RUNTIME_ROOT_ENV: &str = "FOX_RUNTIME_ROOT";

/// Remedy shown whenever the one-time elevated setup is missing.
pub const ENABLE_SANDBOX_REMEDY: &str = "需在设置中启用沙箱（一次管理员授权）";

const INHERIT_ONLY_ACE: u8 = 0x08;
const SID_EVERYONE: &str = "S-1-1-0";
const SID_AUTHENTICATED_USERS: &str = "S-1-5-11";
const SID_BUILTIN_USERS: &str = "S-1-5-32-545";
const SID_INTERACTIVE: &str = "S-1-5-4";
const SID_CREATOR_OWNER: &str = "S-1-3-0";

fn env_dir(name: &str, default: &str) -> PathBuf {
    std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default))
}

/// `%ProgramFiles%\Fox\sandbox-runtime\<version>`.
pub fn default_windows_runtime_root(version: &str) -> PathBuf {
    env_dir("ProgramFiles", r"C:\Program Files").join("Fox").join("sandbox-runtime").join(version)
}

/// The managed runtime directory: [`RUNTIME_ROOT_ENV`] when set, else
/// [`default_windows_runtime_root`].
pub fn windows_runtime_root(version: &str) -> PathBuf {
    std::env::var_os(RUNTIME_ROOT_ENV).filter(|value| !value.is_empty()).map(PathBuf::from).unwrap_or_else(|| default_windows_runtime_root(version))
}

/// `%ProgramData%\Fox\sandbox` (created by the enable script).
pub fn windows_private_base() -> PathBuf {
    env_dir("ProgramData", r"C:\ProgramData").join("Fox").join("sandbox")
}

/// Per-user directories of this process (`%USERPROFILE%`, `%LOCALAPPDATA%`,
/// `%APPDATA%`): a runtime there is user-writable and is refused.
pub fn windows_user_dirs() -> Vec<PathBuf> {
    ["USERPROFILE", "LOCALAPPDATA", "APPDATA"]
        .iter()
        .filter_map(|name| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from))
        .collect()
}

fn admin_sid(sid: &str) -> bool {
    [SID_BUILTIN_ADMINISTRATORS, SID_LOCAL_SYSTEM, SID_TRUSTED_INSTALLER].iter().any(|known| known.eq_ignore_ascii_case(sid))
}

/// Check the managed runtime directory before anything in it is run or
/// granted (Windows only):
///
/// 1. not inside a user directory (`user_dirs`, e.g. `%LOCALAPPDATA%`):
///    `sandbox_runtime_not_installed` (the old per-user layout; the user must
///    run the one-time setup);
/// 2. owner is Administrators / SYSTEM / TrustedInstaller, and no ACE that
///    applies to the directory itself gives any other SID a write-class right
///    ([`WRITE_CLASS_MASK`]; Users / Authenticated Users / Everyone /
///    INTERACTIVE named in the message): otherwise `sandbox_path_exposed`
///    (code there runs as the real user, outside the sandbox).
///
/// The DACL must be readable; an unreadable one fails closed.
pub fn check_windows_runtime_root(root: &Path, user_dirs: &[PathBuf], probe: &dyn WindowsAclProbe) -> Result<(), SandboxError> {
    let os = HostOs::Windows;
    if let Some(dir) = user_dirs.iter().find(|dir| within(root, dir, os)) {
        return Err(SandboxError::new(
            SandboxErrorCode::SandboxRuntimeNotInstalled,
            format!("the sandbox runtime {} is inside the user directory {}; it must be installed machine-wide (Program Files)", root.display(), dir.display()),
        )
        .remedy(ENABLE_SANDBOX_REMEDY));
    }
    let exposed = |message: String| {
        SandboxError::new(SandboxErrorCode::SandboxPathExposed, message)
            .remedy("沙箱运行时目录必须只有管理员可写；请重新运行“启用沙箱”（一次管理员授权）修复其权限")
            .missing(root.display().to_string())
    };
    let owner = probe.owner_sid(root).map_err(|reason| exposed(format!("cannot read the owner of {}: {reason}", root.display())))?;
    if !admin_sid(&owner) {
        return Err(exposed(format!("{} is owned by {owner}, not by Administrators / SYSTEM", root.display())));
    }
    let entries = probe.dacl_entries(root).map_err(|reason| exposed(format!("cannot read the DACL of {}: {reason}", root.display())))?;
    let mut writers = Vec::new();
    for entry in entries {
        if !entry.allow || entry.flags & INHERIT_ONLY_ACE != 0 || entry.mask & WRITE_CLASS_MASK == 0 {
            continue;
        }
        if admin_sid(&entry.sid) || entry.sid.eq_ignore_ascii_case(SID_CREATOR_OWNER) {
            continue;
        }
        let name = match entry.sid.as_str() {
            sid if sid.eq_ignore_ascii_case(SID_BUILTIN_USERS) => "Users".to_owned(),
            sid if sid.eq_ignore_ascii_case(SID_AUTHENTICATED_USERS) => "Authenticated Users".to_owned(),
            sid if sid.eq_ignore_ascii_case(SID_EVERYONE) => "Everyone".to_owned(),
            sid if sid.eq_ignore_ascii_case(SID_INTERACTIVE) => "INTERACTIVE".to_owned(),
            sid => sid.to_owned(),
        };
        writers.push(format!("{name} (0x{:08x})", entry.mask));
    }
    if !writers.is_empty() {
        let broad = writers.iter().any(|writer| ["Users", "Authenticated Users", "Everyone", "INTERACTIVE"].iter().any(|group| writer.starts_with(group)));
        return Err(exposed(format!(
            "{} may be changed by {}{}",
            root.display(),
            writers.join(", "),
            if broad { " (every local account, the sandbox account included)" } else { "" }
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Private HOME / temp root under %ProgramData%
// ---------------------------------------------------------------------------

/// `<base>\<user SID>\{homes, tmp}` with `base = %ProgramData%\Fox\sandbox`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsPrivateLayout {
    pub base: PathBuf,
    pub user_sid: String,
}

impl WindowsPrivateLayout {
    pub fn new(base: PathBuf, user_sid: impl Into<String>) -> Self {
        Self { base, user_sid: user_sid.into() }
    }

    /// `%ProgramData%\Fox\sandbox` and the SID of this process's user.
    pub fn for_current_user(probe: &dyn WindowsAclProbe) -> Result<Self, SandboxError> {
        let sid = probe
            .current_user_sid()
            .map_err(|reason| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("cannot read the current user's SID: {reason}")))?;
        Ok(Self::new(windows_private_base(), sid))
    }

    /// `<base>\<SID>`: protected DACL (the user, SYSTEM, Administrators).
    pub fn user_root(&self) -> PathBuf {
        self.base.join(&self.user_sid)
    }

    /// Root of the private HOMEs ([`crate::sandbox::PrivateHomes::new`]).
    pub fn homes_root(&self) -> PathBuf {
        self.user_root().join("homes")
    }

    /// Parent of the session temp directories
    /// ([`crate::sandbox::SrtLauncher::with_tmp_root`]).
    pub fn tmp_root(&self) -> PathBuf {
        self.user_root().join("tmp")
    }
}

/// Create (first time) and verify the per-user private root, then its
/// `homes` and `tmp` children. Fail-closed at every step:
///
/// 1. the directory name must equal the SID of the current token
///    (`sandbox_policy_invalid`);
/// 2. the base must exist (created by the enable script):
///    `sandbox_runtime_not_installed`;
/// 3. created with a protected DACL (the user, SYSTEM, Administrators; owner
///    the user). If it already existed it is verified, not trusted: the base
///    lets every user create a subdirectory, so another account could have
///    pre-created one named after this SID;
/// 4. owner must be the user, the DACL must not be inherited and may hold
///    only allow ACEs for the user, SYSTEM, Administrators, plus Fox's own
///    ancestor ACE for a recorded srt sandbox SID
///    ([`crate::winancestor::ANCESTOR_ACE_MASK`], no flags); and the existing
///    broad-DACL check must find nothing: else `sandbox_path_exposed`.
pub fn prepare_windows_private_root(
    layout: &WindowsPrivateLayout,
    probe: &dyn WindowsAclProbe,
    writer: &dyn WindowsAclWriter,
    known_srt_sids: &[String],
) -> Result<PathBuf, SandboxError> {
    let current = probe
        .current_user_sid()
        .map_err(|reason| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("cannot read the current user's SID: {reason}")))?;
    if !is_string_sid(&layout.user_sid) || !layout.user_sid.eq_ignore_ascii_case(&current) {
        return Err(SandboxError::new(
            SandboxErrorCode::SandboxPolicyInvalid,
            format!("private directory name {:?} is not the current user's SID {current}", layout.user_sid),
        ));
    }
    if !layout.base.is_dir() {
        return Err(SandboxError::new(SandboxErrorCode::SandboxRuntimeNotInstalled, format!("{} does not exist; the sandbox setup has not run", layout.base.display()))
            .remedy(ENABLE_SANDBOX_REMEDY));
    }
    let root = layout.user_root();
    writer
        .create_private_dir(&root, &current)
        .map_err(|reason| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("cannot create {}: {reason}", root.display())))?;
    verify_private_root(&root, &current, probe, known_srt_sids)?;
    for child in [layout.homes_root(), layout.tmp_root()] {
        std::fs::create_dir_all(&child)
            .map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("cannot create {}: {error}", child.display())))?;
    }
    Ok(root)
}

/// Step 4 of [`prepare_windows_private_root`] (also usable on its own).
pub fn verify_private_root(root: &Path, user_sid: &str, probe: &dyn WindowsAclProbe, known_srt_sids: &[String]) -> Result<(), SandboxError> {
    let exposed = |message: String| {
        SandboxError::new(SandboxErrorCode::SandboxPathExposed, message)
            .remedy("Fox 的沙箱私有目录权限异常（可能被其他账户预先创建）；请删除该目录后重试，或重新运行“启用沙箱”")
            .missing(root.display().to_string())
    };
    let owner = probe.owner_sid(root).map_err(|reason| exposed(format!("cannot read the owner of {}: {reason}", root.display())))?;
    if !owner.eq_ignore_ascii_case(user_sid) {
        return Err(exposed(format!("{} is owned by {owner}, not by the current user {user_sid}", root.display())));
    }
    let entries = probe.dacl_entries(root).map_err(|reason| exposed(format!("cannot read the DACL of {}: {reason}", root.display())))?;
    let mut foreign = Vec::new();
    for entry in &entries {
        if entry.flags & ACE_FLAG_INHERITED != 0 {
            foreign.push(format!("inherited ACE for {}", entry.sid));
            continue;
        }
        if !entry.allow {
            continue; // a deny never widens access
        }
        let trusted = entry.sid.eq_ignore_ascii_case(user_sid) || entry.sid.eq_ignore_ascii_case(SID_LOCAL_SYSTEM) || entry.sid.eq_ignore_ascii_case(SID_BUILTIN_ADMINISTRATORS);
        let fox_ancestor = known_srt_sids.iter().any(|sid| sid.eq_ignore_ascii_case(&entry.sid))
            && entry.mask == crate::winancestor::ANCESTOR_ACE_MASK
            && entry.flags == 0;
        if !trusted && !fox_ancestor {
            foreign.push(format!("{} (0x{:08x})", entry.sid, entry.mask));
        }
    }
    if !foreign.is_empty() {
        return Err(exposed(format!("{} grants more than the current user, SYSTEM and Administrators: {}", root.display(), foreign.join(", "))));
    }
    match probe.broad_access(root) {
        Ok(access) if access.any() => Err(exposed(format!("{} is open to {}", root.display(), access.groups.join(", ")))),
        Ok(_) => Ok(()),
        Err(reason) => Err(exposed(format!("{} DACL unreadable: {reason}", root.display()))),
    }
}

// ---------------------------------------------------------------------------
// srt installation state (HKLM\SOFTWARE\sandbox-runtime)
// ---------------------------------------------------------------------------

/// What srt-win's `windows-install` wrote (srt-win `reg.rs` / `install.rs`
/// `write_setup`, commit 626a5967). `MarkerVersion` is written last and is
/// the commit point; without it the install is incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtRegistryState {
    pub marker_version: u32,
    pub sandbox_user: Option<String>,
    pub sandbox_user_sid: String,
    pub sandbox_group_sid: Option<String>,
    /// `CreatedAt` (QWORD, Unix seconds): changes on every fresh install, so
    /// it tells Fox's install from a later reinstall by someone else.
    pub created_at: Option<u64>,
}

/// Parse `reg query HKLM\SOFTWARE\sandbox-runtime /reg:64` output. `None` when
/// the marker or the SID is missing, or the SID is not a local account SID
/// (`S-1-5-21-...`).
pub fn parse_srt_registry(text: &str) -> Option<SrtRegistryState> {
    let mut marker = None;
    let mut user = None;
    let mut sid = None;
    let mut group = None;
    let mut created = None;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 3 || !fields[1].starts_with("REG_") {
            continue;
        }
        let value = fields[2..].join(" ");
        let number = |value: &str| -> Option<u64> {
            let value = value.trim();
            match value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")) {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None => value.parse().ok(),
            }
        };
        match fields[0] {
            "MarkerVersion" => marker = number(&value).and_then(|value| u32::try_from(value).ok()),
            "SandboxUser" => user = Some(value),
            "SandboxUserSid" => sid = Some(value),
            "SandboxGroupSid" => group = Some(value),
            "CreatedAt" => created = number(&value),
            _ => {}
        }
    }
    let sid = sid?;
    if !is_string_sid(&sid) || !sid.starts_with("S-1-5-21-") {
        return None;
    }
    Some(SrtRegistryState { marker_version: marker?, sandbox_user: user, sandbox_user_sid: sid, sandbox_group_sid: group, created_at: created })
}

/// Read srt's install state through `reg.exe` (64-bit view). Fail-closed:
/// anything but a complete, valid install is `sandbox_not_installed`.
pub fn read_srt_registry(probe: &dyn SystemProbe) -> Result<SrtRegistryState, SandboxError> {
    let reg = probe.which("reg").unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\reg.exe"));
    let not_installed = |message: &str| SandboxError::new(SandboxErrorCode::SandboxNotInstalled, message.to_owned()).remedy(ENABLE_SANDBOX_REMEDY);
    let text = probe
        .run(&reg, &["query", crate::sandbox::WINDOWS_INSTALL_REGISTRY_KEY, "/reg:64"])
        .ok_or_else(|| not_installed("the Windows sandbox (srt windows-install) is not set up on this computer"))?;
    parse_srt_registry(&text).ok_or_else(|| not_installed("the Windows sandbox setup is incomplete (no MarkerVersion or no valid SandboxUserSid)"))
}

// ---------------------------------------------------------------------------
// srt SID ledger (srt-sids.json)
// ---------------------------------------------------------------------------

pub const SRT_SID_LEDGER_FILE: &str = "srt-sids.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownSrtSid {
    pub sid: String,
    pub first_seen_unix: u64,
    #[serde(default)]
    pub srt_created_at: Option<u64>,
}

/// Every srt sandbox SID Fox has seen on this machine (append-only). Only
/// these are treated as "Fox's sandbox accounts" by the ownership repair.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtSidLedger {
    #[serde(default)]
    pub sids: Vec<KnownSrtSid>,
}

impl SrtSidLedger {
    pub fn sid_strings(&self) -> Vec<String> {
        self.sids.iter().map(|known| known.sid.clone()).collect()
    }
}

/// Append the current sandbox SID (no-op when known). Call whenever Fox
/// detects the srt installation (start, after the enable script). Returns
/// every known SID.
pub fn record_srt_sid(state_root: &Path, state: &SrtRegistryState) -> Result<Vec<String>, SandboxError> {
    let path = state_root.join(SRT_SID_LEDGER_FILE);
    let io = |message: String| SandboxError::new(SandboxErrorCode::SandboxUnavailable, message);
    let _lock = StateLock::acquire(&lock_path_for(&path)).map_err(|error| io(format!("cannot lock {}: {error}", path.display())))?;
    let mut ledger: SrtSidLedger = read_json(&path).map_err(io)?;
    if !ledger.sids.iter().any(|known| known.sid.eq_ignore_ascii_case(&state.sandbox_user_sid)) {
        ledger.sids.push(KnownSrtSid { sid: state.sandbox_user_sid.clone(), first_seen_unix: unix_now(), srt_created_at: state.created_at });
        write_json(&path, &ledger).map_err(io)?;
    }
    Ok(ledger.sid_strings())
}

/// The known SIDs (empty when Fox never saw an installation).
pub fn known_srt_sids(state_root: &Path) -> Result<Vec<String>, SandboxError> {
    let ledger: SrtSidLedger = read_json(&state_root.join(SRT_SID_LEDGER_FILE)).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, error))?;
    Ok(ledger.sid_strings())
}

// ---------------------------------------------------------------------------
// Enable script result, Fox's install record, uninstall boundary
// ---------------------------------------------------------------------------

pub const SRT_INSTALL_RECORD_FILE: &str = "srt-install.json";

/// The `srt` part of `enable-sandbox.ps1`'s JSON result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableSrtResult {
    /// `HKLM\SOFTWARE\sandbox-runtime` had a complete install before the
    /// script ran (someone else installed srt: Fox does not own it).
    pub already_installed: bool,
    pub package_version: String,
    pub sandbox_user_sid: String,
    pub marker_version: u32,
    #[serde(default)]
    pub created_at: Option<u64>,
}

/// `enable-sandbox.ps1` writes this as its last stdout line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnableScriptResult {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub runtime_root: Option<PathBuf>,
    #[serde(default)]
    pub private_base: Option<PathBuf>,
    #[serde(default)]
    pub srt: Option<EnableSrtResult>,
    #[serde(default)]
    pub log: Option<PathBuf>,
}

/// What Fox remembers about the srt installation it made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoxSrtInstallRecord {
    /// Fox's enable script ran `windows-install` on a machine without srt.
    pub installed_by_fox: bool,
    pub srt_version: String,
    pub marker_version: u32,
    pub created_at: Option<u64>,
    pub sandbox_user_sid: String,
    pub runtime_root: PathBuf,
    pub recorded_at_unix: u64,
}

/// Turn a successful enable result into Fox state: the install record and
/// the SID ledger. A failed result changes nothing.
pub fn apply_enable_result(state_root: &Path, result: &EnableScriptResult) -> Result<FoxSrtInstallRecord, SandboxError> {
    let invalid = |message: &str| SandboxError::new(SandboxErrorCode::SandboxNotInstalled, message.to_owned()).remedy(ENABLE_SANDBOX_REMEDY);
    if !result.ok {
        return Err(invalid(result.error.as_deref().unwrap_or("the sandbox setup failed")));
    }
    let srt = result.srt.as_ref().ok_or_else(|| invalid("the sandbox setup reported no srt result"))?;
    let runtime_root = result.runtime_root.clone().ok_or_else(|| invalid("the sandbox setup reported no runtime directory"))?;
    let state = SrtRegistryState {
        marker_version: srt.marker_version,
        sandbox_user: None,
        sandbox_user_sid: srt.sandbox_user_sid.clone(),
        sandbox_group_sid: None,
        created_at: srt.created_at,
    };
    if !is_string_sid(&state.sandbox_user_sid) {
        return Err(invalid("the sandbox setup reported an invalid sandbox SID"));
    }
    record_srt_sid(state_root, &state)?;
    let path = state_root.join(SRT_INSTALL_RECORD_FILE);
    let previous: Option<FoxSrtInstallRecord> = read_json(&path).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, error))?;
    // Ownership is decided once, at the run that installed srt: a later
    // re-run finds srt "already installed" by Fox itself.
    let same_install = previous.as_ref().is_some_and(|record| record.created_at == srt.created_at && record.sandbox_user_sid.eq_ignore_ascii_case(&srt.sandbox_user_sid));
    let installed_by_fox = if srt.already_installed { same_install && previous.as_ref().is_some_and(|record| record.installed_by_fox) } else { true };
    let record = FoxSrtInstallRecord {
        installed_by_fox,
        srt_version: srt.package_version.clone(),
        marker_version: srt.marker_version,
        created_at: srt.created_at,
        sandbox_user_sid: srt.sandbox_user_sid.clone(),
        runtime_root,
        recorded_at_unix: unix_now(),
    };
    write_json(&path, &Some(record.clone())).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, error))?;
    Ok(record)
}

/// Evidence for one uninstall condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "detail")]
pub enum Evidence {
    /// Checked: nothing found.
    None,
    /// Checked: found (what).
    Found(Vec<String>),
    /// Could not be checked (why). Counts as "found" for the decision.
    Unknown(String),
}

/// Inputs of [`decide_srt_uninstall`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrtUninstallFacts {
    /// Fox's `srt-install.json`.
    pub record: Option<FoxSrtInstallRecord>,
    /// `HKLM\SOFTWARE\sandbox-runtime` now (`None` = not installed).
    pub registry: Option<SrtRegistryState>,
    /// Version of the srt package in the runtime about to run `windows-uninstall`.
    pub installed_srt_version: Option<String>,
    /// Live srt sessions: processes running as the sandbox account (needs
    /// elevation to see), Fox's own live sessions, live brokers.
    pub active_sessions: Evidence,
    /// Other users of srt on this machine. srt's per-user `state.db`
    /// (`%LOCALAPPDATA%\sandbox-runtime\state.db`: brokers = pid + process
    /// create time, ace_holders, working_aces, placeholders) names no client
    /// program, so it cannot tell Fox from another srt client of the same
    /// Windows user: unless that is excluded some other way (explicit user
    /// confirmation), this is `Unknown`. A `state.db` in another user's
    /// profile is `Found`.
    pub other_users: Evidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "reasons")]
pub enum SrtUninstallDecision {
    /// Run `windows-uninstall` and remove srt's machine state.
    Uninstall,
    /// Keep srt; tell the user why.
    Keep(Vec<String>),
    /// srt is not installed.
    NothingToDo,
}

/// Uninstall only when every condition holds: Fox installed it, it is still
/// that same installation (`CreatedAt`, SID, marker) at the same package
/// version, no srt session is active, and there is no evidence of other users
/// (unknown counts as evidence). Otherwise keep, listing every reason.
pub fn decide_srt_uninstall(facts: &SrtUninstallFacts) -> SrtUninstallDecision {
    let Some(registry) = &facts.registry else { return SrtUninstallDecision::NothingToDo };
    let mut reasons = Vec::new();
    match &facts.record {
        None => reasons.push("Fox has no record of installing srt".to_owned()),
        Some(record) => {
            if !record.installed_by_fox {
                reasons.push("srt was already installed when Fox enabled the sandbox".to_owned());
            }
            if record.created_at != registry.created_at || !record.sandbox_user_sid.eq_ignore_ascii_case(&registry.sandbox_user_sid) || record.marker_version != registry.marker_version {
                reasons.push("srt has been reinstalled since Fox installed it (CreatedAt / SID / marker differ)".to_owned());
            }
            match &facts.installed_srt_version {
                Some(version) if *version == record.srt_version => {}
                Some(version) => reasons.push(format!("srt package version {version} differs from the one Fox installed ({})", record.srt_version)),
                None => reasons.push("the srt package version cannot be determined".to_owned()),
            }
        }
    }
    match &facts.active_sessions {
        Evidence::None => {}
        Evidence::Found(items) => reasons.push(format!("srt sessions are active: {}", items.join(", "))),
        Evidence::Unknown(why) => reasons.push(format!("active srt sessions cannot be ruled out: {why}")),
    }
    match &facts.other_users {
        Evidence::None => {}
        Evidence::Found(items) => reasons.push(format!("other srt users found: {}", items.join(", "))),
        Evidence::Unknown(why) => reasons.push(format!("other srt users cannot be ruled out: {why}")),
    }
    if reasons.is_empty() {
        SrtUninstallDecision::Uninstall
    } else {
        SrtUninstallDecision::Keep(reasons)
    }
}

/// Lower-cased Windows form of a path (for comparisons in the scripts' JSON).
pub fn windows_key(path: &Path) -> String {
    normalized(path, HostOs::Windows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::winacl::{AceEntry, BroadAccess};
    use std::sync::Mutex;

    const USER: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";
    const SRT: &str = "S-1-5-21-1111111111-2222222222-3333333333-1004";

    #[derive(Default)]
    struct Fake {
        owner: Option<&'static str>,
        entries: Vec<AceEntry>,
        broad: bool,
        created: Mutex<Vec<PathBuf>>,
    }

    impl WindowsAclProbe for Fake {
        fn check_write_dac(&self, _path: &Path) -> Result<(), String> {
            Ok(())
        }
        fn broad_access(&self, _path: &Path) -> Result<BroadAccess, String> {
            Ok(BroadAccess { read: self.broad, write: false, groups: if self.broad { vec!["Users".into()] } else { vec![] } })
        }
        fn owner_sid(&self, _path: &Path) -> Result<String, String> {
            self.owner.map(str::to_owned).ok_or_else(|| "no owner".into())
        }
        fn dacl_entries(&self, _path: &Path) -> Result<Vec<AceEntry>, String> {
            Ok(self.entries.clone())
        }
        fn current_user_sid(&self) -> Result<String, String> {
            Ok(USER.into())
        }
    }

    impl WindowsAclWriter for Fake {
        fn has_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<bool, String> {
            unreachable!()
        }
        fn add_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<(), String> {
            unreachable!()
        }
        fn remove_exact_allow_ace(&self, _: &Path, _: &str, _: u32) -> Result<usize, String> {
            unreachable!()
        }
        fn set_owner_to_current_user(&self, _: &Path) -> Result<(), String> {
            unreachable!()
        }
        fn create_private_dir(&self, path: &Path, _: &str) -> Result<bool, String> {
            std::fs::create_dir_all(path).map_err(|error| error.to_string())?;
            self.created.lock().unwrap().push(path.to_path_buf());
            Ok(true)
        }
    }

    fn ace(sid: &str, mask: u32, flags: u8) -> AceEntry {
        AceEntry { sid: sid.into(), allow: true, mask, flags }
    }

    fn program_files_runtime() -> Fake {
        Fake {
            owner: Some(SID_BUILTIN_ADMINISTRATORS),
            entries: vec![ace(SID_BUILTIN_ADMINISTRATORS, 0x1f01ff, 3), ace(SID_LOCAL_SYSTEM, 0x1f01ff, 3), ace(SID_BUILTIN_USERS, 0x1200a9, 3), ace(SID_CREATOR_OWNER, 0x10000000, 0x0b)],
            ..Default::default()
        }
    }

    #[test]
    fn runtime_root_must_be_admin_owned_and_not_writable_by_broad_groups() {
        let root = Path::new(r"C:\Program Files\Fox\sandbox-runtime\0.1.0");
        let user_dirs = [PathBuf::from(r"C:\Users\u"), PathBuf::from(r"C:\Users\u\AppData\Local")];
        check_windows_runtime_root(root, &user_dirs, &program_files_runtime()).unwrap();
        // The previous per-user layout.
        let error = check_windows_runtime_root(Path::new(r"C:\Users\u\AppData\Local\Fox\runtime"), &user_dirs, &program_files_runtime()).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxRuntimeNotInstalled);
        assert_eq!(error.code.as_str(), "sandbox_runtime_not_installed");
        assert_eq!(error.remedy.as_deref(), Some(ENABLE_SANDBOX_REMEDY));
        // Override location owned by the user.
        let mut fake = program_files_runtime();
        fake.owner = Some(USER);
        assert_eq!(check_windows_runtime_root(Path::new(r"D:\fox-rt"), &user_dirs, &fake).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        // Users / Authenticated Users with a write-class right (e.g. inherited from D:\).
        for (sid, mask) in [(SID_BUILTIN_USERS, 0x4u32), (SID_AUTHENTICATED_USERS, 0x1301bf), (SID_EVERYONE, 0x40000), (SID_INTERACTIVE, 0x80000)] {
            let mut fake = program_files_runtime();
            fake.entries.push(ace(sid, mask, 0x10));
            let error = check_windows_runtime_root(Path::new(r"D:\fox-rt"), &user_dirs, &fake).unwrap_err();
            assert_eq!(error.code, SandboxErrorCode::SandboxPathExposed, "{sid}");
            assert!(error.message.contains("every local account"), "{}", error.message);
        }
        // Inherit-only write (CREATOR OWNER style) does not apply to the directory itself.
        let mut fake = program_files_runtime();
        fake.entries.push(ace(SID_BUILTIN_USERS, 0x2, 0x0b));
        check_windows_runtime_root(root, &user_dirs, &fake).unwrap();
        // Unreadable owner / DACL: fail closed.
        let mut fake = program_files_runtime();
        fake.owner = None;
        assert_eq!(check_windows_runtime_root(root, &user_dirs, &fake).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
    }

    #[test]
    fn layout_puts_homes_and_tmp_under_the_user_sid() {
        let layout = WindowsPrivateLayout::new(PathBuf::from(r"C:\ProgramData\Fox\sandbox"), USER);
        assert_eq!(windows_key(&layout.homes_root()), windows_key(Path::new(&format!(r"C:\ProgramData\Fox\sandbox\{USER}\homes"))));
        assert_eq!(windows_key(&layout.tmp_root()), windows_key(Path::new(&format!(r"C:\ProgramData\Fox\sandbox\{USER}\tmp"))));
    }

    #[test]
    fn private_root_is_created_for_the_current_sid_and_verified() {
        let base = std::env::temp_dir().join(format!("fox-private-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let good = || Fake {
            owner: Some(USER),
            entries: vec![ace(USER, 0x1f01ff, 3), ace(SID_LOCAL_SYSTEM, 0x1f01ff, 3), ace(SID_BUILTIN_ADMINISTRATORS, 0x1f01ff, 3)],
            ..Default::default()
        };
        // Base missing: the enable script has not run.
        let layout = WindowsPrivateLayout::new(base.clone(), USER);
        assert_eq!(prepare_windows_private_root(&layout, &good(), &good(), &[]).unwrap_err().code, SandboxErrorCode::SandboxRuntimeNotInstalled);
        std::fs::create_dir_all(&base).unwrap();
        // Another SID as the directory name.
        let other = WindowsPrivateLayout::new(base.clone(), SRT);
        assert_eq!(prepare_windows_private_root(&other, &good(), &good(), &[]).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid);
        // Good.
        let fake = good();
        let root = prepare_windows_private_root(&layout, &fake, &fake, &[]).unwrap();
        assert_eq!(root, base.join(USER));
        assert!(layout.homes_root().is_dir() && layout.tmp_root().is_dir());
        // Fox's own ancestor ACE for a recorded srt SID is accepted ...
        let mut fake = good();
        fake.entries.push(ace(SRT, crate::winancestor::ANCESTOR_ACE_MASK, 0));
        prepare_windows_private_root(&layout, &fake, &fake, &[SRT.to_owned()]).unwrap();
        // ... but not for an unknown SID, nor with another mask.
        assert_eq!(prepare_windows_private_root(&layout, &fake, &fake, &[]).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        let mut fake = good();
        fake.entries.push(ace(SRT, 0x1200a9, 3));
        assert_eq!(prepare_windows_private_root(&layout, &fake, &fake, &[SRT.to_owned()]).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        // Pre-created by another account (squatting), inherited ACEs, broad access.
        let mut fake = good();
        fake.owner = Some(SRT);
        assert_eq!(prepare_windows_private_root(&layout, &fake, &fake, &[]).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        let mut fake = good();
        fake.entries.push(ace(SID_BUILTIN_USERS, 0x1200a9, 0x13));
        assert_eq!(prepare_windows_private_root(&layout, &fake, &fake, &[]).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        let mut fake = good();
        fake.broad = true;
        assert_eq!(prepare_windows_private_root(&layout, &fake, &fake, &[]).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        let _ = std::fs::remove_dir_all(&base);
    }

    const REG: &str = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\sandbox-runtime\r\n    SandboxUser    REG_SZ    srt-sandbox\r\n    SandboxUserSid    REG_SZ    S-1-5-21-1111111111-2222222222-3333333333-1004\r\n    SandboxGroupSid    REG_SZ    S-1-5-21-1111111111-2222222222-3333333333-1003\r\n    CreatedAt    REG_QWORD    0x6713a2f0\r\n    MarkerVersion    REG_DWORD    0x3\r\n\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\sandbox-runtime\\Cred\r\n";

    #[test]
    fn srt_registry_is_parsed_and_incomplete_installs_are_refused() {
        let state = parse_srt_registry(REG).unwrap();
        assert_eq!(state.sandbox_user_sid, SRT);
        assert_eq!(state.marker_version, 3);
        assert_eq!(state.created_at, Some(0x6713a2f0));
        assert_eq!(state.sandbox_user.as_deref(), Some("srt-sandbox"));
        assert!(parse_srt_registry(&REG.replace("    MarkerVersion    REG_DWORD    0x3\r\n", "")).is_none(), "marker is the commit point");
        assert!(parse_srt_registry(&REG.replace(SRT, "S-1-5-18")).is_none(), "must be a local account SID");
        assert!(parse_srt_registry("").is_none());
    }

    #[test]
    fn sid_ledger_appends_each_sid_once_and_enable_results_are_recorded() {
        let state_root = std::env::temp_dir().join(format!("fox-srt-sids-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&state_root);
        let state = parse_srt_registry(REG).unwrap();
        assert_eq!(record_srt_sid(&state_root, &state).unwrap(), [SRT]);
        assert_eq!(record_srt_sid(&state_root, &state).unwrap(), [SRT]);
        let reinstalled = SrtRegistryState { sandbox_user_sid: "S-1-5-21-1111111111-2222222222-3333333333-1010".into(), ..state.clone() };
        assert_eq!(record_srt_sid(&state_root, &reinstalled).unwrap().len(), 2);
        assert_eq!(known_srt_sids(&state_root).unwrap().len(), 2);

        let result: EnableScriptResult = serde_json::from_str(&format!(
            r#"{{"ok":true,"version":"0.1.0","runtimeRoot":"C:\\Program Files\\Fox\\sandbox-runtime\\0.1.0","privateBase":"C:\\ProgramData\\Fox\\sandbox",
               "srt":{{"alreadyInstalled":false,"packageVersion":"0.0.80","sandboxUserSid":"{SRT}","markerVersion":3,"createdAt":1729340144}}}}"#
        ))
        .unwrap();
        let record = apply_enable_result(&state_root, &result).unwrap();
        assert!(record.installed_by_fox);
        // A second run finds srt "already installed" (by Fox itself): still Fox's.
        let mut again = result.clone();
        again.srt.as_mut().unwrap().already_installed = true;
        assert!(apply_enable_result(&state_root, &again).unwrap().installed_by_fox);
        // Installed by someone else first: never Fox's.
        let other_root = state_root.join("other");
        assert!(!apply_enable_result(&other_root, &again).unwrap().installed_by_fox);
        let failed = EnableScriptResult { ok: false, error: Some("UAC declined".into()), ..result };
        assert_eq!(apply_enable_result(&state_root, &failed).unwrap_err().code, SandboxErrorCode::SandboxNotInstalled);
        let _ = std::fs::remove_dir_all(&state_root);
    }

    fn facts() -> SrtUninstallFacts {
        let registry = parse_srt_registry(REG).unwrap();
        SrtUninstallFacts {
            record: Some(FoxSrtInstallRecord {
                installed_by_fox: true,
                srt_version: "0.0.80".into(),
                marker_version: registry.marker_version,
                created_at: registry.created_at,
                sandbox_user_sid: registry.sandbox_user_sid.clone(),
                runtime_root: PathBuf::from(r"C:\Program Files\Fox\sandbox-runtime\0.1.0"),
                recorded_at_unix: 1,
            }),
            registry: Some(registry),
            installed_srt_version: Some("0.0.80".into()),
            active_sessions: Evidence::None,
            other_users: Evidence::None,
        }
    }

    #[test]
    fn srt_is_uninstalled_only_when_every_condition_holds() {
        assert_eq!(decide_srt_uninstall(&facts()), SrtUninstallDecision::Uninstall);
        let keep = |facts: SrtUninstallFacts| match decide_srt_uninstall(&facts) {
            SrtUninstallDecision::Keep(reasons) => reasons,
            other => panic!("expected keep, got {other:?}"),
        };
        let mut f = facts();
        f.record = None;
        assert!(keep(f)[0].contains("no record"));
        let mut f = facts();
        f.record.as_mut().unwrap().installed_by_fox = false;
        keep(f);
        let mut f = facts();
        f.registry.as_mut().unwrap().created_at = Some(1);
        assert!(keep(f)[0].contains("reinstalled"));
        let mut f = facts();
        f.installed_srt_version = Some("0.0.81".into());
        keep(f);
        let mut f = facts();
        f.active_sessions = Evidence::Found(vec!["node.exe (srt-sandbox) pid 4242".into()]);
        keep(f);
        let mut f = facts();
        f.other_users = Evidence::Unknown("state.db does not name its client program".into());
        assert!(keep(f)[0].contains("cannot be ruled out"), "unknown = keep");
        let mut f = facts();
        f.other_users = Evidence::Found(vec![r"C:\Users\bob\AppData\Local\sandbox-runtime\state.db".into()]);
        keep(f);
        let mut f = facts();
        f.registry = None;
        assert_eq!(decide_srt_uninstall(&f), SrtUninstallDecision::NothingToDo);
        let json = serde_json::to_value(decide_srt_uninstall(&facts())).unwrap();
        assert_eq!(json["decision"], "uninstall");
    }
}
