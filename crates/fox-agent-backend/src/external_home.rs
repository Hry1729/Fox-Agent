//! What Fox still manages around an external backend process (pi / Codex /
//! DeepSeek) now that it adds no outer sandbox of its own (plan B2):
//!
//! * the permission mode a session records ([`PermissionMode`]) and how a
//!   Host grant decides it ([`effective_permission_mode`],
//!   [`recheck_permission_mode`]). The value is persisted per conversation;
//!   it no longer changes how the process is started. The Agent's own native
//!   permission mode ([`crate::native_permission`]) is what limits it;
//! * the private HOME per project x backend ([`PrivateHomes`]) and the
//!   per-session temp directory, created before launch and pointed at by
//!   HOME / USERPROFILE / XDG / CODEX_HOME / DSH_HOME ([`private_home_env`]);
//! * the structured launch refusal ([`LaunchError`]).
//!
//! The process itself is an ordinary child of the Fox Host, contained in a
//! Job Object (Windows) / process group (Unix) by [`crate::acp`].

use crate::config::{HostPolicy, SessionSnapshot};
use crate::launch::LaunchSpec;
use crate::port::AgentBackend;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Session/user/project setting key holding the requested mode.
pub const SETTING_PERMISSION_MODE: &str = "permission_mode";

// ---------------------------------------------------------------------------
// Permission mode
// ---------------------------------------------------------------------------

/// The mode a conversation recorded. Persisted (`agent_backend` settings,
/// run events); Fox launches the backend the same way in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Explicit user grant.
    FullAccess,
    /// Default for everything else, including sessions created before modes
    /// existed. Stored name kept for compatibility with existing rows.
    #[default]
    Sandboxed,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FullAccess => "full_access",
            Self::Sandboxed => "sandboxed",
        }
    }

    /// Missing or unknown values read as `Sandboxed`: an old session, a typo or
    /// a value from a newer Fox never widens access.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("full_access") => Self::FullAccess,
            _ => Self::Sandboxed,
        }
    }

    pub fn from_settings(settings: &BTreeMap<String, String>) -> Self {
        Self::parse(settings.get(SETTING_PERMISSION_MODE).map(String::as_str))
    }
}

/// Where an explicit full-access grant was made. The Host stores the keys in
/// [`HostPolicy::full_access_grants`]; revoking removes the key and applies to
/// existing sessions before their next continue (plan 6.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FullAccessGrant {
    Conversation(String),
    Project(String),
}

impl FullAccessGrant {
    pub fn key(&self) -> String {
        match self {
            Self::Conversation(id) => format!("conversation:{id}"),
            Self::Project(root) => format!("project:{root}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeDecision {
    pub mode: PermissionMode,
    /// Why a requested `full_access` was not honoured (for audit and UI).
    pub downgraded: Option<String>,
}

/// The mode a session records. `full_access` needs all of: the session asked
/// for it, the Host did not forbid it (enterprise policy), and a live grant
/// exists for this conversation or this project. A user/project config layer
/// can never produce full access: only the snapshot's request plus a
/// Host-held grant can.
pub fn effective_permission_mode(policy: &HostPolicy, snapshot: &SessionSnapshot, conversation_id: &str) -> ModeDecision {
    let requested = PermissionMode::from_settings(&snapshot.settings);
    if requested == PermissionMode::Sandboxed {
        return ModeDecision { mode: PermissionMode::Sandboxed, downgraded: None };
    }
    if policy.full_access_forbidden {
        return ModeDecision { mode: PermissionMode::Sandboxed, downgraded: Some("full access is disabled by policy".into()) };
    }
    if policy.locked_keys.get(SETTING_PERMISSION_MODE).map(String::as_str) == Some("sandboxed") {
        return ModeDecision { mode: PermissionMode::Sandboxed, downgraded: Some("host policy locks permission_mode to sandboxed".into()) };
    }
    let granted = [FullAccessGrant::Conversation(conversation_id.to_owned()), FullAccessGrant::Project(snapshot.project_root.clone())]
        .iter()
        .any(|grant| policy.full_access_grants.contains(&grant.key()));
    if !granted {
        return ModeDecision {
            mode: PermissionMode::Sandboxed,
            downgraded: Some("no full-access grant for this conversation or project".into()),
        };
    }
    ModeDecision { mode: PermissionMode::FullAccess, downgraded: None }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModeRecheck {
    Unchanged,
    /// The running session recorded full access but the grant has been
    /// withdrawn: the Host stops the running turn and process, and the next
    /// turn starts a new connection in the recorded mode.
    RestartSandboxed(String),
}

/// Run with [`crate::config::recheck_before_continue`]. Tightening applies to
/// existing sessions immediately; loosening is never carried in silently.
pub fn recheck_permission_mode(policy: &HostPolicy, snapshot: &SessionSnapshot, conversation_id: &str, running: PermissionMode) -> ModeRecheck {
    let now = effective_permission_mode(policy, snapshot, conversation_id);
    match (running, now.mode) {
        (PermissionMode::FullAccess, PermissionMode::Sandboxed) => {
            ModeRecheck::RestartSandboxed(now.downgraded.unwrap_or_else(|| "full access withdrawn".into()))
        }
        _ => ModeRecheck::Unchanged,
    }
}

// ---------------------------------------------------------------------------
// Host OS
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostOs {
    Linux,
    MacOs,
    Windows,
    Other,
}

impl HostOs {
    pub fn current() -> Self {
        if cfg!(target_os = "linux") {
            Self::Linux
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// First executable named `name` on PATH (Unix only; Windows always uses the
/// managed Node runtime).
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchErrorCode {
    /// The launch request cannot be prepared as given (malformed project id,
    /// relative private HOME, bad session id ...).
    LaunchInvalid,
    /// The private HOME or session temp directory cannot be created.
    PrivateHomeUnavailable,
}

impl LaunchErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LaunchInvalid => "agent_backend_launch_invalid",
            Self::PrivateHomeUnavailable => "agent_backend_private_home_unavailable",
        }
    }
}

/// Structured refusal. Serializes as `{code, message, missing, remedy}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchError {
    pub code: LaunchErrorCode,
    pub message: String,
    /// What is missing, when known.
    pub missing: Vec<String>,
    /// What clears it, phrased for the user.
    pub remedy: Option<String>,
}

impl LaunchError {
    pub fn new(code: LaunchErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), missing: Vec::new(), remedy: None }
    }

    pub fn missing(mut self, what: impl Into<String>) -> Self {
        self.missing.push(what.into());
        self
    }

    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)?;
        if !self.missing.is_empty() {
            write!(f, " (missing: {})", self.missing.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for LaunchError {}

// ---------------------------------------------------------------------------
// Private HOMEs
// ---------------------------------------------------------------------------

/// A private HOME together with the root all Fox private HOMEs live under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivateHome {
    pub root: PathBuf,
    pub path: PathBuf,
}

/// A bare path is a standalone HOME that is its own root (tests, tools). The
/// Host uses [`PrivateHomes::home_for`].
impl From<PathBuf> for PrivateHome {
    fn from(path: PathBuf) -> Self {
        Self { root: path.clone(), path }
    }
}

/// Granularity of the Fox-managed private HOME (product decision
/// 2026-10-19): one per project x backend, shared by that project's sessions
/// of that backend, so the backend's native login and history carry over
/// between those sessions and never across projects or backends. Temp
/// directories are per session regardless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateHomeScope {
    #[default]
    PerProjectPerBackend,
}

/// Where the private HOMEs live: `<root>/<project uuid>/<backend id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateHomes {
    pub root: PathBuf,
    pub scope: PrivateHomeScope,
}

/// Canonical (lowercase, hyphenated) form of a project UUID from the Host
/// database, or `None` when it is empty, not 8-4-4-4-12 hex, or the nil UUID.
/// Names, paths and display titles are never accepted: two projects can
/// share a name or a folder name, and a path can contain anything.
pub fn canonical_project_uuid(value: &str) -> Option<String> {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    if groups.len() != 5 || groups.iter().zip(lengths).any(|(group, len)| group.len() != len || !group.chars().all(|c| c.is_ascii_hexdigit())) {
        return None;
    }
    let canonical = value.to_ascii_lowercase();
    (canonical != "00000000-0000-0000-0000-000000000000").then_some(canonical)
}

impl PrivateHomes {
    pub fn new(root: PathBuf) -> Self {
        Self { root, scope: PrivateHomeScope::default() }
    }

    /// The private HOME of `backend` in the project with database id
    /// `project_uuid`. An empty or malformed UUID is refused; nothing is
    /// derived from paths or names.
    pub fn home_for(&self, project_uuid: &str, backend: AgentBackend) -> Result<PrivateHome, LaunchError> {
        let uuid = canonical_project_uuid(project_uuid).ok_or_else(|| {
            LaunchError::new(LaunchErrorCode::LaunchInvalid, format!("project id {project_uuid:?} is not a valid project UUID"))
        })?;
        if backend == AgentBackend::Fox {
            return Err(LaunchError::new(LaunchErrorCode::LaunchInvalid, "FoxBackend has no private HOME"));
        }
        let path = match self.scope {
            PrivateHomeScope::PerProjectPerBackend => self.root.join(&uuid).join(backend.as_str()),
        };
        Ok(PrivateHome { root: self.root.clone(), path })
    }
}

/// Windows: the private root earlier Fox builds kept under
/// `%ProgramData%\Fox\sandbox\<user SID>` (`homes\`, `tmp\`). Returned only
/// when it already exists, so logins made there keep working; new installs
/// use the per-user Fox data directory instead. `None` off Windows.
pub fn legacy_windows_private_root() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
            .join("Fox")
            .join("sandbox");
        let root = base.join(win::current_user_sid().ok()?);
        root.is_dir().then_some(root)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, LocalFree};

    /// String SID of the user this process runs as.
    pub fn current_user_sid() -> Result<String, String> {
        use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
        use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        let mut token = std::ptr::null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(format!("OpenProcessToken failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let mut needed = 0u32;
        unsafe { GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut needed) };
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        let ok = unsafe { GetTokenInformation(token, TokenUser, buffer.as_mut_ptr() as *mut _, (buffer.len() * 8) as u32, &mut needed) };
        let error = unsafe { GetLastError() };
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return Err(format!("GetTokenInformation(TokenUser) failed (Win32 error {error})"));
        }
        let user = unsafe { &*(buffer.as_ptr() as *const TOKEN_USER) };
        let mut text: *mut u16 = std::ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut text) } == 0 || text.is_null() {
            return Err(format!("ConvertSidToStringSidW failed (Win32 error {})", unsafe { GetLastError() }));
        }
        let mut len = 0usize;
        while unsafe { *text.add(len) } != 0 {
            len += 1;
        }
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        unsafe { LocalFree(text as _) };
        Ok(value)
    }
}

/// The backend's private HOME variables: HOME / XDG / TMP (Windows also
/// USERPROFILE / APPDATA / LOCALAPPDATA) and the backend's own home variable
/// (CODEX_HOME, DSH_HOME), so it reads its login and configuration from the
/// private HOME, never the user's real one.
pub fn private_home_env(backend: AgentBackend, private_home: &Path, tmp: &Path, os: HostOs) -> Vec<(String, String)> {
    let home = private_home.to_string_lossy().into_owned();
    let sub = |name: &str| private_home.join(name).to_string_lossy().into_owned();
    let tmp = tmp.to_string_lossy().into_owned();
    let mut env = vec![
        ("HOME".to_owned(), home.clone()),
        ("XDG_CONFIG_HOME".to_owned(), sub(".config")),
        ("XDG_CACHE_HOME".to_owned(), sub(".cache")),
        ("XDG_DATA_HOME".to_owned(), sub(".local/share")),
        ("XDG_STATE_HOME".to_owned(), sub(".local/state")),
        ("TMPDIR".to_owned(), tmp.clone()),
        ("TMP".to_owned(), tmp.clone()),
        ("TEMP".to_owned(), tmp),
    ];
    if os == HostOs::Windows {
        env.push(("USERPROFILE".to_owned(), home));
        env.push(("APPDATA".to_owned(), sub(r"AppData\Roaming")));
        env.push(("LOCALAPPDATA".to_owned(), sub(r"AppData\Local")));
    }
    match backend {
        AgentBackend::Codex => env.push(("CODEX_HOME".to_owned(), sub(".codex"))),
        AgentBackend::Deepseek => env.push(("DSH_HOME".to_owned(), sub(".dsh"))),
        AgentBackend::Pi | AgentBackend::Fox => {}
    }
    env
}

/// Subdirectories of the private HOME a backend requires to EXIST before it
/// starts, `/`-separated and relative to the private HOME:
/// * Codex: `CODEX_HOME=<home>/.codex` ([`private_home_env`]); codex-acp
///   refuses to start when it is missing;
/// * DeepSeek: `DSH_HOME=<home>/.dsh`;
/// * pi: `<home>/.pi/agent` (pi-coding-agent `getAgentDir()`).
pub fn backend_home_subdirs(backend: AgentBackend) -> &'static [&'static str] {
    match backend {
        AgentBackend::Codex => &[".codex"],
        AgentBackend::Deepseek => &[".dsh"],
        AgentBackend::Pi => &[".pi/agent"],
        AgentBackend::Fox => &[],
    }
}

/// Create the private HOME and [`backend_home_subdirs`] when missing. Unix:
/// every directory from the private HOME down is 0700. A link on the way is
/// refused.
pub fn ensure_backend_home_dirs(backend: AgentBackend, private_home: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(private_home)?;
    set_private_dir_mode(private_home)?;
    for sub in backend_home_subdirs(backend) {
        let mut dir = private_home.to_path_buf();
        for part in sub.split('/') {
            dir.push(part);
            if std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(std::io::Error::other(format!("{} is a link; refusing to use it as the backend home", dir.display())));
            }
            match std::fs::create_dir(&dir) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => {}
                Err(error) => return Err(error),
            }
            set_private_dir_mode(&dir)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_private_dir_mode(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_dir_mode(_dir: &Path) -> std::io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// Launch preparation
// ---------------------------------------------------------------------------

/// Where a launch keeps its private HOME and session temp.
#[derive(Debug, Clone)]
pub struct PrivateDirs {
    pub private_home: PathBuf,
    pub tmp_root: PathBuf,
}

/// The per-session temp directory; removed on drop, so it goes away with the
/// session's connection whatever path ends it. The private HOME is kept.
#[derive(Debug)]
pub struct SessionTemp(PathBuf);

impl SessionTemp {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SessionTemp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What Fox put around one launch: since B2 nothing but the private HOME, so
/// this only states the mode the session recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchDeclaration {
    pub mode: PermissionMode,
}

impl LaunchDeclaration {
    pub fn declare(mode: PermissionMode) -> Self {
        Self { mode }
    }
}

#[derive(Debug)]
pub struct PreparedLaunch {
    pub mode: PermissionMode,
    pub spec: LaunchSpec,
    /// Held for the life of the connection.
    pub temp: Option<SessionTemp>,
    pub declaration: LaunchDeclaration,
    /// The private HOME variables ([`private_home_env`]), already merged into
    /// `spec.session_env`.
    pub backend_env: Vec<(String, String)>,
}

/// Point the backend's HOME (Windows USERPROFILE ...) and temp at the private
/// ones ([`private_home_env`]); both are created when missing. The session
/// temp is removed when the connection ends.
pub fn prepare_launch(mut spec: LaunchSpec, session_id: &str, private: &PrivateDirs, os: HostOs) -> Result<PreparedLaunch, LaunchError> {
    use LaunchErrorCode::*;
    if !valid_session_id(session_id) {
        return Err(LaunchError::new(LaunchInvalid, "session id must be [A-Za-z0-9_-]{1,128}"));
    }
    if !private.private_home.is_absolute() || !private.tmp_root.is_absolute() {
        return Err(LaunchError::new(
            LaunchInvalid,
            format!("private HOME {} and temp root {} must be absolute", private.private_home.display(), private.tmp_root.display()),
        ));
    }
    let io = |error: std::io::Error| LaunchError::new(PrivateHomeUnavailable, format!("cannot prepare the private HOME: {error}"));
    let session_tmp = private.tmp_root.join(session_id);
    ensure_backend_home_dirs(spec.backend, &private.private_home).map_err(io)?;
    let _ = std::fs::remove_dir_all(&session_tmp);
    std::fs::create_dir_all(&session_tmp).map_err(io)?;
    let backend_env = private_home_env(spec.backend, &private.private_home, &session_tmp, os);
    for (key, value) in &backend_env {
        spec.session_env.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        spec.session_env.push((key.clone(), value.clone()));
    }
    Ok(PreparedLaunch {
        mode: PermissionMode::FullAccess,
        spec,
        temp: Some(SessionTemp(session_tmp)),
        declaration: LaunchDeclaration::declare(PermissionMode::FullAccess),
        backend_env,
    })
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(backend: AgentBackend) -> LaunchSpec {
        LaunchSpec { backend, program: "/x".into(), args: vec![], version_pin: "x", credential_env: &[], experimental: false, session_env: vec![("HOME".into(), "/real".into())] }
    }

    #[test]
    fn unknown_or_missing_mode_reads_as_the_default() {
        assert_eq!(PermissionMode::parse(None), PermissionMode::Sandboxed);
        assert_eq!(PermissionMode::parse(Some("typo")), PermissionMode::Sandboxed);
        assert_eq!(PermissionMode::parse(Some(" full_access ")), PermissionMode::FullAccess);
    }

    #[test]
    fn full_access_needs_a_request_and_a_live_grant() {
        let mut snapshot = SessionSnapshot { backend: AgentBackend::Codex, project_root: "/p".into(), egress_allowed: false, settings: BTreeMap::new() };
        let mut policy = HostPolicy::default();
        assert_eq!(effective_permission_mode(&policy, &snapshot, "c").mode, PermissionMode::Sandboxed);
        snapshot.settings.insert(SETTING_PERMISSION_MODE.into(), "full_access".into());
        assert!(effective_permission_mode(&policy, &snapshot, "c").downgraded.is_some());
        policy.full_access_grants.insert(FullAccessGrant::Conversation("c".into()).key());
        assert_eq!(effective_permission_mode(&policy, &snapshot, "c").mode, PermissionMode::FullAccess);
        assert_eq!(recheck_permission_mode(&policy, &snapshot, "c", PermissionMode::FullAccess), ModeRecheck::Unchanged);
        policy.full_access_forbidden = true;
        assert!(matches!(recheck_permission_mode(&policy, &snapshot, "c", PermissionMode::FullAccess), ModeRecheck::RestartSandboxed(_)));
    }

    #[test]
    fn private_homes_are_per_project_and_backend_and_refuse_bad_ids() {
        let homes = PrivateHomes::new("/homes".into());
        let home = homes.home_for("8F14E45F-CEEA-467A-9A36-DEDD4BEA2543", AgentBackend::Codex).unwrap();
        assert_eq!(home.path, Path::new("/homes/8f14e45f-ceea-467a-9a36-dedd4bea2543/codex"));
        assert_eq!(homes.home_for("my project", AgentBackend::Codex).unwrap_err().code, LaunchErrorCode::LaunchInvalid);
        assert!(homes.home_for("00000000-0000-0000-0000-000000000000", AgentBackend::Pi).is_err());
        assert!(homes.home_for("8f14e45f-ceea-467a-9a36-dedd4bea2543", AgentBackend::Fox).is_err());
    }

    #[test]
    fn prepare_launch_points_home_at_the_private_dirs_and_cleans_the_session_temp() {
        let root = tempfile::tempdir().unwrap();
        let private = PrivateDirs { private_home: root.path().join("home"), tmp_root: root.path().join("tmp") };
        let prepared = prepare_launch(spec(AgentBackend::Codex), "s1", &private, HostOs::Linux).unwrap();
        assert!(root.path().join("home/.codex").is_dir());
        let env: BTreeMap<_, _> = prepared.spec.session_env.iter().cloned().collect();
        assert_eq!(env["HOME"], root.path().join("home").to_string_lossy());
        assert_eq!(env["CODEX_HOME"], root.path().join("home/.codex").to_string_lossy());
        assert_eq!(env["TMPDIR"], root.path().join("tmp/s1").to_string_lossy());
        assert_eq!(prepared.mode, PermissionMode::FullAccess);
        let temp = root.path().join("tmp/s1");
        assert!(temp.is_dir());
        drop(prepared);
        assert!(!temp.exists(), "the session temp goes away with the launch");
        assert!(prepare_launch(spec(AgentBackend::Pi), "bad id", &private, HostOs::Linux).is_err());
    }
}
