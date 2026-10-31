//! Host wiring: conversation-level Agent backend routing.
//!
//! ## Supported Agent paths (plan C1, 2026-10-31)
//!
//! A conversation is driven by one of four backends, chosen once at creation:
//!
//! * [`AgentBackend::Fox`]: the built-in route (PiNative). `fox` is the
//!   storage id of the built-in route, not a driver name; renaming it to `pi`
//!   would route new conversations to the external Pi ACP path.
//! * [`AgentBackend::Pi`] (external pi over ACP), [`AgentBackend::Codex`]
//!   (codex-acp) and [`AgentBackend::Deepseek`] (dsh): the external Agents.
//!
//! The single Pi mainline of 2026-10-27 had retired the three external
//! backends; plan C1 brings them back. The retirement MECHANISM is kept
//! ([`RETIRED_BACKENDS`], [`RouteError::Retired`], [`SessionAccess::ReadOnlyRetired`],
//! `agent_backend_retired`) for a backend that is really taken out of service
//! later; in this build the list is empty.
//!
//! ## Rules implemented
//!
//! * A conversation's `agent_backend` is chosen once, at creation, and never
//!   changes (the database also refuses the update with a trigger, see
//!   [`AGENT_BACKEND_MIGRATION_SQL`]).
//! * Every entry (start / cancel / approve / resume) routes by the frozen
//!   value. A request that names a different backend is refused as misrouted;
//!   it is never silently rebound.
//! * Existing conversations are mapped to `fox` by the migration's column
//!   default (one-shot, plan 6.6). The legacy-path marker keeps living where it
//!   always did (`kernel_runs.kernel_mode = 'legacy'`); it is not rewritten.
//! * A conversation frozen to a retired backend (none in this build) is
//!   read-only: history is shown, no runtime is ever started, and every
//!   runtime entry is refused with [`RouteError::CODE_RETIRED`]. It is never
//!   quietly re-driven by the Fox route.
//! * [`EXTERNAL_BACKENDS_ENV`] is the administrator's off switch: unset (or
//!   `all`) leaves every external backend on, `0` / a list narrows it. It can
//!   only switch a backend OFF; it never revives a retired one. A conversation
//!   frozen to a switched-off backend stays readable (`read_only_disabled`).
//! * A lookup of the frozen value that fails (storage error, missing column)
//!   falls back to the Fox path only while no external backend is on and the
//!   request names none; otherwise it is refused (`agent_backend_lookup_failed`),
//!   so a possibly external conversation never reaches the Fox runtime because
//!   a read failed.
//! * `engine_id` keeps its historical meaning; [`audit_line`] always prints it
//!   next to `agent_backend`.
use crate::port::AgentBackend;
use std::collections::BTreeSet;

/// The administrator's switch for the external backends. It can only turn a
/// supported external backend OFF; it never revives a retired one.
///
/// Accepted values:
/// * unset / empty, `all` / `1` / `on` / `true` / `yes`: every supported
///   external backend on (the default);
/// * `0` / `off` / `false` / `none` / `no`: every external backend off;
/// * a comma- or whitespace-separated list of backend ids (`pi,codex`): only
///   those on. `fox` in the list is ignored (always on). Unknown ids are
///   reported by [`BackendSwitch::unknown_entries`], never treated as "all".
pub const EXTERNAL_BACKENDS_ENV: &str = "FOX_EXTERNAL_BACKENDS";

/// Additive migration for the conversation-level identity field.
///
/// * `ALTER TABLE ... ADD COLUMN` with a constant `NOT NULL DEFAULT 'fox'` is a
///   metadata-only change in SQLite: no existing row is rewritten, and every
///   existing conversation reads as `fox` (plan 6.6: old Fox runs, including
///   old `codex` / `deepseek_harness` model adapters and legacy-path runs, all
///   map to `fox`).
/// * The CHECK keeps the column to the four public backend ids, so a retired
///   conversation's value is still a legal, readable value.
/// * The trigger makes the value immutable after insert. It is a `BEFORE
///   UPDATE OF agent_backend` trigger, so updates that do not touch the column
///   are unaffected, and an update that writes the same value is allowed.
///
/// The desktop migration runner records this as schema version 95.
pub const AGENT_BACKEND_MIGRATION_SQL: &str = r#"
ALTER TABLE conversations ADD COLUMN agent_backend TEXT NOT NULL DEFAULT 'fox'
    CHECK(agent_backend IN ('fox', 'pi', 'codex', 'deepseek'));

CREATE TRIGGER IF NOT EXISTS conversations_agent_backend_immutable
BEFORE UPDATE OF agent_backend ON conversations
WHEN NEW.agent_backend IS NOT OLD.agent_backend
BEGIN
    SELECT RAISE(ABORT, 'agent_backend_immutable');
END;
"#;

/// Schema version the desktop records for [`AGENT_BACKEND_MIGRATION_SQL`].
pub const AGENT_BACKEND_SCHEMA_VERSION: i64 = 95;

/// Schema v96 (UX batch ①②): the per-conversation full-access grant the
/// user chose when creating an external conversation (semantics A: chosen at
/// creation, locked, revocable mid-session, never enabled mid-session), and
/// an audit log for external-backend Host actions (credential import:
/// file / key names only). Additive; no existing row is touched.
///
/// The external Agents' full access now lives in their own native permission
/// mode ([`crate::native_permission`]); no new grant row is created and the
/// table stays for the records already made with it.
pub const FULL_ACCESS_MIGRATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_backend_full_access_grants (
    grant_key TEXT PRIMARY KEY CHECK(grant_key = 'conversation:' || conversation_id),
    conversation_id TEXT NOT NULL UNIQUE REFERENCES conversations(id) ON DELETE CASCADE,
    granted_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_backend_audit_log (
    id TEXT PRIMARY KEY,
    conversation_id TEXT,
    agent_backend TEXT NOT NULL,
    event TEXT NOT NULL,
    details_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_agent_backend_audit_log_created
    ON agent_backend_audit_log(created_at);
"#;

/// Schema version the desktop records for [`FULL_ACCESS_MIGRATION_SQL`].
pub const FULL_ACCESS_SCHEMA_VERSION: i64 = 96;

/// Permission mode a new external conversation can be created with: no
/// full-access grant ([`crate::external_home::PermissionMode::Standard`]) or
/// full access. The pre-B3 name of the first,
/// [`crate::external_home::LEGACY_STANDARD_MODE`] (`"sandboxed"`), is still
/// accepted.
pub const NEW_MODE_STANDARD: &str = "standard";
pub const NEW_MODE_FULL_ACCESS: &str = "full_access";

/// Refusal code: full access requested where it cannot be granted.
pub const CODE_FULL_ACCESS_FORBIDDEN: &str = "agent_backend_full_access_forbidden";

/// The backends this build can start, in display order (plan C1: the
/// built-in route plus the three external Agents).
pub const SUPPORTED_BACKENDS: [AgentBackend; 4] = [AgentBackend::Fox, AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek];

/// Backends really taken out of service. Their recorded values stay
/// readable; nothing of theirs is ever started (`agent_backend_retired`).
/// Empty in this build: the mechanism is kept for a later retirement.
pub const RETIRED_BACKENDS: [AgentBackend; 0] = [];

/// Whether this backend is retired in this build (readable, never runnable).
pub fn is_retired(backend: AgentBackend) -> bool {
    is_retired_in(&RETIRED_BACKENDS, backend)
}

fn is_retired_in(retired: &[AgentBackend], backend: AgentBackend) -> bool {
    backend != AgentBackend::Fox && retired.contains(&backend)
}

/// Whether this backend is a supported (startable) path of this build.
pub fn is_supported(backend: AgentBackend) -> bool {
    SUPPORTED_BACKENDS.contains(&backend) && !is_retired(backend)
}

/// Whether a new external conversation starts in its Agent's own full-access
/// mode. `requested` is the UI's choice (absent = standard). Full access here
/// is only for external backends (a Fox conversation sets it through its own
/// execution mode) and is refused outright (not silently downgraded) when the
/// machine forbids it (`FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1`), so a UI that
/// ignores the disabled option cannot get around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRefusal {
    pub code: &'static str,
    pub message: String,
}

pub fn decide_new_conversation_mode(requested: Option<&str>, backend: AgentBackend, forbidden: bool) -> Result<bool, ModeRefusal> {
    match requested.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(false),
        Some(NEW_MODE_STANDARD | crate::external_home::LEGACY_STANDARD_MODE) => Ok(false),
        Some(NEW_MODE_FULL_ACCESS) if backend == AgentBackend::Fox => Err(ModeRefusal {
            code: RouteError::CODE_INVALID,
            message: "Fox 会话的完全访问在输入框的执行方式里开启，不在新建外部会话时选择".into(),
        }),
        Some(NEW_MODE_FULL_ACCESS) if forbidden => Err(ModeRefusal {
            code: CODE_FULL_ACCESS_FORBIDDEN,
            message: "本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1），外部后端不能获得完全访问".into(),
        }),
        Some(NEW_MODE_FULL_ACCESS) => Ok(true),
        Some(other) => Err(ModeRefusal { code: RouteError::CODE_INVALID, message: format!("未知的权限模式：{other}") }),
    }
}

/// The four Host entries that touch a runtime (plan 5.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HostEntry {
    Start,
    Cancel,
    Approve,
    Resume,
}

impl HostEntry {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Cancel => "cancel",
            Self::Approve => "approve",
            Self::Resume => "resume",
        }
    }
}

/// What [`EXTERNAL_BACKENDS_ENV`] asked for. It narrows the supported
/// external backends (administrator off switch); it never enables a retired
/// one ([`Self::is_enabled`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendSwitch {
    requested: BTreeSet<AgentBackend>,
    unknown: Vec<String>,
}

impl BackendSwitch {
    /// Nothing requested (`FOX_EXTERNAL_BACKENDS=0`).
    pub fn all_off() -> Self {
        Self::default()
    }

    /// Every external backend requested (`FOX_EXTERNAL_BACKENDS=all`).
    pub fn all_on() -> Self {
        Self {
            requested: AgentBackend::ALL.into_iter().filter(|backend| *backend != AgentBackend::Fox).collect(),
            unknown: Vec::new(),
        }
    }

    /// Parse the raw value of [`EXTERNAL_BACKENDS_ENV`].
    pub fn from_env_value(value: Option<&str>) -> Self {
        let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Self::all_on();
        };
        let lowered = raw.to_ascii_lowercase();
        match lowered.as_str() {
            "0" | "off" | "false" | "none" | "no" => return Self::all_off(),
            "1" | "on" | "true" | "all" | "yes" => return Self::all_on(),
            _ => {}
        }
        let mut switch = Self::all_off();
        for entry in lowered
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|entry| !entry.is_empty())
        {
            match AgentBackend::parse(entry) {
                Some(AgentBackend::Fox) => {}
                Some(backend) => {
                    switch.requested.insert(backend);
                }
                None => switch.unknown.push(entry.to_owned()),
            }
        }
        switch
    }

    /// Read [`EXTERNAL_BACKENDS_ENV`] from the process environment.
    pub fn from_env() -> Self {
        Self::from_env_value(std::env::var(EXTERNAL_BACKENDS_ENV).ok().as_deref())
    }

    /// Whether this backend may run. The Fox path always may; a supported
    /// external backend may when the variable leaves it on; a retired backend
    /// never may, whatever the variable asked for.
    pub fn is_enabled(&self, backend: AgentBackend) -> bool {
        self.is_enabled_in(&RETIRED_BACKENDS, backend)
    }

    fn is_enabled_in(&self, retired: &[AgentBackend], backend: AgentBackend) -> bool {
        match backend {
            AgentBackend::Fox => true,
            other => SUPPORTED_BACKENDS.contains(&other) && !is_retired_in(retired, other) && self.requested.contains(&other),
        }
    }

    /// Whether the environment variable asked for this backend (before
    /// retirement is applied).
    pub fn request_includes(&self, backend: AgentBackend) -> bool {
        self.requested.contains(&backend)
    }

    /// Entries of the variable that name no backend (reported, never "all").
    pub fn unknown_entries(&self) -> &[String] {
        &self.unknown
    }

    /// Whether any external backend may run.
    pub fn any_external(&self) -> bool {
        AgentBackend::ALL
            .into_iter()
            .filter(|backend| *backend != AgentBackend::Fox)
            .any(|backend| self.is_enabled(backend))
    }
}

/// A routing refusal. Each variant has a stable, machine-readable code that
/// the desktop returns as `ApiError.code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteError {
    /// The request named a value that is not one of the four backend ids.
    InvalidBackend(String),
    /// The stored value is not a valid backend id (corrupt row). Fail closed.
    InvalidStoredBackend(String),
    /// The administrator switched this backend off ([`EXTERNAL_BACKENDS_ENV`]).
    /// A retired backend is refused with [`Self::Retired`] instead.
    Disabled {
        backend: AgentBackend,
        existing_conversation: bool,
    },
    /// The backend is retired in this build (none is, today). `existing_conversation`
    /// is true for a conversation frozen to it (read-only history), false for
    /// a new conversation that asked for it (refused).
    Retired {
        backend: AgentBackend,
        existing_conversation: bool,
    },
    /// The request named another backend than the conversation's frozen one.
    Misrouted {
        entry: HostEntry,
        bound: AgentBackend,
        requested: AgentBackend,
    },
    /// The backend is enabled but its runtime is not wired. No longer
    /// produced; kept for the code.
    NotYetWired {
        entry: HostEntry,
        backend: AgentBackend,
    },
    /// The entry has no meaning for an external backend (resume of a yuxi /
    /// Kernel run).
    EntryUnsupported {
        entry: HostEntry,
        backend: AgentBackend,
    },
    /// The frozen value could not be read while an external backend may be
    /// involved (retryable).
    LookupFailed {
        entry: HostEntry,
        detail: String,
    },
    /// A new conversation asked for a backend that is hidden from users
    /// ([`AgentBackend::HIDDEN`]). Existing conversations are never refused
    /// for this reason.
    Hidden(AgentBackend),
}

impl RouteError {
    pub const CODE_INVALID: &'static str = "agent_backend_invalid";
    pub const CODE_DISABLED: &'static str = "agent_backend_disabled";
    /// The backend is retired in this build (readable, never runnable). Kept
    /// for a backend that is really taken out of service later.
    pub const CODE_RETIRED: &'static str = "agent_backend_retired";
    pub const CODE_MISROUTED: &'static str = "agent_backend_misrouted";
    pub const CODE_NOT_YET_WIRED: &'static str = "agent_backend_not_yet_wired";
    pub const CODE_ENTRY_UNSUPPORTED: &'static str = "agent_backend_entry_unsupported";
    pub const CODE_LOOKUP_FAILED: &'static str = "agent_backend_lookup_failed";
    pub const CODE_HIDDEN: &'static str = "agent_backend_hidden";

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidBackend(_) | Self::InvalidStoredBackend(_) => Self::CODE_INVALID,
            Self::Disabled { .. } => Self::CODE_DISABLED,
            Self::Retired { .. } => Self::CODE_RETIRED,
            Self::Misrouted { .. } => Self::CODE_MISROUTED,
            Self::NotYetWired { .. } => Self::CODE_NOT_YET_WIRED,
            Self::EntryUnsupported { .. } => Self::CODE_ENTRY_UNSUPPORTED,
            Self::LookupFailed { .. } => Self::CODE_LOOKUP_FAILED,
            Self::Hidden(_) => Self::CODE_HIDDEN,
        }
    }

    /// User-facing message (Simplified Chinese, matching the desktop UI).
    pub fn message(&self) -> String {
        match self {
            Self::InvalidBackend(value) => {
                format!("不支持的 Agent 后端「{value}」，可选值为 fox、pi、codex、deepseek")
            }
            Self::InvalidStoredBackend(value) => {
                format!("会话记录的 Agent 后端「{value}」无法识别，已拒绝启动任何运行时")
            }
            Self::Disabled {
                backend,
                existing_conversation: false,
            } => format!(
                "外部 Agent 后端「{}」已被管理员关闭（环境变量 {EXTERNAL_BACKENDS_ENV}），不能用于新会话",
                backend.as_str()
            ),
            Self::Disabled {
                backend,
                existing_conversation: true,
            } => format!(
                "此会话绑定外部 Agent 后端「{}」，该后端已被管理员关闭（{EXTERNAL_BACKENDS_ENV}），会话只读，不会启动运行时",
                backend.as_str()
            ),
            Self::Retired {
                backend,
                existing_conversation: false,
            } => format!(
                "Agent 后端「{}」已在本版本退休：不能再选择或启动它",
                backend.as_str()
            ),
            Self::Retired {
                backend,
                existing_conversation: true,
            } => format!(
                "此会话绑定已退休的 Agent 后端「{}」，只能查看历史，不会启动运行时，也不会改用其他驱动重跑",
                backend.as_str()
            ),
            Self::Misrouted {
                entry,
                bound,
                requested,
            } => format!(
                "错投：此会话已绑定 Agent 后端「{}」，请求的 {} 却指向「{}」，已拒绝",
                bound.as_str(),
                entry.as_str(),
                requested.as_str()
            ),
            Self::NotYetWired { entry, backend } => format!(
                "外部 Agent 后端「{}」已启用，但本版本尚未接通其运行时（{}），请改用 Fox 会话",
                backend.as_str(),
                entry.as_str()
            ),
            Self::EntryUnsupported { entry, backend } => format!(
                "外部 Agent 后端「{}」的会话不支持此操作（{}）；请直接发送新消息继续",
                backend.as_str(),
                entry.as_str()
            ),
            Self::LookupFailed { entry, detail } => format!(
                "无法读取会话绑定的 Agent 后端（{}），请稍后重试：{detail}",
                entry.as_str()
            ),
            Self::Hidden(backend) => format!(
                "Agent「{}」暂未开放，不能用于新会话；已有会话不受影响",
                backend.as_str()
            ),
        }
    }

    /// Whether retrying the same request can succeed without a change.
    /// Retirement and the administrator switch are decisions, not transient
    /// conditions.
    pub fn retryable(&self) -> bool {
        matches!(self, Self::LookupFailed { .. })
    }
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for RouteError {}

/// Parse an optional request value. Absent or empty means "not specified".
pub fn parse_requested(value: Option<&str>) -> Result<Option<AgentBackend>, RouteError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(None),
        Some(raw) => AgentBackend::parse(raw)
            .map(Some)
            .ok_or_else(|| RouteError::InvalidBackend(raw.to_owned())),
    }
}

/// Read the frozen value from storage.
///
/// `None` only happens when the column is absent or NULL, which the migration
/// rules out; it is mapped to `fox`, the same one-shot mapping the migration
/// default applies to every pre-existing conversation. Any other unknown value
/// fails closed.
pub fn bound_backend(stored: Option<&str>) -> Result<AgentBackend, RouteError> {
    match stored {
        None => Ok(AgentBackend::Fox),
        Some(raw) => {
            AgentBackend::parse(raw).ok_or_else(|| RouteError::InvalidStoredBackend(raw.to_owned()))
        }
    }
}

/// Decide the backend a new conversation is frozen to.
///
/// Absent → `fox`. A retired, hidden or switched-off backend is a refusal
/// naming that backend, never a silent substitution by another driver: a
/// caller that asked for Codex must be told why it cannot have Codex, not
/// receive a Fox conversation.
pub fn choose_for_new_conversation(
    requested: Option<&str>,
    switch: &BackendSwitch,
) -> Result<AgentBackend, RouteError> {
    choose_for_new_conversation_in(&RETIRED_BACKENDS, requested, switch)
}

fn choose_for_new_conversation_in(
    retired: &[AgentBackend],
    requested: Option<&str>,
    switch: &BackendSwitch,
) -> Result<AgentBackend, RouteError> {
    let backend = parse_requested(requested)?.unwrap_or(AgentBackend::Fox);
    if is_retired_in(retired, backend) {
        return Err(RouteError::Retired {
            backend,
            existing_conversation: false,
        });
    }
    if backend.hidden() {
        return Err(RouteError::Hidden(backend));
    }
    if !switch.is_enabled_in(retired, backend) {
        return Err(RouteError::Disabled {
            backend,
            existing_conversation: false,
        });
    }
    Ok(backend)
}

/// Where an accepted entry goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The existing Fox path (Kernel / legacy / yuxi selection unchanged).
    Fox,
    /// The external runtime of this backend (ACP, see `crate::external`).
    External(AgentBackend),
}

/// What to do when the frozen value could not be read (storage error,
/// pre-v95 column). `Ok(())` = continue on the Fox path (audited by the
/// caller); `Err` = refuse.
///
/// * an explicit request that names a retired backend: refused as retired;
/// * every external backend off and the request names none: the Fox path
///   (nothing else could run);
/// * otherwise: refused with `agent_backend_lookup_failed` (retryable). A
///   conversation that may be bound to an external backend must never run on
///   the Fox runtime because a read failed.
pub fn on_lookup_failure(
    entry: HostEntry,
    requested: Option<&str>,
    switch: &BackendSwitch,
    detail: &str,
) -> Result<(), RouteError> {
    on_lookup_failure_in(&RETIRED_BACKENDS, entry, requested, switch, detail)
}

fn on_lookup_failure_in(
    retired: &[AgentBackend],
    entry: HostEntry,
    requested: Option<&str>,
    switch: &BackendSwitch,
    detail: &str,
) -> Result<(), RouteError> {
    let requested = parse_requested(requested)?;
    if let Some(backend) = requested.filter(|backend| is_retired_in(retired, *backend)) {
        return Err(RouteError::Retired { backend, existing_conversation: true });
    }
    let any_external = AgentBackend::ALL
        .into_iter()
        .filter(|backend| *backend != AgentBackend::Fox)
        .any(|backend| switch.is_enabled_in(retired, backend));
    let names_external = requested.is_some_and(|backend| backend != AgentBackend::Fox);
    if any_external || names_external {
        let detail: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(RouteError::LookupFailed { entry, detail });
    }
    Ok(())
}

/// Route one Host entry for a conversation whose frozen backend is `bound`.
///
/// Order of checks: a misrouted request is refused first (whatever the
/// backend), then retirement, then the administrator switch, then entries
/// that have no meaning for an external backend (resume).
pub fn route(
    entry: HostEntry,
    bound: AgentBackend,
    requested: Option<AgentBackend>,
    switch: &BackendSwitch,
) -> Result<Route, RouteError> {
    route_in(&RETIRED_BACKENDS, entry, bound, requested, switch)
}

fn route_in(
    retired: &[AgentBackend],
    entry: HostEntry,
    bound: AgentBackend,
    requested: Option<AgentBackend>,
    switch: &BackendSwitch,
) -> Result<Route, RouteError> {
    if let Some(requested) = requested {
        if requested != bound {
            return Err(RouteError::Misrouted {
                entry,
                bound,
                requested,
            });
        }
    }
    match bound {
        AgentBackend::Fox => Ok(Route::Fox),
        external if is_retired_in(retired, external) => Err(RouteError::Retired {
            backend: external,
            existing_conversation: true,
        }),
        external if !switch.is_enabled_in(retired, external) => Err(RouteError::Disabled {
            backend: external,
            existing_conversation: true,
        }),
        external if entry == HostEntry::Resume => Err(RouteError::EntryUnsupported { entry, backend: external }),
        external => Ok(Route::External(external)),
    }
}

/// Convenience for the glue: parse the stored and requested strings, then route.
pub fn route_stored(
    entry: HostEntry,
    stored: Option<&str>,
    requested: Option<&str>,
    switch: &BackendSwitch,
) -> Result<(AgentBackend, Route), RouteError> {
    let bound = bound_backend(stored)?;
    let requested = parse_requested(requested)?;
    route(entry, bound, requested, switch).map(|route| (bound, route))
}

/// How the UI may present an existing conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAccess {
    /// Runtime entries are accepted.
    Runnable,
    /// History is shown; no runtime is started (the administrator switched
    /// the backend off, [`EXTERNAL_BACKENDS_ENV`]).
    ReadOnlyDisabled,
    /// History is shown and the backend is not wired. No longer produced.
    ReadOnlyNotWired,
    /// History is shown; the backend is retired in this build (none is,
    /// today). No runtime is started and the conversation is not re-bound to
    /// another driver.
    ReadOnlyRetired,
}

impl SessionAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runnable => "runnable",
            Self::ReadOnlyDisabled => "read_only_disabled",
            Self::ReadOnlyNotWired => "read_only_not_wired",
            Self::ReadOnlyRetired => "read_only_retired",
        }
    }
}

/// Reading a conversation never fails because of retirement or the switch:
/// keeping old data readable is the rollback path (plan 6.6).
pub fn session_access(bound: AgentBackend, switch: &BackendSwitch) -> SessionAccess {
    session_access_in(&RETIRED_BACKENDS, bound, switch)
}

fn session_access_in(retired: &[AgentBackend], bound: AgentBackend, switch: &BackendSwitch) -> SessionAccess {
    match bound {
        AgentBackend::Fox => SessionAccess::Runnable,
        external if is_retired_in(retired, external) => SessionAccess::ReadOnlyRetired,
        external if !switch.is_enabled_in(retired, external) => SessionAccess::ReadOnlyDisabled,
        _ => SessionAccess::Runnable,
    }
}

/// Availability of each backend for the new-conversation picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOption {
    pub backend: AgentBackend,
    pub enabled: bool,
    /// Runtime wired in this build. False only for a retired backend.
    pub wired: bool,
    pub experimental: bool,
    /// Hidden from user-facing choices ([`AgentBackend::HIDDEN`]).
    pub hidden: bool,
    /// Retired in this build: readable history, no runtime.
    pub retired: bool,
    /// Why it cannot be picked / used, for the UI tooltip.
    pub reason: Option<String>,
}

pub fn backend_options(switch: &BackendSwitch) -> Vec<BackendOption> {
    backend_options_in(&RETIRED_BACKENDS, switch)
}

fn backend_options_in(retired_list: &[AgentBackend], switch: &BackendSwitch) -> Vec<BackendOption> {
    AgentBackend::ALL
        .into_iter()
        .map(|backend| {
            let retired = is_retired_in(retired_list, backend);
            let enabled = switch.is_enabled_in(retired_list, backend);
            let wired = !retired;
            let reason = if retired {
                Some(format!(
                    "Agent 后端「{}」已在本版本退休；历史会话仍可查看，但不会启动运行时",
                    backend.as_str()
                ))
            } else if !enabled {
                Some(format!("已被管理员关闭（环境变量 {EXTERNAL_BACKENDS_ENV}）"))
            } else {
                None
            };
            BackendOption {
                backend,
                enabled,
                wired,
                experimental: backend.experimental(),
                hidden: backend.hidden(),
                retired,
                reason,
            }
        })
        .collect()
}

/// One log / audit line carrying both identities (plan 6.6). `engine_id` is
/// printed as `-` when the run has no Kernel record yet (for example before
/// dispatch) — it is never inferred from `agent_backend`.
pub fn audit_line(
    entry: HostEntry,
    conversation_id: &str,
    run_id: Option<&str>,
    agent_backend: Option<AgentBackend>,
    engine_id: Option<&str>,
    outcome: Result<Route, &RouteError>,
) -> String {
    let outcome = match outcome {
        Ok(Route::Fox) => "routed=fox".to_owned(),
        Ok(Route::External(backend)) => format!("routed=external:{}", backend.as_str()),
        Err(error) => format!("refused={}", error.code()),
    };
    format!(
        "fox.host_routing entry={} conversation={} run={} agent_backend={} engine_id={} {}",
        entry.as_str(),
        conversation_id,
        run_id.unwrap_or("-"),
        agent_backend.map(AgentBackend::as_str).unwrap_or("-"),
        engine_id.filter(|id| !id.is_empty()).unwrap_or("-"),
        outcome
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::legacy::{map_legacy, LegacyRecord};

    const ENTRIES: [HostEntry; 4] = [HostEntry::Start, HostEntry::Cancel, HostEntry::Approve, HostEntry::Resume];
    const EXTERNAL: [AgentBackend; 3] = [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek];

    fn on() -> BackendSwitch {
        BackendSwitch::from_env_value(None)
    }

    #[test]
    fn new_conversation_mode_is_standard_by_default_and_full_access_is_external_only() {
        use super::decide_new_conversation_mode as decide;
        assert_eq!(decide(None, AgentBackend::Fox, false), Ok(false));
        assert_eq!(decide(Some("standard"), AgentBackend::Fox, true), Ok(false));
        // The pre-B3 name of the same choice is still accepted.
        assert_eq!(decide(Some("sandboxed"), AgentBackend::Codex, true), Ok(false));
        for backend in EXTERNAL {
            assert_eq!(decide(Some("full_access"), backend, false), Ok(true), "{backend:?}");
            let refused = decide(Some("full_access"), backend, true).unwrap_err();
            assert_eq!(refused.code, CODE_FULL_ACCESS_FORBIDDEN, "FORBIDDEN refuses, never downgrades");
        }
        // A Fox conversation sets full access through its own execution mode.
        let refused = decide(Some("full_access"), AgentBackend::Fox, false).unwrap_err();
        assert_eq!(refused.code, RouteError::CODE_INVALID);
        let refused = decide(Some("root"), AgentBackend::Codex, false).unwrap_err();
        assert_eq!(refused.code, "agent_backend_invalid");
    }

    /// C1: the built-in route and the three external Agents are supported;
    /// the retirement list is empty but kept.
    #[test]
    fn this_build_supports_fox_and_the_three_external_agents() {
        assert_eq!(SUPPORTED_BACKENDS, [AgentBackend::Fox, AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek]);
        assert!(RETIRED_BACKENDS.is_empty());
        for backend in AgentBackend::ALL {
            assert!(is_supported(backend), "{backend:?}");
            assert!(!is_retired(backend), "{backend:?}");
            assert!(on().is_enabled(backend), "{backend:?} is on by default");
            assert_eq!(session_access(backend, &on()), SessionAccess::Runnable);
        }
        assert!(on().any_external());
    }

    #[test]
    fn the_administrator_switch_only_narrows() {
        let off = BackendSwitch::from_env_value(Some("0"));
        assert!(off.is_enabled(AgentBackend::Fox));
        assert!(!off.any_external());
        let some = BackendSwitch::from_env_value(Some("pi, Codex fox"));
        assert!(some.is_enabled(AgentBackend::Pi) && some.is_enabled(AgentBackend::Codex));
        assert!(!some.is_enabled(AgentBackend::Deepseek));
        assert!(some.request_includes(AgentBackend::Pi));
        for value in [None, Some(""), Some("1"), Some("all"), Some("ALL"), Some("yes")] {
            let switch = BackendSwitch::from_env_value(value);
            for backend in EXTERNAL {
                assert!(switch.is_enabled(backend), "{value:?} {backend:?}");
            }
        }
        // A switched-off backend: refused for a new conversation, read-only for an old one.
        let error = choose_for_new_conversation(Some("deepseek"), &some).unwrap_err();
        assert_eq!(error, RouteError::Disabled { backend: AgentBackend::Deepseek, existing_conversation: false });
        assert_eq!(error.code(), "agent_backend_disabled");
        assert!(!error.retryable());
        assert_eq!(session_access(AgentBackend::Deepseek, &some), SessionAccess::ReadOnlyDisabled);
        for entry in ENTRIES {
            assert_eq!(
                route(entry, AgentBackend::Deepseek, None, &some),
                Err(RouteError::Disabled { backend: AgentBackend::Deepseek, existing_conversation: true })
            );
        }
    }

    #[test]
    fn unknown_switch_entries_are_reported_and_never_mean_all() {
        let switch = BackendSwitch::from_env_value(Some("claude,deepseek_harness"));
        assert_eq!(switch.unknown_entries(), ["claude".to_owned(), "deepseek_harness".to_owned()]);
        for backend in EXTERNAL {
            assert!(!switch.request_includes(backend));
            assert!(!switch.is_enabled(backend));
        }
        assert!(BackendSwitch::from_env_value(None).unknown_entries().is_empty());
    }

    #[test]
    fn a_new_conversation_takes_the_backend_it_asked_for() {
        for requested in [None, Some(""), Some("fox"), Some("  fox  ")] {
            assert_eq!(choose_for_new_conversation(requested, &on()), Ok(AgentBackend::Fox));
        }
        for backend in EXTERNAL {
            assert_eq!(choose_for_new_conversation(Some(backend.as_str()), &on()), Ok(backend));
        }
        // `engine_id` values are still not backend ids.
        let error = choose_for_new_conversation(Some("deepseek_harness"), &on()).unwrap_err();
        assert_eq!(error, RouteError::InvalidBackend("deepseek_harness".into()));
        assert_eq!(error.code(), "agent_backend_invalid");
    }

    #[test]
    fn every_backend_routes_to_its_own_runtime() {
        for entry in ENTRIES {
            assert_eq!(route(entry, AgentBackend::Fox, None, &on()), Ok(Route::Fox));
            assert_eq!(route(entry, AgentBackend::Fox, Some(AgentBackend::Fox), &on()), Ok(Route::Fox));
        }
        for backend in EXTERNAL {
            for entry in [HostEntry::Start, HostEntry::Cancel, HostEntry::Approve] {
                assert_eq!(route(entry, backend, None, &on()), Ok(Route::External(backend)), "{entry:?} {backend:?}");
                assert_eq!(route(entry, backend, Some(backend), &on()), Ok(Route::External(backend)));
            }
            // Resuming a Kernel / yuxi run has no meaning for an external Agent.
            let error = route(HostEntry::Resume, backend, None, &on()).unwrap_err();
            assert_eq!(error, RouteError::EntryUnsupported { entry: HostEntry::Resume, backend });
            assert_eq!(error.code(), "agent_backend_entry_unsupported");
        }
    }

    #[test]
    fn misrouted_requests_are_refused_on_every_entry() {
        for entry in ENTRIES {
            for bound in AgentBackend::ALL {
                for requested in AgentBackend::ALL.into_iter().filter(|candidate| *candidate != bound) {
                    let error = route(entry, bound, Some(requested), &on()).unwrap_err();
                    assert_eq!(error, RouteError::Misrouted { entry, bound, requested });
                    assert_eq!(error.code(), "agent_backend_misrouted");
                    assert!(error.message().contains("错投"));
                }
            }
        }
    }

    /// The retirement mechanism is kept for a backend really taken out of
    /// service later: exercised with a list naming Codex.
    #[test]
    fn the_retirement_mechanism_still_freezes_a_retired_backend() {
        let retired = [AgentBackend::Codex];
        let switch = BackendSwitch::from_env_value(Some("all"));
        assert!(!switch.is_enabled_in(&retired, AgentBackend::Codex), "the switch never revives a retired backend");
        assert!(switch.is_enabled_in(&retired, AgentBackend::Pi));
        assert!(!is_retired_in(&retired, AgentBackend::Fox) && !is_retired_in(&[AgentBackend::Fox], AgentBackend::Fox), "the built-in route is never retired");
        let error = choose_for_new_conversation_in(&retired, Some("codex"), &switch).unwrap_err();
        assert_eq!(error, RouteError::Retired { backend: AgentBackend::Codex, existing_conversation: false });
        assert_eq!(error.code(), RouteError::CODE_RETIRED);
        assert!(error.message().contains("退休"), "{}", error.message());
        assert!(!error.retryable());
        for entry in ENTRIES {
            for requested in [None, Some(AgentBackend::Codex)] {
                let error = route_in(&retired, entry, AgentBackend::Codex, requested, &switch).unwrap_err();
                assert_eq!(error, RouteError::Retired { backend: AgentBackend::Codex, existing_conversation: true });
                assert!(error.message().contains("只能查看历史"));
            }
            assert_eq!(
                on_lookup_failure_in(&retired, entry, Some("codex"), &switch, "x"),
                Err(RouteError::Retired { backend: AgentBackend::Codex, existing_conversation: true })
            );
        }
        assert_eq!(session_access_in(&retired, AgentBackend::Codex, &switch), SessionAccess::ReadOnlyRetired);
        assert_eq!(session_access_in(&retired, AgentBackend::Codex, &switch).as_str(), "read_only_retired");
        let options = backend_options_in(&retired, &switch);
        let codex = options.iter().find(|option| option.backend == AgentBackend::Codex).unwrap();
        assert!(codex.retired && !codex.enabled && !codex.wired);
        assert!(codex.reason.as_deref().unwrap().contains("退休"));
        // The frozen value itself is still read back unchanged.
        assert_eq!(bound_backend(Some("codex")), Ok(AgentBackend::Codex));
    }

    /// A failed lookup may only fall back to Fox while nothing external could
    /// be involved.
    #[test]
    fn a_failed_lookup_is_refused_while_an_external_backend_may_be_involved() {
        let off = BackendSwitch::from_env_value(Some("0"));
        for entry in ENTRIES {
            assert_eq!(on_lookup_failure(entry, None, &off, "database is locked"), Ok(()));
            assert_eq!(on_lookup_failure(entry, Some("fox"), &off, "x"), Ok(()));
            assert_eq!(on_lookup_failure(entry, Some(" "), &off, "x"), Ok(()));
            let error = on_lookup_failure(entry, Some("pi"), &off, "x").unwrap_err();
            assert_eq!(error.code(), "agent_backend_lookup_failed");
            let error = on_lookup_failure(entry, None, &on(), "database   is\nlocked").unwrap_err();
            assert_eq!(error, RouteError::LookupFailed { entry, detail: "database is locked".into() });
            assert!(error.retryable());
            assert_eq!(on_lookup_failure(entry, Some("gpt"), &on(), "x").unwrap_err().code(), "agent_backend_invalid");
        }
    }

    #[test]
    fn stored_values_map_old_rows_to_fox_and_fail_closed_on_garbage() {
        assert_eq!(bound_backend(None), Ok(AgentBackend::Fox));
        assert_eq!(bound_backend(Some("fox")), Ok(AgentBackend::Fox));
        for garbage in ["", "FOX", "codex_acp", "deepseek_harness"] {
            let error = bound_backend(Some(garbage)).unwrap_err();
            assert_eq!(error.code(), "agent_backend_invalid");
            for entry in ENTRIES {
                assert!(route_stored(entry, Some(garbage), None, &BackendSwitch::all_on()).is_err());
            }
        }
    }

    #[test]
    fn old_kernel_runs_of_every_engine_stay_on_fox() {
        for (mode, engine) in [
            ("authoritative", "pi"),
            ("authoritative", "codex"),
            ("authoritative", "deepseek_harness"),
            ("legacy", "pi"),
        ] {
            let mapped = map_legacy(&LegacyRecord { kernel_mode: Some(mode), engine_id: Some(engine) });
            assert_eq!(mapped.agent_backend(), Some(AgentBackend::Fox));
            assert_eq!(
                route_stored(HostEntry::Resume, None, None, &BackendSwitch::all_on()),
                Ok((AgentBackend::Fox, Route::Fox))
            );
        }
    }

    #[test]
    fn route_stored_parses_both_sides() {
        assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some("fox"), &on()), Ok((AgentBackend::Fox, Route::Fox)));
        assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some(" "), &on()), Ok((AgentBackend::Fox, Route::Fox)));
        assert_eq!(route_stored(HostEntry::Approve, Some("fox"), Some("codex"), &on()).unwrap_err().code(), "agent_backend_misrouted");
        assert_eq!(route_stored(HostEntry::Approve, Some("fox"), Some("gpt"), &on()).unwrap_err(), RouteError::InvalidBackend("gpt".into()));
        for backend in EXTERNAL {
            assert_eq!(route_stored(HostEntry::Start, Some(backend.as_str()), None, &on()), Ok((backend, Route::External(backend))));
        }
    }

    #[test]
    fn picker_options_offer_every_supported_backend() {
        let options = backend_options(&on());
        assert_eq!(options.iter().map(|option| option.backend).collect::<Vec<_>>(), AgentBackend::ALL);
        for option in &options {
            assert!(option.enabled && option.wired && !option.retired && option.reason.is_none(), "{:?}", option.backend);
        }
        assert!(options.iter().find(|option| option.backend == AgentBackend::Deepseek).unwrap().experimental);
        let off = backend_options(&BackendSwitch::from_env_value(Some("codex")));
        let pi = off.iter().find(|option| option.backend == AgentBackend::Pi).unwrap();
        assert!(!pi.enabled && pi.wired && !pi.retired);
        assert!(pi.reason.as_deref().unwrap().contains(EXTERNAL_BACKENDS_ENV));
    }

    #[test]
    fn audit_line_carries_both_identities() {
        let line = audit_line(HostEntry::Cancel, "c1", Some("r1"), Some(AgentBackend::Fox), Some("codex"), Ok(Route::Fox));
        assert_eq!(line, "fox.host_routing entry=cancel conversation=c1 run=r1 agent_backend=fox engine_id=codex routed=fox");
        let line = audit_line(HostEntry::Start, "c3", None, Some(AgentBackend::Pi), None, Ok(Route::External(AgentBackend::Pi)));
        assert!(line.ends_with("agent_backend=pi engine_id=- routed=external:pi"), "{line}");
        let error = RouteError::Misrouted { entry: HostEntry::Start, bound: AgentBackend::Fox, requested: AgentBackend::Pi };
        let line = audit_line(HostEntry::Start, "c1", None, Some(AgentBackend::Fox), None, Err(&error));
        assert!(line.contains("agent_backend=fox engine_id=- refused=agent_backend_misrouted"));
    }

    #[test]
    fn migration_sql_is_additive_only() {
        let upper = AGENT_BACKEND_MIGRATION_SQL.to_ascii_uppercase();
        assert!(upper.contains("ADD COLUMN AGENT_BACKEND"));
        assert!(upper.contains("DEFAULT 'FOX'"));
        for forbidden in ["DROP TABLE", "UPDATE CONVERSATIONS", "DELETE FROM", "RENAME"] {
            assert!(!upper.contains(forbidden), "migration must not contain {forbidden}");
        }
        assert_eq!(AGENT_BACKEND_SCHEMA_VERSION, 95);
        for backend in EXTERNAL {
            assert!(upper.contains(&format!("'{}'", backend.as_str().to_ascii_uppercase())));
        }
    }
}
