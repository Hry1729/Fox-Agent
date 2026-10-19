//! Thin Host glue for the conversation-level `agent_backend` (four-agent plan
//! v0.3.1, 5.4 / 6.6; Host wiring first batch).
//!
//! All decisions live in `fox_agent_backend::host_routing` (unit-tested there).
//! This file only reads the frozen value from the database, calls the routing
//! function, turns a refusal into an `ApiResponse` failure with the stable
//! code (`agent_backend_disabled` / `agent_backend_misrouted` /
//! `agent_backend_not_yet_wired` / `agent_backend_invalid`), and prints one
//! audit line carrying both `agent_backend` and `engine_id`.
//!
//! A gate returns `Err` only when the stored backend is confirmed to be
//! something other than `fox` (or the request names a backend that differs
//! from the stored one). Everything else, including an unknown conversation /
//! run / approval, a database without the v95 column, or a storage error
//! while reading it, returns `Ok(())` and the caller continues on its
//! existing, unchanged path (Kernel / legacy / yuxi); a lookup that could not
//! be answered is audited as `routed=fox_fallback`.
use super::*;
use fox_agent_backend::host_routing::{self, BackendSwitch, HostEntry, Route, RouteError};
use fox_agent_backend::port::AgentBackend;

pub(crate) fn route_failure<T: serde::Serialize>(error: &RouteError) -> ApiResponse<T> {
    ApiResponse::failure(error.code(), error.message(), error.retryable())
}

fn log_route(
    entry: HostEntry,
    conversation_id: &str,
    run_id: Option<&str>,
    stored: Option<&str>,
    engine_id: Option<&str>,
    outcome: &Result<(AgentBackend, Route), RouteError>,
) {
    let backend = stored.and_then(AgentBackend::parse);
    let line = match outcome {
        Ok((bound, route)) => host_routing::audit_line(entry, conversation_id, run_id, Some(*bound), engine_id, Ok(*route)),
        Err(error) => host_routing::audit_line(entry, conversation_id, run_id, backend, engine_id, Err(error)),
    };
    eprintln!("{line}");
}

/// Why a backend lookup could not be answered. `column_missing` is a
/// database that has not run v95 yet (only possible for a connection opened
/// below the current schema); everything else is a storage error.
pub(crate) fn lookup_fallback_reason(error: &str) -> &'static str {
    if error.contains("no such column") && error.contains("agent_backend") {
        "column_missing"
    } else {
        "storage_error"
    }
}

/// Audit line for a lookup that fell back to the existing path. The error
/// text is reduced to one line so the audit stays one line per entry.
fn log_lookup_fallback(entry: HostEntry, conversation_id: &str, run_id: Option<&str>, error: &str) {
    let detail: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
    eprintln!(
        "fox.host_routing entry={} conversation={} run={} agent_backend=- engine_id=- routed=fox_fallback reason={} detail={:?}",
        entry.as_str(),
        conversation_id,
        run_id.unwrap_or("-"),
        lookup_fallback_reason(error),
        detail
    );
}

/// Choose the frozen backend for a conversation about to be created.
pub(crate) fn choose_new_conversation_backend<T: serde::Serialize>(
    requested: Option<&str>,
) -> Result<AgentBackend, ApiResponse<T>> {
    host_routing::choose_for_new_conversation(requested, &BackendSwitch::from_env()).map_err(|error| {
        eprintln!("fox.host_routing entry=create agent_backend={} engine_id=- refused={}", requested.unwrap_or("-"), error.code());
        route_failure(&error)
    })
}

/// Gate an entry that names a conversation (start / resume).
///
/// A conversation that does not exist is left to the caller's existing error
/// path (`Ok(())`), so the old error codes are unchanged.
pub(crate) fn gate_conversation<T: serde::Serialize>(
    database: &crate::database::Database,
    entry: HostEntry,
    conversation_id: &str,
    run_id: Option<&str>,
    requested: Option<&str>,
) -> Result<(), ApiResponse<T>> {
    let stored = match database.conversation_agent_backend(conversation_id) {
        Ok(Some(stored)) => stored,
        // Unknown conversation: the existing path answers exactly as before.
        Ok(None) => return Ok(()),
        // Fix 1 (Windows acceptance of 3a82f5d): a failed read never changes
        // the existing path's behaviour. Audit it and let the original path
        // run (and report its own storage error if the database is really
        // unusable). Only a conversation confirmed as non-Fox is refused.
        Err(error) => {
            log_lookup_fallback(entry, conversation_id, run_id, &error);
            return Ok(());
        }
    };
    let outcome = host_routing::route_stored(entry, Some(&stored), requested, &BackendSwitch::from_env());
    let refused = outcome.is_err();
    // Successful Fox routes are not logged per entry on start/resume: the
    // Kernel already logs the run with its engine_id; refusals always are.
    if refused {
        log_route(entry, conversation_id, run_id, Some(&stored), None, &outcome);
    }
    outcome.map(|_| ()).map_err(|error| route_failure(&error))
}

/// Gate an entry that names a run or an approval (cancel / approve). The
/// conversation is read from the run, never from the request.
pub(crate) fn gate_run_identity<T: serde::Serialize>(
    entry: HostEntry,
    identity: Result<Option<crate::database::RunBackendIdentity>, String>,
    requested: Option<&str>,
) -> Result<(), ApiResponse<T>> {
    let identity = match identity {
        Ok(Some(identity)) => identity,
        // Unknown run / approval: the existing path answers exactly as before.
        Ok(None) => return Ok(()),
        // Same rule as `gate_conversation`: audit, then the existing path.
        Err(error) => {
            log_lookup_fallback(entry, "-", None, &error);
            return Ok(());
        }
    };
    let outcome = host_routing::route_stored(
        entry,
        Some(&identity.agent_backend),
        requested,
        &BackendSwitch::from_env(),
    );
    log_route(
        entry,
        &identity.conversation_id,
        Some(&identity.run_id),
        Some(&identity.agent_backend),
        identity.engine_id.as_deref(),
        &outcome,
    );
    outcome.map(|_| ()).map_err(|error| route_failure(&error))
}

/// One backend in the new-conversation picker.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendOption {
    pub backend: String,
    pub enabled: bool,
    pub wired: bool,
    pub experimental: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendStatus {
    /// Name of the Host switch, for the UI explanation.
    pub switch_env: String,
    pub any_external_enabled: bool,
    pub options: Vec<AgentBackendOption>,
    /// Entries of the switch that name no backend (ignored, shown for support).
    pub unknown_entries: Vec<String>,
}

/// Picker / lock state for the UI. Never fails: closing the switch keeps
/// every conversation readable.
#[tauri::command]
pub fn agent_backends_status() -> ApiResponse<AgentBackendStatus> {
    let switch = BackendSwitch::from_env();
    let options: Vec<AgentBackendOption> = host_routing::backend_options(&switch)
        .into_iter()
        .map(|option| AgentBackendOption {
            backend: option.backend.as_str().to_owned(),
            enabled: option.enabled,
            wired: option.wired,
            experimental: option.experimental,
            reason: option.reason,
        })
        .collect();
    ApiResponse::success(AgentBackendStatus {
        switch_env: host_routing::EXTERNAL_BACKENDS_ENV.to_owned(),
        any_external_enabled: options.iter().any(|option| option.backend != "fox" && option.enabled),
        options,
        unknown_entries: switch.unknown_entries().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_fallback_reason_separates_old_schema_from_storage_errors() {
        assert_eq!(lookup_fallback_reason("no such column: agent_backend"), "column_missing");
        assert_eq!(lookup_fallback_reason("no such column: c.agent_backend"), "column_missing");
        assert_eq!(lookup_fallback_reason("database is locked"), "storage_error");
        assert_eq!(lookup_fallback_reason("no such column: title"), "storage_error");
    }

    #[test]
    fn gates_fall_back_to_the_existing_path_when_the_lookup_cannot_be_answered() {
        // A Fox conversation and an unknown conversation both continue on the
        // existing path; a failed lookup (storage error / pre-v95 column) too.
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("fox"), None, None)
            .expect("conversation");
        let gated: Result<(), ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, &conversation.id, None, None);
        assert!(gated.is_ok(), "fox conversation routes to the existing path");
        let gated: Result<(), ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, "missing", None, None);
        assert!(gated.is_ok(), "unknown conversation is left to the existing path");

        let storage_error: Result<(), ApiResponse<()>> =
            gate_run_identity(HostEntry::Cancel, Err("database is locked".to_owned()), None);
        assert!(storage_error.is_ok(), "storage errors never change the existing path");
        let missing_column: Result<(), ApiResponse<()>> =
            gate_run_identity(HostEntry::Approve, Err("no such column: c.agent_backend".to_owned()), None);
        assert!(missing_column.is_ok());
        let unknown: Result<(), ApiResponse<()>> = gate_run_identity(HostEntry::Cancel, Ok(None), None);
        assert!(unknown.is_ok());
    }

    #[test]
    fn only_a_confirmed_non_fox_backend_is_refused() {
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let pi = database
            .create_conversation_with_backend(database.default_agent_id(), Some("pi"), None, None, "pi")
            .expect("pi conversation");
        let gated: Result<(), ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, &pi.id, None, None);
        let failure = gated.expect_err("pi is refused before any runtime");
        let code = failure.error.as_ref().map(|error| error.code.clone());
        assert!(
            matches!(code.as_deref(), Some("agent_backend_disabled") | Some("agent_backend_not_yet_wired")),
            "{code:?}"
        );
        let identity = crate::database::RunBackendIdentity {
            conversation_id: "c".to_owned(),
            run_id: "r".to_owned(),
            agent_backend: "fox".to_owned(),
            engine_id: None,
        };
        let fox: Result<(), ApiResponse<()>> = gate_run_identity(HostEntry::Cancel, Ok(Some(identity)), None);
        assert!(fox.is_ok());
    }
}
