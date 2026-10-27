//! Host wiring: conversation-level Agent backend routing.
//!
//! ## This build has exactly one supported Agent path
//!
//! Product decision 2026-10-27 (single Pi mainline): the only supported way
//! to drive a conversation is the built-in Fox route, whose model engine is
//! pinned to Pi. The three external backends (`pi` / ACP, `codex`, `deepseek`
//! / DSH) are RETIRED: they cannot be picked, cannot be started, and no
//! environment variable, stored preference, or direct IPC call can bring one
//! back. Retirement does not delete data: a conversation frozen to a retired
//! backend keeps its value, its history stays readable, and the UI must show
//! the original binding instead of relabelling it as Pi.
//!
//! A NEW conversation is frozen to [`AgentBackend::Fox`]. That value is the
//! storage id of the built-in route, not a driver name; renaming it to `pi`
//! would route new conversations to the retired external Pi ACP path.
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
//! * A conversation frozen to a retired backend is read-only: history is
//!   shown, no runtime is ever started, and every runtime entry is refused
//!   with [`RouteError::CODE_RETIRED`]. It is never quietly re-driven by the
//!   Fox route, because that would run one conversation's history on a
//!   different driver than the one it was created with.
//! * [`EXTERNAL_BACKENDS_ENV`] is still parsed so an old deployment's value is
//!   reported (unknown entries, `request_includes`), but it can no longer
//!   enable anything: [`BackendSwitch::is_enabled`] is true for `fox` and
//!   false for every retired backend whatever the variable says.
//! * A lookup of the frozen value that fails (storage error, missing column)
//!   falls back to the Fox path, because the Fox path is now the only path
//!   that can run. A request that explicitly names a retired backend is still
//!   refused, so an explicit request is never satisfied by a driver the caller
//!   did not ask for. `agent_backend_lookup_failed` is no longer produced; the
//!   code stays known so an older UI still reads it.
//! * `engine_id` keeps its historical meaning; [`audit_line`] always prints it
//!   next to `agent_backend`.

use crate::port::AgentBackend;
use std::collections::BTreeSet;

/// Environment variable that used to allow the external backends to be
/// picked. Since the single Pi mainline it can no longer enable one; it is
/// parsed only so an old deployment's value is reported and audited.
///
/// Accepted values (all of them now mean "no retired backend runs"):
/// * unset / empty, `all` / `1` / `on` / `true` / `yes`;
/// * `0` / `off` / `false` / `none` / `no`;
/// * a comma- or whitespace-separated list of backend ids (`pi,codex`).
///   `fox` in the list is ignored (it is the supported path). Unknown ids are
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
/// Since the single Pi mainline no new grant is created; the table stays for
/// the records already made with it.
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

/// Permission mode a retired external conversation was created with.
pub const NEW_MODE_SANDBOXED: &str = "sandboxed";
pub const NEW_MODE_FULL_ACCESS: &str = "full_access";

/// Refusal code: full access requested where it cannot be granted.
pub const CODE_FULL_ACCESS_FORBIDDEN: &str = "agent_backend_full_access_forbidden";

/// The backends this build can start, in display order. Exactly one.
pub const SUPPORTED_BACKENDS: [AgentBackend; 1] = [AgentBackend::Fox];

/// The backends retired by the single Pi mainline. Their recorded values stay
/// readable; nothing of theirs is ever started.
pub const RETIRED_BACKENDS: [AgentBackend; 3] = [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek];

/// Whether this backend is retired in this build (readable, never runnable).
pub fn is_retired(backend: AgentBackend) -> bool {
    RETIRED_BACKENDS.contains(&backend)
}

/// Whether this backend is the one supported path of this build.
pub fn is_supported(backend: AgentBackend) -> bool {
    SUPPORTED_BACKENDS.contains(&backend)
}

/// Whether a new conversation gets a full-access grant. `requested` is the
/// UI's choice (absent = sandboxed). Full access is only for external
/// backends and is refused outright (not silently downgraded) when the
/// machine forbids it (`FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1`), so a UI that
/// ignores the disabled option cannot get around it. With every external
/// backend retired, `full_access` is always a refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeRefusal {
    pub code: &'static str,
    pub message: String,
}

pub fn decide_new_conversation_mode(requested: Option<&str>, backend: AgentBackend, forbidden: bool) -> Result<bool, ModeRefusal> {
    match requested.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(false),
        Some(NEW_MODE_SANDBOXED) => Ok(false),
        Some(NEW_MODE_FULL_ACCESS) if backend == AgentBackend::Fox => Err(ModeRefusal {
            code: RouteError::CODE_INVALID,
            message: "完全访问只适用于外部 Agent 后端，而本版本的会话使用内置 Pi 驱动".into(),
        }),
        Some(NEW_MODE_FULL_ACCESS) if forbidden => Err(ModeRefusal {
            code: CODE_FULL_ACCESS_FORBIDDEN,
            message: "本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1），外部后端只能在沙箱中运行".into(),
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

/// What [`EXTERNAL_BACKENDS_ENV`] asked for, for diagnostics only.
///
/// It is deliberately NOT a permission: [`Self::is_enabled`] ignores it for
/// every retired backend. Keeping the parsed request lets a support report say
/// "this deployment sets FOX_EXTERNAL_BACKENDS=all, which no longer enables
/// anything" instead of silently answering "nothing was asked".
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

    /// Whether this backend may run. The Fox path always may; a retired
    /// backend never may, whatever the variable asked for.
    pub fn is_enabled(&self, backend: AgentBackend) -> bool {
        is_supported(backend)
    }

    /// Whether the environment variable asked for this backend. Diagnostic
    /// only: [`Self::is_enabled`] decides, and it ignores this.
    pub fn request_includes(&self, backend: AgentBackend) -> bool {
        self.requested.contains(&backend)
    }

    /// Entries of the variable that name no backend (reported, never "all").
    pub fn unknown_entries(&self) -> &[String] {
        &self.unknown
    }

    /// Whether any retired backend may run. Always false in this build; kept
    /// so the Host glue keeps one predicate instead of hard-coding "fox".
    pub fn any_external(&self) -> bool {
        // Only the supported path runs; a retired backend never does, whatever
        // EXTERNAL_BACKENDS_ENV asked for. Spelled out so the invariant stays
        // visible instead of being a bare `false`.
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
    /// Host policy has this backend off. Kept for older callers; a retired
    /// backend is refused with [`Self::Retired`] instead.
    Disabled {
        backend: AgentBackend,
        existing_conversation: bool,
    },
    /// The backend is retired in this build. `existing_conversation` is true
    /// for a conversation frozen to it (read-only history), false for a new
    /// conversation that asked for it (refused).
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
    /// Kernel run). No longer produced, because no external backend runs.
    EntryUnsupported {
        entry: HostEntry,
        backend: AgentBackend,
    },
    /// The frozen value could not be read. No longer produced (the Fox path is
    /// the only runnable one, so a failed lookup falls back to it); kept for
    /// older UIs.
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
    /// The backend is retired in this build (readable, never runnable).
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
                "外部 Agent 后端「{}」当前不可用，新会话使用内置 Pi 驱动",
                backend.as_str()
            ),
            Self::Disabled {
                backend,
                existing_conversation: true,
            } => format!(
                "此会话绑定外部 Agent 后端「{}」，该后端当前不可用，会话只读，不会启动运行时",
                backend.as_str()
            ),
            Self::Retired {
                backend,
                existing_conversation: false,
            } => format!(
                "Agent 后端「{}」已在本版本退休：新会话只使用内置 Pi 驱动，不能再选择或启动它",
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
    /// Retirement is a product decision, not a transient condition.
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
/// Absent → `fox`. Every retired backend is a refusal naming that backend,
/// never a silent substitution by the supported path: a caller that asked for
/// Codex must be told Codex is retired, not receive a Pi conversation.
pub fn choose_for_new_conversation(
    requested: Option<&str>,
    switch: &BackendSwitch,
) -> Result<AgentBackend, RouteError> {
    let backend = parse_requested(requested)?.unwrap_or(AgentBackend::Fox);
    if is_retired(backend) {
        return Err(RouteError::Retired {
            backend,
            existing_conversation: false,
        });
    }
    if backend.hidden() {
        return Err(RouteError::Hidden(backend));
    }
    // Only one backend remains supported; the switch cannot narrow it further.
    debug_assert!(switch.is_enabled(backend), "the supported backend is always enabled");
    Ok(backend)
}

/// Where an accepted entry goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The existing Fox path (Kernel / legacy / yuxi selection unchanged).
    Fox,
    /// The external runtime of this backend. Since the single Pi mainline no
    /// route produces it; the variant stays so the Host glue and the older UI
    /// keep one exhaustive match.
    External(AgentBackend),
}

/// What to do when the frozen value could not be read (storage error,
/// pre-v95 column). `Ok(())` = continue on the Fox path (audited by the
/// caller); `Err` = refuse.
///
/// Since the Fox path is the only runnable path, a failed read no longer has
/// a wrong-runtime risk. But an explicit request that names a retired backend
/// is still refused, so the answer always matches what was asked for.
pub fn on_lookup_failure(
    _entry: HostEntry,
    requested: Option<&str>,
    _switch: &BackendSwitch,
    _detail: &str,
) -> Result<(), RouteError> {
    match parse_requested(requested)? {
        Some(backend) if is_retired(backend) => Err(RouteError::Retired {
            backend,
            existing_conversation: true,
        }),
        // Nothing but the Fox path can run, so a failed read cannot send a
        // conversation to the wrong runtime.
        _ => Ok(()),
    }
}

/// Route one Host entry for a conversation whose frozen backend is `bound`.
///
/// Order of checks: a misrouted request is refused first (whatever the
/// backend), then retirement.
pub fn route(
    entry: HostEntry,
    bound: AgentBackend,
    requested: Option<AgentBackend>,
    _switch: &BackendSwitch,
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
        retired => Err(RouteError::Retired {
            backend: retired,
            existing_conversation: true,
        }),
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
    /// History is shown; no runtime is started (legacy: external backend
    /// switched off). No longer produced; kept for older UIs.
    ReadOnlyDisabled,
    /// History is shown and the backend is not wired. No longer produced.
    ReadOnlyNotWired,
    /// History is shown; the backend is retired in this build. No runtime is
    /// started and the conversation is not re-bound to another driver.
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

/// Reading a conversation never fails because of retirement: keeping old data
/// readable is the rollback path (plan 6.6).
pub fn session_access(bound: AgentBackend, _switch: &BackendSwitch) -> SessionAccess {
    match bound {
        AgentBackend::Fox => SessionAccess::Runnable,
        retired if is_retired(retired) => SessionAccess::ReadOnlyRetired,
        _ => SessionAccess::ReadOnlyRetired,
    }
}

/// Availability of each backend for the new-conversation picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOption {
    pub backend: AgentBackend,
    pub enabled: bool,
    /// Runtime wired in this build. False for every retired backend.
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
    AgentBackend::ALL
        .into_iter()
        .map(|backend| {
            let retired = is_retired(backend);
            let enabled = switch.is_enabled(backend);
            let wired = !retired;
            let reason = if retired {
                Some(format!(
                    "本版本只支持内置 Pi 驱动，此前的外部 Agent 路由（{}）已退休；历史会话仍可查看，但不会启动运行时",
                    backend.as_str()
                ))
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
    const RETIRED: [AgentBackend; 3] = [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek];

    fn every_env_switch() -> Vec<BackendSwitch> {
        [None, Some(""), Some("0"), Some("off"), Some("1"), Some("all"), Some("ALL"), Some("pi,codex"), Some("deepseek"), Some("claude,pi")]
            .into_iter()
            .map(BackendSwitch::from_env_value)
            .collect()
    }

    #[test]
    fn new_conversation_mode_is_sandboxed_by_default_and_forbidden_refuses_full_access() {
        use super::{decide_new_conversation_mode as decide, CODE_FULL_ACCESS_FORBIDDEN};
        assert_eq!(decide(None, AgentBackend::Fox, false), Ok(false));
        assert_eq!(decide(Some("sandboxed"), AgentBackend::Fox, true), Ok(false));
        // Full access only ever applied to an external backend; the single
        // supported path refuses it instead of silently downgrading.
        let refused = decide(Some("full_access"), AgentBackend::Fox, false).unwrap_err();
        assert_eq!(refused.code, RouteError::CODE_INVALID);
        assert!(refused.message.contains("内置 Pi 驱动"));
        let refused = decide(Some("root"), AgentBackend::Fox, false).unwrap_err();
        assert_eq!(refused.code, "agent_backend_invalid");
    }

    /// The single supported path is declared once and is the Fox route.
    #[test]
    fn this_build_declares_exactly_one_supported_path() {
        assert_eq!(SUPPORTED_BACKENDS, [AgentBackend::Fox]);
        assert_eq!(RETIRED_BACKENDS, RETIRED);
        assert!(is_supported(AgentBackend::Fox));
        for backend in RETIRED {
            assert!(is_retired(backend), "{backend:?} is retired");
            assert!(!is_supported(backend));
        }
    }

    /// Regression guard for the whole stage: no value of the administrator
    /// variable, and no stored preference, can make a retired backend runnable.
    #[test]
    fn the_environment_cannot_enable_a_retired_backend() {
        for switch in every_env_switch() {
            assert!(switch.is_enabled(AgentBackend::Fox));
            for backend in RETIRED {
                assert!(!switch.is_enabled(backend), "{backend:?} must never be enabled");
                assert!(!switch.any_external(), "no retired backend may ever run");
            }
        }
        // The variable is still parsed: an old deployment's request is visible
        // for support, it just grants nothing.
        let switch = BackendSwitch::from_env_value(Some("pi, Codex fox"));
        assert!(switch.request_includes(AgentBackend::Pi));
        assert!(switch.request_includes(AgentBackend::Codex));
        assert!(!switch.is_enabled(AgentBackend::Pi));
        assert!(!switch.is_enabled(AgentBackend::Codex));
        let switch = BackendSwitch::from_env_value(Some("all"));
        assert!(switch.request_includes(AgentBackend::Deepseek));
        assert!(!switch.is_enabled(AgentBackend::Deepseek));
    }

    #[test]
    fn unknown_switch_entries_are_reported_and_never_mean_all() {
        let switch = BackendSwitch::from_env_value(Some("claude,deepseek_harness"));
        assert_eq!(switch.unknown_entries(), ["claude".to_owned(), "deepseek_harness".to_owned()]);
        for backend in AgentBackend::ALL {
            assert!(!switch.request_includes(backend));
        }
        assert!(BackendSwitch::from_env_value(None).unknown_entries().is_empty());
    }

    #[test]
    fn a_new_conversation_is_always_the_supported_path() {
        for switch in every_env_switch() {
            for requested in [None, Some(""), Some("fox"), Some("  fox  ")] {
                assert_eq!(choose_for_new_conversation(requested, &switch), Ok(AgentBackend::Fox));
            }
        }
    }

    /// An explicit request for a retired driver is a refusal that names it. It
    /// must never be answered with the Fox path: that is the "silently swap the
    /// driver" failure the product decision forbids.
    #[test]
    fn an_explicit_retired_request_is_refused_and_never_substituted() {
        for switch in every_env_switch() {
            for backend in RETIRED {
                let error = choose_for_new_conversation(Some(backend.as_str()), &switch).unwrap_err();
                assert_eq!(
                    error,
                    RouteError::Retired {
                        backend,
                        existing_conversation: false
                    }
                );
                assert_eq!(error.code(), "agent_backend_retired");
                assert_eq!(error.code(), RouteError::CODE_RETIRED);
                assert!(error.message().contains("退休"), "{}", error.message());
                assert!(!error.retryable(), "a product decision is not retryable");
            }
        }
        // `engine_id` values are still not backend ids.
        let error = choose_for_new_conversation(Some("deepseek_harness"), &BackendSwitch::all_on()).unwrap_err();
        assert_eq!(error, RouteError::InvalidBackend("deepseek_harness".into()));
        assert_eq!(error.code(), "agent_backend_invalid");
    }

    #[test]
    fn fox_conversations_route_to_fox_on_all_four_entries() {
        for switch in every_env_switch() {
            for entry in ENTRIES {
                assert_eq!(route(entry, AgentBackend::Fox, None, &switch), Ok(Route::Fox));
                assert_eq!(route(entry, AgentBackend::Fox, Some(AgentBackend::Fox), &switch), Ok(Route::Fox));
            }
        }
    }

    #[test]
    fn misrouted_requests_are_refused_on_every_entry() {
        for switch in every_env_switch() {
            for entry in ENTRIES {
                for bound in AgentBackend::ALL {
                    for requested in AgentBackend::ALL.into_iter().filter(|candidate| *candidate != bound) {
                        let error = route(entry, bound, Some(requested), &switch).unwrap_err();
                        assert_eq!(error, RouteError::Misrouted { entry, bound, requested });
                        assert_eq!(error.code(), "agent_backend_misrouted");
                        assert!(error.message().contains("错投"));
                    }
                }
            }
        }
    }

    /// A conversation frozen to a retired backend is read-only on every entry,
    /// including when the caller repeats its own backend id. It is never
    /// re-driven by the Fox route.
    #[test]
    fn a_retired_conversation_is_read_only_on_every_entry() {
        for switch in every_env_switch() {
            for entry in ENTRIES {
                for backend in RETIRED {
                    for requested in [None, Some(backend)] {
                        let error = route(entry, backend, requested, &switch).unwrap_err();
                        assert_eq!(
                            error,
                            RouteError::Retired {
                                backend,
                                existing_conversation: true
                            }
                        );
                        assert_eq!(error.code(), "agent_backend_retired");
                        assert!(error.message().contains("只读") || error.message().contains("只能查看历史"));
                    }
                }
            }
        }
    }

    /// The Fox path is the only runnable path, so a failed lookup is safe to
    /// answer with it. An explicit retired request is still refused.
    #[test]
    fn a_failed_lookup_falls_back_to_fox_and_still_refuses_explicit_retired_requests() {
        for switch in every_env_switch() {
            for entry in ENTRIES {
                assert_eq!(on_lookup_failure(entry, None, &switch, "database is locked"), Ok(()));
                assert_eq!(on_lookup_failure(entry, Some("fox"), &switch, "x"), Ok(()));
                assert_eq!(on_lookup_failure(entry, Some(" "), &switch, "x"), Ok(()));
                for backend in RETIRED {
                    let error = on_lookup_failure(entry, Some(backend.as_str()), &switch, "x").unwrap_err();
                    assert_eq!(
                        error,
                        RouteError::Retired {
                            backend,
                            existing_conversation: true
                        }
                    );
                }
                assert_eq!(
                    on_lookup_failure(entry, Some("gpt"), &switch, "x").unwrap_err().code(),
                    "agent_backend_invalid"
                );
            }
        }
    }

    /// Keeping old data readable is the rollback path: closing the feature
    /// never hides a conversation.
    #[test]
    fn retired_conversations_stay_readable() {
        for switch in every_env_switch() {
            assert_eq!(session_access(AgentBackend::Fox, &switch), SessionAccess::Runnable);
            for backend in RETIRED {
                assert_eq!(session_access(backend, &switch), SessionAccess::ReadOnlyRetired);
                assert_eq!(session_access(backend, &switch).as_str(), "read_only_retired");
                // The frozen value itself is still read back unchanged.
                assert_eq!(bound_backend(Some(backend.as_str())), Ok(backend));
            }
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
        // Conversation-level mapping (column default) agrees with the run-level
        // mapping of `legacy.rs` for every historical engine and the legacy path.
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
        for switch in every_env_switch() {
            assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some("fox"), &switch), Ok((AgentBackend::Fox, Route::Fox)));
            assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some(" "), &switch), Ok((AgentBackend::Fox, Route::Fox)));
            assert_eq!(
                route_stored(HostEntry::Approve, Some("fox"), Some("codex"), &switch).unwrap_err().code(),
                "agent_backend_misrouted"
            );
            assert_eq!(
                route_stored(HostEntry::Approve, Some("fox"), Some("gpt"), &switch).unwrap_err(),
                RouteError::InvalidBackend("gpt".into())
            );
            assert_eq!(
                route_stored(HostEntry::Start, Some("codex"), None, &switch).unwrap_err().code(),
                "agent_backend_retired"
            );
        }
    }

    #[test]
    fn picker_options_explain_retired_backends() {
        let options = backend_options(&BackendSwitch::all_on());
        assert_eq!(options.len(), 4);
        let fox = &options[0];
        assert_eq!(fox.backend, AgentBackend::Fox);
        assert!(fox.enabled && fox.wired && !fox.retired && fox.reason.is_none());
        for option in &options[1..] {
            assert!(option.retired, "{:?} is retired", option.backend);
            assert!(!option.enabled);
            assert!(!option.wired);
            assert!(option.reason.as_deref().unwrap().contains("退休"));
        }
    }

    #[test]
    fn audit_line_carries_both_identities() {
        let line = audit_line(HostEntry::Cancel, "c1", Some("r1"), Some(AgentBackend::Fox), Some("codex"), Ok(Route::Fox));
        assert_eq!(line, "fox.host_routing entry=cancel conversation=c1 run=r1 agent_backend=fox engine_id=codex routed=fox");
        let error = RouteError::Misrouted { entry: HostEntry::Start, bound: AgentBackend::Fox, requested: AgentBackend::Pi };
        let line = audit_line(HostEntry::Start, "c1", None, Some(AgentBackend::Fox), None, Err(&error));
        assert!(line.contains("agent_backend=fox engine_id=- refused=agent_backend_misrouted"));
        let error = RouteError::Retired { backend: AgentBackend::Codex, existing_conversation: true };
        let line = audit_line(HostEntry::Start, "c2", Some("r2"), Some(AgentBackend::Codex), Some("codex"), Err(&error));
        assert!(line.ends_with("agent_backend=codex engine_id=codex refused=agent_backend_retired"), "{line}");
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
        // Retired values stay in the CHECK so their rows remain legal.
        for backend in RETIRED {
            assert!(upper.contains(&format!("'{}'", backend.as_str().to_ascii_uppercase())));
        }
    }
}
