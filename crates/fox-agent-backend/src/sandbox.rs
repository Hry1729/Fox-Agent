//! Permission modes and the srt isolation layer for external backends
//! (v0.3.2 addendum to the four-agent plan; product decision 2026-10-19).
//!
//! Two modes, nothing in between:
//!
//! * [`PermissionMode::Sandboxed`] (default, and the value every session that
//!   never recorded a mode gets): the whole external backend process
//!   (codex-acp, pi-acp, dsh) runs inside one srt
//!   (`@anthropic-ai/sandbox-runtime`, Apache-2.0) sandbox per session. It may
//!   read and write only the authorized directories, a private HOME and the
//!   system directories it needs to run; outbound network is limited to that
//!   backend's model API domains.
//! * [`PermissionMode::FullAccess`]: an explicit user grant for one
//!   conversation or one project. No sandbox and no promised directory
//!   boundary; the Host floor (environment allowlist, no credential hand-out,
//!   cancel and process-tree cleanup, records, faithful forwarding of native
//!   approvals) still applies because it lives in the Host, not here.
//!
//! **Fail-closed**: when the sandbox cannot be started (srt or Node missing,
//! a platform dependency missing, Windows `windows-install` not run, policy
//! invalid, backend not installed where the sandbox can read it) the backend
//! is refused with a structured [`SandboxError`]. Nothing here ever falls back
//! to full access.
//!
//! srt reads are allowed everywhere by default (deny-then-allow, `allowRead`
//! beats `denyRead`). On Linux and macOS the settings therefore deny the
//! filesystem root `/` (srt then denies every top-level directory except
//! `/proc`, `/dev`, `/sys`) and re-allow only: the minimal system paths needed
//! to run ([`default_system_read_only`]), the authorized directories, this
//! project's private HOME for this backend, this session's temp directory and
//! the read-only paths the backend needs (its Fox-managed install directory,
//! the Node runtime, the srt package itself).
//!
//! Windows is a different model (srt 0.0.80 source, see `crate::winacl` and
//! `docs/sandbox.md` section 7): the backend runs as the separate local
//! account `srt-sandbox`, which by NTFS default ACLs cannot open the calling
//! user's profile at all, and srt reaches paths by adding ACEs for that account
//! as the calling, non-elevated user. So on Windows Fox writes no `denyRead`
//! by default (a deny on `C:\Users` is redundant and cannot be written: the
//! user has no `WRITE_DAC` there), writes ACEs only on paths the user owns
//! (authorized folders, Fox's private HOME / temp, Fox's managed runtime
//! directory), never on system paths the Users group can already read
//! (Program Files, Windows), and refuses anything else up front
//! ([`SandboxErrorCode::SandboxPathNotOwned`]). What stays readable is every
//! location whose DACL is open to all local accounts, declared as
//! [`ReadScope::UsersGroupReadable`]; [`crate::winacl::audit_broad_exposure`]
//! lists the ones on fixed drives for the UI. See `docs/sandbox.md`.

use crate::config::{HostPolicy, SessionSnapshot};
use crate::launch::LaunchSpec;
use crate::port::{AgentBackend, CapabilityDeclaration, Enforcement};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Session/user/project setting key holding the requested mode.
pub const SETTING_PERMISSION_MODE: &str = "permission_mode";
/// Setting key (comma-separated) that may narrow the network allowlist. Any
/// layer may set it; it can only remove domains, never add them.
pub const SETTING_ALLOWED_DOMAINS: &str = "sandbox.allowed_domains";

pub const SRT_PACKAGE: &str = "@anthropic-ai/sandbox-runtime";
/// Version the settings shape (literal path entries) and the Linux
/// measurement were taken with.
pub const SRT_VERSION: &str = "0.0.80";
pub const SRT_MIN_VERSION: (u64, u64, u64) = (0, 0, 80);
/// srt CLI requirement (`engines.node` of the package).
pub const SRT_MIN_NODE: (u64, u64, u64) = (22, 12, 0);

// ---------------------------------------------------------------------------
// Permission mode
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Explicit user grant; no sandbox, no directory boundary promised.
    FullAccess,
    /// Default for everything else, including sessions created before modes
    /// existed.
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

/// The mode a session runs in. `full_access` needs all of: the session asked
/// for it, the Host did not forbid it (enterprise policy), and a live grant
/// exists for this conversation or this project. Anything else is
/// `sandboxed`. A user/project config layer can never produce full access:
/// only the snapshot's request plus a Host-held grant can.
pub fn effective_permission_mode(policy: &HostPolicy, snapshot: &SessionSnapshot, conversation_id: &str) -> ModeDecision {
    let requested = PermissionMode::from_settings(&snapshot.settings);
    if requested == PermissionMode::Sandboxed {
        return ModeDecision { mode: PermissionMode::Sandboxed, downgraded: None };
    }
    if policy.full_access_forbidden {
        return ModeDecision {
            mode: PermissionMode::Sandboxed,
            downgraded: Some("full access is disabled by policy".into()),
        };
    }
    if policy.locked_keys.get(SETTING_PERMISSION_MODE).map(String::as_str) == Some("sandboxed") {
        return ModeDecision {
            mode: PermissionMode::Sandboxed,
            downgraded: Some("host policy locks permission_mode to sandboxed".into()),
        };
    }
    let granted = [
        FullAccessGrant::Conversation(conversation_id.to_owned()),
        FullAccessGrant::Project(snapshot.project_root.clone()),
    ]
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
    /// The running process is outside the sandbox but full access has been
    /// withdrawn. The Host calls the revocation stop
    /// ([`crate::port::stop_for_revocation`], or
    /// `AcpConnection::stop_for_revocation` on a live connection): cancel the
    /// turn, settle pending requests (in flight -> `outcome_unknown`), kill
    /// the process tree; then [`crate::port::rebuild_after_revocation`]
    /// starts it again under srt (preflight included, fail-closed, no replay
    /// of the last operation) before the next continue / resume.
    RestartSandboxed(String),
}

/// Run with [`crate::config::recheck_before_continue`]. Tightening applies to
/// existing sessions immediately; loosening (a new grant while a sandboxed
/// process runs) is never carried in silently, the session keeps its sandbox
/// until the Host deliberately restarts it.
pub fn recheck_permission_mode(
    policy: &HostPolicy,
    snapshot: &SessionSnapshot,
    conversation_id: &str,
    running: PermissionMode,
) -> ModeRecheck {
    let now = effective_permission_mode(policy, snapshot, conversation_id);
    match (running, now.mode) {
        (PermissionMode::FullAccess, PermissionMode::Sandboxed) => ModeRecheck::RestartSandboxed(
            now.downgraded.unwrap_or_else(|| "full access withdrawn".into()),
        ),
        _ => ModeRecheck::Unchanged,
    }
}

// ---------------------------------------------------------------------------
// Network allowlist
// ---------------------------------------------------------------------------

/// Model API domains per pi provider id. Unknown providers get nothing: their
/// endpoint must be added by the Host ([`HostPolicy::sandbox_extra_domains`]).
/// Source: provider public API hosts; the provider ids follow pi's naming and
/// were not checked one by one against pi 0.84.2's provider table.
pub fn pi_provider_domains(provider: &str) -> Option<&'static [&'static str]> {
    Some(match provider {
        "anthropic" => &["api.anthropic.com"],
        "openai" => &["api.openai.com"],
        "google" => &["generativelanguage.googleapis.com"],
        "deepseek" => &["api.deepseek.com"],
        "openrouter" => &["openrouter.ai"],
        "xai" => &["api.x.ai"],
        "groq" => &["api.groq.com"],
        "mistral" => &["api.mistral.ai"],
        "cerebras" => &["api.cerebras.ai"],
        _ => return None,
    })
}

/// Default outbound allowlist of a backend's model API.
///
/// * codex: `api.openai.com` (API-key auth), `chatgpt.com` (ChatGPT-login
///   Responses endpoint), `auth.openai.com` (token refresh). Source-read, not
///   yet measured through a live sandboxed turn.
/// * deepseek: `api.deepseek.com`.
/// * pi: the union over the providers configured for the session.
pub fn default_allowed_domains(backend: AgentBackend, pi_providers: &[&str]) -> Vec<String> {
    let mut domains: BTreeSet<String> = BTreeSet::new();
    match backend {
        AgentBackend::Fox => {}
        AgentBackend::Codex => {
            domains.extend(["api.openai.com", "chatgpt.com", "auth.openai.com"].map(String::from));
        }
        AgentBackend::Deepseek => {
            domains.insert("api.deepseek.com".into());
        }
        AgentBackend::Pi => {
            for provider in pi_providers {
                if let Some(hosts) = pi_provider_domains(provider) {
                    domains.extend(hosts.iter().map(|host| (*host).to_owned()));
                }
            }
        }
    }
    domains.into_iter().collect()
}

fn valid_domain(entry: &str) -> bool {
    let host = entry.rsplit_once(':').map_or(entry, |(host, port)| {
        if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() { host } else { entry }
    });
    let bare = host.strip_prefix("*.").unwrap_or(host);
    !bare.is_empty()
        && bare.contains('.')
        && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
        && !bare.starts_with('.')
        && !bare.ends_with('.')
}

fn split_port(entry: &str) -> (&str, Option<&str>) {
    match entry.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
        _ => (entry, None),
    }
}

/// Whether ceiling entry `allowed` already permits everything `requested`
/// permits (so narrowing to `requested` is a tightening).
fn covers(allowed: &str, requested: &str) -> bool {
    let (allowed_host, allowed_port) = split_port(allowed);
    let (requested_host, requested_port) = split_port(requested);
    if allowed_port.is_some() && allowed_port != requested_port {
        return false;
    }
    let allowed_host = allowed_host.to_ascii_lowercase();
    let requested_host = requested_host.to_ascii_lowercase();
    if allowed_host == requested_host {
        return true;
    }
    match allowed_host.strip_prefix("*.") {
        Some(suffix) => {
            let requested_bare = requested_host.strip_prefix("*.").unwrap_or(&requested_host);
            requested_bare.ends_with(&format!(".{suffix}"))
        }
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DomainResolution {
    pub domains: Vec<String>,
    /// Entries a lower layer tried to add (or malformed ones), for audit. They
    /// never reach the sandbox.
    pub rejected: Vec<String>,
    /// pi providers without a known domain (only the Host can allow them).
    pub unknown_providers: Vec<String>,
}

/// Effective allowlist for one session.
///
/// Ceiling = backend defaults + Host extras for that backend (layer 1 only).
/// Then every layer that sets [`SETTING_ALLOWED_DOMAINS`] (Host locked key,
/// session snapshot, user, project) narrows it: the result is the
/// intersection, so any layer can tighten and none can loosen.
pub fn resolve_allowed_domains(
    backend: AgentBackend,
    pi_providers: &[&str],
    policy: &HostPolicy,
    layers: &[&BTreeMap<String, String>],
) -> DomainResolution {
    let mut resolution = DomainResolution::default();
    if backend == AgentBackend::Pi {
        resolution.unknown_providers = pi_providers
            .iter()
            .filter(|provider| pi_provider_domains(provider).is_none())
            .map(|provider| (*provider).to_owned())
            .collect();
    }
    let mut current: Vec<String> = default_allowed_domains(backend, pi_providers);
    if let Some(extra) = policy.sandbox_extra_domains.get(&backend) {
        for entry in extra {
            if valid_domain(entry) {
                current.push(entry.clone());
            } else {
                resolution.rejected.push(format!("host policy: malformed domain {entry:?}"));
            }
        }
    }
    let locked = policy.locked_keys.get(SETTING_ALLOWED_DOMAINS);
    let narrowing = locked.into_iter().chain(layers.iter().filter_map(|layer| layer.get(SETTING_ALLOWED_DOMAINS)));
    for value in narrowing {
        let requested: Vec<String> =
            value.split(',').map(str::trim).filter(|entry| !entry.is_empty()).map(String::from).collect();
        let mut next = Vec::new();
        for entry in requested {
            if !valid_domain(&entry) {
                resolution.rejected.push(format!("malformed domain {entry:?}"));
            } else if current.iter().any(|allowed| covers(allowed, &entry)) {
                next.push(entry);
            } else {
                resolution.rejected.push(format!("{entry} is not in the allowlist ceiling and cannot be added"));
            }
        }
        current = next;
    }
    let mut seen = BTreeSet::new();
    current.retain(|entry| seen.insert(entry.to_ascii_lowercase()));
    current.sort();
    resolution.domains = current;
    resolution
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxErrorCode {
    /// The platform cannot run srt here (unsupported OS/arch, user namespaces
    /// unavailable, probe failed).
    SandboxUnavailable,
    /// Windows: `windows-install` has not been run (needs one UAC prompt).
    SandboxNotInstalled,
    /// srt package, Node >= 22.12, bubblewrap, socat or ripgrep missing.
    SandboxDependencyMissing,
    /// The policy cannot be turned into a safe sandbox (relative path,
    /// authorizing a home root, no authorized directory ...).
    SandboxPolicyInvalid,
    /// The backend program is not installed where the sandbox can read it
    /// (for example launched through `npx`, which needs the registry and the
    /// user's npm cache).
    SandboxBackendNotInstalled,
    /// Windows: srt would have to add an ACE on a path the current user does
    /// not own / may not change permissions on (`WRITE_DAC`), or a runtime the
    /// sandbox must read is outside Fox's managed runtime directory. srt-win
    /// would fail with Access Denied (Win32 error 5) and roll back.
    SandboxPathNotOwned,
    /// Windows: a Fox private directory (state, private HOMEs, session temp,
    /// managed runtime) has a DACL open to every local account, so the
    /// sandbox account could reach it without any grant.
    SandboxPathExposed,
    /// Windows: the managed sandbox runtime is not installed machine-wide
    /// (`%ProgramFiles%\Fox\sandbox-runtime\<version>`), e.g. a runtime
    /// left in the old per-user location under `%LOCALAPPDATA%`, or the
    /// `%ProgramData%\Fox\sandbox` base is missing. The one-time elevated
    /// setup (`enable-sandbox.ps1`) installs it.
    SandboxRuntimeNotInstalled,
    /// Windows: the ACP bridge cannot be set up: `acp-bridge\bridge.mjs` is
    /// missing from the managed runtime or differs from the copy Fox embeds,
    /// or no srt sandbox SID is recorded for the pipe DACL. srt 0.0.80 does
    /// not deliver stdin to the sandboxed process on Windows, so there is no
    /// stdio fallback (docs/sandbox.md 7.12).
    SandboxBridgeUnavailable,
    /// A file the Agent says it delivered is not a readable regular file
    /// inside an authorized writable folder, as seen from outside the sandbox
    /// ([`verify_delivered_file`]).
    DeliveredFileRejected,
}

impl SandboxErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::SandboxNotInstalled => "sandbox_not_installed",
            Self::SandboxDependencyMissing => "sandbox_dependency_missing",
            Self::SandboxPolicyInvalid => "sandbox_policy_invalid",
            Self::SandboxBackendNotInstalled => "sandbox_backend_not_installed",
            Self::SandboxPathNotOwned => "sandbox_path_not_owned",
            Self::SandboxPathExposed => "sandbox_path_exposed",
            Self::SandboxRuntimeNotInstalled => "sandbox_runtime_not_installed",
            Self::SandboxBridgeUnavailable => "sandbox_bridge_unavailable",
            Self::DeliveredFileRejected => "delivered_file_rejected",
        }
    }
}

/// Structured refusal. Serializes as `{code, message, missing, remedy}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxError {
    pub code: SandboxErrorCode,
    pub message: String,
    /// What is missing (executables, packages), when known.
    pub missing: Vec<String>,
    /// What clears it, phrased for the user.
    pub remedy: Option<String>,
}

impl SandboxError {
    pub fn new(code: SandboxErrorCode, message: impl Into<String>) -> Self {
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

impl std::fmt::Display for SandboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)?;
        if !self.missing.is_empty() {
            write!(f, " (missing: {})", self.missing.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for SandboxError {}

// ---------------------------------------------------------------------------
// Policy -> srt settings
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

/// Read-denied roots.
///
/// * Linux / macOS: the filesystem root `/`. srt expands it to every
///   top-level directory except `/proc`, `/dev` and `/sys` (srt README,
///   "Path Syntax (Linux)"), so `/data`, `/mnt`, `/opt`, `/srv`, `/tmp`,
///   `/var`, `/workspace` ... are all unreadable unless re-allowed. Measured on
///   Linux (docs/sandbox.md section 6); unverified on macOS.
/// * Windows: none. The sandbox account already cannot open the user's
///   profile (NTFS default ACLs), and srt could not write a deny on
///   `C:\Users` / a drive root anyway: the calling user has no `WRITE_DAC`
///   there (the Windows run failed exactly so). The Host may still add deny
///   paths the user owns (e.g. a secrets folder inside an authorized project);
///   the launcher checks `WRITE_DAC` on each and on its parent. The declaration
///   reports [`ReadScope::UsersGroupReadable`].
pub fn default_deny_read_roots(os: HostOs) -> Vec<PathBuf> {
    match os {
        HostOs::Linux | HostOs::MacOs | HostOs::Other => vec!["/".into()],
        HostOs::Windows => Vec::new(),
    }
}

/// Windows system locations whose stock DACL lets the Users group read and
/// execute (`%ProgramFiles%`, `%ProgramFiles(x86)%`, `%ProgramW6432%`,
/// `%SystemRoot%`). The sandbox account can read them without any ACE, so Fox
/// generates none for paths under them (a Node or srt installed machine-wide
/// there needs no grant; the user could not write one anyway). Defaults are
/// used where a variable is unset (and off Windows, for tests).
pub fn windows_users_readable_roots() -> Vec<PathBuf> {
    let var = |name: &str, default: &str| std::env::var_os(name).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(default));
    let mut roots = vec![
        var("ProgramFiles", r"C:\Program Files"),
        var("ProgramFiles(x86)", r"C:\Program Files (x86)"),
        var("SystemRoot", r"C:\Windows"),
    ];
    if let Some(w6432) = std::env::var_os("ProgramW6432") {
        roots.push(PathBuf::from(w6432));
    }
    let mut seen = BTreeSet::new();
    roots.retain(|root| seen.insert(normalized(root, HostOs::Windows)));
    roots
}

/// Minimal system paths re-allowed (read-only) under `denyRead: ["/"]`.
///
/// Linux (measured with pi-acp + pi + bash/coreutils/curl/git/python inside,
/// docs/sandbox.md section 6): `/usr` and the top-level library/binary
/// directories (symlinks into `/usr` on merged-usr systems, real directories
/// elsewhere), and only the parts of `/etc` that name resolution, the dynamic
/// loader, TLS trust, user lookup and Debian alternatives need. `/etc` is NOT
/// allowed as a whole. Paths that do not exist on the machine are dropped by
/// the launcher before the settings are written.
///
/// macOS: unverified starting set (system frameworks, dyld cache, `/etc` and
/// `/var` live under `/private`). Windows: empty (system directories are not
/// denied there).
pub fn default_system_read_only(os: HostOs) -> Vec<PathBuf> {
    let list: &[&str] = match os {
        HostOs::Linux | HostOs::Other => &[
            "/usr",
            "/bin",
            "/sbin",
            "/lib",
            "/lib32",
            "/lib64",
            "/libx32",
            "/etc/alternatives",
            "/etc/ca-certificates",
            "/etc/crypto-policies",
            "/etc/gai.conf",
            "/etc/group",
            "/etc/host.conf",
            "/etc/hosts",
            "/etc/ld.so.cache",
            "/etc/ld.so.conf",
            "/etc/ld.so.conf.d",
            "/etc/localtime",
            "/etc/nsswitch.conf",
            "/etc/passwd",
            "/etc/pki",
            "/etc/resolv.conf",
            "/etc/ssl",
        ],
        HostOs::MacOs => &[
            "/usr",
            "/bin",
            "/sbin",
            "/System",
            "/Library/Apple",
            "/Library/Preferences/SystemConfiguration",
            "/private/etc/hosts",
            "/private/etc/resolv.conf",
            "/private/etc/ssl",
            "/private/etc/passwd",
            "/private/etc/group",
            "/private/etc/localtime",
            "/private/var/db/timezone",
            "/private/var/db/dyld",
            "/dev",
        ],
        HostOs::Windows => &[],
    };
    list.iter().map(PathBuf::from).collect()
}

/// What the read boundary actually protects. The UI wording is chosen from
/// this, never from the mode alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadScope {
    /// Everything outside the authorized directories, the private HOME, the
    /// session temp directory, the backend/Node/srt install paths and the
    /// listed system paths is unreadable (`denyRead: ["/"]`).
    AuthorizedOnly,
    /// Only the user-profile roots (and any Host-added roots) are unreadable;
    /// other drives and folders remain readable ("partial").
    UserDirsOnly,
    /// Windows (srt-win, separate `srt-sandbox` account): the sandbox reads the
    /// authorized folders, its private HOME / temp, Fox's runtime, and every
    /// location whose DACL is open to all local accounts (system directories;
    /// on other fixed drives, folders that inherit e.g. `Authenticated
    /// Users: Modify` from a stock drive root, which are then writable too).
    /// The user's own profile folders are protected as far as their actual
    /// DACLs go (stock profile DACLs admit only the user, SYSTEM and
    /// Administrators; a folder the user opened up is readable). Product
    /// decision B: warn, never claim directory isolation.
    UsersGroupReadable,
    /// No read boundary (full access, or nothing denied).
    None,
}

impl ReadScope {
    /// UI text (Chinese product copy) for this scope.
    pub fn ui_label(self, verified: bool) -> &'static str {
        match (self, verified) {
            (Self::AuthorizedOnly, true) => "读取限定在授权文件夹（另含运行必需的系统路径、私有 HOME 和会话临时目录）",
            (Self::AuthorizedOnly, false) => {
                "读取限定在授权文件夹（另含运行必需的系统路径、私有 HOME 和会话临时目录；本平台待实测）"
            }
            (Self::UserDirsOnly, true) => "部分：仅用户目录受保护",
            (Self::UserDirsOnly, false) => "部分：仅用户目录受保护（本平台待实测）",
            (Self::UsersGroupReadable, true) => {
                "沙箱账户可读取授权文件夹和系统中对所有用户开放的位置；个人文件夹按实际 ACL 保护；其他盘上对所有用户开放的目录可能可读写"
            }
            (Self::UsersGroupReadable, false) => {
                "沙箱账户可读取授权文件夹和系统中对所有用户开放的位置；个人文件夹按实际 ACL 保护；其他盘上对所有用户开放的目录可能可读写（本平台待实测）"
            }
            (Self::None, _) => "不限制读取",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedDir {
    pub path: PathBuf,
    pub writable: bool,
}

/// A private HOME together with the root all Fox private HOMEs live under.
/// Built by [`PrivateHomes::home_for`]; the root lets
/// [`SandboxPolicy::validate`] refuse an authorized directory that would
/// expose another project's (or another backend's) private HOME.
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

/// What one sandboxed session may touch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPolicy {
    /// The session's authorized directories (from the Host's authorization
    /// snapshot). Read-only entries are readable but not writable.
    pub authorized: Vec<AuthorizedDir>,
    /// Fox-managed HOME for this project and this backend (its own login
    /// store, history and caches live here, never in the user's real home).
    /// Readable and writable. Shared by the sessions of the same project and
    /// backend.
    pub private_home: PathBuf,
    /// Root of every Fox private HOME (`<fox_data>/sandbox-homes`). No
    /// authorized directory may contain it or lie inside it.
    pub private_home_root: PathBuf,
    /// This session's temp directory (TMPDIR/TMP/TEMP), set by the launcher,
    /// removed when the session ends. Readable and writable.
    pub session_tmp: Option<PathBuf>,
    /// System paths re-allowed read-only under a `/` deny
    /// ([`default_system_read_only`]).
    pub system_read_only: Vec<PathBuf>,
    /// Extra read-only paths the backend needs to run: its Fox-managed install
    /// directory, the Node runtime, the srt package. The launcher adds the
    /// last two itself.
    pub extra_read_only: Vec<PathBuf>,
    /// Outbound network allowlist (see [`resolve_allowed_domains`]).
    pub allowed_domains: Vec<String>,
    /// Read-denied roots, [`default_deny_read_roots`] plus Host additions.
    pub deny_read_roots: Vec<PathBuf>,
    /// Windows only: system locations the Users group can already read
    /// ([`windows_users_readable_roots`]); no ACE is generated for readable
    /// roots under them. Empty on other platforms.
    #[serde(default)]
    pub users_readable_roots: Vec<PathBuf>,
}

fn is_absolute_for(path: &Path, os: HostOs) -> bool {
    match os {
        HostOs::Windows => {
            let text = path.to_string_lossy();
            let bytes = text.as_bytes();
            (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && (bytes[2] == b'\\' || bytes[2] == b'/'))
                || text.starts_with(r"\\")
        }
        _ => path.to_string_lossy().starts_with('/'),
    }
}

pub(crate) fn normalized(path: &Path, os: HostOs) -> String {
    let mut text = path.to_string_lossy().into_owned();
    if os == HostOs::Windows {
        text = text.replace('/', "\\").to_ascii_lowercase();
        while text.len() > 3 && text.ends_with('\\') {
            text.pop();
        }
    } else {
        while text.len() > 1 && text.ends_with('/') {
            text.pop();
        }
    }
    text
}

/// `inner` equals `outer` or lies beneath it (lexically; the Host passes
/// canonical paths).
pub(crate) fn within(inner: &Path, outer: &Path, os: HostOs) -> bool {
    let inner = normalized(inner, os);
    let outer = normalized(outer, os);
    let sep = if os == HostOs::Windows { '\\' } else { '/' };
    inner == outer || inner.starts_with(&format!("{}{sep}", outer.trim_end_matches(sep)))
}

/// `C:\`, `C:`, `\\server\share` (with or without a trailing separator).
pub(crate) fn is_volume_root(path: &Path) -> bool {
    let text = normalized(path, HostOs::Windows);
    let bytes = text.as_bytes();
    if bytes.len() <= 3 && bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return true;
    }
    if let Some(rest) = text.strip_prefix(r"\\") {
        return rest.trim_end_matches('\\').split('\\').filter(|part| !part.is_empty()).count() <= 2;
    }
    false
}

// ---------------------------------------------------------------------------
// Path normalization (before overlap checks and before writing settings)
// ---------------------------------------------------------------------------

fn host_canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    // Windows: resolves symlinks and junctions (GetFinalPathNameByHandle) and
    // drops the `\\?\` prefix whenever the result is an ordinary drive path.
    #[cfg(windows)]
    {
        dunce::canonicalize(path)
    }
    #[cfg(not(windows))]
    {
        std::fs::canonicalize(path)
    }
}

/// Windows: what is left starting with `\\` after simplification is a UNC
/// share, a device path or a verbatim path that cannot be expressed as a
/// drive path. srt-win cannot place ACEs there safely; refused.
fn refuse_device_path(original: &Path, resolved: PathBuf) -> Result<PathBuf, SandboxError> {
    if cfg!(windows) && resolved.to_string_lossy().starts_with(r"\\") {
        return Err(SandboxError::new(
            SandboxErrorCode::SandboxPolicyInvalid,
            format!("{} resolves to a UNC or device path ({}); only folders on a local drive can be used", original.display(), resolved.display()),
        ));
    }
    Ok(resolved)
}

/// Canonical form of a path that must exist (an authorized folder, a
/// delivered file): absolute, symlinks / junctions resolved, Windows
/// `\\?\` prefix removed, UNC and device paths refused. Never compared by
/// text alone.
pub fn canonical_existing_path(path: &Path) -> Result<PathBuf, SandboxError> {
    let invalid = |message: String| SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, message);
    if !is_absolute_for(path, HostOs::current()) {
        return Err(invalid(format!("path must be absolute: {}", path.display())));
    }
    let resolved = host_canonicalize(path).map_err(|error| invalid(format!("{} does not exist or cannot be resolved: {error}", path.display())))?;
    refuse_device_path(path, resolved)
}

/// Canonical form of a Fox-managed path that may not exist yet (private HOME
/// and its root, session temp, state root, runtime and install paths, deny
/// roots): the deepest existing ancestor is canonicalized and the missing
/// components are appended as written. A missing component may not be `..`.
pub fn canonical_managed_path(path: &Path) -> Result<PathBuf, SandboxError> {
    let invalid = |message: String| SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, message);
    if !is_absolute_for(path, HostOs::current()) {
        return Err(invalid(format!("path must be absolute: {}", path.display())));
    }
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match host_canonicalize(&current) {
            Ok(base) => {
                let mut resolved = refuse_device_path(path, base)?;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) => {
                let (Some(name), Some(parent)) = (current.file_name(), current.parent()) else {
                    return Err(invalid(format!("{} cannot be resolved: {error}", path.display())));
                };
                if current.ends_with("..") {
                    return Err(invalid(format!("{} contains `..` below a missing directory", path.display())));
                }
                missing.push(name.to_os_string());
                current = parent.to_path_buf();
            }
        }
    }
}

fn literal(path: &Path) -> Value {
    // Literal entries are never read as globs, so a folder named `[WIP] x`
    // means exactly that folder (srt >= 0.0.80).
    json!({ "path": path.to_string_lossy(), "literal": true })
}

impl SandboxPolicy {
    pub fn new(
        authorized: Vec<AuthorizedDir>,
        private_home: impl Into<PrivateHome>,
        extra_read_only: Vec<PathBuf>,
        allowed_domains: Vec<String>,
        os: HostOs,
    ) -> Self {
        let home = private_home.into();
        Self {
            authorized,
            private_home: home.path,
            private_home_root: home.root,
            session_tmp: None,
            system_read_only: default_system_read_only(os),
            extra_read_only,
            allowed_domains,
            deny_read_roots: default_deny_read_roots(os),
            users_readable_roots: if os == HostOs::Windows { windows_users_readable_roots() } else { Vec::new() },
        }
    }

    /// Paths the policy itself grants (authorized, private HOME, session temp,
    /// backend install paths), without the system paths.
    fn granted_roots(&self) -> Vec<&Path> {
        let mut roots: Vec<&Path> = self.authorized.iter().map(|dir| dir.path.as_path()).collect();
        roots.push(&self.private_home);
        roots.extend(self.session_tmp.as_deref());
        roots.extend(self.extra_read_only.iter().map(PathBuf::as_path));
        roots
    }

    fn readable_roots(&self) -> Vec<&Path> {
        let mut roots = self.granted_roots();
        roots.extend(self.system_read_only.iter().map(PathBuf::as_path));
        roots
    }

    fn writable_roots(&self) -> Vec<&Path> {
        let mut roots: Vec<&Path> =
            self.authorized.iter().filter(|dir| dir.writable).map(|dir| dir.path.as_path()).collect();
        roots.push(&self.private_home);
        roots.extend(self.session_tmp.as_deref());
        roots
    }

    /// What the read boundary of this policy protects on `os`. A `/` deny is
    /// [`ReadScope::AuthorizedOnly`]; a narrower deny list on Linux / macOS is
    /// [`ReadScope::UserDirsOnly`]. On Windows the boundary is the separate
    /// sandbox account plus NTFS, whatever the deny list:
    /// [`ReadScope::UsersGroupReadable`].
    pub fn read_scope(&self, os: HostOs) -> ReadScope {
        if os == HostOs::Windows {
            return ReadScope::UsersGroupReadable;
        }
        if self.deny_read_roots.is_empty() {
            return ReadScope::None;
        }
        let denies_root = self.deny_read_roots.iter().any(|root| normalized(root, os) == "/");
        if os != HostOs::Windows && denies_root {
            ReadScope::AuthorizedOnly
        } else {
            ReadScope::UserDirsOnly
        }
    }

    /// Whether a path is readable inside the sandbox this policy builds.
    ///
    /// On Windows this is what the policy grants or what the Users group
    /// reads by default (system locations); a folder elsewhere that happens
    /// to have a broad DACL is readable too but cannot be known lexically
    /// (see [`crate::winacl::audit_broad_exposure`]).
    pub fn is_readable(&self, path: &Path, os: HostOs) -> bool {
        if os == HostOs::Windows {
            if self.readable_roots().iter().any(|root| within(path, root, os)) {
                return true;
            }
            if self.deny_read_roots.iter().any(|root| within(path, root, os)) {
                return false;
            }
            return self.users_readable_roots.iter().any(|root| within(path, root, os));
        }
        let denied = self.deny_read_roots.iter().any(|root| within(path, root, os));
        !denied || self.readable_roots().iter().any(|root| within(path, root, os))
    }

    pub fn validate(&self, os: HostOs) -> Result<(), SandboxError> {
        let invalid = |message: String| Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, message));
        if self.authorized.is_empty() {
            return invalid("no authorized directory".into());
        }
        // Linux / macOS: srt reads everything not denied. Windows: the
        // sandbox account reads only what NTFS opens to it, so no deny root is
        // needed there.
        if self.deny_read_roots.is_empty() && os != HostOs::Windows {
            return invalid("no read-denied roots: srt would allow reading the whole disk".into());
        }
        if os == HostOs::Windows {
            // srt-win would add an ACE on these; on a drive root or share
            // root that rewrites DACLs volume-wide (srt ambient.rs) and a
            // normal user may not do it anyway.
            let ace_targets = self.granted_roots().into_iter().chain(self.deny_read_roots.iter().map(PathBuf::as_path));
            for path in ace_targets {
                if is_volume_root(path) {
                    return invalid(format!("{} is a drive or share root; authorize a folder inside it", path.display()));
                }
            }
        }
        let all = self
            .readable_roots()
            .into_iter()
            .chain(self.deny_read_roots.iter().map(PathBuf::as_path))
            .chain(std::iter::once(self.private_home_root.as_path()));
        for path in all {
            if !is_absolute_for(path, os) {
                return invalid(format!("path must be absolute: {}", path.display()));
            }
            if path.to_string_lossy().contains('\0') {
                return invalid("path contains NUL".into());
            }
        }
        // Re-allowing a deny root itself (or something above it) would undo
        // the deny: `allowRead` beats `denyRead`.
        for root in self.readable_roots() {
            for deny in &self.deny_read_roots {
                if within(deny, root, os) {
                    return invalid(format!(
                        "{} would re-open the read-denied root {}",
                        root.display(),
                        deny.display()
                    ));
                }
            }
        }
        // Private HOMEs: this one must sit under the root, and no authorized
        // directory may reach any of them (another project's login and
        // history live there) or the session temp directory.
        if !within(&self.private_home, &self.private_home_root, os) {
            return invalid(format!(
                "private HOME {} is outside the private HOME root {}",
                self.private_home.display(),
                self.private_home_root.display()
            ));
        }
        for dir in &self.authorized {
            let reserved = std::iter::once(&self.private_home_root).chain(self.session_tmp.iter());
            for fox in reserved {
                if within(&dir.path, fox, os) || within(fox, &dir.path, os) {
                    return invalid(format!(
                        "authorized directory {} overlaps Fox's private directory {}",
                        dir.path.display(),
                        fox.display()
                    ));
                }
            }
        }
        for domain in &self.allowed_domains {
            if !valid_domain(domain) {
                return invalid(format!("malformed network domain {domain:?}"));
            }
        }
        Ok(())
    }

    /// Readable roots that need an ACE: on Windows, the ones not under a
    /// location the Users group already reads.
    fn readable_roots_needing_grant(&self) -> Vec<&Path> {
        self.readable_roots()
            .into_iter()
            .filter(|root| !self.users_readable_roots.iter().any(|system| within(root, system, HostOs::Windows)))
            .collect()
    }

    /// This policy with every Host-supplied path in canonical form
    /// (symlinks and Windows junctions resolved, `\\?\` removed, UNC /
    /// device paths refused): authorized folders must exist
    /// ([`canonical_existing_path`]); Fox-managed paths may not exist yet
    /// ([`canonical_managed_path`]). System read-only paths are Fox's own
    /// measured list and stay as written. Run on the host the sandbox starts
    /// on; [`SrtLauncher::wrap`] does it before validating.
    pub fn canonicalized(&self) -> Result<Self, SandboxError> {
        let mut policy = self.clone();
        for dir in &mut policy.authorized {
            dir.path = canonical_existing_path(&dir.path).map_err(|error| {
                SandboxError::new(error.code, format!("authorized folder: {}", error.message))
                    .remedy("授权文件夹必须存在；请重新选择文件夹")
            })?;
        }
        policy.private_home = canonical_managed_path(&policy.private_home)?;
        policy.private_home_root = canonical_managed_path(&policy.private_home_root)?;
        if let Some(tmp) = &policy.session_tmp {
            policy.session_tmp = Some(canonical_managed_path(tmp)?);
        }
        policy.extra_read_only = policy.extra_read_only.iter().map(|path| canonical_managed_path(path)).collect::<Result<_, _>>()?;
        policy.deny_read_roots = policy.deny_read_roots.iter().map(|path| canonical_managed_path(path)).collect::<Result<_, _>>()?;
        Ok(policy)
    }

    /// The three path lists written into the srt settings, deduplicated, in
    /// the order written. The UI declaration is generated from the same lists
    /// ([`SandboxDeclaration::declare`]), so it cannot drift from the file.
    pub fn settings_roots(&self) -> SettingsRoots {
        let dedup = |paths: Vec<&Path>| {
            let mut seen = BTreeSet::new();
            paths.into_iter().filter(|path| seen.insert(path.to_path_buf())).map(Path::to_path_buf).collect::<Vec<_>>()
        };
        SettingsRoots {
            allow_read: dedup(self.readable_roots_needing_grant()),
            allow_write: dedup(self.writable_roots()),
            deny_read: dedup(self.deny_read_roots.iter().map(PathBuf::as_path).collect()),
        }
    }

    /// The srt settings document for this policy.
    pub fn to_srt_settings(&self) -> Value {
        let roots = self.settings_roots();
        let allow_read: Vec<Value> = roots.allow_read.iter().map(|root| literal(root)).collect();
        let allow_write: Vec<Value> = roots.allow_write.iter().map(|root| literal(root)).collect();
        json!({
            "network": {
                "allowedDomains": self.allowed_domains,
                "deniedDomains": [],
                "allowLocalBinding": false,
            },
            "filesystem": {
                "denyRead": roots.deny_read.iter().map(|root| literal(root)).collect::<Vec<_>>(),
                "allowRead": allow_read,
                "allowWrite": allow_write,
                "denyWrite": [],
            },
            "enableWeakerNestedSandbox": false,
            "allowNestedUserNamespaces": false,
        })
    }
}

/// The path lists of one srt settings file ([`SandboxPolicy::settings_roots`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsRoots {
    pub allow_read: Vec<PathBuf>,
    pub allow_write: Vec<PathBuf>,
    pub deny_read: Vec<PathBuf>,
}

/// Granularity of the Fox-managed private HOME (product decision
/// 2026-10-19): one per project x backend, shared by that project's sessions
/// of that backend, so the backend's native login and history carry over
/// between those sessions and never across projects or backends. Temp
/// directories are per session regardless (see [`SrtLauncher`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivateHomeScope {
    #[default]
    PerProjectPerBackend,
}

/// Where the private HOMEs live: `<root>/<project uuid>/<backend id>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivateHomes {
    /// `<fox_data>/sandbox-homes`; Fox-owned, never an authorized directory.
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
    if groups.len() != 5
        || groups.iter().zip(lengths).any(|(group, len)| group.len() != len || !group.chars().all(|c| c.is_ascii_hexdigit()))
    {
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
    /// `project_uuid`. An empty or malformed UUID is refused
    /// (`sandbox_policy_invalid`); nothing is derived from paths or names.
    pub fn home_for(&self, project_uuid: &str, backend: AgentBackend) -> Result<PrivateHome, SandboxError> {
        let uuid = canonical_project_uuid(project_uuid).ok_or_else(|| {
            SandboxError::new(
                SandboxErrorCode::SandboxPolicyInvalid,
                format!("project id {project_uuid:?} is not a valid project UUID"),
            )
        })?;
        if backend == AgentBackend::Fox {
            return Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, "FoxBackend has no sandboxed private HOME"));
        }
        let path = match self.scope {
            PrivateHomeScope::PerProjectPerBackend => self.root.join(&uuid).join(backend.as_str()),
        };
        Ok(PrivateHome { root: self.root.clone(), path })
    }
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Where Fox installed srt and which Node runs it (Fox-managed, never the
/// user's npx cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SrtInstall {
    pub node: PathBuf,
    /// `<prefix>/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js`
    pub cli_js: PathBuf,
}

impl SrtInstall {
    /// `npm install --prefix <prefix> @anthropic-ai/sandbox-runtime@<SRT_VERSION>`
    pub fn under_prefix(node: PathBuf, prefix: &Path) -> Self {
        Self {
            node,
            cli_js: prefix.join("node_modules").join("@anthropic-ai").join("sandbox-runtime").join("dist").join("cli.js"),
        }
    }

    /// The package root (`.../sandbox-runtime`), which must be readable inside
    /// the sandbox (Linux runs its `vendor/seccomp` helper there).
    pub fn package_root(&self) -> Option<PathBuf> {
        self.cli_js.parent()?.parent().map(Path::to_path_buf)
    }
}

/// System access used by preflight; faked in tests.
pub trait SystemProbe {
    fn os(&self) -> HostOs;
    fn exists(&self, path: &Path) -> bool;
    fn which(&self, program: &str) -> Option<PathBuf>;
    /// Stdout of a successful run, `None` when it failed or could not start.
    fn run(&self, program: &Path, args: &[&str]) -> Option<String>;
    fn read_to_string(&self, path: &Path) -> Option<String>;
}

/// The real system.
pub struct RealProbe;

impl SystemProbe for RealProbe {
    fn os(&self) -> HostOs {
        HostOs::current()
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn which(&self, program: &str) -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        let names: Vec<String> = if cfg!(windows) {
            vec![format!("{program}.exe"), format!("{program}.cmd"), program.to_owned()]
        } else {
            vec![program.to_owned()]
        };
        std::env::split_paths(&path)
            .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
            .find(|candidate| candidate.is_file())
    }

    fn run(&self, program: &Path, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn read_to_string(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }
}

/// Parse `v22.12.0` / `0.0.80` / `bubblewrap 0.12.0`.
pub fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let token = text.split_whitespace().find(|word| word.trim_start_matches('v').starts_with(|c: char| c.is_ascii_digit()))?;
    let mut parts = token.trim_start_matches('v').split(['.', '-', '+']);
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|part| part.parse().ok()).unwrap_or(0);
    Some((major, minor, patch))
}

/// Everything a launcher needs, established by [`preflight`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxReady {
    pub os: HostOs,
    pub install: SrtInstall,
    pub node_version: String,
    pub srt_version: String,
    /// Resolved platform helpers (bwrap, socat, rg, sandbox-exec).
    pub helpers: Vec<(String, PathBuf)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowsInstallState {
    Installed,
    /// `windows-install` must run once (one UAC prompt) before sandboxed mode.
    NotInstalled,
    NotWindows,
}

/// Registry key `windows-install` writes (srt README "Setup").
pub const WINDOWS_INSTALL_REGISTRY_KEY: &str = r"HKLM\SOFTWARE\sandbox-runtime";

pub fn windows_install_state(probe: &dyn SystemProbe) -> WindowsInstallState {
    if probe.os() != HostOs::Windows {
        return WindowsInstallState::NotWindows;
    }
    let reg = probe.which("reg").unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\reg.exe"));
    match probe.run(&reg, &["query", WINDOWS_INSTALL_REGISTRY_KEY]) {
        Some(_) => WindowsInstallState::Installed,
        None => WindowsInstallState::NotInstalled,
    }
}

/// Command the installer runs the first time the user picks sandboxed mode on
/// Windows (self-elevates, one UAC prompt; idempotent).
pub fn windows_install_command(install: &SrtInstall) -> (PathBuf, Vec<String>) {
    (install.node.clone(), vec![install.cli_js.to_string_lossy().into_owned(), "windows-install".into()])
}

/// Command Fox's uninstaller must run (removes the WFP filters, the
/// `srt-sandbox` account and its registry key; one UAC prompt).
pub fn windows_uninstall_command(install: &SrtInstall) -> (PathBuf, Vec<String>) {
    (install.node.clone(), vec![install.cli_js.to_string_lossy().into_owned(), "windows-uninstall".into()])
}

/// Check that srt can run here. Any failure is a refusal for sandboxed mode;
/// callers must not fall back to full access.
pub fn preflight(install: &SrtInstall, probe: &dyn SystemProbe) -> Result<SandboxReady, SandboxError> {
    use SandboxErrorCode::*;
    let os = probe.os();
    if os == HostOs::Other {
        return Err(SandboxError::new(SandboxUnavailable, "srt supports macOS, Linux and Windows only"));
    }
    if !probe.exists(&install.cli_js) {
        return Err(SandboxError::new(SandboxDependencyMissing, "sandbox runtime is not installed")
            .missing(format!("{SRT_PACKAGE}@{SRT_VERSION}"))
            .remedy("reinstall Fox's sandbox component"));
    }
    let srt_version = install
        .package_root()
        .and_then(|root| probe.read_to_string(&root.join("package.json")))
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|package| package.get("version").and_then(Value::as_str).map(String::from))
        .unwrap_or_default();
    match parse_version(&srt_version) {
        Some(version) if version >= SRT_MIN_VERSION => {}
        _ => {
            return Err(SandboxError::new(
                SandboxDependencyMissing,
                format!("sandbox runtime version {srt_version:?} is older than {SRT_VERSION}"),
            )
            .missing(format!("{SRT_PACKAGE}@{SRT_VERSION}")))
        }
    }
    let node_version = probe.run(&install.node, &["--version"]).map(|out| out.trim().to_owned());
    match node_version.as_deref().and_then(parse_version) {
        Some(version) if version >= SRT_MIN_NODE => {}
        _ => {
            return Err(SandboxError::new(
                SandboxDependencyMissing,
                format!(
                    "sandbox runtime needs Node >= {}.{}.{} (found {})",
                    SRT_MIN_NODE.0,
                    SRT_MIN_NODE.1,
                    SRT_MIN_NODE.2,
                    node_version.as_deref().unwrap_or("none")
                ),
            )
            .missing("node>=22.12.0"))
        }
    }
    let mut helpers = Vec::new();
    match os {
        HostOs::Linux => {
            let mut missing = Vec::new();
            for tool in ["bwrap", "socat", "rg"] {
                match probe.which(tool) {
                    Some(path) => helpers.push((tool.to_owned(), path)),
                    None => missing.push(tool),
                }
            }
            if !missing.is_empty() {
                let mut error = SandboxError::new(SandboxDependencyMissing, "sandbox needs bubblewrap, socat and ripgrep")
                    .remedy("install the packages bubblewrap, socat and ripgrep");
                for tool in missing {
                    error = error.missing(tool);
                }
                return Err(error);
            }
            let bwrap = helpers[0].1.clone();
            if probe
                .run(&bwrap, &["--ro-bind", "/", "/", "--unshare-user", "--unshare-net", "--", "/bin/true"])
                .is_none()
            {
                return Err(SandboxError::new(
                    SandboxUnavailable,
                    "bubblewrap cannot create user/network namespaces on this system",
                )
                .remedy("allow unprivileged user namespaces (Ubuntu 24.04+: kernel.apparmor_restrict_unprivileged_userns=0)"));
            }
        }
        HostOs::MacOs => {
            let sandbox_exec = PathBuf::from("/usr/bin/sandbox-exec");
            if !probe.exists(&sandbox_exec) {
                return Err(SandboxError::new(SandboxUnavailable, "sandbox-exec is not available").missing("sandbox-exec"));
            }
            helpers.push(("sandbox-exec".into(), sandbox_exec));
            match probe.which("rg") {
                Some(path) => helpers.push(("rg".into(), path)),
                None => {
                    return Err(SandboxError::new(SandboxDependencyMissing, "sandbox needs ripgrep")
                        .missing("rg")
                        .remedy("install ripgrep"))
                }
            }
        }
        HostOs::Windows => {
            if windows_install_state(probe) != WindowsInstallState::Installed {
                return Err(SandboxError::new(
                    SandboxNotInstalled,
                    "the Windows sandbox has not been set up on this computer",
                )
                .remedy("run the one-time sandbox setup (one administrator prompt)"));
            }
        }
        HostOs::Other => unreachable!(),
    }
    Ok(SandboxReady { os, install: install.clone(), node_version: node_version.unwrap_or_default(), srt_version, helpers })
}

// ---------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------

/// Per-session sandbox state on disk: the settings file (outside the
/// sandbox) and the session temp directory (inside it). Both are removed on
/// drop, so they go away with the session's connection whatever path ends
/// it. The private HOME is per project x backend and is kept.
#[derive(Debug)]
pub struct SandboxSession {
    dir: PathBuf,
    settings_path: PathBuf,
    tmp: Option<PathBuf>,
}

impl SandboxSession {
    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The session temp directory (TMPDIR inside the sandbox).
    pub fn tmp_dir(&self) -> Option<&Path> {
        self.tmp.as_deref()
    }

    /// Remove the session directories now (drop does the same).
    pub fn cleanup(self) {}

    #[cfg(test)]
    pub(crate) fn for_test(dir: PathBuf) -> Self {
        Self { settings_path: dir.join("srt-settings.json"), dir, tmp: None }
    }
}

impl Drop for SandboxSession {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
        if let Some(tmp) = &self.tmp {
            let _ = std::fs::remove_dir_all(tmp);
        }
    }
}

#[derive(Debug)]
pub struct SandboxedLaunch {
    pub spec: LaunchSpec,
    pub session: SandboxSession,
    /// The policy as actually written into the srt settings (canonical paths,
    /// session temp, srt / Node runtime directories added).
    pub effective_policy: SandboxPolicy,
    /// Windows: ancestors that need Fox's attribute-read ACE before srt
    /// starts ([`WindowsAcePlan::ancestors`]). Empty elsewhere.
    pub windows_ancestor_aces: Vec<PathBuf>,
    /// The backend's own variables (private HOME, temp, PATH, the spec's
    /// session variables). They travel inside the command (Windows `set` in
    /// `-c`, Linux / macOS `env K=V` after `--`), never in the srt broker's
    /// environment; `spec.session_env` holds only what srt itself needs.
    pub backend_env: Vec<(String, String)>,
    /// Windows: the ACP bridge endpoint (named pipe + one-shot token) the
    /// sandboxed bridge connects back to. `None` elsewhere (stdio works).
    pub acp_bridge: Option<crate::launch::BridgeEndpoint>,
    /// Startup report: srt state directories found inside the private HOME
    /// (`AppData\Local\sandbox-runtime`), left by earlier versions that gave
    /// the srt broker the private `LOCALAPPDATA`. Reported only; see
    /// [`remove_stray_srt_state`].
    pub stray_srt_state: Vec<PathBuf>,
}

/// Wraps a backend launch in a sandbox.
pub trait SandboxLauncher {
    fn wrap(&self, spec: &LaunchSpec, policy: &SandboxPolicy, session_id: &str) -> Result<SandboxedLaunch, SandboxError>;
}

/// srt implementation: `node <srt cli.js> --settings <per-session file> --
/// <backend program> <args>`.
pub struct SrtLauncher {
    pub ready: SandboxReady,
    /// Fox private directory for per-session settings, outside every path
    /// the sandbox may write or read.
    pub state_root: PathBuf,
    /// Parent of the per-session temp directories (`<tmp_root>/<session id>`).
    /// Defaults to `<state_root>/sandbox-tmp`.
    pub tmp_root: PathBuf,
    /// Windows: Fox's managed runtime directory,
    /// `%ProgramFiles%\Fox\sandbox-runtime\<version>`
    /// ([`crate::winruntime::windows_runtime_root`]; administrator-owned,
    /// Users read+execute), where Node, srt and the backends are installed by
    /// the one-time elevated setup. Checked by
    /// [`crate::winruntime::check_windows_runtime_root`] before every launch;
    /// runtime paths under it need no ACE (Users can read them); runtime paths
    /// anywhere else are refused with `sandbox_path_not_owned`. Unused on
    /// Linux / macOS.
    pub managed_runtime_root: Option<PathBuf>,
    /// Windows ACL queries (ownership / `WRITE_DAC` preflight, broad-DACL
    /// check of Fox's private directories). Real API by default.
    pub windows_acl: std::sync::Arc<dyn crate::winacl::WindowsAclProbe>,
    /// Windows: where the sandbox account cannot `lstat` ancestors on its own
    /// (profile root, `%ProgramData%\Fox\sandbox`). Defaults to this
    /// process's environment on Windows, empty elsewhere.
    pub ancestor_bases: crate::winancestor::AncestorBases,
    /// Windows: per-user directories a managed runtime may not live in
    /// ([`crate::winruntime::windows_user_dirs`] by default).
    pub windows_user_dirs: Vec<PathBuf>,
    /// Windows: the installed bridge script (default
    /// `<managed runtime>\acp-bridge\bridge.mjs`); must equal
    /// [`crate::launch::BRIDGE_SCRIPT`].
    pub acp_bridge_script: Option<PathBuf>,
    /// Windows: srt sandbox account SID for the bridge pipe DACL. `None` =
    /// read `HKLM\SOFTWARE\sandbox-runtime` `SandboxUserSid` at launch.
    pub srt_sandbox_sid: Option<String>,
}

/// What srt-win will be asked to do on Windows, worked out (and checked) by
/// [`SrtLauncher::windows_ace_plan`] before anything is written.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowsAcePlan {
    /// `acl grant` read-only ACEs (authorized read-only folders, runtime).
    pub grant_read: Vec<PathBuf>,
    /// `acl grant` modify ACEs (writable authorized folders, private HOME,
    /// session temp).
    pub grant_write: Vec<PathBuf>,
    /// `acl stamp` deny ACEs (Host-added deny paths); srt also stamps each
    /// one's parent.
    pub deny: Vec<PathBuf>,
    /// Readable without any ACE (under a Users-readable system location or
    /// the managed runtime directory).
    pub system_readable: Vec<PathBuf>,
    /// Ancestors that need Fox's attribute-read ACE for the sandbox account
    /// ([`crate::winancestor::plan_ancestor_aces`]); Fox, not srt, writes
    /// them, through [`crate::winancestor::SrtTransition::begin_session`].
    #[serde(default)]
    pub ancestors: Vec<PathBuf>,
}

fn path_sep(os: HostOs) -> &'static str {
    if os == HostOs::Windows { ";" } else { ":" }
}

fn system_path(os: HostOs) -> &'static str {
    match os {
        HostOs::Windows => r"C:\Windows\System32;C:\Windows",
        HostOs::MacOs => "/usr/bin:/bin:/usr/sbin:/sbin",
        _ => "/usr/local/bin:/usr/bin:/bin",
    }
}

/// Parent directory, also for a Windows-style path examined on another host
/// (backslash separators), so the Windows planning is testable anywhere.
pub(crate) fn lexical_parent(path: &Path) -> Option<PathBuf> {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => Some(parent.to_path_buf()),
        _ => {
            let text = path.to_string_lossy();
            let (head, _) = text.trim_end_matches('\\').rsplit_once('\\')?;
            Some(PathBuf::from(if head.len() == 2 && head.ends_with(':') { format!("{head}\\") } else { head.to_owned() }))
        }
    }
}

/// Install prefix of a runtime binary: `<x>/bin/node` -> `<x>`, else its dir.
fn runtime_root(binary: &Path) -> Option<PathBuf> {
    let dir = lexical_parent(binary)?;
    let is_bin = dir.to_string_lossy().rsplit(['/', '\\']).next().is_some_and(|name| name == "bin");
    if is_bin {
        lexical_parent(&dir)
    } else {
        Some(dir)
    }
}

impl SrtLauncher {
    pub fn new(ready: SandboxReady, state_root: PathBuf) -> Self {
        let tmp_root = state_root.join("sandbox-tmp");
        Self {
            ready,
            state_root,
            tmp_root,
            managed_runtime_root: None,
            windows_acl: std::sync::Arc::new(crate::winacl::RealWindowsAcl),
            ancestor_bases: crate::winancestor::AncestorBases::from_env(),
            windows_user_dirs: if cfg!(windows) { crate::winruntime::windows_user_dirs() } else { Vec::new() },
            acp_bridge_script: None,
            srt_sandbox_sid: None,
        }
    }

    /// Windows: where the bridge script is installed (default: under the
    /// managed runtime root).
    pub fn with_acp_bridge_script(mut self, path: PathBuf) -> Self {
        self.acp_bridge_script = Some(path);
        self
    }

    /// Windows: the srt sandbox SID for the bridge pipe DACL (tests, or a
    /// Host that already read it).
    pub fn with_srt_sandbox_sid(mut self, sid: String) -> Self {
        self.srt_sandbox_sid = Some(sid);
        self
    }

    fn bridge_script_path(&self) -> Option<PathBuf> {
        self.acp_bridge_script.clone().or_else(|| {
            self.managed_runtime_root.as_ref().map(|root| crate::launch::BRIDGE_RELATIVE_PATH.iter().fold(root.clone(), |path, part| path.join(part)))
        })
    }

    /// Windows: override the per-user directories (tests).
    pub fn with_windows_user_dirs(mut self, dirs: Vec<PathBuf>) -> Self {
        self.windows_user_dirs = dirs;
        self
    }

    /// Windows: override the ancestor traversal bases (tests, Host).
    pub fn with_ancestor_bases(mut self, bases: crate::winancestor::AncestorBases) -> Self {
        self.ancestor_bases = bases;
        self
    }

    /// Windows: where Fox installs Node, srt and the backends (see
    /// [`Self::managed_runtime_root`]).
    pub fn with_managed_runtime_root(mut self, root: PathBuf) -> Self {
        self.managed_runtime_root = Some(root);
        self
    }

    /// Replace the Windows ACL queries (tests).
    pub fn with_windows_acl(mut self, acl: std::sync::Arc<dyn crate::winacl::WindowsAclProbe>) -> Self {
        self.windows_acl = acl;
        self
    }

    /// Windows: classify every path srt-win would touch and check it before
    /// anything is written (fail-closed, each failure names its path):
    ///
    /// 0. The managed runtime directory itself
    ///    ([`crate::winruntime::check_windows_runtime_root`]): not in a user
    ///    directory (`sandbox_runtime_not_installed`), administrator-owned and
    ///    not writable by other accounts (`sandbox_path_exposed`).
    /// 1. Runtime paths the sandbox must read (backend install, Node, srt
    ///    package, other read-only extras): under a Users-readable system
    ///    location or under [`Self::managed_runtime_root`] -> no ACE;
    ///    elsewhere -> `sandbox_path_not_owned`.
    /// 2. Every ACE target (grants, denies and each deny's parent): the
    ///    current user must hold `WRITE_DAC`, else `sandbox_path_not_owned`
    ///    (srt-win would fail with Access Denied and roll back).
    /// 3. Fox's private directories (state, private HOME root, session temp
    ///    root) must not have a DACL open to every local account (read or
    ///    write), and the managed runtime root must not be writable by every
    ///    local account (code there runs as the real user):
    ///    `sandbox_path_exposed`.
    /// 4. Ancestor metadata ACEs ([`crate::winancestor::plan_ancestor_aces`]):
    ///    the current user needs `WRITE_DAC` on each, else
    ///    `sandbox_path_not_owned`.
    ///
    /// `policy` is the effective policy (session temp set).
    pub fn windows_ace_plan(&self, policy: &SandboxPolicy) -> Result<WindowsAcePlan, SandboxError> {
        use SandboxErrorCode::*;
        let os = HostOs::Windows;
        let mut plan = WindowsAcePlan::default();
        let system = |path: &Path| policy.users_readable_roots.iter().any(|root| within(path, root, os));
        let not_owned = |message: String, paths: Vec<PathBuf>| {
            let mut error = SandboxError::new(SandboxPathNotOwned, message).remedy(
                "只授权你自己创建的文件夹；Node、srt 与各后端需由 Fox 安装到 Fox 管理的运行时目录。系统目录（C:\\Users、Program Files、盘根）不能作为授权或拒读目标",
            );
            for path in paths {
                error = error.missing(path.display().to_string());
            }
            error
        };

        // 0. The managed runtime directory.
        if let Some(root) = &self.managed_runtime_root {
            let mut user_dirs = self.windows_user_dirs.clone();
            user_dirs.extend(self.ancestor_bases.profile_root.iter().cloned());
            crate::winruntime::check_windows_runtime_root(root, &user_dirs, self.windows_acl.as_ref())?;
        }
        // 1. Runtime / read-only extras.
        let mut outside = Vec::new();
        for path in &policy.extra_read_only {
            if system(path) || self.managed_runtime_root.as_ref().is_some_and(|root| within(path, root, os)) {
                plan.system_readable.push(path.clone());
            } else {
                outside.push(path.clone());
            }
        }
        if !outside.is_empty() {
            let root = self
                .managed_runtime_root
                .as_ref()
                .map(|root| root.display().to_string())
                .unwrap_or_else(|| "(not configured)".into());
            return Err(not_owned(
                format!("runtime paths outside Fox's managed runtime directory {root} cannot be granted to the sandbox account"),
                outside,
            ));
        }
        if let Some(root) = &self.managed_runtime_root {
            if policy.writable_roots().iter().any(|writable| within(root, writable, os) || within(writable, root, os)) {
                return Err(SandboxError::new(
                    SandboxPolicyInvalid,
                    format!("Fox's managed runtime directory {} overlaps a folder the sandbox may write", root.display()),
                ));
            }
        }
        // Grants from the policy itself.
        for dir in &policy.authorized {
            if dir.writable {
                plan.grant_write.push(dir.path.clone());
            } else if system(&dir.path) {
                plan.system_readable.push(dir.path.clone());
            } else {
                plan.grant_read.push(dir.path.clone());
            }
        }
        plan.grant_write.push(policy.private_home.clone());
        plan.grant_write.extend(policy.session_tmp.iter().cloned());
        plan.deny = policy.deny_read_roots.clone();

        // 2. WRITE_DAC on every ACE target (deny parents included).
        let mut targets: Vec<PathBuf> = plan.grant_read.iter().chain(&plan.grant_write).chain(&plan.deny).cloned().collect();
        for deny in &plan.deny {
            targets.extend(deny.parent().map(Path::to_path_buf));
        }
        let mut seen = BTreeSet::new();
        targets.retain(|path| seen.insert(normalized(path, os)));
        let mut refused = Vec::new();
        let mut reasons = Vec::new();
        for path in targets {
            if let Err(reason) = self.windows_acl.check_write_dac(&path) {
                reasons.push(format!("{}: {reason}", path.display()));
                refused.push(path);
            }
        }
        if !refused.is_empty() {
            return Err(not_owned(
                format!("srt would have to change permissions on paths the current user does not own: {}", reasons.join("; ")),
                refused,
            ));
        }

        // 3. Fox private directories must not be open to every account.
        let mut exposed = Vec::new();
        let private_roots = [&self.state_root, &policy.private_home_root, &self.tmp_root];
        for root in private_roots {
            match self.windows_acl.broad_access(root) {
                Ok(access) if access.any() => exposed.push(format!("{} ({})", root.display(), access.groups.join(", "))),
                Ok(_) => {}
                Err(reason) => exposed.push(format!("{} (DACL unreadable: {reason})", root.display())),
            }
        }
        if let Some(root) = &self.managed_runtime_root {
            match self.windows_acl.broad_access(root) {
                Ok(access) if access.write => exposed.push(format!("{} (writable by {})", root.display(), access.groups.join(", "))),
                Ok(_) => {}
                Err(reason) => exposed.push(format!("{} (DACL unreadable: {reason})", root.display())),
            }
        }
        if !exposed.is_empty() {
            return Err(SandboxError::new(
                SandboxPathExposed,
                format!("Fox's private directories are open to every local account: {}", exposed.join("; ")),
            )
            .remedy("Fox 状态目录应在 %LOCALAPPDATA%\\Fox，私有 HOME 与会话临时目录应在 %ProgramData%\\Fox\\sandbox\\<你的 SID>（由“启用沙箱”创建）；不要放在其他盘根下继承“所有用户可修改”权限的位置"));
        }

        // 4. Ancestor metadata ACEs (Fox writes them; same WRITE_DAC rule).
        plan.ancestors = crate::winancestor::plan_ancestor_aces(&plan, &self.ancestor_bases, self.windows_acl.as_ref());
        let mut refused = Vec::new();
        let mut reasons = Vec::new();
        for path in &plan.ancestors {
            if let Err(reason) = self.windows_acl.check_write_dac(path) {
                reasons.push(format!("{}: {reason}", path.display()));
                refused.push(path.clone());
            }
        }
        if !refused.is_empty() {
            let mut error = SandboxError::new(
                SandboxPathNotOwned,
                format!(
                    "the sandbox account must be able to read the attributes of the folders above an authorized folder (up to the user profile root), but the current user may not change permissions on: {}",
                    reasons.join("; ")
                ),
            )
            .remedy("把授权文件夹放在你有完全控制权限的位置（例如你自己的用户目录内）；若用户配置文件根目录的权限被改过，需要恢复你对它的“修改权限”");
            for path in refused {
                error = error.missing(path.display().to_string());
            }
            return Err(error);
        }
        Ok(plan)
    }

    pub fn with_tmp_root(mut self, tmp_root: PathBuf) -> Self {
        self.tmp_root = tmp_root;
        self
    }

    /// Temp directory of one session.
    pub fn session_tmp(&self, session_id: &str) -> PathBuf {
        self.tmp_root.join(session_id)
    }

    /// Policy plus what srt itself needs readable. System paths that do not
    /// exist on this machine are dropped.
    pub fn effective_policy(&self, policy: &SandboxPolicy) -> SandboxPolicy {
        let mut policy = policy.clone();
        policy.system_read_only.retain(|path| std::fs::symlink_metadata(path).is_ok());
        // Windows: the managed runtime directory is Users read+execute by
        // contract (checked in windows_ace_plan), so nothing under it is
        // listed in allowRead and srt never tries to ACE it (the user has no
        // WRITE_DAC on an administrator-owned directory).
        if self.ready.os == HostOs::Windows {
            if let Some(root) = &self.managed_runtime_root {
                if !policy.users_readable_roots.iter().any(|known| within(root, known, HostOs::Windows)) {
                    policy.users_readable_roots.push(root.clone());
                }
            }
        }
        let mut extra: Vec<PathBuf> = Vec::new();
        extra.extend(self.ready.install.package_root());
        extra.extend(runtime_root(&self.ready.install.node));
        for path in extra {
            if !policy.is_readable(&path, self.ready.os) {
                policy.extra_read_only.push(path);
            }
        }
        policy
    }

    /// Variables pointing the backend at its private HOME. Added after the
    /// launch spec's own session variables, so they win over anything the
    /// allowlist let through (HOME, CODEX_HOME ...).
    pub fn sandbox_env(&self, spec: &LaunchSpec, policy: &SandboxPolicy) -> Vec<(String, String)> {
        let os = self.ready.os;
        let home = policy.private_home.to_string_lossy().into_owned();
        let sub = |name: &str| policy.private_home.join(name).to_string_lossy().into_owned();
        // Per-session temp directory. srt itself also inherits it and puts
        // its proxy bridge sockets there, so it must be readable inside the
        // sandbox (measured: with srt's TMPDIR under a denied `/tmp` the
        // in-sandbox relay cannot reach the socket and every proxied request
        // fails with "Proxy CONNECT aborted").
        let tmp = policy
            .session_tmp
            .as_ref()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_else(|| sub("tmp"));
        let mut env = vec![
            ("HOME".to_owned(), home.clone()),
            ("XDG_CONFIG_HOME".to_owned(), sub(".config")),
            ("XDG_CACHE_HOME".to_owned(), sub(".cache")),
            ("XDG_DATA_HOME".to_owned(), sub(".local/share")),
            ("XDG_STATE_HOME".to_owned(), sub(".local/state")),
            ("TMPDIR".to_owned(), tmp.clone()),
            ("TMP".to_owned(), tmp.clone()),
            ("TEMP".to_owned(), tmp.clone()),
            // srt replaces TMPDIR inside the sandbox with this variable's value
            // (default `/tmp/claude`, which under a `/` deny is an in-memory
            // tmpfs); point it at the session temp directory too (measured).
            ("CLAUDE_CODE_TMPDIR".to_owned(), tmp),
            // Node's built-in fetch ignores HTTP(S)_PROXY unless this is set;
            // inside the sandbox the srt proxy is the only way out. Measured
            // with `node -e fetch(...)` inside srt (EAI_AGAIN without it, 200
            // with it); not yet measured with a real model provider.
            ("NODE_USE_ENV_PROXY".to_owned(), "1".to_owned()),
        ];
        if os == HostOs::Windows {
            // Auxiliary only (docs/sandbox.md 7.6): fewer realpath / lstat
            // calls on ancestors during module resolution. The ancestor ACEs
            // are what makes `lstat` of the profile chain work.
            env.push(("NODE_OPTIONS".to_owned(), crate::launch::windows_sandbox_node_options()));
            env.push(("USERPROFILE".to_owned(), home.clone()));
            env.push(("APPDATA".to_owned(), sub(r"AppData\Roaming")));
            env.push(("LOCALAPPDATA".to_owned(), sub(r"AppData\Local")));
        }
        match spec.backend {
            AgentBackend::Codex => env.push(("CODEX_HOME".to_owned(), sub(".codex"))),
            AgentBackend::Deepseek => env.push(("DSH_HOME".to_owned(), sub(".dsh"))),
            AgentBackend::Pi | AgentBackend::Fox => {}
        }
        // PATH: the backend's own bin dir, Node's bin dir, then system dirs.
        // Never the user's PATH (nvm, ~/.local/bin, npx caches are denied).
        let mut path_dirs: Vec<String> = Vec::new();
        if let Some(dir) = lexical_parent(Path::new(&spec.program)) {
            path_dirs.push(dir.to_string_lossy().into_owned());
        }
        if let Some(dir) = lexical_parent(&self.ready.install.node) {
            path_dirs.push(dir.to_string_lossy().into_owned());
        }
        path_dirs.push(system_path(os).to_owned());
        path_dirs.dedup();
        env.push(("PATH".to_owned(), path_dirs.join(path_sep(os))));
        env
    }
}

fn valid_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl SandboxLauncher for SrtLauncher {
    /// On the host the sandbox starts on, every root (Fox state, temp root,
    /// managed runtime root, and the policy's paths) is canonicalized first,
    /// so overlap checks compare real locations rather than spellings. A
    /// policy planned for another OS (tests) is used as written.
    fn wrap(&self, spec: &LaunchSpec, policy: &SandboxPolicy, session_id: &str) -> Result<SandboxedLaunch, SandboxError> {
        if self.ready.os != HostOs::current() {
            return self.wrap_resolved(spec, policy, session_id, false);
        }
        let resolved = SrtLauncher {
            ready: self.ready.clone(),
            state_root: canonical_managed_path(&self.state_root)?,
            tmp_root: canonical_managed_path(&self.tmp_root)?,
            managed_runtime_root: self.managed_runtime_root.as_deref().map(canonical_managed_path).transpose()?,
            windows_acl: std::sync::Arc::clone(&self.windows_acl),
            ancestor_bases: self.ancestor_bases.clone(),
            windows_user_dirs: self.windows_user_dirs.clone(),
            acp_bridge_script: self.acp_bridge_script.clone(),
            srt_sandbox_sid: self.srt_sandbox_sid.clone(),
        };
        resolved.wrap_resolved(spec, policy, session_id, true)
    }
}

impl SrtLauncher {
    fn wrap_resolved(&self, spec: &LaunchSpec, policy: &SandboxPolicy, session_id: &str, canonicalize: bool) -> Result<SandboxedLaunch, SandboxError> {
        use SandboxErrorCode::*;
        let os = self.ready.os;
        if !valid_session_id(session_id) {
            return Err(SandboxError::new(SandboxPolicyInvalid, "session id must be [A-Za-z0-9_-]{1,128}"));
        }
        let program_name = Path::new(&spec.program).file_name().map(|name| name.to_string_lossy().to_ascii_lowercase());
        if matches!(program_name.as_deref(), Some("npx" | "npx.cmd" | "npx.exe")) {
            return Err(SandboxError::new(
                SandboxBackendNotInstalled,
                format!("{} is launched through npx; sandboxed mode needs it installed in Fox's backend directory", spec.backend.as_str()),
            )
            .remedy("install the backend from Fox settings"));
        }
        let program = Path::new(&spec.program);
        if !is_absolute_for(program, os) {
            return Err(SandboxError::new(
                SandboxBackendNotInstalled,
                format!("backend program must be an absolute path inside a readable directory, got {:?}", spec.program),
            ));
        }
        let mut policy = self.effective_policy(policy);
        let session_tmp = self.session_tmp(session_id);
        policy.session_tmp = Some(session_tmp.clone());
        // Relative paths are left for validate() to name.
        let all_absolute = policy
            .authorized
            .iter()
            .map(|dir| dir.path.as_path())
            .chain(std::iter::once(policy.private_home.as_path()))
            .all(|path| is_absolute_for(path, os));
        if canonicalize && all_absolute {
            policy = policy.canonicalized()?;
        }
        policy.validate(os)?;
        let mut windows_ancestor_aces = Vec::new();
        if os == HostOs::Windows {
            windows_ancestor_aces = self.windows_ace_plan(&policy)?.ancestors;
        }
        let program_resolved = if canonicalize { canonical_managed_path(program).unwrap_or_else(|_| program.to_path_buf()) } else { program.to_path_buf() };
        if !policy.is_readable(program, os) && !policy.is_readable(&program_resolved, os) {
            return Err(SandboxError::new(
                SandboxBackendNotInstalled,
                format!("{} is not readable inside the sandbox", program.display()),
            )
            .remedy("install the backend in Fox's backend directory and list it as read-only"));
        }
        if within(&self.state_root, &policy.private_home, os)
            || policy.writable_roots().iter().any(|root| within(&self.state_root, root, os))
            || policy.readable_roots().iter().any(|root| within(&self.state_root, root, os))
            || policy.granted_roots().iter().any(|root| within(root, &self.state_root, os) && !within(root, &self.tmp_root, os))
        {
            return Err(SandboxError::new(
                SandboxPolicyInvalid,
                "sandbox state directory must be neither writable nor readable by the sandbox",
            ));
        }

        let io = |error: std::io::Error| SandboxError::new(SandboxUnavailable, format!("cannot prepare sandbox: {error}"));
        let dir = self.state_root.join("sandbox-sessions").join(session_id);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let _ = std::fs::remove_dir_all(&session_tmp);
        std::fs::create_dir_all(&session_tmp).map_err(io)?;
        let session = SandboxSession { settings_path: dir.join("srt-settings.json"), dir, tmp: Some(session_tmp) };
        std::fs::create_dir_all(&policy.private_home).map_err(io)?;
        let settings = serde_json::to_vec_pretty(&policy.to_srt_settings()).expect("settings serialize");
        write_private(&session.settings_path, &settings).map_err(io)?;

        // The backend's variables go into the command, not into the srt
        // broker's environment: the broker (srt CLI / srt-win, running as the
        // real user) must see the real LOCALAPPDATA / USERPROFILE / TEMP, or it
        // keeps its state.db and ACL lock under the private HOME and disagrees
        // with Fox about the lock (docs/sandbox.md 7.11).
        let mut backend_env = spec.session_env.clone();
        for (key, value) in self.sandbox_env(spec, &policy) {
            backend_env.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&key));
            backend_env.push((key, value));
        }
        let stray_srt_state = if os == HostOs::Windows { find_stray_srt_state(&policy.private_home) } else { Vec::new() };
        let mut args = vec![
            self.ready.install.cli_js.to_string_lossy().into_owned(),
            "--settings".to_owned(),
            session.settings_path.to_string_lossy().into_owned(),
        ];
        let mut acp_bridge = None;
        let broker_env: Vec<(String, String)>;
        if os == HostOs::Windows {
            // srt 0.0.80 on Windows: the argv form is POSIX-quoted and then
            // run by `cmd.exe /d /s /c`, which mangles any path with a
            // backslash; and the child starts from the `srt-sandbox`
            // profile environment plus only PATH/PATHEXT/proxy variables,
            // so the session variables would not arrive. `-c` with a
            // cmd.exe command line fixes both (see windows_command_line).
            // stdin does not reach the child at all, so ACP goes through the
            // bridge (docs/sandbox.md 7.12).
            let endpoint = self.bridge_endpoint(canonicalize)?;
            let script = self.verified_bridge_script()?;
            let mut line_env = backend_env.clone();
            line_env.extend(endpoint.env());
            let mut bridge_args = vec![script.to_string_lossy().into_owned(), "--".to_owned(), spec.program.clone()];
            bridge_args.extend(spec.args.iter().cloned());
            args.push("-c".to_owned());
            args.push(windows_command_line(&self.ready.install.node.to_string_lossy(), &bridge_args, &line_env)?);
            acp_bridge = Some(endpoint);
            broker_env = broker_session_env(os, &backend_env);
        } else {
            // srt hands its own environment to the sandboxed child (bwrap /
            // sandbox-exec inherit it), so the backend's variables are set
            // with `env` inside the sandbox instead.
            broker_env = broker_session_env(os, &backend_env);
            args.push("--".to_owned());
            args.push(UNIX_ENV_PROGRAM.to_owned());
            for (key, value) in &backend_env {
                if key.is_empty() || key.contains('=') {
                    return Err(SandboxError::new(SandboxPolicyInvalid, format!("invalid environment variable name {key:?}")));
                }
                args.push(format!("{key}={value}"));
            }
            args.push(spec.program.clone());
            args.extend(spec.args.iter().cloned());
        }
        let wrapped = LaunchSpec {
            backend: spec.backend,
            program: self.ready.install.node.to_string_lossy().into_owned(),
            args,
            version_pin: spec.version_pin,
            // Windows: host credentials never reach the sandboxed child
            // anyway, so the broker does not get them either. Linux / macOS:
            // they reach the backend through srt's environment.
            credential_env: if os == HostOs::Windows { &[] } else { spec.credential_env },
            experimental: spec.experimental,
            session_env: broker_env,
        };
        Ok(SandboxedLaunch { spec: wrapped, session, effective_policy: policy, windows_ancestor_aces, backend_env, acp_bridge, stray_srt_state })
    }

    /// Windows: a fresh bridge endpoint whose pipe DACL admits the srt
    /// sandbox SID. On the real host the SID is read from the registry when
    /// it was not given.
    fn bridge_endpoint(&self, on_host: bool) -> Result<crate::launch::BridgeEndpoint, SandboxError> {
        let sid = match &self.srt_sandbox_sid {
            Some(sid) => Some(sid.clone()),
            None if on_host => read_srt_sandbox_sid(&RealProbe),
            None => None,
        };
        let sid = sid.filter(|sid| crate::winacl::is_string_sid(sid)).ok_or_else(|| {
            SandboxError::new(SandboxErrorCode::SandboxBridgeUnavailable, "no srt sandbox account SID is recorded (HKLM\\SOFTWARE\\sandbox-runtime SandboxUserSid)")
                .remedy("重新运行“启用沙箱”")
        })?;
        crate::launch::BridgeEndpoint::new_local(Some(sid)).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxBridgeUnavailable, error))
    }

    /// Windows: the installed bridge script, checked against the embedded
    /// copy (line endings normalized). Fail-closed like the dsh patches.
    fn verified_bridge_script(&self) -> Result<PathBuf, SandboxError> {
        let unavailable = |message: String| {
            SandboxError::new(SandboxErrorCode::SandboxBridgeUnavailable, message).remedy("重新运行“启用沙箱”以安装与本版本 Fox 匹配的运行时包")
        };
        let path = self.bridge_script_path().ok_or_else(|| unavailable("no managed runtime directory, so no ACP bridge".into()))?;
        let found = std::fs::read_to_string(&path).map_err(|error| unavailable(format!("ACP bridge {} is not installed: {error}", path.display())).missing(path.display().to_string()))?;
        if !bridge_script_matches(&found) {
            return Err(unavailable(format!("{} differs from the ACP bridge this Fox version ships", path.display())));
        }
        Ok(path)
    }
}

/// What the srt broker process itself gets on top of the Host allowlist.
///
/// * Windows: nothing. srt-win runs as the real user and keeps its state
///   (`%LOCALAPPDATA%\sandbox-runtime\{state.db,.acl-init.lock}`) under the
///   real `LOCALAPPDATA`, the same one [`crate::winancestor::srt_acl_lock_path`]
///   locks; USERPROFILE / APPDATA / TEMP / TMP / HOMEDRIVE / HOMEPATH /
///   SystemRoot are the real ones too.
/// * Linux / macOS: the session temp only. srt puts its proxy bridge sockets
///   in TMPDIR and passes CLAUDE_CODE_TMPDIR to the child as TMPDIR
///   (measured); the private HOME is not given to srt.
pub fn broker_session_env(os: HostOs, backend_env: &[(String, String)]) -> Vec<(String, String)> {
    match os {
        HostOs::Windows => Vec::new(),
        _ => backend_env.iter().filter(|(key, _)| matches!(key.as_str(), "TMPDIR" | "CLAUDE_CODE_TMPDIR")).cloned().collect(),
    }
}

/// Linux / macOS: the program that sets the backend's variables inside the
/// sandbox (`/usr` is in the system read-only set on both).
pub const UNIX_ENV_PROGRAM: &str = "/usr/bin/env";

/// Whether an installed bridge script is the one Fox embeds (CRLF and LF
/// treated alike).
pub fn bridge_script_matches(installed: &str) -> bool {
    installed.replace("\r\n", "\n") == crate::launch::BRIDGE_SCRIPT.replace("\r\n", "\n")
}

/// srt sandbox account SID from `reg query HKLM\SOFTWARE\sandbox-runtime /v
/// SandboxUserSid` (the value `windows-install` writes).
pub fn read_srt_sandbox_sid(probe: &dyn SystemProbe) -> Option<String> {
    let reg = probe.which("reg").unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32\reg.exe"));
    let output = probe.run(&reg, &["query", WINDOWS_INSTALL_REGISTRY_KEY, "/v", "SandboxUserSid"])?;
    parse_reg_sz(&output, "SandboxUserSid")
}

/// The data of `name` in `reg query` output (`    Name    REG_SZ    value`).
pub fn parse_reg_sz(output: &str, name: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        (parts.next()? .eq_ignore_ascii_case(name) && parts.next()? == "REG_SZ").then(|| parts.collect::<Vec<_>>().join(" ")).filter(|value| !value.is_empty())
    })
}

/// srt state that should never be inside a private HOME
/// (`<home>\AppData\Local\sandbox-runtime`, from a broker that was given
/// the private `LOCALAPPDATA`). Returns what exists; changes nothing.
pub fn find_stray_srt_state(private_home: &Path) -> Vec<PathBuf> {
    let dir = private_home.join("AppData").join("Local").join("sandbox-runtime");
    if std::fs::symlink_metadata(&dir).is_ok() {
        vec![dir]
    } else {
        Vec::new()
    }
}

/// Explicit clean-up of [`find_stray_srt_state`] results. Never automatic:
/// the caller must have established that no srt broker is alive (a running
/// broker may still hold that state.db). Refuses anything that is not a
/// `sandbox-runtime` directory under an `AppData\Local` of `private_home`,
/// and does not follow a reparse point.
pub fn remove_stray_srt_state(private_home: &Path, stray: &Path) -> Result<(), SandboxError> {
    let expected = private_home.join("AppData").join("Local").join("sandbox-runtime");
    if stray != expected {
        return Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, format!("{} is not the stray srt state of {}", stray.display(), private_home.display())));
    }
    let metadata = std::fs::symlink_metadata(stray).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("{}: {error}", stray.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, format!("{} is a link or not a directory; left alone", stray.display())));
    }
    std::fs::remove_dir_all(stray).map_err(|error| SandboxError::new(SandboxErrorCode::SandboxUnavailable, format!("cannot remove {}: {error}", stray.display())))
}

/// cmd.exe command line for srt's `-c` on Windows: `set "K=V"` for each
/// session variable, then the quoted program and arguments.
///
/// srt-win runs it as `cmd.exe /d /s /c "<line>"` inside the sandbox
/// (vendor/srt-win-src/src/launch.rs `build_cmdline`, verbatim after `/c`).
/// Values or arguments containing `"`, `%` or a line break cannot be passed
/// safely through cmd.exe and are refused (`sandbox_policy_invalid`).
/// Secret-shaped variables (a name containing `KEY`, `TOKEN`, `SECRET` or
/// `PASSWORD`, e.g. `OPENAI_API_KEY`, `DEEPSEEK_API_KEY`) are never put on the
/// line: it is visible in the process list. Host-environment credentials
/// (`credential_env`) do not reach the sandboxed child on Windows at all
/// (srt-win starts it from the `srt-sandbox` profile environment), so a
/// sandboxed backend there authenticates with its native login inside the
/// private HOME.
pub fn windows_command_line(program: &str, args: &[String], env: &[(String, String)]) -> Result<String, SandboxError> {
    let refuse = |what: &str, value: &str| {
        Err(SandboxError::new(
            SandboxErrorCode::SandboxPolicyInvalid,
            format!("{what} {value:?} cannot be passed through cmd.exe safely (contains a quote, % or a line break)"),
        ))
    };
    let unsafe_text = |text: &str| text.contains(['"', '%', '\r', '\n']);
    let quote = |text: &str| -> String {
        let needs = text.is_empty() || text.contains([' ', '\t', '&', '|', '<', '>', '^', '(', ')', ',', ';', '=', '!']);
        if !needs {
            return text.to_owned();
        }
        // MSVCRT argv rules: backslashes before the closing quote are doubled.
        let trailing = text.len() - text.trim_end_matches('\\').len();
        format!("\"{}{}\"", text, "\\".repeat(trailing))
    };
    let mut parts = Vec::new();
    for (key, value) in env {
        let upper = key.to_ascii_uppercase();
        if upper != crate::launch::BRIDGE_TOKEN_ENV && ["KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|marker| upper.contains(marker)) {
            continue;
        }
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return refuse("environment variable name", key);
        }
        if unsafe_text(value) {
            return refuse(&format!("value of {key}"), value);
        }
        parts.push(format!("set \"{key}={value}\""));
    }
    let mut command = Vec::new();
    for text in std::iter::once(program).chain(args.iter().map(String::as_str)) {
        if unsafe_text(text) {
            return refuse("argument", text);
        }
        command.push(quote(text));
    }
    parts.push(command.join(" "));
    Ok(parts.join(" && "))
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

// ---------------------------------------------------------------------------
// Launch preparation (the entry point the Host calls)
// ---------------------------------------------------------------------------

/// What the Host has for sandboxing at launch time.
pub enum SandboxSetup<'a> {
    Ready { launcher: &'a dyn SandboxLauncher, policy: &'a SandboxPolicy },
    /// Preflight (or building the policy) failed.
    Unavailable(SandboxError),
}

#[derive(Debug)]
pub struct PreparedLaunch {
    pub mode: PermissionMode,
    pub spec: LaunchSpec,
    /// Held for the life of the connection; dropping it removes the settings.
    pub sandbox: Option<SandboxSession>,
    /// Sandboxed: the policy exactly as written into the srt settings
    /// (canonical paths, session temp, runtime directories). `declaration`
    /// is generated from it, and [`verify_delivered_file`] takes it.
    pub effective_policy: Option<SandboxPolicy>,
    pub declaration: SandboxDeclaration,
    /// Windows sandboxed: ancestors the Host must give the sandbox account an
    /// attribute-read ACE on, inside the global srt transition, before
    /// spawning (docs/sandbox.md 7.6). Empty otherwise.
    pub windows_ancestor_aces: Vec<PathBuf>,
    /// Sandboxed: the backend's own variables (see
    /// [`SandboxedLaunch::backend_env`]). Empty in full access.
    pub backend_env: Vec<(String, String)>,
    /// Windows sandboxed: ACP runs over this bridge endpoint, not stdio.
    pub acp_bridge: Option<crate::launch::BridgeEndpoint>,
    /// Startup report: stray srt state inside the private HOME (reported,
    /// never removed automatically).
    pub stray_srt_state: Vec<PathBuf>,
}

/// Turn a backend launch spec into what actually runs.
///
/// * `FullAccess`: unchanged spec, no sandbox, declaration says so. Sandbox
///   problems are irrelevant and ignored.
/// * `Sandboxed`: wrapped in srt; any problem is returned as the refusal.
///   There is no fallback.
pub fn prepare_launch(
    mode: PermissionMode,
    spec: LaunchSpec,
    session_id: &str,
    setup: SandboxSetup<'_>,
    os: HostOs,
) -> Result<PreparedLaunch, SandboxError> {
    match mode {
        PermissionMode::FullAccess => Ok(PreparedLaunch {
            mode,
            spec,
            sandbox: None,
            effective_policy: None,
            declaration: SandboxDeclaration::declare(PermissionMode::FullAccess, None, os),
            windows_ancestor_aces: Vec::new(),
            backend_env: Vec::new(),
            acp_bridge: None,
            stray_srt_state: Vec::new(),
        }),
        PermissionMode::Sandboxed => match setup {
            SandboxSetup::Unavailable(error) => Err(error),
            SandboxSetup::Ready { launcher, policy } => {
                let SandboxedLaunch { spec, session, effective_policy, windows_ancestor_aces, backend_env, acp_bridge, stray_srt_state } = launcher.wrap(&spec, policy, session_id)?;
                // From the policy that was written, not the one requested:
                // the session temp, private HOME and runtime directories the
                // launcher added are part of what the UI shows.
                let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&effective_policy), os);
                Ok(PreparedLaunch {
                    mode,
                    spec,
                    sandbox: Some(session),
                    effective_policy: Some(effective_policy),
                    declaration,
                    windows_ancestor_aces,
                    backend_env,
                    acp_bridge,
                    stray_srt_state,
                })
            }
        },
    }
}

// ---------------------------------------------------------------------------
// Capability declaration ("sandbox scope" dimension)
// ---------------------------------------------------------------------------

/// How strong srt's boundary is, as measured per platform. Only Linux has been
/// measured (2026-10-19: Linux aarch64, bubblewrap 0.12.0, socat 1.8.0, srt
/// 0.0.80, pi-acp 0.0.34 inside; see docs/sandbox.md "T04"). macOS and Windows
/// stay `Unverified` until an engineer runs the same checks there.
pub fn srt_enforcement(os: HostOs) -> Enforcement {
    match os {
        HostOs::Linux => Enforcement::Enforced,
        HostOs::MacOs | HostOs::Windows => Enforcement::Unverified,
        HostOs::Other => Enforcement::Absent,
    }
}

/// Whether the read boundary of a policy has been measured on `os` with the
/// settings shape Fox writes there. Linux: measured 2026-10-19 with
/// `denyRead: ["/"]` and real pi-acp (docs/sandbox.md section 6). macOS and
/// Windows: not yet.
pub fn srt_read_scope_verified(os: HostOs) -> bool {
    os == HostOs::Linux
}

/// The "sandbox scope" dimension for one session, stated per mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxDeclaration {
    pub mode: PermissionMode,
    /// `Some("srt")` when the backend process runs inside srt.
    pub mechanism: Option<String>,
    pub filesystem: Enforcement,
    pub network: Enforcement,
    /// What the read boundary actually protects (UI wording comes from this
    /// and [`Self::read_scope_verified`], see [`ReadScope::ui_label`]).
    pub read_scope: ReadScope,
    /// The read boundary was measured on this platform.
    pub read_scope_verified: bool,
    /// The UI may say "limited to these folders" only when true: srt enforced,
    /// read scope `AuthorizedOnly` and measured.
    pub directory_isolation: bool,
    /// Exactly the srt settings `allowWrite` list.
    pub writable_roots: Vec<String>,
    /// Exactly the srt settings `allowRead` list (authorized folders, private
    /// HOME, session temp, backend / Node / srt runtime, system paths; on
    /// Windows without the system locations the Users group reads anyway).
    pub readable_roots: Vec<String>,
    /// Exactly the srt settings `denyRead` list.
    #[serde(default)]
    pub denied_read_roots: Vec<String>,
    pub allowed_domains: Vec<String>,
    /// Windows: folders on fixed drives whose DACL lets every local account
    /// (the sandbox account included) read or write them, from
    /// [`crate::winacl::audit_broad_exposure`]; filled by
    /// [`Self::with_exposure`]. Empty = not audited or nothing found.
    #[serde(default)]
    pub exposed_dirs: Vec<String>,
    /// The warning the UI must show next to the scope when `exposed_dirs` is
    /// non-empty ([`WINDOWS_EXPOSURE_WARNING`]).
    #[serde(default)]
    pub exposure_warning: Option<String>,
    /// Windows: whether the exposure audit read every path. `None` = not
    /// audited. `incomplete` is never a successful audit.
    #[serde(default)]
    pub exposure_audit: Option<crate::winacl::ExposureAuditStatus>,
    /// Windows: paths the audit could not check (`path: reason`).
    #[serde(default)]
    pub audit_errors: Vec<String>,
}

/// Product decision B (2026-10-19): on Windows the sandbox is not limited to
/// the authorized folders; shown whenever the exposure audit found folders
/// open to every account.
pub const WINDOWS_EXPOSURE_WARNING: &str = "此模式不限制在授权目录内。对所有用户开放的目录仍可能被读取或修改；本机已检测到此类目录。";
/// Appended (or used alone after the first sentence) when the audit could not
/// read every path.
pub const WINDOWS_EXPOSURE_INCOMPLETE: &str = "部分目录无法检查。";
/// First sentence of [`WINDOWS_EXPOSURE_WARNING`], used when nothing exposed
/// was found but the audit is incomplete.
pub const WINDOWS_SCOPE_NOT_LIMITED: &str = "此模式不限制在授权目录内。";

impl SandboxDeclaration {
    /// Attach the exposure audit (Windows sandboxed only; ignored otherwise).
    /// `directory_isolation` stays false whatever it finds.
    pub fn with_exposure(mut self, report: &crate::winacl::ExposureReport) -> Self {
        if self.mode != PermissionMode::Sandboxed || self.read_scope != ReadScope::UsersGroupReadable {
            return self;
        }
        self.exposed_dirs = report.exposed.iter().map(|item| item.path.to_string_lossy().into_owned()).collect();
        self.audit_errors = report.audit_errors.iter().map(|error| format!("{}: {}", error.path.display(), error.reason)).collect();
        self.exposure_audit = Some(if report.audit_errors.is_empty() { crate::winacl::ExposureAuditStatus::Complete } else { crate::winacl::ExposureAuditStatus::Incomplete });
        self.exposure_warning = match (self.exposed_dirs.is_empty(), self.audit_errors.is_empty()) {
            (false, true) => Some(WINDOWS_EXPOSURE_WARNING.to_owned()),
            (false, false) => Some(format!("{WINDOWS_EXPOSURE_WARNING}{WINDOWS_EXPOSURE_INCOMPLETE}")),
            (true, false) => Some(format!("{WINDOWS_SCOPE_NOT_LIMITED}{WINDOWS_EXPOSURE_INCOMPLETE}")),
            (true, true) => None,
        };
        self
    }

    pub fn declare(mode: PermissionMode, policy: Option<&SandboxPolicy>, os: HostOs) -> Self {
        match (mode, policy) {
            (PermissionMode::Sandboxed, Some(policy)) => {
                let enforcement = srt_enforcement(os);
                let read_scope = policy.read_scope(os);
                let read_scope_verified = srt_read_scope_verified(os);
                let roots = policy.settings_roots();
                let text = |paths: &[PathBuf]| paths.iter().map(|path| path.to_string_lossy().into_owned()).collect::<Vec<_>>();
                Self {
                    mode,
                    mechanism: Some("srt".into()),
                    filesystem: enforcement,
                    network: enforcement,
                    read_scope,
                    read_scope_verified,
                    directory_isolation: enforcement == Enforcement::Enforced
                        && read_scope == ReadScope::AuthorizedOnly
                        && read_scope_verified,
                    writable_roots: text(&roots.allow_write),
                    readable_roots: text(&roots.allow_read),
                    denied_read_roots: text(&roots.deny_read),
                    allowed_domains: policy.allowed_domains.clone(),
                    exposed_dirs: Vec::new(),
                    exposure_warning: None,
                    exposure_audit: None,
                    audit_errors: Vec::new(),
                }
            }
            // Full access, or sandboxed without a policy (never launched):
            // claim nothing.
            _ => Self {
                mode,
                mechanism: None,
                filesystem: Enforcement::Absent,
                network: Enforcement::Absent,
                read_scope: ReadScope::None,
                read_scope_verified: false,
                directory_isolation: false,
                writable_roots: Vec::new(),
                readable_roots: Vec::new(),
                denied_read_roots: Vec::new(),
                allowed_domains: Vec::new(),
                exposed_dirs: Vec::new(),
                exposure_warning: None,
                exposure_audit: None,
                audit_errors: Vec::new(),
            },
        }
    }

    /// Fold this into a backend's measured capabilities.
    ///
    /// * full access: sandbox dimensions become `Absent` whatever the backend
    ///   natively has, because Fox promises no boundary in this mode.
    /// * sandboxed: the stronger of the native measurement and srt's, so a
    ///   measured native sandbox is not hidden and srt is not overstated.
    pub fn apply_to(&self, declared: &mut CapabilityDeclaration) {
        match self.mode {
            PermissionMode::FullAccess => {
                declared.sandbox_filesystem = Enforcement::Absent;
                declared.sandbox_network = Enforcement::Absent;
            }
            PermissionMode::Sandboxed => {
                declared.sandbox_filesystem = stronger(declared.sandbox_filesystem, self.filesystem);
                declared.sandbox_network = stronger(declared.sandbox_network, self.network);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Delivered files (Host-side check, docs/sandbox.md section 11)
// ---------------------------------------------------------------------------

/// A file the Agent delivered, checked from outside the sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveredFile {
    /// Canonical path (symlinks resolved).
    pub path: PathBuf,
    /// The authorized writable folder it lies in (canonical).
    pub authorized_root: PathBuf,
    /// Size in bytes at check time.
    pub len: u64,
}

/// Host-side check of a file an Agent says it delivered. It must be a real,
/// persistent file the user can open after the session ends:
///
/// 1. resolved to its canonical path (symlinks, Windows junctions; UNC /
///    device paths refused) - a link pointing out of the folder is judged by
///    where it points;
/// 2. inside an authorized folder that is writable for the session (not a
///    read-only one), with `policy` canonicalized the same way;
/// 3. not inside the session temp directory, the private HOME or the private
///    HOME root (removed / Fox-private), and on Linux not on a `tmpfs` mount
///    (lost at reboot; also what srt's in-sandbox temp is);
/// 4. as seen outside the sandbox: exists, is a regular file, opens and its
///    first byte reads (no byte-by-byte comparison).
///
/// `policy` is the session's effective policy ([`PreparedLaunch::effective_policy`]).
/// Every refusal is `delivered_file_rejected` with a user-facing remedy.
pub fn verify_delivered_file(policy: &SandboxPolicy, path: &Path) -> Result<DeliveredFile, SandboxError> {
    use std::io::Read;
    let reject = |message: String| {
        SandboxError::new(SandboxErrorCode::DeliveredFileRejected, message)
            .remedy("交付文件必须保存在本会话授权的可写文件夹中（不能放在会话临时目录、私有 HOME 或内存盘里）；请让 Agent 把文件保存到授权文件夹后重新交付")
    };
    let os = HostOs::current();
    let resolved = canonical_existing_path(path)
        .map_err(|error| reject(format!("delivered file {} is not available outside the sandbox: {}", path.display(), error.message)))?;
    let policy = policy.canonicalized().map_err(|error| reject(format!("authorization snapshot cannot be resolved: {}", error.message)))?;
    let private = [Some(&policy.private_home_root), Some(&policy.private_home), policy.session_tmp.as_ref()];
    for root in private.into_iter().flatten() {
        if within(&resolved, root, os) {
            return Err(reject(format!("{} lies in Fox's private session directory {}", resolved.display(), root.display())));
        }
    }
    let Some(root) = policy.authorized.iter().filter(|dir| dir.writable).find(|dir| within(&resolved, &dir.path, os)) else {
        let read_only = policy.authorized.iter().any(|dir| !dir.writable && within(&resolved, &dir.path, os));
        return Err(reject(if read_only {
            format!("{} is in a read-only authorized folder; deliveries go to a writable one", resolved.display())
        } else {
            format!("{} is outside every authorized writable folder", resolved.display())
        }));
    };
    #[cfg(target_os = "linux")]
    {
        if on_tmpfs(&resolved) {
            return Err(reject(format!("{} is on a tmpfs (in-memory) mount and would not persist", resolved.display())));
        }
    }
    let metadata = std::fs::metadata(&resolved).map_err(|error| reject(format!("{}: {error}", resolved.display())))?;
    if !metadata.is_file() {
        return Err(reject(format!("{} is not a regular file", resolved.display())));
    }
    let mut first = [0u8; 1];
    std::fs::File::open(&resolved)
        .and_then(|mut file| file.read(&mut first))
        .map_err(|error| reject(format!("{} cannot be opened for reading: {error}", resolved.display())))?;
    Ok(DeliveredFile { path: resolved, authorized_root: root.path.clone(), len: metadata.len() })
}

#[cfg(target_os = "linux")]
fn on_tmpfs(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    let mut buf: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: valid NUL-terminated path and a zeroed out-buffer.
    if unsafe { libc::statfs(c_path.as_ptr(), &mut buf) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let kind = buf.f_type as i64;
    kind == libc::TMPFS_MAGIC as i64
}

fn rank(value: Enforcement) -> u8 {
    match value {
        Enforcement::Enforced => 3,
        Enforcement::Unverified => 2,
        Enforcement::Advisory => 1,
        Enforcement::Absent => 0,
    }
}

fn stronger(a: Enforcement, b: Enforcement) -> Enforcement {
    if rank(a) >= rank(b) { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::CapabilityDeclaration;

    const PROJECT: &str = "3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5c";
    const HOME_ROOT: &str = "/home/u/.local/share/fox/sandbox-homes";

    fn homes() -> PrivateHomes {
        PrivateHomes::new(HOME_ROOT.into())
    }

    fn linux_policy() -> SandboxPolicy {
        SandboxPolicy::new(
            vec![
                AuthorizedDir { path: "/home/u/proj".into(), writable: true },
                AuthorizedDir { path: "/home/u/docs".into(), writable: false },
            ],
            homes().home_for(PROJECT, AgentBackend::Pi).unwrap(),
            vec!["/home/u/.local/share/fox/backends/pi/install".into()],
            vec!["api.anthropic.com".into()],
            HostOs::Linux,
        )
    }

    fn pi_home() -> String {
        format!("{HOME_ROOT}/{PROJECT}/pi")
    }

    fn paths(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                assert_eq!(entry["literal"], json!(true), "paths are literal, never globs");
                entry["path"].as_str().unwrap().to_owned()
            })
            .collect()
    }

    /// A path written for `os`, whatever the host: Linux `/a/b`, Windows
    /// `C:\\a\\b` (the first segment is the drive, e.g. `C:`). Policy tests
    /// build their paths with this so they do not depend on the host's
    /// `PathBuf::join` separator.
    fn os_path(os: HostOs, segments: &[&str]) -> PathBuf {
        match os {
            HostOs::Windows => {
                let (drive, rest) = segments.split_first().expect("drive");
                PathBuf::from(format!("{drive}\\{}", rest.join("\\")))
            }
            _ => PathBuf::from(format!("/{}", segments.join("/"))),
        }
    }

    /// The private HOME of `backend` in `project` with each platform's layout:
    /// Linux `<fox_data>/sandbox-homes/<uuid>/<backend>`, Windows
    /// `%ProgramData%\\Fox\\sandbox\\<user SID>\\homes\\<uuid>\\<backend>`.
    fn test_home(os: HostOs, project: &str, backend: AgentBackend) -> PrivateHome {
        let root = match os {
            HostOs::Windows => os_path(os, &["C:", "ProgramData", "Fox", "sandbox", WIN_SID, "homes"]),
            _ => os_path(os, &["home", "u", ".local", "share", "fox", "sandbox-homes"]),
        };
        let sep = if os == HostOs::Windows { "\\" } else { "/" };
        let path = PathBuf::from(format!("{}{sep}{project}{sep}{}", root.display(), backend.as_str()));
        PrivateHome { root, path }
    }

    #[test]
    fn os_path_and_test_home_follow_the_target_platform_not_the_host() {
        assert_eq!(os_path(HostOs::Linux, &["home", "u", "proj"]), PathBuf::from("/home/u/proj"));
        assert_eq!(os_path(HostOs::Windows, &["C:", "Users", "u", "proj"]), PathBuf::from(r"C:\Users\u\proj"));
        let home = test_home(HostOs::Windows, PROJECT, AgentBackend::Pi);
        assert_eq!(home.path.display().to_string(), format!(r"C:\ProgramData\Fox\sandbox\{WIN_SID}\homes\{PROJECT}\pi"));
        assert!(within(&home.path, &home.root, HostOs::Windows));
        let home = test_home(HostOs::Linux, PROJECT, AgentBackend::Pi);
        assert_eq!(home.path, PathBuf::from(pi_home()));
    }

    // Linux rules (`/` deny, Linux system paths). The policy is built through
    // `PrivateHomes::home_for`, whose `join` uses the host separator, so the
    // Linux expectations only hold off Windows; the Windows rules have their
    // own variant below.
    #[cfg(not(windows))]
    #[test]
    fn settings_deny_the_root_and_reallow_only_granted_and_system_paths() {
        let policy = linux_policy();
        policy.validate(HostOs::Linux).unwrap();
        let settings = policy.to_srt_settings();
        let fs = &settings["filesystem"];
        assert_eq!(paths(&fs["denyRead"]), ["/"]);
        let allow_read = paths(&fs["allowRead"]);
        let granted = ["/home/u/proj".to_owned(), "/home/u/docs".to_owned(), pi_home(), "/home/u/.local/share/fox/backends/pi/install".to_owned()];
        assert_eq!(allow_read[..4], granted);
        let system: Vec<String> = default_system_read_only(HostOs::Linux).iter().map(|p| p.display().to_string()).collect();
        assert_eq!(allow_read[4..], system[..]);
        assert!(!allow_read.iter().any(|p| p == "/etc"), "/etc is never allowed as a whole");
        assert!(system.contains(&"/etc/ssl".to_owned()) && system.contains(&"/usr".to_owned()));
        assert_eq!(paths(&fs["allowWrite"]), ["/home/u/proj".to_owned(), pi_home()]);
        assert_eq!(settings["network"]["allowedDomains"], json!(["api.anthropic.com"]));
        assert_eq!(settings["network"]["allowLocalBinding"], json!(false));
        assert_eq!(settings["enableWeakerNestedSandbox"], json!(false));
        assert!(policy.is_readable(Path::new("/home/u/proj/src/a.rs"), HostOs::Linux));
        assert!(!policy.is_readable(Path::new("/home/u/.ssh/id_ed25519"), HostOs::Linux));
        assert!(!policy.is_readable(Path::new("/home/u/project-other"), HostOs::Linux), "prefix is not containment");
        // Beyond the home roots: other volumes and system data are denied too.
        for outside in ["/data/x.csv", "/mnt/usb/a", "/opt/secret", "/srv/db", "/var/lib/x", "/tmp/other", "/etc/shadow", "/etc/environment", "/workspace/repo"] {
            assert!(!policy.is_readable(Path::new(outside), HostOs::Linux), "{outside} must be unreadable");
        }
        assert!(policy.is_readable(Path::new("/usr/lib/libc.so.6"), HostOs::Linux));
        assert!(policy.is_readable(Path::new("/etc/ssl/certs/ca-certificates.crt"), HostOs::Linux));
        assert_eq!(policy.read_scope(HostOs::Linux), ReadScope::AuthorizedOnly);
    }

    /// Windows rules: no denyRead, allowRead lists only what needs an ACE
    /// (authorized folders, private HOME), never system or runtime paths.
    #[test]
    fn windows_settings_grant_only_user_paths_and_deny_nothing() {
        let policy = windows_policy();
        policy.validate(HostOs::Windows).unwrap();
        let settings = policy.to_srt_settings();
        let fs = &settings["filesystem"];
        assert!(paths(&fs["denyRead"]).is_empty());
        let home = test_home(HostOs::Windows, PROJECT, AgentBackend::Pi).path.display().to_string();
        assert_eq!(paths(&fs["allowRead"]), [r"C:\Users\u\proj".to_owned(), r"D:\work\docs".to_owned(), home.clone()]);
        assert_eq!(paths(&fs["allowWrite"]), [r"C:\Users\u\proj".to_owned(), home]);
        assert!(!policy.is_readable(Path::new(r"C:\Users\u\Documents\x.txt"), HostOs::Windows));
        assert!(policy.is_readable(Path::new(&format!(r"{WIN_RUNTIME}\backends\pi\pi-acp.cmd")), HostOs::Windows), "runtime in Program Files, Users-readable");
        assert_eq!(policy.read_scope(HostOs::Windows), ReadScope::UsersGroupReadable);
    }

    #[test]
    fn macos_denies_the_root_and_windows_denies_nothing_by_default() {
        assert_eq!(default_deny_read_roots(HostOs::MacOs), [PathBuf::from("/")]);
        assert!(default_system_read_only(HostOs::MacOs).contains(&PathBuf::from("/System")));
        assert!(default_system_read_only(HostOs::Windows).is_empty());
        // Windows: no denyRead C:\Users (redundant for the separate sandbox
        // account, and srt-win cannot write it: Access Denied as a normal user).
        assert!(default_deny_read_roots(HostOs::Windows).is_empty());
        let policy = windows_policy();
        policy.validate(HostOs::Windows).unwrap();
        let settings = policy.to_srt_settings();
        assert!(paths(&settings["filesystem"]["denyRead"]).is_empty(), "no denyRead on Windows by default");
        assert!(!paths(&settings["filesystem"]["allowRead"]).iter().any(|p| p.eq_ignore_ascii_case(r"C:\Users")));
        assert!(!policy.is_readable(Path::new(r"c:\users\u\Desktop\x.txt"), HostOs::Windows), "profile not granted");
        assert!(policy.is_readable(Path::new(r"C:\Users\u\proj\a.txt"), HostOs::Windows));
        assert!(policy.is_readable(Path::new(r"C:\Program Files\Git\bin\bash.exe"), HostOs::Windows), "Users-readable system path");
        // Not lexically knowable: a D:\ folder is readable only if its DACL is
        // open to everyone (exposure audit), never by policy.
        assert!(!policy.is_readable(Path::new(r"D:\data\x.csv"), HostOs::Windows));
        assert_eq!(policy.read_scope(HostOs::Windows), ReadScope::UsersGroupReadable);
    }

    /// `%LOCALAPPDATA%\\Fox`: Fox state stays here.
    const WIN_DATA: &str = r"C:\Users\u\AppData\Local\Fox";
    const WIN_SID: &str = "S-1-5-21-1111111111-2222222222-3333333333-1001";
    /// `%ProgramData%\\Fox\\sandbox\\<SID>`: private HOMEs and session temp.
    const WIN_PRIVATE: &str = r"C:\ProgramData\Fox\sandbox\S-1-5-21-1111111111-2222222222-3333333333-1001";
    /// `%ProgramFiles%\\Fox\\sandbox-runtime\\<version>`: Node, srt, backends.
    const WIN_RUNTIME: &str = r"C:\Program Files\Fox\sandbox-runtime\0.1.0";

    fn windows_policy() -> SandboxPolicy {
        SandboxPolicy::new(
            vec![
                AuthorizedDir { path: os_path(HostOs::Windows, &["C:", "Users", "u", "proj"]), writable: true },
                AuthorizedDir { path: os_path(HostOs::Windows, &["D:", "work", "docs"]), writable: false },
            ],
            test_home(HostOs::Windows, PROJECT, AgentBackend::Pi),
            vec![PathBuf::from(format!(r"{WIN_RUNTIME}\backends\pi"))],
            vec!["api.anthropic.com".into()],
            HostOs::Windows,
        )
    }

    fn windows_bases() -> crate::winancestor::AncestorBases {
        crate::winancestor::AncestorBases {
            profile_root: Some(PathBuf::from(r"C:\Users\u")),
            private_base: Some(PathBuf::from(r"C:\ProgramData\Fox\sandbox")),
        }
    }

    /// Fake Windows ACLs: `WRITE_DAC` everywhere except the listed prefixes
    /// (or exact paths); broad DACLs on the listed prefixes; owner
    /// Administrators except under `user_owned`.
    #[derive(Default)]
    struct FakeAcl {
        not_owned: Vec<&'static str>,
        not_owned_exact: Vec<&'static str>,
        broad_read: Vec<&'static str>,
        broad_write: Vec<&'static str>,
        user_owned: Vec<&'static str>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl FakeAcl {
        fn hit(list: &[&str], path: &Path) -> bool {
            list.iter().any(|prefix| within(path, Path::new(prefix), HostOs::Windows))
        }
    }

    impl crate::winacl::WindowsAclProbe for FakeAcl {
        fn check_write_dac(&self, path: &Path) -> Result<(), String> {
            self.asked.lock().unwrap().push(path.display().to_string());
            let exact = self.not_owned_exact.iter().any(|item| normalized(path, HostOs::Windows) == normalized(Path::new(item), HostOs::Windows));
            if exact || Self::hit(&self.not_owned, path) {
                Err("WRITE_DAC refused, Win32 error 5".into())
            } else {
                Ok(())
            }
        }
        fn broad_access(&self, path: &Path) -> Result<crate::winacl::BroadAccess, String> {
            let (read, write) = (Self::hit(&self.broad_read, path), Self::hit(&self.broad_write, path));
            Ok(crate::winacl::BroadAccess {
                read: read || write,
                write,
                groups: if read || write { vec!["Authenticated Users".into()] } else { vec![] },
            })
        }
        fn owner_sid(&self, path: &Path) -> Result<String, String> {
            Ok(if Self::hit(&self.user_owned, path) { WIN_SID.into() } else { crate::winacl::SID_BUILTIN_ADMINISTRATORS.into() })
        }
        fn dacl_entries(&self, path: &Path) -> Result<Vec<crate::winacl::AceEntry>, String> {
            let ace = |sid: &str, mask: u32| crate::winacl::AceEntry { sid: sid.into(), allow: true, mask, flags: 3 };
            let mut entries = vec![ace(crate::winacl::SID_BUILTIN_ADMINISTRATORS, 0x1f01ff), ace(crate::winacl::SID_LOCAL_SYSTEM, 0x1f01ff), ace("S-1-5-32-545", 0x1200a9)];
            if Self::hit(&self.broad_write, path) {
                entries.push(ace("S-1-5-11", 0x1301bf));
            }
            Ok(entries)
        }
        fn current_user_sid(&self) -> Result<String, String> {
            Ok(WIN_SID.into())
        }
    }

    fn windows_ready(node: &str, srt_prefix: &str) -> SandboxReady {
        SandboxReady {
            os: HostOs::Windows,
            install: SrtInstall::under_prefix(node.into(), Path::new(srt_prefix)),
            node_version: "v24.15.0".into(),
            srt_version: SRT_VERSION.into(),
            helpers: vec![],
        }
    }

    fn windows_launcher(node: &str, srt_prefix: &str, acl: FakeAcl) -> (SrtLauncher, std::sync::Arc<FakeAcl>) {
        let acl = std::sync::Arc::new(acl);
        let launcher = SrtLauncher::new(windows_ready(node, srt_prefix), PathBuf::from(WIN_DATA).join("state"))
            .with_managed_runtime_root(PathBuf::from(WIN_RUNTIME))
            .with_tmp_root(PathBuf::from(format!(r"{WIN_PRIVATE}\tmp")))
            .with_windows_acl(acl.clone())
            .with_ancestor_bases(windows_bases())
            .with_windows_user_dirs(vec![PathBuf::from(r"C:\Users\u"), PathBuf::from(WIN_DATA)]);
        (launcher, acl)
    }

    fn effective_windows(launcher: &SrtLauncher) -> SandboxPolicy {
        // effective_policy probes the disk only for Linux / macOS system paths
        // (none on Windows), so this runs on any host.
        let mut policy = launcher.effective_policy(&windows_policy());
        policy.session_tmp = Some(launcher.session_tmp("s1"));
        policy
    }

    fn srt_prefix() -> String {
        format!(r"{WIN_RUNTIME}\srt")
    }

    #[test]
    fn windows_plan_grants_only_user_owned_paths_and_adds_ancestor_aces() {
        // Node and srt in the machine-wide runtime (Users read+execute: no ACE).
        let (launcher, acl) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), FakeAcl::default());
        let policy = effective_windows(&launcher);
        policy.validate(HostOs::Windows).unwrap();
        let plan = launcher.windows_ace_plan(&policy).unwrap();
        let win = |list: &[PathBuf]| list.iter().map(|p| normalized(p, HostOs::Windows)).collect::<Vec<_>>();
        let lower = |text: &str| text.to_ascii_lowercase();
        assert_eq!(win(&plan.grant_read), [lower(r"D:\work\docs")], "runtime needs no ACE");
        assert_eq!(
            win(&plan.grant_write),
            [lower(r"C:\Users\u\proj"), normalized(&policy.private_home, HostOs::Windows), normalized(&launcher.session_tmp("s1"), HostOs::Windows)]
        );
        assert!(plan.system_readable.iter().all(|p| within(p, Path::new(WIN_RUNTIME), HostOs::Windows)));
        assert!(plan.deny.is_empty());
        // Ancestors (top-down by depth): D:\work (no broad read there), the
        // profile root above the project, the private root chain above HOME
        // and temp.
        assert_eq!(
            win(&plan.ancestors),
            [
                lower(r"D:\work"),
                lower(r"C:\Users\u"),
                lower(WIN_PRIVATE),
                lower(&format!(r"{WIN_PRIVATE}\homes")),
                lower(&format!(r"{WIN_PRIVATE}\tmp")),
                lower(&format!(r"{WIN_PRIVATE}\homes\{PROJECT}")),
            ]
        );
        let asked = acl.asked.lock().unwrap().clone();
        assert!(!asked.iter().any(|p| p.eq_ignore_ascii_case(r"C:\Users") || p.starts_with(r"C:\Program Files")), "{asked:?}");
        assert!(asked.iter().any(|p| p.eq_ignore_ascii_case(r"C:\Users\u")), "WRITE_DAC checked on the profile root (ancestor ACE)");
        let settings = policy.to_srt_settings();
        let allow_read = paths(&settings["filesystem"]["allowRead"]);
        assert!(!allow_read.iter().any(|p| p.starts_with(r"C:\Program Files") || p.eq_ignore_ascii_case(r"C:\Users")), "{allow_read:?}");
        assert!(paths(&settings["filesystem"]["denyRead"]).is_empty());
    }

    #[test]
    fn windows_runtime_must_be_the_machine_wide_install() {
        // The old per-user runtime under %LOCALAPPDATA%: structured "not installed".
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), FakeAcl::default());
        let launcher = launcher.with_managed_runtime_root(PathBuf::from(WIN_DATA).join("runtime"));
        let error = launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxRuntimeNotInstalled);
        assert_eq!(error.code.as_str(), "sandbox_runtime_not_installed");
        assert_eq!(error.remedy.as_deref(), Some("需在设置中启用沙箱（一次管理员授权）"));
        // An override location the user owns, or one Users may write.
        let acl = FakeAcl { user_owned: vec![r"D:\fox-rt"], ..Default::default() };
        let (launcher, _) = windows_launcher(r"D:\fox-rt\node\node.exe", r"D:\fox-rt\srt", acl);
        let launcher = launcher.with_managed_runtime_root(PathBuf::from(r"D:\fox-rt"));
        assert_eq!(launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        let acl = FakeAcl { broad_write: vec![r"D:\fox-rt"], ..Default::default() };
        let (launcher, _) = windows_launcher(r"D:\fox-rt\node\node.exe", r"D:\fox-rt\srt", acl);
        let launcher = launcher.with_managed_runtime_root(PathBuf::from(r"D:\fox-rt"));
        assert_eq!(launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
        // An administrator-owned override: accepted, and nothing under it is
        // put in allowRead (srt would fail to ACE it).
        let (launcher, _) = windows_launcher(r"D:\fox-rt\node\node.exe", r"D:\fox-rt\srt", FakeAcl::default());
        let launcher = launcher.with_managed_runtime_root(PathBuf::from(r"D:\fox-rt"));
        let mut policy = launcher.effective_policy(&windows_policy());
        policy.extra_read_only = vec![PathBuf::from(r"D:\fox-rt\backends\pi")];
        let policy = launcher.effective_policy(&policy);
        let mut policy = policy;
        policy.session_tmp = Some(launcher.session_tmp("s1"));
        let plan = launcher.windows_ace_plan(&policy).unwrap();
        assert!(!plan.grant_read.iter().any(|p| within(p, Path::new(r"D:\fox-rt"), HostOs::Windows)));
        assert!(!paths(&policy.to_srt_settings()["filesystem"]["allowRead"]).iter().any(|p| p.starts_with(r"D:\fox-rt")));
    }

    #[test]
    fn windows_plan_refuses_runtimes_outside_the_managed_directory() {
        // Node from a user-level install outside Fox's runtime directory.
        let (launcher, _) = windows_launcher(r"D:\tools\node\node.exe", &srt_prefix(), FakeAcl::default());
        let policy = effective_windows(&launcher);
        let error = launcher.windows_ace_plan(&policy).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathNotOwned);
        assert_eq!(error.code.as_str(), "sandbox_path_not_owned");
        assert_eq!(error.missing, [r"D:\tools\node"]);
        assert!(error.remedy.as_deref().unwrap().contains("运行时目录"));
        // No managed runtime directory configured at all.
        let mut bare = SrtLauncher::new(windows_ready(r"D:\tools\node\node.exe", &srt_prefix()), PathBuf::from(WIN_DATA).join("state"))
            .with_tmp_root(PathBuf::from(format!(r"{WIN_PRIVATE}\tmp")))
            .with_ancestor_bases(windows_bases())
            .with_windows_acl(std::sync::Arc::new(FakeAcl::default()));
        let policy = effective_windows(&bare);
        assert_eq!(bare.windows_ace_plan(&policy).unwrap_err().code, SandboxErrorCode::SandboxPathNotOwned);
        // Runtime directory inside a writable authorized folder: planting risk.
        bare = bare.with_managed_runtime_root(r"D:\proj\runtime".into());
        let mut policy = windows_policy();
        policy.authorized.push(AuthorizedDir { path: r"D:\proj".into(), writable: true });
        policy.extra_read_only = vec![r"D:\proj\runtime\pi".into()];
        policy.session_tmp = Some(bare.session_tmp("s1"));
        assert_eq!(bare.windows_ace_plan(&policy).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid);
    }

    #[test]
    fn windows_plan_fails_closed_naming_paths_without_write_dac() {
        // A project in a folder another account owns, and a Host deny whose
        // parent the user does not own.
        let acl = FakeAcl { not_owned: vec![r"D:\work", r"C:\Users\Public"], ..Default::default() };
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        let policy = effective_windows(&launcher);
        let error = launcher.windows_ace_plan(&policy).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathNotOwned);
        assert_eq!(error.missing, [r"D:\work\docs"]);
        assert!(error.message.contains(r"D:\work\docs") && error.message.contains("error 5"), "{}", error.message);

        let acl = FakeAcl { not_owned: vec![r"C:\Users\Public"], ..Default::default() };
        let (launcher, acl) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        let mut policy = effective_windows(&launcher);
        policy.deny_read_roots = vec![r"C:\Users\u\proj\secrets".into()];
        let plan = launcher.windows_ace_plan(&policy).unwrap();
        assert_eq!(plan.deny, [PathBuf::from(r"C:\Users\u\proj\secrets")]);
        let asked = acl.asked.lock().unwrap().clone();
        assert!(asked.contains(&r"C:\Users\u\proj".to_owned()), "deny parent checked too (srt stamps FILE_DELETE_CHILD there)");
        policy.deny_read_roots = vec![r"C:\Users\Public\x".into()];
        assert_eq!(launcher.windows_ace_plan(&policy).unwrap_err().code, SandboxErrorCode::SandboxPathNotOwned);

        // No WRITE_DAC on the profile root itself: the ancestor ACE cannot be
        // written, fail closed and say so.
        let acl = FakeAcl { not_owned_exact: vec![r"C:\Users\u"], ..Default::default() };
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        let error = launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathNotOwned);
        assert_eq!(error.missing, [r"C:\Users\u"]);
        assert!(error.message.contains("profile root"), "{}", error.message);
    }

    #[test]
    fn windows_plan_refuses_private_dirs_open_to_every_account() {
        // Fox state / private root on a location that inherits Authenticated Users:(M).
        let acl = FakeAcl { broad_write: vec![WIN_DATA, WIN_PRIVATE], ..Default::default() };
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        let policy = effective_windows(&launcher);
        let error = launcher.windows_ace_plan(&policy).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPathExposed);
        assert_eq!(error.code.as_str(), "sandbox_path_exposed");
        assert!(error.message.contains("state") && error.message.contains("homes") && error.message.contains("tmp"), "{}", error.message);
        // Read-only exposure of the runtime is expected (Users read+execute);
        // of the state directory it is not.
        let acl = FakeAcl { broad_read: vec![WIN_RUNTIME], ..Default::default() };
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap();
        let acl = FakeAcl { broad_read: vec![r"C:\Users\u\AppData\Local\Fox\state"], ..Default::default() };
        let (launcher, _) = windows_launcher(&format!(r"{WIN_RUNTIME}\node\node.exe"), &srt_prefix(), acl);
        assert_eq!(launcher.windows_ace_plan(&effective_windows(&launcher)).unwrap_err().code, SandboxErrorCode::SandboxPathExposed);
    }

    #[test]
    fn windows_command_line_sets_session_vars_and_quotes_for_cmd() {
        let env = vec![
            ("HOME".to_owned(), r"C:\Users\张 三\AppData\Local\Fox\sandbox-homes\x\pi".to_owned()),
            ("PATH".to_owned(), r"C:\rt\pi;C:\Windows\System32".to_owned()),
            ("OPENAI_API_KEY".to_owned(), "sk-should-never-appear".to_owned()),
            ("PI_ACP_PI_COMMAND".to_owned(), r"C:\rt\fx\pi-gated.cmd".to_owned()),
        ];
        let line = windows_command_line(
            r"C:\rt\pi\node_modules\.bin\pi-acp.cmd",
            &["--flag".into(), r"C:\My Dir\".into(), "a&b".into(), String::new()],
            &env,
        )
        .unwrap();
        let expected = concat!(
            r#"set "HOME=C:\Users\张 三\AppData\Local\Fox\sandbox-homes\x\pi" && "#,
            r#"set "PATH=C:\rt\pi;C:\Windows\System32" && "#,
            r#"set "PI_ACP_PI_COMMAND=C:\rt\fx\pi-gated.cmd" && "#,
            r#"C:\rt\pi\node_modules\.bin\pi-acp.cmd --flag "C:\My Dir\\" "a&b" """#,
        );
        assert_eq!(line, expected);
        assert!(!line.contains("sk-should-never-appear"), "secrets never on the command line");
        for bad in ["50%", "a\"b", "x\ny"] {
            let error = windows_command_line(r"C:\p.exe", &[bad.into()], &[]).unwrap_err();
            assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid, "{bad:?}");
            assert!(windows_command_line(r"C:\p.exe", &[], &[("X".into(), bad.into())]).is_err());
        }
        assert!(windows_command_line(r"C:\p.exe", &[], &[("BAD NAME".into(), "v".into())]).is_err());
    }

    #[test]
    fn windows_broker_gets_the_real_localappdata_so_the_acl_lock_paths_agree() {
        let (launcher, _) = windows_launcher(r"C:\Program Files\Fox\sandbox-runtime\0.1.0\node\node.exe", &srt_prefix(), FakeAcl::default());
        let policy = effective_windows(&launcher);
        let spec = LaunchSpec {
            backend: AgentBackend::Pi,
            program: format!(r"{WIN_RUNTIME}\backends\pi\pi-acp.cmd"),
            args: vec![],
            version_pin: "x",
            credential_env: &["OPENAI_API_KEY"],
            experimental: false,
            session_env: vec![],
        };
        let backend_env = launcher.sandbox_env(&spec, &policy);
        let private_local = backend_env.iter().find(|(k, _)| k == "LOCALAPPDATA").unwrap().1.clone();
        assert!(private_local.starts_with(WIN_PRIVATE), "{private_local}");
        // What AcpConnection hands the srt broker on Windows: the Host
        // allowlist of the parent plus broker_session_env (empty), no
        // credentials (the wrapped spec's credential_env is empty there).
        let broker_spec = LaunchSpec { session_env: broker_session_env(HostOs::Windows, &backend_env), credential_env: &[], ..spec.clone() };
        let parent: Vec<(String, String)> = [
            ("LOCALAPPDATA", r"C:\Users\u\AppData\Local"),
            ("USERPROFILE", r"C:\Users\u"),
            ("APPDATA", r"C:\Users\u\AppData\Roaming"),
            ("TEMP", r"C:\Users\u\AppData\Local\Temp"),
            ("TMP", r"C:\Users\u\AppData\Local\Temp"),
            ("HOMEDRIVE", "C:"),
            ("HOMEPATH", r"\Users\u"),
            ("SystemRoot", r"C:\Windows"),
            ("OPENAI_API_KEY", "sk-x"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let broker = crate::launch::process_environment_for(true, &broker_spec, parent.clone());
        let lock_fox = crate::winancestor::srt_acl_lock_path_in(parent.clone()).unwrap();
        let lock_broker = crate::winancestor::srt_acl_lock_path_in(broker.clone()).unwrap();
        assert_eq!(lock_fox, lock_broker, "Fox locks the file srt-win locks");
        assert_eq!(lock_broker, PathBuf::from(r"C:\Users\u\AppData\Local").join("sandbox-runtime").join(".acl-init.lock"));
        let get = |name: &str| broker.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str());
        for name in ["LOCALAPPDATA", "USERPROFILE", "APPDATA", "TEMP", "TMP", "HOMEDRIVE", "HOMEPATH", "SystemRoot"] {
            assert_eq!(get(name), parent.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str()), "{name} is the real one");
        }
        assert_eq!(get("OPENAI_API_KEY"), None);
        assert_eq!(get("HOME"), None, "no private HOME for the broker");
        // The old behaviour, for contrast: private LOCALAPPDATA in the broker
        // moves srt's lock under the private HOME.
        let old = LaunchSpec { session_env: backend_env.clone(), ..broker_spec };
        let lock_old = crate::winancestor::srt_acl_lock_path_in(crate::launch::process_environment_for(true, &old, parent)).unwrap();
        assert_ne!(lock_old, lock_fox);
        // Unix: only the session temp reaches srt.
        let unix = broker_session_env(HostOs::Linux, &backend_env);
        assert!(unix.iter().all(|(k, _)| k == "TMPDIR" || k == "CLAUDE_CODE_TMPDIR"));
    }

    #[test]
    fn windows_bridge_line_carries_pipe_and_token_and_runs_the_backend_behind_the_bridge() {
        let endpoint = crate::launch::BridgeEndpoint::new_local(Some(WIN_SID.into())).unwrap();
        let mut env = vec![("HOME".to_owned(), r"C:\h".to_owned()), ("OPENAI_API_KEY".to_owned(), "sk-no".to_owned())];
        env.extend(endpoint.env());
        let line = windows_command_line(
            r"C:\Program Files\Fox\sandbox-runtime\0.1.0\node\node.exe",
            &[r"C:\Program Files\Fox\sandbox-runtime\0.1.0\acp-bridge\bridge.mjs".into(), "--".into(), r"C:\rt\pi-acp.cmd".into()],
            &env,
        )
        .unwrap();
        assert!(line.contains(&format!("set \"FOX_ACP_TOKEN={}\"", endpoint.token)), "the one token allowed on the line");
        assert!(line.contains("set \"FOX_ACP_PIPE=") && !line.contains("sk-no"));
        assert!(line.ends_with(r#""C:\Program Files\Fox\sandbox-runtime\0.1.0\node\node.exe" "C:\Program Files\Fox\sandbox-runtime\0.1.0\acp-bridge\bridge.mjs" -- C:\rt\pi-acp.cmd"#), "{line}");
    }

    #[test]
    fn bridge_asset_must_match_the_embedded_copy() {
        assert!(bridge_script_matches(crate::launch::BRIDGE_SCRIPT));
        assert!(bridge_script_matches(&crate::launch::BRIDGE_SCRIPT.replace('\n', "\r\n")), "CRLF checkout is the same script");
        assert!(!bridge_script_matches(&format!("{}\n// edited", crate::launch::BRIDGE_SCRIPT)));
        let root = temp("bridge-asset");
        let script = root.join("bridge.mjs");
        let launcher = launcher(root.join("state")).with_acp_bridge_script(script.clone());
        assert_eq!(launcher.verified_bridge_script().unwrap_err().code, SandboxErrorCode::SandboxBridgeUnavailable);
        std::fs::write(&script, "tampered").unwrap();
        assert_eq!(launcher.verified_bridge_script().unwrap_err().code.as_str(), "sandbox_bridge_unavailable");
        std::fs::write(&script, crate::launch::BRIDGE_SCRIPT).unwrap();
        assert_eq!(launcher.verified_bridge_script().unwrap(), script);
        // No SID recorded (off the real host): refused, never stdio.
        assert_eq!(launcher.bridge_endpoint(false).unwrap_err().code, SandboxErrorCode::SandboxBridgeUnavailable);
        let with_sid = launcher.with_srt_sandbox_sid(WIN_SID.into());
        assert_eq!(with_sid.bridge_endpoint(false).unwrap().allowed_sid.as_deref(), Some(WIN_SID));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn srt_sandbox_sid_is_read_from_reg_query_output() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\sandbox-runtime\r\n    SandboxUserSid    REG_SZ    S-1-5-21-1-2-3-1010\r\n\r\n";
        assert_eq!(parse_reg_sz(output, "SandboxUserSid").as_deref(), Some("S-1-5-21-1-2-3-1010"));
        assert_eq!(parse_reg_sz(output, "MarkerVersion"), None);
    }

    #[test]
    fn stray_srt_state_in_a_private_home_is_reported_and_removed_only_on_request() {
        let root = temp("stray");
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        assert!(find_stray_srt_state(&home).is_empty());
        let stray = home.join("AppData").join("Local").join("sandbox-runtime");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("state.db"), "x").unwrap();
        assert_eq!(find_stray_srt_state(&home), vec![stray.clone()]);
        assert!(stray.join("state.db").exists(), "finding changes nothing");
        assert!(remove_stray_srt_state(&home, &home.join("AppData")).is_err(), "only the sandbox-runtime dir itself");
        remove_stray_srt_state(&home, &stray).unwrap();
        assert!(!stray.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn windows_policy_refuses_drive_and_share_roots_as_ace_targets() {
        for root in [r"D:\", "D:", r"\\nas\share", r"\\nas\share\"] {
            let mut policy = windows_policy();
            policy.authorized.push(AuthorizedDir { path: root.into(), writable: false });
            assert_eq!(policy.validate(HostOs::Windows).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid, "{root}");
        }
        let mut policy = windows_policy();
        policy.authorized.push(AuthorizedDir { path: r"\\nas\share\proj".into(), writable: false });
        policy.validate(HostOs::Windows).unwrap();
    }

    #[test]
    fn policy_refuses_to_reopen_a_denied_root_or_use_relative_paths() {
        let mut policy = linux_policy();
        policy.authorized.push(AuthorizedDir { path: "/".into(), writable: true });
        assert_eq!(policy.validate(HostOs::Linux).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid, "authorizing / re-opens everything");
        let mut policy = linux_policy();
        policy.extra_read_only.push("/".into());
        assert!(policy.validate(HostOs::Linux).is_err());
        let mut policy = linux_policy();
        policy.authorized[0].path = "proj".into();
        assert!(policy.validate(HostOs::Linux).is_err());
        let mut policy = linux_policy();
        policy.authorized.clear();
        assert!(policy.validate(HostOs::Linux).is_err());
        let mut policy = linux_policy();
        policy.deny_read_roots.clear();
        assert!(policy.validate(HostOs::Linux).is_err(), "no deny roots = whole disk readable");
    }

    #[test]
    fn legacy_and_unknown_modes_read_as_sandboxed() {
        assert_eq!(PermissionMode::default(), PermissionMode::Sandboxed);
        assert_eq!(PermissionMode::from_settings(&BTreeMap::new()), PermissionMode::Sandboxed);
        assert_eq!(PermissionMode::parse(Some("FULL")), PermissionMode::Sandboxed);
        assert_eq!(PermissionMode::parse(Some("full_access")), PermissionMode::FullAccess);
        assert_eq!(serde_json::to_value(PermissionMode::FullAccess).unwrap(), json!("full_access"));
    }

    fn snapshot(mode: Option<&str>) -> SessionSnapshot {
        let mut settings = BTreeMap::new();
        if let Some(mode) = mode {
            settings.insert(SETTING_PERMISSION_MODE.to_owned(), mode.to_owned());
        }
        SessionSnapshot { backend: AgentBackend::Pi, project_root: "/p".into(), egress_allowed: true, settings }
    }

    #[test]
    fn full_access_needs_a_live_grant_and_revocation_restarts_sandboxed() {
        let mut policy = HostPolicy::default();
        let requested = snapshot(Some("full_access"));
        let decision = effective_permission_mode(&policy, &requested, "c1");
        assert_eq!(decision.mode, PermissionMode::Sandboxed, "request alone grants nothing");
        assert!(decision.downgraded.is_some());

        policy.full_access_grants.insert(FullAccessGrant::Project("/p".into()).key());
        assert_eq!(effective_permission_mode(&policy, &requested, "c1").mode, PermissionMode::FullAccess);
        assert_eq!(effective_permission_mode(&policy, &snapshot(None), "c1").mode, PermissionMode::Sandboxed);
        assert_eq!(recheck_permission_mode(&policy, &requested, "c1", PermissionMode::FullAccess), ModeRecheck::Unchanged);

        // Revocation (tightening) applies to the running session.
        policy.full_access_grants.clear();
        assert!(matches!(
            recheck_permission_mode(&policy, &requested, "c1", PermissionMode::FullAccess),
            ModeRecheck::RestartSandboxed(_)
        ));
        // A new grant (loosening) is not carried into a running sandboxed process.
        policy.full_access_grants.insert(FullAccessGrant::Conversation("c1".into()).key());
        assert_eq!(recheck_permission_mode(&policy, &requested, "c1", PermissionMode::Sandboxed), ModeRecheck::Unchanged);
        // Enterprise switch and locked key beat any grant.
        policy.full_access_forbidden = true;
        assert_eq!(effective_permission_mode(&policy, &requested, "c1").mode, PermissionMode::Sandboxed);
        policy.full_access_forbidden = false;
        policy.locked_keys.insert(SETTING_PERMISSION_MODE.into(), "sandboxed".into());
        assert_eq!(effective_permission_mode(&policy, &requested, "c1").mode, PermissionMode::Sandboxed);
    }

    #[test]
    fn backend_default_allowlists() {
        assert_eq!(default_allowed_domains(AgentBackend::Codex, &[]), ["api.openai.com", "auth.openai.com", "chatgpt.com"]);
        assert_eq!(default_allowed_domains(AgentBackend::Deepseek, &[]), ["api.deepseek.com"]);
        assert_eq!(
            default_allowed_domains(AgentBackend::Pi, &["anthropic", "deepseek", "mystery"]),
            ["api.anthropic.com", "api.deepseek.com"]
        );
        assert!(default_allowed_domains(AgentBackend::Fox, &[]).is_empty());
    }

    #[test]
    fn allowlist_overrides_can_only_tighten() {
        let mut policy = HostPolicy::default();
        let user = BTreeMap::from([(SETTING_ALLOWED_DOMAINS.to_owned(), "chatgpt.com, evil.example.com".to_owned())]);
        let project = BTreeMap::from([(SETTING_ALLOWED_DOMAINS.to_owned(), "chatgpt.com,api.openai.com".to_owned())]);
        let resolved = resolve_allowed_domains(AgentBackend::Codex, &[], &policy, &[&user, &project]);
        assert_eq!(resolved.domains, ["chatgpt.com"], "intersection of every narrowing layer");
        assert_eq!(resolved.rejected.len(), 2, "evil.example.com added by user, api.openai.com re-added by project");

        // Only the Host may extend the ceiling (custom provider endpoint) ...
        policy.sandbox_extra_domains.insert(AgentBackend::Pi, BTreeSet::from(["*.corp.example".to_owned()]));
        let resolved = resolve_allowed_domains(AgentBackend::Pi, &["anthropic", "custom"], &policy, &[]);
        assert_eq!(resolved.domains, ["*.corp.example", "api.anthropic.com"]);
        assert_eq!(resolved.unknown_providers, ["custom"]);
        // ... and lower layers may narrow a wildcard to one host, not widen it.
        let narrow = BTreeMap::from([(SETTING_ALLOWED_DOMAINS.to_owned(), "llm.corp.example,*.example".to_owned())]);
        let resolved = resolve_allowed_domains(AgentBackend::Pi, &["custom"], &policy, &[&narrow]);
        assert_eq!(resolved.domains, ["llm.corp.example"]);
        assert_eq!(resolved.rejected.len(), 1);
        // Host locked key narrows too, and an empty value means no network.
        policy.locked_keys.insert(SETTING_ALLOWED_DOMAINS.into(), String::new());
        assert!(resolve_allowed_domains(AgentBackend::Codex, &[], &policy, &[]).domains.is_empty());
    }

    struct FakeProbe {
        os: HostOs,
        files: Vec<PathBuf>,
        on_path: Vec<&'static str>,
        node_version: Option<&'static str>,
        srt_version: &'static str,
        userns: bool,
        windows_installed: bool,
    }

    impl FakeProbe {
        fn linux_ok() -> Self {
            Self {
                os: HostOs::Linux,
                files: vec![install().cli_js],
                on_path: vec!["bwrap", "socat", "rg"],
                node_version: Some("v22.23.3\n"),
                srt_version: "0.0.80",
                userns: true,
                windows_installed: false,
            }
        }
    }

    impl SystemProbe for FakeProbe {
        fn os(&self) -> HostOs {
            self.os
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.iter().any(|file| file == path) || path == Path::new("/usr/bin/sandbox-exec")
        }
        fn which(&self, program: &str) -> Option<PathBuf> {
            self.on_path.contains(&program).then(|| PathBuf::from(format!("/usr/bin/{program}")))
        }
        fn run(&self, program: &Path, _args: &[&str]) -> Option<String> {
            let name = program.file_name()?.to_string_lossy().into_owned();
            match name.as_str() {
                "node" => self.node_version.map(String::from),
                "bwrap" => self.userns.then(String::new),
                "reg" | "reg.exe" => self.windows_installed.then(String::new),
                _ => None,
            }
        }
        fn read_to_string(&self, path: &Path) -> Option<String> {
            path.ends_with("package.json").then(|| format!("{{\"version\":\"{}\"}}", self.srt_version))
        }
    }

    fn install() -> SrtInstall {
        SrtInstall::under_prefix("/opt/fox/node/bin/node".into(), Path::new("/opt/fox/srt"))
    }

    #[test]
    fn preflight_reports_each_missing_piece_with_its_code() {
        assert!(preflight(&install(), &FakeProbe::linux_ok()).is_ok());

        let mut probe = FakeProbe::linux_ok();
        probe.files.clear();
        assert_eq!(preflight(&install(), &probe).unwrap_err().code, SandboxErrorCode::SandboxDependencyMissing);

        let mut probe = FakeProbe::linux_ok();
        probe.srt_version = "0.0.70";
        assert_eq!(preflight(&install(), &probe).unwrap_err().code, SandboxErrorCode::SandboxDependencyMissing);

        let mut probe = FakeProbe::linux_ok();
        probe.node_version = Some("v20.11.0");
        let error = preflight(&install(), &probe).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxDependencyMissing);
        assert!(error.message.contains("22.12.0"));

        let mut probe = FakeProbe::linux_ok();
        probe.on_path = vec!["rg"];
        let error = preflight(&install(), &probe).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxDependencyMissing);
        assert_eq!(error.missing, ["bwrap", "socat"]);

        let mut probe = FakeProbe::linux_ok();
        probe.userns = false;
        assert_eq!(preflight(&install(), &probe).unwrap_err().code, SandboxErrorCode::SandboxUnavailable);

        let mut probe = FakeProbe::linux_ok();
        probe.os = HostOs::Windows;
        probe.on_path = vec!["reg"];
        let error = preflight(&install(), &probe).unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxNotInstalled);
        assert_eq!(windows_install_state(&probe), WindowsInstallState::NotInstalled);
        probe.windows_installed = true;
        assert!(preflight(&install(), &probe).is_ok());
        assert_eq!(windows_install_state(&FakeProbe::linux_ok()), WindowsInstallState::NotWindows);
        let (_, args) = windows_uninstall_command(&install());
        assert_eq!(args.last().unwrap(), "windows-uninstall");

        let mut probe = FakeProbe::linux_ok();
        probe.os = HostOs::Other;
        assert_eq!(preflight(&install(), &probe).unwrap_err().code, SandboxErrorCode::SandboxUnavailable);

        let json = serde_json::to_value(preflight(&install(), &FakeProbe { userns: false, ..FakeProbe::linux_ok() }).unwrap_err()).unwrap();
        assert_eq!(json["code"], "sandbox_unavailable", "structured, snake_case code");
    }

    fn spec(program: &str) -> LaunchSpec {
        LaunchSpec {
            backend: AgentBackend::Codex,
            program: program.into(),
            args: vec!["--flag".into()],
            version_pin: "x",
            credential_env: &["OPENAI_API_KEY", "CODEX_HOME"],
            experimental: false,
            session_env: vec![("INITIAL_AGENT_MODE".into(), "read-only".into()), ("HOME".into(), "/home/u".into())],
        }
    }

    /// A fresh directory in canonical form (macOS `/var` -> `/private/var`,
    /// Windows 8.3 short names expanded), since wrap canonicalizes too.
    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fox-sandbox-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        canonical_existing_path(&dir).unwrap()
    }

    fn launcher(state_root: PathBuf) -> SrtLauncher {
        let ready = preflight(&install(), &FakeProbe::linux_ok()).unwrap();
        SrtLauncher::new(ready, state_root)
    }

    // The Linux FakeProbe needs `/`-rooted paths; on a Windows host the temp
    // directory is `C:\...`, so the Windows variant below runs there instead.
    #[cfg(not(windows))]
    #[test]
    fn srt_wrap_writes_per_session_settings_and_points_home_at_the_private_dir() {
        let root = temp("wrap");
        std::fs::create_dir_all(root.join("home/u/proj")).unwrap();
        let state = root.join("state");
        let homes = PrivateHomes::new(root.join("fox/sandbox-homes"));
        let home = homes.home_for(PROJECT, AgentBackend::Codex).unwrap().path;
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: root.join("home/u/proj"), writable: true }],
            homes.home_for(PROJECT, AgentBackend::Codex).unwrap(),
            vec!["/opt/fox/backends/codex".into()],
            vec!["api.openai.com".into()],
            HostOs::Linux,
        );
        let launcher = launcher(state.clone());
        let wrapped = launcher.wrap(&spec("/opt/fox/backends/codex/bin/codex-acp"), &policy, "sess-1").unwrap();
        assert_eq!(wrapped.spec.program, "/opt/fox/node/bin/node");
        let settings_path = wrapped.session.settings_path().to_path_buf();
        assert_eq!(
            wrapped.spec.args[..5],
            [
                "/opt/fox/srt/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js".to_owned(),
                "--settings".to_owned(),
                settings_path.display().to_string(),
                "--".to_owned(),
                UNIX_ENV_PROGRAM.to_owned(),
            ]
        );
        // The backend's variables ride on `env K=V` inside the sandbox ...
        let n = wrapped.spec.args.len();
        assert_eq!(wrapped.spec.args[n - 2..], ["/opt/fox/backends/codex/bin/codex-acp".to_owned(), "--flag".to_owned()]);
        let on_line: Vec<String> = wrapped.backend_env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(wrapped.spec.args[5..n - 2], on_line[..]);
        let env: BTreeMap<_, _> = wrapped.backend_env.iter().cloned().collect();
        let tmp = state.join("sandbox-tmp").join("sess-1");
        // ... and the srt broker gets only the session temp it needs itself
        // (no private HOME: it keeps the real one from the Host allowlist).
        let broker: BTreeMap<_, _> = wrapped.spec.session_env.iter().cloned().collect();
        assert_eq!(broker.keys().map(String::as_str).collect::<Vec<_>>(), ["CLAUDE_CODE_TMPDIR", "TMPDIR"]);
        assert_eq!(broker["TMPDIR"], tmp.display().to_string());
        assert!(wrapped.acp_bridge.is_none(), "stdio on Linux / macOS");
        assert!(wrapped.stray_srt_state.is_empty());
        assert_eq!(wrapped.session.tmp_dir(), Some(tmp.as_path()));
        assert_eq!(env["HOME"], home.display().to_string(), "private HOME overrides the spec's HOME");
        assert_eq!(env["CODEX_HOME"], home.join(".codex").display().to_string());
        assert_eq!(env["TMPDIR"], tmp.display().to_string(), "temp is per session, not in the shared HOME");
        assert_eq!(env["TEMP"], tmp.display().to_string());
        assert_eq!(env["CLAUDE_CODE_TMPDIR"], tmp.display().to_string(), "srt hands this to the child as TMPDIR");
        assert_eq!(env["NODE_USE_ENV_PROXY"], "1");
        assert_eq!(env["INITIAL_AGENT_MODE"], "read-only", "backend session vars kept");
        assert_eq!(env["PATH"], "/opt/fox/backends/codex/bin:/opt/fox/node/bin:/usr/local/bin:/usr/bin:/bin");
        assert_eq!(wrapped.backend_env.iter().filter(|(k, _)| k == "HOME").count(), 1);

        let written: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        let allow_read = paths(&written["filesystem"]["allowRead"]);
        assert_eq!(
            allow_read[..4],
            [
                root.join("home/u/proj").display().to_string(),
                home.display().to_string(),
                tmp.display().to_string(),
                "/opt/fox/backends/codex".to_owned()
            ]
        );
        // srt package and Node (under /opt/fox, denied by `/`) are re-allowed;
        // the rest are system paths that exist on this machine.
        assert!(allow_read.contains(&"/opt/fox/srt/node_modules/@anthropic-ai/sandbox-runtime".to_owned()));
        assert!(allow_read.contains(&"/opt/fox/node".to_owned()));
        for path in &allow_read[4..] {
            let system = default_system_read_only(HostOs::Linux).iter().any(|p| p.display().to_string() == *path);
            assert!(system || path.starts_with("/opt/fox/"), "unexpected allowRead {path}");
            if system {
                assert!(std::fs::symlink_metadata(path).is_ok(), "missing system path {path} kept");
            }
        }
        assert_eq!(paths(&written["filesystem"]["allowWrite"]), [root.join("home/u/proj").display().to_string(), home.display().to_string(), tmp.display().to_string()]);
        assert_eq!(paths(&written["filesystem"]["denyRead"]), ["/"]);
        assert!(home.is_dir() && tmp.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&settings_path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        std::fs::write(home.join("login.json"), "{}").unwrap();
        drop(wrapped);
        assert!(!settings_path.exists(), "per-session settings removed with the session");
        assert!(!tmp.exists(), "per-session temp removed with the session");
        assert!(home.join("login.json").exists(), "the project x backend HOME is kept for the next session");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows host: the same wrap with Windows rules (managed runtime
    /// directory, no denyRead, private HOME as USERPROFILE). ACL queries are
    /// faked; the real ones are exercised by the T04 harness.
    #[cfg(windows)]
    #[test]
    fn srt_wrap_writes_per_session_settings_and_points_home_at_the_private_dir() {
        let root = temp("wrap");
        let state = root.join("state");
        let runtime = root.join("runtime");
        let node = runtime.join("node").join("node.exe");
        let srt_prefix = runtime.join("srt");
        let backend_dir = runtime.join("codex");
        let program = backend_dir.join("codex-acp.cmd");
        let homes = PrivateHomes::new(root.join("fox").join("sandbox-homes"));
        let home = homes.home_for(PROJECT, AgentBackend::Codex).unwrap().path;
        let proj = root.join("home").join("u").join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: proj.clone(), writable: true }],
            homes.home_for(PROJECT, AgentBackend::Codex).unwrap(),
            vec![backend_dir.clone()],
            vec!["api.openai.com".into()],
            HostOs::Windows,
        );
        // The temp directory is under %LOCALAPPDATA%: user directories are
        // cleared so the (faked, administrator-owned) runtime is accepted.
        let launcher = SrtLauncher::new(windows_ready(&node.display().to_string(), &srt_prefix.display().to_string()), state.clone())
            .with_managed_runtime_root(runtime.clone())
            .with_windows_user_dirs(vec![])
            .with_ancestor_bases(Default::default())
            .with_windows_acl(std::sync::Arc::new(FakeAcl::default()))
            .with_srt_sandbox_sid(WIN_SID.into());
        // Without the bridge asset there is no Windows sandboxed launch.
        assert_eq!(launcher.wrap(&spec(&program.display().to_string()), &policy, "sess-0").unwrap_err().code, SandboxErrorCode::SandboxBridgeUnavailable);
        let bridge = runtime.join("acp-bridge").join("bridge.mjs");
        std::fs::create_dir_all(bridge.parent().unwrap()).unwrap();
        std::fs::write(&bridge, crate::launch::BRIDGE_SCRIPT).unwrap();
        let wrapped = launcher.wrap(&spec(&program.display().to_string()), &policy, "sess-1").unwrap();
        assert_eq!(wrapped.spec.program, node.display().to_string());
        let endpoint = wrapped.acp_bridge.clone().expect("Windows sandboxed ACP goes through the bridge");
        assert!(endpoint.address.starts_with(r"\\.\pipe\fox-acp-"));
        assert_eq!(endpoint.allowed_sid.as_deref(), Some(WIN_SID));
        assert!(wrapped.spec.session_env.is_empty(), "nothing private for the srt broker");
        assert!(wrapped.spec.credential_env.is_empty());
        let settings_path = wrapped.session.settings_path().to_path_buf();
        // Windows: `-c` with a cmd.exe line (srt's argv form POSIX-quotes
        // backslash paths, and its child would not see session variables).
        assert_eq!(wrapped.spec.args[1..4], ["--settings".to_owned(), settings_path.display().to_string(), "-c".to_owned()]);
        let line = &wrapped.spec.args[4];
        assert!(line.contains(&format!("set \"HOME={}\"", home.display())), "{line}");
        assert!(line.ends_with(&format!("{} -- {} --flag", bridge.display(), program.display())), "{line}");
        assert!(line.contains(&format!("set \"FOX_ACP_TOKEN={}\"", endpoint.token)));
        assert!(!line.contains("OPENAI_API_KEY"));
        let env: BTreeMap<_, _> = wrapped.backend_env.iter().cloned().collect();
        let tmp = state.join("sandbox-tmp").join("sess-1");
        assert_eq!(env["HOME"], home.display().to_string());
        assert_eq!(env["USERPROFILE"], home.display().to_string());
        assert_eq!(env["LOCALAPPDATA"], home.join(r"AppData\Local").display().to_string());
        assert_eq!(env["TEMP"], tmp.display().to_string());
        assert_eq!(
            env["PATH"],
            format!("{};{};C:\\Windows\\System32;C:\\Windows", backend_dir.display(), node.parent().unwrap().display())
        );
        let written: Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
        assert!(paths(&written["filesystem"]["denyRead"]).is_empty(), "no denyRead on Windows");
        let allow_read = paths(&written["filesystem"]["allowRead"]);
        for expected in [&proj, &home, &tmp] {
            assert!(allow_read.contains(&expected.display().to_string()), "missing allowRead {}: {allow_read:?}", expected.display());
        }
        // The managed runtime is Users-readable by contract: never in allowRead.
        assert!(!allow_read.iter().any(|path| within(Path::new(path), &runtime, HostOs::Windows)), "{allow_read:?}");
        assert_eq!(env["NODE_OPTIONS"], "--preserve-symlinks --preserve-symlinks-main");
        assert_eq!(paths(&written["filesystem"]["allowWrite"]), [proj.display().to_string(), home.display().to_string(), tmp.display().to_string()]);
        drop(wrapped);
        assert!(!settings_path.exists() && !tmp.exists());
        assert!(home.is_dir(), "the project x backend HOME is kept");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn srt_wrap_refuses_npx_unreadable_programs_and_writable_state() {
        let root = temp("refuse");
        std::fs::create_dir_all(root.join("home/u/proj")).unwrap();
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: root.join("home/u/proj"), writable: true }],
            PrivateHomes::new(root.join("fox/sandbox-homes")).home_for(PROJECT, AgentBackend::Codex).unwrap(),
            vec![],
            vec![],
            HostOs::Linux,
        );
        let launcher = launcher(root.join("state"));
        let code = |program: &str| launcher.wrap(&spec(program), &policy, "s").unwrap_err().code;
        assert_eq!(code("npx"), SandboxErrorCode::SandboxBackendNotInstalled);
        assert_eq!(code("codex-acp"), SandboxErrorCode::SandboxBackendNotInstalled, "relative program");
        let hidden = root.join("home/u/.npm/_npx/bin/codex-acp");
        assert_eq!(code(&hidden.display().to_string()), SandboxErrorCode::SandboxBackendNotInstalled);
        assert_eq!(
            launcher.wrap(&spec("/opt/x/codex-acp"), &policy, "../escape").unwrap_err().code,
            SandboxErrorCode::SandboxPolicyInvalid
        );
        let mut readable = policy.clone();
        readable.extra_read_only.push("/opt/x".into());
        let inside = SrtLauncher::new(launcher.ready.clone(), root.join("home/u/proj/.fox-state"));
        assert_eq!(
            inside.wrap(&spec("/opt/x/codex-acp"), &readable, "s").unwrap_err().code,
            SandboxErrorCode::SandboxPolicyInvalid,
            "settings must not be writable from inside"
        );
        let below_readonly = SrtLauncher::new(launcher.ready.clone(), "/opt/x/state".into());
        assert_eq!(
            below_readonly.wrap(&spec("/opt/x/codex-acp"), &readable, "s").unwrap_err().code,
            SandboxErrorCode::SandboxPolicyInvalid,
            "settings must not be readable from inside either"
        );
        let tmp_in_project = SrtLauncher::new(launcher.ready.clone(), root.join("state")).with_tmp_root(root.join("home/u/proj/tmp"));
        assert_eq!(
            tmp_in_project.wrap(&spec("/opt/x/codex-acp"), &readable, "s").unwrap_err().code,
            SandboxErrorCode::SandboxPolicyInvalid,
            "session temp may not overlap an authorized directory"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn launcher_reallows_srt_and_node_when_they_live_under_a_denied_root() {
        let ready = SandboxReady {
            os: HostOs::Linux,
            install: SrtInstall::under_prefix("/home/u/.nvm/versions/node/v22.12.0/bin/node".into(), Path::new("/home/u/.fox/srt")),
            node_version: "v22.12.0".into(),
            srt_version: SRT_VERSION.into(),
            helpers: vec![],
        };
        let launcher = SrtLauncher::new(ready, "/var/fox-state".into());
        let effective = launcher.effective_policy(&linux_policy());
        assert!(effective.extra_read_only.contains(&PathBuf::from("/home/u/.fox/srt/node_modules/@anthropic-ai/sandbox-runtime")));
        assert!(effective.extra_read_only.contains(&PathBuf::from("/home/u/.nvm/versions/node/v22.12.0")));
        assert!(!effective.is_readable(Path::new("/home/u/.nvm/versions/node/v20.0.0/bin/node"), HostOs::Linux));
    }

    struct Broken;
    impl SandboxLauncher for Broken {
        fn wrap(&self, _: &LaunchSpec, _: &SandboxPolicy, _: &str) -> Result<SandboxedLaunch, SandboxError> {
            Err(SandboxError::new(SandboxErrorCode::SandboxUnavailable, "boom"))
        }
    }

    #[test]
    fn sandboxed_mode_fails_closed_and_full_access_runs_unchanged() {
        let missing = || SandboxError::new(SandboxErrorCode::SandboxDependencyMissing, "srt missing").missing("srt");
        // srt missing: sandboxed refuses, never degrades to full access.
        let refused = prepare_launch(PermissionMode::Sandboxed, spec("/x"), "s", SandboxSetup::Unavailable(missing()), HostOs::Linux);
        assert_eq!(refused.unwrap_err().code, SandboxErrorCode::SandboxDependencyMissing);
        // A launcher that fails at wrap time refuses as well.
        let policy = linux_policy();
        let refused = prepare_launch(
            PermissionMode::Sandboxed,
            spec("/x"),
            "s",
            SandboxSetup::Ready { launcher: &Broken, policy: &policy },
            HostOs::Linux,
        );
        assert_eq!(refused.unwrap_err().code, SandboxErrorCode::SandboxUnavailable);
        // Full access ignores the missing sandbox and runs the spec as is.
        let prepared =
            prepare_launch(PermissionMode::FullAccess, spec("npx"), "s", SandboxSetup::Unavailable(missing()), HostOs::Linux)
                .unwrap();
        assert_eq!(prepared.spec, spec("npx"));
        assert!(prepared.sandbox.is_none());
        assert!(!prepared.declaration.directory_isolation);
    }

    // Linux rules (and the macOS / Windows "not measured" fold-in over a
    // Linux-shaped policy). Built through `home_for`, so off Windows only; the
    // Windows declaration has its own test below.
    #[cfg(not(windows))]
    #[test]
    fn capability_declaration_follows_the_mode() {
        let policy = linux_policy();
        // Native backend with nothing enforced (pi stock).
        let mut native = CapabilityDeclaration::unverified();
        native.sandbox_filesystem = Enforcement::Absent;
        native.sandbox_network = Enforcement::Absent;

        let sandboxed = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::Linux);
        assert_eq!(sandboxed.mechanism.as_deref(), Some("srt"));
        assert_eq!(sandboxed.read_scope, ReadScope::AuthorizedOnly);
        assert!(sandboxed.read_scope_verified);
        assert!(sandboxed.directory_isolation);
        assert_eq!(sandboxed.writable_roots, ["/home/u/proj".to_owned(), pi_home()]);
        assert_eq!(
            ReadScope::AuthorizedOnly.ui_label(true),
            "读取限定在授权文件夹（另含运行必需的系统路径、私有 HOME 和会话临时目录）",
            "the label names what else is readable"
        );
        assert!(ReadScope::AuthorizedOnly.ui_label(false).ends_with("本平台待实测）"));

        // A Linux policy that only denies home roots is partial, and the UI
        // may not say "limited to these folders".
        let mut partial = policy.clone();
        partial.deny_read_roots = vec!["/home".into(), "/root".into()];
        let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&partial), HostOs::Linux);
        assert_eq!(declaration.read_scope, ReadScope::UserDirsOnly);
        assert!(!declaration.directory_isolation);
        let mut declared = native.clone();
        sandboxed.apply_to(&mut declared);
        assert_eq!((declared.sandbox_filesystem, declared.sandbox_network), (Enforcement::Enforced, Enforcement::Enforced));

        // Not measured on macOS / Windows: never shown as enforced.
        let mac = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::MacOs);
        assert_eq!((mac.read_scope, mac.read_scope_verified), (ReadScope::AuthorizedOnly, false));
        for os in [HostOs::MacOs, HostOs::Windows] {
            let declaration = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), os);
            assert!(!declaration.directory_isolation);
            let mut declared = native.clone();
            declaration.apply_to(&mut declared);
            assert_eq!(declared.sandbox_filesystem, Enforcement::Unverified);
        }

        // Full access: no boundary claimed, even over a native Enforced sandbox.
        let full = SandboxDeclaration::declare(PermissionMode::FullAccess, Some(&policy), HostOs::Linux);
        assert!(!full.directory_isolation);
        assert_eq!(full.read_scope, ReadScope::None);
        assert!(full.mechanism.is_none() && full.writable_roots.is_empty());
        let mut declared = native.clone();
        declared.sandbox_filesystem = Enforcement::Enforced;
        full.apply_to(&mut declared);
        assert_eq!((declared.sandbox_filesystem, declared.sandbox_network), (Enforcement::Absent, Enforcement::Absent));

        // Sandboxed keeps a stronger native measurement.
        let mut declared = native.clone();
        declared.sandbox_filesystem = Enforcement::Enforced;
        SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::MacOs).apply_to(&mut declared);
        assert_eq!(declared.sandbox_filesystem, Enforcement::Enforced);
    }

    /// Windows (decision B): partial scope, never directory isolation, the
    /// exposure audit carried as `exposed_dirs` with the fixed warning.
    #[test]
    fn windows_declaration_is_partial_and_carries_the_exposure_warning() {
        let windows = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&windows_policy()), HostOs::Windows);
        assert_eq!((windows.read_scope, windows.read_scope_verified), (ReadScope::UsersGroupReadable, false));
        assert_eq!(serde_json::to_value(windows.read_scope).unwrap(), json!("users_group_readable"));
        let label = windows.read_scope.ui_label(windows.read_scope_verified);
        assert_eq!(
            label,
            "沙箱账户可读取授权文件夹和系统中对所有用户开放的位置；个人文件夹按实际 ACL 保护；其他盘上对所有用户开放的目录可能可读写（本平台待实测）"
        );
        assert!(!label.contains("受保护；") && label.contains("按实际 ACL 保护"), "no absolute promise about personal folders");
        assert!(!windows.directory_isolation);
        assert!(windows.exposed_dirs.is_empty() && windows.exposure_warning.is_none(), "not audited yet");
        let report = crate::winacl::ExposureReport {
            supported: true,
            drives: vec![PathBuf::from(r"C:\"), PathBuf::from(r"D:\")],
            exposed: vec![crate::winacl::ExposedPath {
                path: PathBuf::from(r"D:\data"),
                access: crate::winacl::BroadAccess { read: true, write: true, groups: vec!["Authenticated Users".into()] },
            }],
            errors: vec![],
            ..Default::default()
        };
        let audited = windows.clone().with_exposure(&report);
        assert_eq!(audited.exposed_dirs, [r"D:\data"]);
        assert_eq!(audited.exposure_warning.as_deref(), Some(WINDOWS_EXPOSURE_WARNING));
        assert_eq!(WINDOWS_EXPOSURE_WARNING, "此模式不限制在授权目录内。对所有用户开放的目录仍可能被读取或修改；本机已检测到此类目录。");
        assert!(!audited.directory_isolation, "B: warn only");
        assert_eq!(audited.exposure_audit, Some(crate::winacl::ExposureAuditStatus::Complete));
        let clean = windows.clone().with_exposure(&crate::winacl::ExposureReport { supported: true, ..Default::default() });
        assert!(clean.exposure_warning.is_none());
        // Read failures: listed, the audit is incomplete, and the warning
        // says so (also when nothing exposed was found).
        let errors = vec![crate::winacl::AuditError { path: PathBuf::from(r"D:ault"), reason: "cannot read the DACL (Win32 error 5)".into() }];
        let partial = windows.clone().with_exposure(&crate::winacl::ExposureReport { audit_errors: errors.clone(), ..report.clone() });
        assert_eq!(partial.exposure_audit, Some(crate::winacl::ExposureAuditStatus::Incomplete));
        assert_eq!(partial.audit_errors, [r"D:ault: cannot read the DACL (Win32 error 5)"]);
        assert_eq!(partial.exposure_warning.as_deref(), Some("此模式不限制在授权目录内。对所有用户开放的目录仍可能被读取或修改；本机已检测到此类目录。部分目录无法检查。"));
        let only_errors = windows.clone().with_exposure(&crate::winacl::ExposureReport { supported: true, audit_errors: errors, ..Default::default() });
        assert_eq!(only_errors.exposure_warning.as_deref(), Some("此模式不限制在授权目录内。部分目录无法检查。"));
        assert_eq!(serde_json::to_value(&only_errors).unwrap()["exposure_audit"], "incomplete");
        let json = serde_json::to_value(&audited).unwrap();
        assert_eq!(json["exposed_dirs"], json!([r"D:\data"]));
        // Full access / other platforms ignore the audit.
        let full = SandboxDeclaration::declare(PermissionMode::FullAccess, None, HostOs::Windows).with_exposure(&report);
        assert!(full.exposed_dirs.is_empty() && full.exposure_warning.is_none());
        let mut declared = CapabilityDeclaration::unverified();
        declared.sandbox_filesystem = Enforcement::Absent;
        audited.apply_to(&mut declared);
        assert_eq!(declared.sandbox_filesystem, Enforcement::Unverified);
    }

    #[test]
    fn private_home_is_per_project_uuid_and_backend() {
        assert_eq!(PrivateHomeScope::default(), PrivateHomeScope::PerProjectPerBackend);
        let homes = homes();
        let pi = homes.home_for(PROJECT, AgentBackend::Pi).unwrap();
        assert_eq!(pi.path, PathBuf::from(pi_home()));
        assert_eq!(pi.root, PathBuf::from(HOME_ROOT));
        // Same project, other backend: separate HOME. Same backend, same
        // project: the same HOME (sessions share it).
        let codex = homes.home_for(PROJECT, AgentBackend::Codex).unwrap();
        assert_ne!(codex.path, pi.path);
        assert_eq!(homes.home_for(PROJECT, AgentBackend::Pi).unwrap(), pi);
        // Uppercase input is the same project, not a second directory.
        assert_eq!(homes.home_for(&PROJECT.to_ascii_uppercase(), AgentBackend::Pi).unwrap(), pi);
        // Two projects that share a name and a folder name still differ: only
        // the UUID is used.
        let other = homes.home_for("9b7e0d3c-1a2b-4c3d-8e4f-5a6b7c8d9e0f", AgentBackend::Pi).unwrap();
        assert_ne!(other.path, pi.path);
        assert!(!other.path.starts_with(&pi.path) && !pi.path.starts_with(&other.path));
    }

    #[test]
    fn private_home_refuses_empty_or_malformed_project_ids() {
        let homes = homes();
        for bad in [
            "",
            " ",
            "00000000-0000-0000-0000-000000000000",
            "3f2a6c1e8b4d4e0a9c550d1e2f3a4b5c",
            "3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5",
            "3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5c/..",
            "../3f2a6c1e-8b4d-4e0a-9c55-0d1e2f3a4b5c",
            "zzzzzzzz-8b4d-4e0a-9c55-0d1e2f3a4b5c",
            "我的项目",
            "/home/u/我的项目",
        ] {
            let error = homes.home_for(bad, AgentBackend::Pi).unwrap_err();
            assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid, "{bad:?} must be refused");
        }
        assert_eq!(homes.home_for(PROJECT, AgentBackend::Fox).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid);
    }

    // Linux rules; the Windows profile case is its own test below.
    #[cfg(not(windows))]
    #[test]
    fn chinese_paths_are_literal_and_do_not_affect_the_home_name() {
        let homes = PrivateHomes::new("/home/用户/.local/share/fox/sandbox-homes".into());
        let home = homes.home_for(PROJECT, AgentBackend::Deepseek).unwrap();
        assert_eq!(home.path, PathBuf::from(format!("/home/用户/.local/share/fox/sandbox-homes/{PROJECT}/deepseek")));
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: "/home/用户/文档/[进行中] 项目 A".into(), writable: true }],
            home,
            vec![],
            vec![],
            HostOs::Linux,
        );
        policy.validate(HostOs::Linux).unwrap();
        let allow_read = paths(&policy.to_srt_settings()["filesystem"]["allowRead"]);
        assert_eq!(allow_read[0], "/home/用户/文档/[进行中] 项目 A");
        assert!(policy.is_readable(Path::new("/home/用户/文档/[进行中] 项目 A/说明.md"), HostOs::Linux));
        assert!(!policy.is_readable(Path::new("/home/用户/文档/[进行中] 项目 B/说明.md"), HostOs::Linux));
    }

    /// Windows profile with a Chinese user name and a project under
    /// `Documents\中文 空格`: literal paths, the private HOME name comes from
    /// the UUID only, and every folder between the profile root and the
    /// project gets an ancestor ACE.
    #[test]
    fn windows_chinese_profile_paths_are_literal_and_get_ancestor_aces() {
        let proj = os_path(HostOs::Windows, &["C:", "Users", "张三", "Documents", "中文 空格", "proj"]);
        let home = test_home(HostOs::Windows, PROJECT, AgentBackend::Codex);
        assert!(home.path.display().to_string().ends_with(&format!(r"\homes\{PROJECT}\codex")));
        let policy = SandboxPolicy::new(vec![AuthorizedDir { path: proj.clone(), writable: true }], home, vec![], vec![], HostOs::Windows);
        policy.validate(HostOs::Windows).unwrap();
        let allow_write = paths(&policy.to_srt_settings()["filesystem"]["allowWrite"]);
        assert_eq!(allow_write[0], r"C:\Users\张三\Documents\中文 空格\proj");
        assert!(!policy.is_readable(Path::new(r"C:\Users\张三\桌面\a.txt"), HostOs::Windows));
        assert!(!policy.is_readable(Path::new(r"C:\Users\张三\Documents\other.txt"), HostOs::Windows), "Documents itself is not granted");
        let bases = crate::winancestor::AncestorBases { profile_root: Some(PathBuf::from(r"C:\Users\张三")), private_base: None };
        let plan = WindowsAcePlan { grant_write: vec![proj], ..Default::default() };
        let ancestors = crate::winancestor::plan_ancestor_aces(&plan, &bases, &FakeAcl::default());
        let text: Vec<String> = ancestors.iter().map(|path| path.display().to_string()).collect();
        assert_eq!(text, [r"C:\Users\张三", r"C:\Users\张三\Documents", r"C:\Users\张三\Documents\中文 空格"]);
    }

    // Linux rules (lexical containment with `/`); Windows variant below.
    #[cfg(not(windows))]
    #[test]
    fn authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp() {
        let other_project_home = format!("{HOME_ROOT}/9b7e0d3c-1a2b-4c3d-8e4f-5a6b7c8d9e0f/pi");
        for authorized in [
            "/home/u/.local/share/fox".to_owned(),     // contains the root
            HOME_ROOT.to_owned(),                       // is the root
            other_project_home.clone(),                 // another project's HOME
            format!("{other_project_home}/.pi"),        // inside it
            pi_home(),                                  // its own HOME as a project folder
        ] {
            let mut policy = linux_policy();
            policy.authorized.push(AuthorizedDir { path: authorized.clone().into(), writable: false });
            let error = policy.validate(HostOs::Linux).unwrap_err();
            assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid, "{authorized} must be refused");
        }
        // The private HOME must be under its root.
        let mut policy = linux_policy();
        policy.private_home = "/home/u/elsewhere".into();
        assert!(policy.validate(HostOs::Linux).is_err());
        // Session temp overlapping an authorized directory.
        let mut policy = linux_policy();
        policy.session_tmp = Some("/home/u/proj/.tmp".into());
        assert!(policy.validate(HostOs::Linux).is_err());
        // Siblings are fine.
        let mut policy = linux_policy();
        policy.authorized.push(AuthorizedDir { path: "/home/u/.local/share/fox-notes".into(), writable: false });
        policy.validate(HostOs::Linux).unwrap();
    }

    /// Windows: the private root lives under `%ProgramData%\\Fox\\sandbox\\<SID>`;
    /// no authorized folder may contain it, be it, or reach into another
    /// project's HOME or the session temp (case-insensitive).
    #[test]
    fn windows_authorized_dirs_may_not_reach_any_private_home_or_the_session_tmp() {
        let other_home = format!(r"{WIN_PRIVATE}\homes\9b7e0d3c-1a2b-4c3d-8e4f-5a6b7c8d9e0f\pi");
        for authorized in [
            r"C:\ProgramData\Fox".to_owned(),
            WIN_PRIVATE.to_owned(),
            format!(r"{WIN_PRIVATE}\HOMES"),
            other_home.clone(),
            format!(r"{other_home}\.pi"),
            test_home(HostOs::Windows, PROJECT, AgentBackend::Pi).path.display().to_string().to_ascii_uppercase(),
        ] {
            let mut policy = windows_policy();
            policy.authorized.push(AuthorizedDir { path: authorized.clone().into(), writable: false });
            assert_eq!(policy.validate(HostOs::Windows).unwrap_err().code, SandboxErrorCode::SandboxPolicyInvalid, "{authorized} must be refused");
        }
        let mut policy = windows_policy();
        policy.private_home = PathBuf::from(r"C:\Users\u\elsewhere");
        assert!(policy.validate(HostOs::Windows).is_err(), "HOME outside its root");
        let mut policy = windows_policy();
        policy.session_tmp = Some(PathBuf::from(r"C:\Users\u\proj\.tmp"));
        assert!(policy.validate(HostOs::Windows).is_err());
        let mut policy = windows_policy();
        policy.session_tmp = Some(PathBuf::from(format!(r"{WIN_PRIVATE}\tmp\s1")));
        policy.authorized.push(AuthorizedDir { path: r"C:\ProgramData\Fox\notes".into(), writable: false });
        policy.validate(HostOs::Windows).unwrap();
    }

    // ---- effective policy -> declaration (item: UI declaration source) ----

    /// The declaration shown to the user is generated from the policy that
    /// was written: its three lists equal the settings file, including the
    /// session temp, the private HOME and the srt / Node directories the
    /// launcher added.
    #[cfg(not(windows))]
    #[test]
    fn declaration_lists_exactly_what_the_written_settings_contain() {
        let root = temp("declare");
        std::fs::create_dir_all(root.join("proj")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let homes = PrivateHomes::new(root.join("fox/sandbox-homes"));
        let home = homes.home_for(PROJECT, AgentBackend::Codex).unwrap();
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: root.join("proj"), writable: true }, AuthorizedDir { path: root.join("docs"), writable: false }],
            home.clone(),
            vec!["/opt/fox/backends/codex".into()],
            vec!["api.openai.com".into()],
            HostOs::Linux,
        );
        let launcher = launcher(root.join("state"));
        let prepared = prepare_launch(
            PermissionMode::Sandboxed,
            spec("/opt/fox/backends/codex/bin/codex-acp"),
            "sess-d",
            SandboxSetup::Ready { launcher: &launcher, policy: &policy },
            HostOs::Linux,
        )
        .unwrap();
        let written: Value =
            serde_json::from_slice(&std::fs::read(prepared.sandbox.as_ref().unwrap().settings_path()).unwrap()).unwrap();
        let declaration = &prepared.declaration;
        assert_eq!(declaration.readable_roots, paths(&written["filesystem"]["allowRead"]), "allowRead");
        assert_eq!(declaration.writable_roots, paths(&written["filesystem"]["allowWrite"]), "allowWrite");
        assert_eq!(declaration.denied_read_roots, paths(&written["filesystem"]["denyRead"]), "denyRead");
        assert_eq!(declaration.allowed_domains, ["api.openai.com"]);
        let effective = prepared.effective_policy.as_ref().expect("sandboxed launch carries its effective policy");
        assert_eq!(effective.to_srt_settings(), written, "effective policy is what was written");
        let tmp = root.join("state/sandbox-tmp/sess-d").display().to_string();
        for expected in [
            tmp.clone(),
            home.path.display().to_string(),
            "/opt/fox/node".to_owned(),
            "/opt/fox/srt/node_modules/@anthropic-ai/sandbox-runtime".to_owned(),
        ] {
            assert!(declaration.readable_roots.contains(&expected), "declaration misses {expected}: {:?}", declaration.readable_roots);
        }
        assert!(declaration.writable_roots.contains(&tmp));
        assert!(!declaration.writable_roots.contains(&root.join("docs").display().to_string()), "read-only stays read-only");
        // Declaring from the requested policy (the old behaviour) would have
        // hidden the session temp and the runtime directories.
        let requested = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&policy), HostOs::Linux);
        assert!(!requested.readable_roots.contains(&tmp) && !requested.readable_roots.contains(&"/opt/fox/node".to_owned()));
        drop(prepared);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- path normalization ----

    #[cfg(not(windows))]
    #[test]
    fn managed_paths_resolve_the_existing_part_and_keep_the_missing_tail() {
        let root = temp("managed");
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        assert_eq!(canonical_managed_path(&root.join("link/not/yet")).unwrap(), root.join("real/not/yet"));
        assert_eq!(canonical_managed_path(&root.join("link")).unwrap(), root.join("real"));
        assert!(canonical_managed_path(&root.join("missing/../escape")).is_err(), "`..` below a missing directory");
        assert!(canonical_managed_path(Path::new("relative/dir")).is_err());
        assert!(canonical_existing_path(&root.join("missing")).is_err(), "authorized folders must exist");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// An authorized folder that is a symlink is judged by its target: a link
    /// to the private HOME root (or to `/`) is refused although its spelling
    /// overlaps nothing; a harmless link is written as its target.
    #[cfg(unix)]
    #[test]
    fn symlinked_authorized_folder_is_judged_by_its_target() {
        let root = temp("symlink");
        let homes_root = root.join("fox/sandbox-homes");
        std::fs::create_dir_all(&homes_root).unwrap();
        std::fs::create_dir_all(root.join("real-proj")).unwrap();
        std::os::unix::fs::symlink(&homes_root, root.join("innocent")).unwrap();
        std::os::unix::fs::symlink(root.join("real-proj"), root.join("proj-link")).unwrap();
        std::os::unix::fs::symlink("/", root.join("everything")).unwrap();
        let homes = PrivateHomes::new(homes_root.clone());
        let launcher = launcher(root.join("state"));
        let policy_for = |authorized: PathBuf| {
            SandboxPolicy::new(
                vec![AuthorizedDir { path: authorized, writable: true }],
                homes.home_for(PROJECT, AgentBackend::Codex).unwrap(),
                vec!["/opt/fox/backends/codex".into()],
                vec![],
                HostOs::Linux,
            )
        };
        let program = spec("/opt/fox/backends/codex/bin/codex-acp");
        // Text alone says "no overlap"; the target is the private HOME root.
        let lexical = policy_for(root.join("innocent"));
        lexical.validate(HostOs::Linux).expect("lexically this looks fine");
        let error = launcher.wrap(&program, &lexical, "s1").unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid);
        assert!(error.message.contains("overlaps"), "{}", error.message);
        // A link to `/` would re-open the denied root.
        let error = launcher.wrap(&program, &policy_for(root.join("everything")), "s2").unwrap_err();
        assert!(error.message.contains("re-open"), "{}", error.message);
        // A harmless link is written as its target.
        let wrapped = launcher.wrap(&program, &policy_for(root.join("proj-link")), "s3").unwrap();
        let written: Value = serde_json::from_slice(&std::fs::read(wrapped.session.settings_path()).unwrap()).unwrap();
        let allow_write = paths(&written["filesystem"]["allowWrite"]);
        assert_eq!(allow_write[0], root.join("real-proj").display().to_string());
        assert!(!allow_write.iter().any(|path| path.contains("proj-link")));
        drop(wrapped);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Windows: a junction to the private HOME root is refused the same way
    /// (compiled everywhere Windows is checked; runs on Windows only).
    #[cfg(windows)]
    #[test]
    fn junctioned_authorized_folder_is_judged_by_its_target() {
        let root = temp("junction");
        let runtime = root.join("runtime");
        let node = runtime.join("node").join("node.exe");
        let srt_prefix = runtime.join("srt");
        let backend_dir = runtime.join("codex");
        let homes_root = root.join("fox").join("sandbox-homes");
        std::fs::create_dir_all(&homes_root).unwrap();
        let junction = root.join("innocent");
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&homes_root)
            .status()
            .expect("cmd mklink");
        assert!(status.success(), "mklink /J failed");
        assert_eq!(canonical_existing_path(&junction).unwrap(), canonical_existing_path(&homes_root).unwrap());
        let homes = PrivateHomes::new(homes_root.clone());
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: junction.clone(), writable: true }],
            homes.home_for(PROJECT, AgentBackend::Codex).unwrap(),
            vec![backend_dir.clone()],
            vec![],
            HostOs::Windows,
        );
        let launcher = SrtLauncher::new(windows_ready(&node.display().to_string(), &srt_prefix.display().to_string()), root.join("state"))
            .with_managed_runtime_root(runtime.clone())
            .with_windows_user_dirs(vec![])
            .with_ancestor_bases(Default::default())
            .with_windows_acl(std::sync::Arc::new(FakeAcl::default()));
        let error = launcher.wrap(&spec(&backend_dir.join("codex-acp.cmd").display().to_string()), &policy, "s1").unwrap_err();
        assert_eq!(error.code, SandboxErrorCode::SandboxPolicyInvalid, "{error}");
        // UNC paths are refused outright.
        assert!(canonical_managed_path(Path::new(r"\\server\share\proj")).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ---- delivered files ----

    fn delivery_policy(root: &Path) -> SandboxPolicy {
        std::fs::create_dir_all(root.join("proj")).unwrap();
        std::fs::create_dir_all(root.join("ref")).unwrap();
        let homes = PrivateHomes::new(root.join("fox").join("sandbox-homes"));
        let home = homes.home_for(PROJECT, AgentBackend::Pi).unwrap();
        std::fs::create_dir_all(&home.path).unwrap();
        let mut policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: root.join("proj"), writable: true }, AuthorizedDir { path: root.join("ref"), writable: false }],
            home,
            vec![],
            vec![],
            HostOs::current(),
        );
        let tmp = root.join("state").join("sandbox-tmp").join("s1");
        std::fs::create_dir_all(&tmp).unwrap();
        policy.session_tmp = Some(tmp);
        policy
    }

    #[test]
    fn delivered_files_must_be_real_files_in_a_writable_authorized_folder() {
        let root = temp("deliver");
        let policy = delivery_policy(&root);
        let rejected = |path: &Path| {
            let error = verify_delivered_file(&policy, path).unwrap_err();
            assert_eq!(error.code, SandboxErrorCode::DeliveredFileRejected, "{}", path.display());
            assert!(error.remedy.is_some());
            error.message
        };
        let report = root.join("proj").join("report.md");
        std::fs::write(&report, "# 结果").unwrap();
        let delivered = verify_delivered_file(&policy, &report).unwrap();
        assert_eq!(delivered.path, report);
        assert_eq!(delivered.authorized_root, root.join("proj"));
        assert_eq!(delivered.len, "# 结果".len() as u64);
        // Empty files are files too.
        std::fs::write(root.join("proj").join("empty.txt"), "").unwrap();
        assert_eq!(verify_delivered_file(&policy, &root.join("proj").join("empty.txt")).unwrap().len, 0);

        rejected(&root.join("proj").join("never-written.md"));
        rejected(&root.join("proj"));
        std::fs::write(root.join("ref").join("a.md"), "x").unwrap();
        assert!(rejected(&root.join("ref").join("a.md")).contains("read-only"));
        let in_tmp = policy.session_tmp.clone().unwrap().join("out.md");
        std::fs::write(&in_tmp, "x").unwrap();
        assert!(rejected(&in_tmp).contains("private"));
        let in_home = policy.private_home.join("out.md");
        std::fs::write(&in_home, "x").unwrap();
        assert!(rejected(&in_home).contains("private"));
        let outside = root.join("elsewhere.md");
        std::fs::write(&outside, "x").unwrap();
        assert!(rejected(&outside).contains("outside"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn delivered_symlinks_are_judged_by_their_target_and_unreadable_files_are_refused() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("deliver-link");
        let policy = delivery_policy(&root);
        let outside = root.join("secret.txt");
        std::fs::write(&outside, "x").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("proj").join("looks-inside.txt")).unwrap();
        let error = verify_delivered_file(&policy, &root.join("proj").join("looks-inside.txt")).unwrap_err();
        assert!(error.message.contains("outside"), "{}", error.message);
        std::fs::write(root.join("proj").join("real.txt"), "x").unwrap();
        std::os::unix::fs::symlink(root.join("proj").join("real.txt"), root.join("proj").join("alias.txt")).unwrap();
        assert_eq!(verify_delivered_file(&policy, &root.join("proj").join("alias.txt")).unwrap().path, root.join("proj").join("real.txt"));
        // Unreadable (meaningless as root, who can read anything).
        if unsafe { libc::geteuid() } != 0 {
            let locked = root.join("proj").join("locked.txt");
            std::fs::write(&locked, "x").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
            let error = verify_delivered_file(&policy, &locked).unwrap_err();
            assert!(error.message.contains("cannot be opened"), "{}", error.message);
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Linux: an authorized folder on tmpfs (here /dev/shm) does not persist;
    /// a delivery there is refused. Skipped when /dev/shm is not a writable
    /// tmpfs.
    #[cfg(target_os = "linux")]
    #[test]
    fn delivered_files_on_tmpfs_are_refused() {
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() || !on_tmpfs(shm) {
            eprintln!("SKIPPED delivered_files_on_tmpfs_are_refused: /dev/shm is not a tmpfs here");
            return;
        }
        let dir = shm.join(format!("fox-deliver-{}", std::process::id()));
        if std::fs::create_dir_all(&dir).is_err() {
            eprintln!("SKIPPED delivered_files_on_tmpfs_are_refused: /dev/shm not writable");
            return;
        }
        let root = temp("deliver-tmpfs");
        let mut policy = delivery_policy(&root);
        policy.authorized.push(AuthorizedDir { path: dir.clone(), writable: true });
        std::fs::write(dir.join("out.md"), "x").unwrap();
        let error = verify_delivered_file(&policy, &dir.join("out.md")).unwrap_err();
        assert!(error.message.contains("tmpfs"), "{}", error.message);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root);
    }
}
