//! Host wiring, first batch (plan v0.3.1 sections 5.4 and 6.6; work packages
//! P3 four-entry routing, P5 identity field, P6 minimal UI).
//!
//! Pure routing logic shared by the desktop Host. The Tauri commands are thin
//! glue: they read the conversation's frozen `agent_backend` from the database
//! and ask this module what to do. Everything that can be decided without a
//! database or a window is decided here, so it is unit-tested on any host.
//!
//! Rules implemented:
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
//! * Non-Fox backends are off by default and gated by the
//!   [`EXTERNAL_BACKENDS_ENV`] environment variable. With the switch off, a new
//!   non-Fox conversation is refused (`agent_backend_disabled`) and an already
//!   bound one is read-only: no runtime is started for it.
//! * With the switch on (external backend wiring batch), start / cancel /
//!   approve of a non-Fox conversation route to the external runtime
//!   ([`Route::External`]); resume (the yuxi / Kernel continuation entries)
//!   has no external meaning and is refused with
//!   `agent_backend_entry_unsupported`. `agent_backend_not_yet_wired` is no
//!   longer produced (the code stays known so an older UI still reads it).
//! * A lookup of the frozen value that fails (storage error, missing column)
//!   is decided by [`on_lookup_failure`]: with every external backend off it
//!   falls back to the Fox path as before; as soon as one is on, or the
//!   request names a non-Fox backend, it is refused
//!   (`agent_backend_lookup_failed`), because a non-Fox conversation must
//!   never reach the Fox runtime by accident.
//! * `engine_id` keeps its historical meaning; [`audit_line`] always prints it
//!   next to `agent_backend`.

use crate::port::AgentBackend;
use std::collections::BTreeSet;

/// Environment variable that enables non-Fox backends.
///
/// Chosen over a settings row (see `docs/sandbox.md`, "Host 接线第一批"): the
/// external runtime is not wired yet, so the switch is an engineering /
/// enterprise control, not a user preference. An environment variable cannot
/// be flipped from the UI by accident, is trivially absent on every existing
/// install (so the default is "off" with no migration), sits in the Host
/// policy layer of plan 6.5 (highest precedence, can only tighten), and
/// mirrors the existing `FOX_KERNEL_MODE` switch.
///
/// Accepted values: unset / empty / `0` / `off` / `false` / `none` → all
/// external backends off; `all` / `1` / `on` / `true` → all on; otherwise a
/// comma- or whitespace-separated list of backend ids (`pi,codex`). `fox` in
/// the list is ignored (Fox is always available). Unknown ids are ignored and
/// reported by [`BackendSwitch::unknown_entries`], never treated as "all".
pub const EXTERNAL_BACKENDS_ENV: &str = "FOX_EXTERNAL_BACKENDS";

/// Additive migration for the conversation-level identity field.
///
/// * `ALTER TABLE ... ADD COLUMN` with a constant `NOT NULL DEFAULT 'fox'` is a
///   metadata-only change in SQLite: no existing row is rewritten, and every
///   existing conversation reads as `fox` (plan 6.6: old Fox runs, including
///   old `codex` / `deepseek_harness` model adapters and legacy-path runs, all
///   map to `fox`).
/// * The CHECK keeps the column to the four public backend ids.
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

/// Which non-Fox backends the Host policy enables. Fox is always enabled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BackendSwitch {
    enabled: BTreeSet<AgentBackend>,
    unknown: Vec<String>,
}

impl BackendSwitch {
    /// Everything external off (the default).
    pub fn all_off() -> Self {
        Self::default()
    }

    /// Parse the raw value of [`EXTERNAL_BACKENDS_ENV`].
    pub fn from_env_value(value: Option<&str>) -> Self {
        let Some(raw) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
            return Self::all_off();
        };
        let lowered = raw.to_ascii_lowercase();
        match lowered.as_str() {
            "0" | "off" | "false" | "none" | "no" => return Self::all_off(),
            "1" | "on" | "true" | "all" | "yes" => {
                return Self {
                    enabled: AgentBackend::ALL
                        .into_iter()
                        .filter(|backend| *backend != AgentBackend::Fox)
                        .collect(),
                    unknown: Vec::new(),
                }
            }
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
                    switch.enabled.insert(backend);
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

    pub fn is_enabled(&self, backend: AgentBackend) -> bool {
        backend == AgentBackend::Fox || self.enabled.contains(&backend)
    }

    /// Entries of the variable that name no backend (reported, never "all").
    pub fn unknown_entries(&self) -> &[String] {
        &self.unknown
    }

    /// Whether any non-Fox backend is enabled.
    pub fn any_external(&self) -> bool {
        !self.enabled.is_empty()
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
    /// Host policy has this backend off. For an existing conversation this
    /// means read-only.
    Disabled {
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
    /// produced since the external backend wiring batch; kept for the code.
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
    /// involved; refused instead of falling back to the Fox path.
    LookupFailed {
        entry: HostEntry,
        detail: String,
    },
}

impl RouteError {
    pub const CODE_INVALID: &'static str = "agent_backend_invalid";
    pub const CODE_DISABLED: &'static str = "agent_backend_disabled";
    pub const CODE_MISROUTED: &'static str = "agent_backend_misrouted";
    pub const CODE_NOT_YET_WIRED: &'static str = "agent_backend_not_yet_wired";
    pub const CODE_ENTRY_UNSUPPORTED: &'static str = "agent_backend_entry_unsupported";
    pub const CODE_LOOKUP_FAILED: &'static str = "agent_backend_lookup_failed";

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidBackend(_) | Self::InvalidStoredBackend(_) => Self::CODE_INVALID,
            Self::Disabled { .. } => Self::CODE_DISABLED,
            Self::Misrouted { .. } => Self::CODE_MISROUTED,
            Self::NotYetWired { .. } => Self::CODE_NOT_YET_WIRED,
            Self::EntryUnsupported { .. } => Self::CODE_ENTRY_UNSUPPORTED,
            Self::LookupFailed { .. } => Self::CODE_LOOKUP_FAILED,
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
                "外部 Agent 后端「{}」未启用（由 {EXTERNAL_BACKENDS_ENV} 控制，默认关闭），只能新建 Fox 会话",
                backend.as_str()
            ),
            Self::Disabled {
                backend,
                existing_conversation: true,
            } => format!(
                "此会话绑定外部 Agent 后端「{}」，该后端当前未启用（{EXTERNAL_BACKENDS_ENV}），会话只读，不会启动运行时",
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
                "无法读取会话绑定的 Agent 后端（{}），为避免把外部会话交给错误的运行时，已拒绝本次操作，请稍后重试：{detail}",
                entry.as_str()
            ),
        }
    }

    /// Whether retrying the same request can succeed without a change.
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

/// Decide the backend a new conversation is frozen to. Absent → `fox`.
pub fn choose_for_new_conversation(
    requested: Option<&str>,
    switch: &BackendSwitch,
) -> Result<AgentBackend, RouteError> {
    let backend = parse_requested(requested)?.unwrap_or(AgentBackend::Fox);
    if !switch.is_enabled(backend) {
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
/// pre-v95 column). `Ok(())` = continue on the existing Fox path (audited by
/// the caller); `Err` = refuse.
///
/// * every external backend off and the request names no non-Fox backend:
///   the existing path, exactly as before this batch (switch-off behaviour
///   unchanged);
/// * otherwise: refused with `agent_backend_lookup_failed` (retryable). A
///   conversation that may be bound to an external backend must never run on
///   the Fox runtime because a read failed.
pub fn on_lookup_failure(entry: HostEntry, requested: Option<&str>, switch: &BackendSwitch, detail: &str) -> Result<(), RouteError> {
    let names_external = match parse_requested(requested) {
        Ok(Some(backend)) => backend != AgentBackend::Fox,
        Ok(None) => false,
        Err(error) => return Err(error),
    };
    if switch.any_external() || names_external {
        let detail: String = detail.split_whitespace().collect::<Vec<_>>().join(" ");
        return Err(RouteError::LookupFailed { entry, detail });
    }
    Ok(())
}

/// Route one Host entry for a conversation whose frozen backend is `bound`.
///
/// Order of checks: a misrouted request is refused first (whatever the switch
/// says), then Host policy, then wiring.
pub fn route(
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
        external if !switch.is_enabled(external) => Err(RouteError::Disabled {
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
    /// History is shown; no runtime is started (switch off).
    ReadOnlyDisabled,
    /// History is shown; the backend is on but not wired. No longer produced
    /// (every enabled external backend is wired); kept for the UI code.
    ReadOnlyNotWired,
}

impl SessionAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Runnable => "runnable",
            Self::ReadOnlyDisabled => "read_only_disabled",
            Self::ReadOnlyNotWired => "read_only_not_wired",
        }
    }
}

/// Reading a conversation never fails because of the switch: closing the
/// switch keeps old data readable (plan 6.6 rollback).
pub fn session_access(bound: AgentBackend, switch: &BackendSwitch) -> SessionAccess {
    match bound {
        AgentBackend::Fox => SessionAccess::Runnable,
        external if !switch.is_enabled(external) => SessionAccess::ReadOnlyDisabled,
        _ => SessionAccess::Runnable,
    }
}

/// Availability of each backend for the new-conversation picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOption {
    pub backend: AgentBackend,
    pub enabled: bool,
    /// Runtime wired in this build (every backend since the external backend
    /// wiring batch).
    pub wired: bool,
    pub experimental: bool,
    /// Why it cannot be picked / used, for the UI tooltip.
    pub reason: Option<String>,
}

pub fn backend_options(switch: &BackendSwitch) -> Vec<BackendOption> {
    AgentBackend::ALL
        .into_iter()
        .map(|backend| {
            let enabled = switch.is_enabled(backend);
            let wired = true;
            let reason = if !enabled {
                Some(format!(
                    "默认关闭；需由管理员设置环境变量 {EXTERNAL_BACKENDS_ENV} 后重启 Fox"
                ))
            } else {
                None
            };
            BackendOption {
                backend,
                enabled,
                wired,
                experimental: backend.experimental(),
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
        BackendSwitch::from_env_value(Some("all"))
    }

    #[test]
    fn switch_defaults_off_and_parses_lists() {
        for value in [None, Some(""), Some("  "), Some("0"), Some("off"), Some("FALSE"), Some("none")] {
            let switch = BackendSwitch::from_env_value(value);
            assert!(switch.is_enabled(AgentBackend::Fox));
            for backend in EXTERNAL {
                assert!(!switch.is_enabled(backend), "{value:?} must keep {backend:?} off");
            }
        }
        for value in ["1", "on", "ALL", "true"] {
            let switch = BackendSwitch::from_env_value(Some(value));
            assert!(EXTERNAL.iter().all(|backend| switch.is_enabled(*backend)));
        }
        let switch = BackendSwitch::from_env_value(Some("pi, Codex fox"));
        assert!(switch.is_enabled(AgentBackend::Pi));
        assert!(switch.is_enabled(AgentBackend::Codex));
        assert!(!switch.is_enabled(AgentBackend::Deepseek));
        assert!(switch.unknown_entries().is_empty());
    }

    #[test]
    fn unknown_switch_entries_never_enable_everything() {
        let switch = BackendSwitch::from_env_value(Some("claude,deepseek_harness"));
        assert!(EXTERNAL.iter().all(|backend| !switch.is_enabled(*backend)));
        assert_eq!(switch.unknown_entries(), ["claude".to_owned(), "deepseek_harness".to_owned()]);
    }

    #[test]
    fn new_conversation_defaults_to_fox_and_respects_the_switch() {
        assert_eq!(choose_for_new_conversation(None, &BackendSwitch::all_off()), Ok(AgentBackend::Fox));
        assert_eq!(choose_for_new_conversation(Some(""), &BackendSwitch::all_off()), Ok(AgentBackend::Fox));
        assert_eq!(choose_for_new_conversation(Some("fox"), &BackendSwitch::all_off()), Ok(AgentBackend::Fox));
        for backend in EXTERNAL {
            let error = choose_for_new_conversation(Some(backend.as_str()), &BackendSwitch::all_off()).unwrap_err();
            assert_eq!(error.code(), "agent_backend_disabled");
            assert_eq!(choose_for_new_conversation(Some(backend.as_str()), &on()), Ok(backend));
        }
        // Only the selected backend is opened by a list.
        let only_pi = BackendSwitch::from_env_value(Some("pi"));
        assert_eq!(choose_for_new_conversation(Some("pi"), &only_pi), Ok(AgentBackend::Pi));
        assert_eq!(choose_for_new_conversation(Some("codex"), &only_pi).unwrap_err().code(), "agent_backend_disabled");
        // `engine_id` values are not backend ids.
        let error = choose_for_new_conversation(Some("deepseek_harness"), &on()).unwrap_err();
        assert_eq!(error, RouteError::InvalidBackend("deepseek_harness".into()));
        assert_eq!(error.code(), "agent_backend_invalid");
    }

    #[test]
    fn fox_conversations_route_to_fox_on_all_four_entries() {
        for switch in [BackendSwitch::all_off(), on()] {
            for entry in ENTRIES {
                assert_eq!(route(entry, AgentBackend::Fox, None, &switch), Ok(Route::Fox));
                assert_eq!(route(entry, AgentBackend::Fox, Some(AgentBackend::Fox), &switch), Ok(Route::Fox));
            }
        }
    }

    #[test]
    fn misrouted_requests_are_refused_on_every_entry() {
        // Misrouting is checked before the switch: it is refused even when the
        // requested backend is enabled, and even when it is off.
        for switch in [BackendSwitch::all_off(), on()] {
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

    #[test]
    fn external_conversations_are_read_only_when_the_switch_is_off() {
        for entry in ENTRIES {
            for backend in EXTERNAL {
                let error = route(entry, backend, None, &BackendSwitch::all_off()).unwrap_err();
                assert_eq!(error, RouteError::Disabled { backend, existing_conversation: true });
                assert_eq!(error.code(), "agent_backend_disabled");
                assert!(error.message().contains("只读"));
                // Same-backend request does not bypass the switch.
                assert_eq!(route(entry, backend, Some(backend), &BackendSwitch::all_off()).unwrap_err().code(), "agent_backend_disabled");
            }
        }
    }

    #[test]
    fn external_conversations_route_to_their_runtime_when_the_switch_is_on() {
        for entry in [HostEntry::Start, HostEntry::Cancel, HostEntry::Approve] {
            for backend in EXTERNAL {
                assert_eq!(route(entry, backend, Some(backend), &on()), Ok(Route::External(backend)));
                assert_eq!(route(entry, backend, None, &on()), Ok(Route::External(backend)));
            }
        }
        for backend in EXTERNAL {
            let error = route(HostEntry::Resume, backend, None, &on()).unwrap_err();
            assert_eq!(error, RouteError::EntryUnsupported { entry: HostEntry::Resume, backend });
            assert_eq!(error.code(), "agent_backend_entry_unsupported");
        }
        // Only the listed backend is routed; the others stay read-only.
        let only_pi = BackendSwitch::from_env_value(Some("pi"));
        assert_eq!(route(HostEntry::Start, AgentBackend::Pi, None, &only_pi), Ok(Route::External(AgentBackend::Pi)));
        assert_eq!(route(HostEntry::Start, AgentBackend::Codex, None, &only_pi).unwrap_err().code(), "agent_backend_disabled");
    }

    #[test]
    fn a_failed_lookup_falls_back_only_while_every_external_backend_is_off() {
        for entry in ENTRIES {
            assert_eq!(on_lookup_failure(entry, None, &BackendSwitch::all_off(), "database is locked"), Ok(()));
            assert_eq!(on_lookup_failure(entry, Some("fox"), &BackendSwitch::all_off(), "x"), Ok(()));
            let error = on_lookup_failure(entry, None, &on(), "database\n is locked").unwrap_err();
            assert_eq!(error.code(), "agent_backend_lookup_failed");
            assert!(error.retryable());
            assert!(error.message().contains("database is locked"));
            // A request that names a non-Fox backend is refused even with the switch off.
            assert_eq!(on_lookup_failure(entry, Some("pi"), &BackendSwitch::all_off(), "x").unwrap_err().code(), "agent_backend_lookup_failed");
            assert_eq!(on_lookup_failure(entry, Some("gpt"), &BackendSwitch::all_off(), "x").unwrap_err().code(), "agent_backend_invalid");
            let only_codex = BackendSwitch::from_env_value(Some("codex"));
            assert!(on_lookup_failure(entry, None, &only_codex, "x").is_err());
        }
    }

    #[test]
    fn closing_the_switch_keeps_bound_conversations_readable() {
        for backend in EXTERNAL {
            assert_eq!(session_access(backend, &on()), SessionAccess::Runnable);
            assert_eq!(session_access(backend, &BackendSwitch::all_off()), SessionAccess::ReadOnlyDisabled);
            // The frozen value itself is still read back unchanged.
            assert_eq!(bound_backend(Some(backend.as_str())), Ok(backend));
        }
        assert_eq!(session_access(AgentBackend::Fox, &BackendSwitch::all_off()), SessionAccess::Runnable);
    }

    #[test]
    fn stored_values_map_old_rows_to_fox_and_fail_closed_on_garbage() {
        // Absent/NULL only arises before the migration; it maps to fox exactly
        // like the migration's column default.
        assert_eq!(bound_backend(None), Ok(AgentBackend::Fox));
        assert_eq!(bound_backend(Some("fox")), Ok(AgentBackend::Fox));
        for garbage in ["", "FOX", "codex_acp", "deepseek_harness"] {
            let error = bound_backend(Some(garbage)).unwrap_err();
            assert_eq!(error.code(), "agent_backend_invalid");
            for entry in ENTRIES {
                assert!(route_stored(entry, Some(garbage), None, &on()).is_err());
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
            assert_eq!(route_stored(HostEntry::Resume, None, None, &BackendSwitch::all_off()), Ok((AgentBackend::Fox, Route::Fox)));
        }
    }

    #[test]
    fn route_stored_parses_both_sides() {
        assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some("fox"), &BackendSwitch::all_off()), Ok((AgentBackend::Fox, Route::Fox)));
        assert_eq!(route_stored(HostEntry::Cancel, Some("fox"), Some(" "), &BackendSwitch::all_off()), Ok((AgentBackend::Fox, Route::Fox)));
        assert_eq!(
            route_stored(HostEntry::Approve, Some("fox"), Some("codex"), &on()).unwrap_err().code(),
            "agent_backend_misrouted"
        );
        assert_eq!(
            route_stored(HostEntry::Approve, Some("fox"), Some("gpt"), &on()).unwrap_err(),
            RouteError::InvalidBackend("gpt".into())
        );
    }

    #[test]
    fn picker_options_explain_disabled_backends() {
        let off = backend_options(&BackendSwitch::all_off());
        assert_eq!(off.len(), 4);
        assert!(off[0].enabled && off[0].wired && off[0].reason.is_none());
        for option in &off[1..] {
            assert!(!option.enabled);
            assert!(option.reason.as_deref().unwrap().contains(EXTERNAL_BACKENDS_ENV));
        }
        let all = backend_options(&on());
        for option in &all[1..] {
            assert!(option.enabled && option.wired);
            assert!(option.reason.is_none());
        }
        assert!(all.iter().find(|option| option.backend == AgentBackend::Deepseek).unwrap().experimental);
    }

    #[test]
    fn audit_line_carries_both_identities() {
        let line = audit_line(HostEntry::Cancel, "c1", Some("r1"), Some(AgentBackend::Fox), Some("codex"), Ok(Route::Fox));
        assert_eq!(line, "fox.host_routing entry=cancel conversation=c1 run=r1 agent_backend=fox engine_id=codex routed=fox");
        let error = RouteError::Misrouted { entry: HostEntry::Start, bound: AgentBackend::Fox, requested: AgentBackend::Pi };
        let line = audit_line(HostEntry::Start, "c1", None, Some(AgentBackend::Fox), None, Err(&error));
        assert!(line.contains("agent_backend=fox engine_id=- refused=agent_backend_misrouted"));
        let line = audit_line(HostEntry::Start, "c2", Some("r2"), Some(AgentBackend::Pi), None, Ok(Route::External(AgentBackend::Pi)));
        assert!(line.ends_with("agent_backend=pi engine_id=- routed=external:pi"), "{line}");
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
    }
}
