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
//! the Node runtime, the srt package itself). Windows cannot cheaply deny a
//! whole drive, so there only `C:\Users` (plus Host additions) is denied and
//! the capability declaration says so ([`ReadScope::UserDirsOnly`]). See
//! `docs/sandbox.md`.

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
}

impl SandboxErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SandboxUnavailable => "sandbox_unavailable",
            Self::SandboxNotInstalled => "sandbox_not_installed",
            Self::SandboxDependencyMissing => "sandbox_dependency_missing",
            Self::SandboxPolicyInvalid => "sandbox_policy_invalid",
            Self::SandboxBackendNotInstalled => "sandbox_backend_not_installed",
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
/// * Windows: `C:\Users` only. Other fixed drives (`D:\` ...) stay readable;
///   the Host may add their roots, which is an engineer-verification item
///   (docs/sandbox.md section 7). The declaration reports
///   [`ReadScope::UserDirsOnly`] there.
pub fn default_deny_read_roots(os: HostOs) -> Vec<PathBuf> {
    match os {
        HostOs::Linux | HostOs::MacOs | HostOs::Other => vec!["/".into()],
        HostOs::Windows => vec![r"C:\Users".into()],
    }
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
    /// No read boundary (full access, or nothing denied).
    None,
}

impl ReadScope {
    /// UI text (Chinese product copy) for this scope.
    pub fn ui_label(self, verified: bool) -> &'static str {
        match (self, verified) {
            (Self::AuthorizedOnly, true) => "读取限定在授权文件夹",
            (Self::AuthorizedOnly, false) => "读取限定在授权文件夹（本平台待实测）",
            (Self::UserDirsOnly, true) => "部分：仅用户目录受保护",
            (Self::UserDirsOnly, false) => "部分：仅用户目录受保护（本平台待实测）",
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

fn normalized(path: &Path, os: HostOs) -> String {
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
fn within(inner: &Path, outer: &Path, os: HostOs) -> bool {
    let inner = normalized(inner, os);
    let outer = normalized(outer, os);
    let sep = if os == HostOs::Windows { '\\' } else { '/' };
    inner == outer || inner.starts_with(&format!("{}{sep}", outer.trim_end_matches(sep)))
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
    /// [`ReadScope::AuthorizedOnly`]; on Windows (no whole-drive deny) any
    /// deny list is [`ReadScope::UserDirsOnly`].
    pub fn read_scope(&self, os: HostOs) -> ReadScope {
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
    pub fn is_readable(&self, path: &Path, os: HostOs) -> bool {
        let denied = self.deny_read_roots.iter().any(|root| within(path, root, os));
        !denied || self.readable_roots().iter().any(|root| within(path, root, os))
    }

    pub fn validate(&self, os: HostOs) -> Result<(), SandboxError> {
        let invalid = |message: String| Err(SandboxError::new(SandboxErrorCode::SandboxPolicyInvalid, message));
        if self.authorized.is_empty() {
            return invalid("no authorized directory".into());
        }
        if self.deny_read_roots.is_empty() {
            return invalid("no read-denied roots: srt would allow reading the whole disk".into());
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

    /// The srt settings document for this policy.
    pub fn to_srt_settings(&self) -> Value {
        let mut allow_read: Vec<Value> = Vec::new();
        let mut seen = BTreeSet::new();
        for root in self.readable_roots() {
            if seen.insert(root.to_path_buf()) {
                allow_read.push(literal(root));
            }
        }
        let mut allow_write = Vec::new();
        let mut seen = BTreeSet::new();
        for root in self.writable_roots() {
            if seen.insert(root.to_path_buf()) {
                allow_write.push(literal(root));
            }
        }
        json!({
            "network": {
                "allowedDomains": self.allowed_domains,
                "deniedDomains": [],
                "allowLocalBinding": false,
            },
            "filesystem": {
                "denyRead": self.deny_read_roots.iter().map(|root| literal(root)).collect::<Vec<_>>(),
                "allowRead": allow_read,
                "allowWrite": allow_write,
                "denyWrite": [],
            },
            "enableWeakerNestedSandbox": false,
            "allowNestedUserNamespaces": false,
        })
    }
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

/// Install prefix of a runtime binary: `<x>/bin/node` -> `<x>`, else its dir.
fn runtime_root(binary: &Path) -> Option<PathBuf> {
    let dir = binary.parent()?;
    if dir.file_name().is_some_and(|name| name == "bin") {
        dir.parent().map(Path::to_path_buf)
    } else {
        Some(dir.to_path_buf())
    }
}

impl SrtLauncher {
    pub fn new(ready: SandboxReady, state_root: PathBuf) -> Self {
        let tmp_root = state_root.join("sandbox-tmp");
        Self { ready, state_root, tmp_root }
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
        if let Some(dir) = Path::new(&spec.program).parent() {
            path_dirs.push(dir.to_string_lossy().into_owned());
        }
        if let Some(dir) = self.ready.install.node.parent() {
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
    fn wrap(&self, spec: &LaunchSpec, policy: &SandboxPolicy, session_id: &str) -> Result<SandboxedLaunch, SandboxError> {
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
        policy.validate(os)?;
        if !policy.is_readable(program, os) {
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

        let mut args = vec![
            self.ready.install.cli_js.to_string_lossy().into_owned(),
            "--settings".to_owned(),
            session.settings_path.to_string_lossy().into_owned(),
            "--".to_owned(),
            spec.program.clone(),
        ];
        args.extend(spec.args.iter().cloned());
        let mut session_env = spec.session_env.clone();
        for (key, value) in self.sandbox_env(spec, &policy) {
            session_env.retain(|(existing, _)| !existing.eq_ignore_ascii_case(&key));
            session_env.push((key, value));
        }
        let wrapped = LaunchSpec {
            backend: spec.backend,
            program: self.ready.install.node.to_string_lossy().into_owned(),
            args,
            version_pin: spec.version_pin,
            credential_env: spec.credential_env,
            experimental: spec.experimental,
            session_env,
        };
        Ok(SandboxedLaunch { spec: wrapped, session })
    }
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
    pub declaration: SandboxDeclaration,
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
            declaration: SandboxDeclaration::declare(PermissionMode::FullAccess, None, os),
        }),
        PermissionMode::Sandboxed => match setup {
            SandboxSetup::Unavailable(error) => Err(error),
            SandboxSetup::Ready { launcher, policy } => {
                let wrapped = launcher.wrap(&spec, policy, session_id)?;
                Ok(PreparedLaunch {
                    mode,
                    spec: wrapped.spec,
                    sandbox: Some(wrapped.session),
                    declaration: SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(policy), os),
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
    pub writable_roots: Vec<String>,
    pub readable_roots: Vec<String>,
    pub allowed_domains: Vec<String>,
}

impl SandboxDeclaration {
    pub fn declare(mode: PermissionMode, policy: Option<&SandboxPolicy>, os: HostOs) -> Self {
        match (mode, policy) {
            (PermissionMode::Sandboxed, Some(policy)) => {
                let enforcement = srt_enforcement(os);
                let read_scope = policy.read_scope(os);
                let read_scope_verified = srt_read_scope_verified(os);
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
                    writable_roots: policy.writable_roots().iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                    readable_roots: policy.readable_roots().iter().map(|p| p.to_string_lossy().into_owned()).collect(),
                    allowed_domains: policy.allowed_domains.clone(),
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
                allowed_domains: Vec::new(),
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

    #[test]
    fn macos_denies_the_root_and_windows_only_the_profile_root() {
        assert_eq!(default_deny_read_roots(HostOs::MacOs), [PathBuf::from("/")]);
        assert!(default_system_read_only(HostOs::MacOs).contains(&PathBuf::from("/System")));
        assert!(default_system_read_only(HostOs::Windows).is_empty());
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: r"C:\Users\u\proj".into(), writable: true }],
            PathBuf::from(r"C:\Users\u\AppData\Local\Fox\backends\codex\home"),
            vec![],
            vec![],
            HostOs::Windows,
        );
        policy.validate(HostOs::Windows).unwrap();
        assert_eq!(paths(&policy.to_srt_settings()["filesystem"]["denyRead"]), [r"C:\Users"]);
        assert!(!policy.is_readable(Path::new(r"c:\users\u\Desktop\x.txt"), HostOs::Windows));
        assert!(policy.is_readable(Path::new(r"C:\Users\u\proj\a.txt"), HostOs::Windows));
        // Partial boundary, stated as such: other drives stay readable.
        assert!(policy.is_readable(Path::new(r"D:\data\x.csv"), HostOs::Windows));
        assert_eq!(policy.read_scope(HostOs::Windows), ReadScope::UserDirsOnly);
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

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fox-sandbox-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn launcher(state_root: PathBuf) -> SrtLauncher {
        let ready = preflight(&install(), &FakeProbe::linux_ok()).unwrap();
        SrtLauncher::new(ready, state_root)
    }

    #[test]
    fn srt_wrap_writes_per_session_settings_and_points_home_at_the_private_dir() {
        let root = temp("wrap");
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
            wrapped.spec.args,
            [
                "/opt/fox/srt/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js".to_owned(),
                "--settings".to_owned(),
                settings_path.display().to_string(),
                "--".to_owned(),
                "/opt/fox/backends/codex/bin/codex-acp".to_owned(),
                "--flag".to_owned(),
            ]
        );
        let env: BTreeMap<_, _> = wrapped.spec.session_env.iter().cloned().collect();
        let tmp = state.join("sandbox-tmp").join("sess-1");
        assert_eq!(wrapped.session.tmp_dir(), Some(tmp.as_path()));
        assert_eq!(env["HOME"], home.display().to_string(), "private HOME overrides the spec's HOME");
        assert_eq!(env["CODEX_HOME"], home.join(".codex").display().to_string());
        assert_eq!(env["TMPDIR"], tmp.display().to_string(), "temp is per session, not in the shared HOME");
        assert_eq!(env["TEMP"], tmp.display().to_string());
        assert_eq!(env["CLAUDE_CODE_TMPDIR"], tmp.display().to_string(), "srt hands this to the child as TMPDIR");
        assert_eq!(env["NODE_USE_ENV_PROXY"], "1");
        assert_eq!(env["INITIAL_AGENT_MODE"], "read-only", "backend session vars kept");
        assert_eq!(env["PATH"], "/opt/fox/backends/codex/bin:/opt/fox/node/bin:/usr/local/bin:/usr/bin:/bin");
        assert_eq!(wrapped.spec.session_env.iter().filter(|(k, _)| k == "HOME").count(), 1);

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

    #[test]
    fn srt_wrap_refuses_npx_unreadable_programs_and_writable_state() {
        let root = temp("refuse");
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
        assert_eq!(ReadScope::AuthorizedOnly.ui_label(true), "读取限定在授权文件夹");

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
        let windows_policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: r"C:\Users\u\proj".into(), writable: true }],
            PathBuf::from(r"C:\Users\u\AppData\Local\Fox\sandbox-homes\x\codex"),
            vec![],
            vec![],
            HostOs::Windows,
        );
        let windows = SandboxDeclaration::declare(PermissionMode::Sandboxed, Some(&windows_policy), HostOs::Windows);
        assert_eq!((windows.read_scope, windows.read_scope_verified), (ReadScope::UserDirsOnly, false));
        assert_eq!(windows.read_scope.ui_label(windows.read_scope_verified), "部分：仅用户目录受保护（本平台待实测）");
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
        // Windows profile with a Chinese user name.
        let homes = PrivateHomes::new(r"C:\Users\张三\AppData\Local\Fox\sandbox-homes".into());
        let policy = SandboxPolicy::new(
            vec![AuthorizedDir { path: r"C:\Users\张三\项目".into(), writable: true }],
            homes.home_for(PROJECT, AgentBackend::Codex).unwrap(),
            vec![],
            vec![],
            HostOs::Windows,
        );
        policy.validate(HostOs::Windows).unwrap();
        assert!(!policy.is_readable(Path::new(r"C:\Users\张三\桌面\a.txt"), HostOs::Windows));
    }

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
}
