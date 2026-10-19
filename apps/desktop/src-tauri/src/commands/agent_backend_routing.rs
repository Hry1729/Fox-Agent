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
//! A gate returns `Ok(())` only for the Fox route, in which case the caller
//! continues on its existing, unchanged path (Kernel / legacy / yuxi).
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
        Ok(None) => return Ok(()),
        Err(error) => return Err(ApiResponse::failure("storage.operation_failed", error, true)),
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
        Err(error) => return Err(ApiResponse::failure("storage.operation_failed", error, true)),
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
