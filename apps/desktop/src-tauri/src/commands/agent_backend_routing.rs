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
//! A gate returns the route: `Route::Fox` (the existing, unchanged path),
//! `Route::External(backend)` (the external runtime, `crate::external_backend`;
//! only with `FOX_EXTERNAL_BACKENDS` on), or `Err` with the routing code. An
//! unknown conversation / run / approval returns `Route::Fox` so the old error
//! codes are unchanged. A lookup that fails (storage error, pre-v95 column)
//! falls back to the Fox path (audited `routed=fox_fallback`) only while every
//! external backend is off and the request names none; otherwise it is
//! refused (`agent_backend_lookup_failed`), so a possibly external
//! conversation never reaches the Fox runtime because a read failed
//! (external backend wiring batch, item 5).
use super::*;
use fox_agent_backend::host_routing::{self, BackendSwitch, HostEntry, RouteError};
pub(crate) use fox_agent_backend::host_routing::Route;
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

/// A lookup that could not be answered: Fox fallback (audited) only while no
/// external backend is involved, else refused.
fn on_lookup_error<T: serde::Serialize>(
    entry: HostEntry,
    conversation_id: &str,
    run_id: Option<&str>,
    requested: Option<&str>,
    error: &str,
) -> Result<Route, ApiResponse<T>> {
    match host_routing::on_lookup_failure(entry, requested, &BackendSwitch::from_env(), error) {
        Ok(()) => {
            log_lookup_fallback(entry, conversation_id, run_id, error);
            Ok(Route::Fox)
        }
        Err(refusal) => {
            eprintln!(
                "fox.host_routing entry={} conversation={} run={} agent_backend=- engine_id=- refused={} reason={}",
                entry.as_str(),
                conversation_id,
                run_id.unwrap_or("-"),
                refusal.code(),
                lookup_fallback_reason(error)
            );
            Err(route_failure(&refusal))
        }
    }
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
) -> Result<Route, ApiResponse<T>> {
    let stored = match database.conversation_agent_backend(conversation_id) {
        Ok(Some(stored)) => stored,
        // Unknown conversation: the existing path answers exactly as before.
        Ok(None) => return Ok(Route::Fox),
        // Switch off: unchanged since fix 1 (audit, existing path). Any
        // external backend on: refused (`on_lookup_error`).
        Err(error) => return on_lookup_error(entry, conversation_id, run_id, requested, &error),
    };
    let outcome = host_routing::route_stored(entry, Some(&stored), requested, &BackendSwitch::from_env());
    let refused = outcome.is_err();
    // Successful Fox routes are not logged per entry on start/resume: the
    // Kernel already logs the run with its engine_id; refusals always are.
    if refused || matches!(outcome, Ok((_, Route::External(_)))) {
        log_route(entry, conversation_id, run_id, Some(&stored), None, &outcome);
    }
    outcome.map(|(_, route)| route).map_err(|error| route_failure(&error))
}

/// Gate an entry that names a run or an approval (cancel / approve). The
/// conversation is read from the run, never from the request.
pub(crate) fn gate_run_identity<T: serde::Serialize>(
    entry: HostEntry,
    identity: Result<Option<crate::database::RunBackendIdentity>, String>,
    requested: Option<&str>,
) -> Result<Route, ApiResponse<T>> {
    let identity = match identity {
        Ok(Some(identity)) => identity,
        // Unknown run / approval: the existing path answers exactly as before.
        Ok(None) => return Ok(Route::Fox),
        // Same rule as `gate_conversation`.
        Err(error) => return on_lookup_error(entry, "-", None, requested, &error),
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
    outcome.map(|(_, route)| route).map_err(|error| route_failure(&error))
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

/// The four effective capability dimensions of an external backend
/// (approval / sandbox / cancel / observability), for the UI. `live` when a
/// session of this conversation ran in this Fox process, `planned` before.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendCapabilitiesRequest {
    pub backend: String,
    pub conversation_id: Option<String>,
}

#[tauri::command]
pub fn agent_backend_capabilities(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: AgentBackendCapabilitiesRequest,
) -> ApiResponse<fox_agent_backend::capability_view::CapabilityView> {
    match AgentBackend::parse(&request.backend) {
        Some(AgentBackend::Fox) | None => ApiResponse::failure(
            RouteError::CODE_INVALID,
            format!("只有外部 Agent 后端有能力声明：{}", request.backend),
            false,
        ),
        Some(backend) => ApiResponse::success(external.capabilities(request.conversation_id.as_deref(), backend)),
    }
}

/// UX batch ①②: requests that name a conversation.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendImportRequest {
    /// The conversation, or a new conversation's backend + project.
    #[serde(flatten)]
    pub target: crate::external_backend::AgentBackendTarget,
    /// Ids from `agent_backend_import_candidates` the user confirmed.
    pub selected: Vec<String>,
    /// "套用到我所有项目".
    #[serde(default)]
    pub all_projects: bool,
    /// "覆盖已有登录" (off by default: an existing login is kept and listed).
    #[serde(default)]
    pub overwrite: bool,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendPermissionStatusRequest {
    #[serde(default)]
    pub conversation_id: Option<String>,
}

/// Login probe of the private HOME (names only): an existing conversation,
/// or a new conversation's backend + project before its first message.
#[tauri::command]
pub fn agent_backend_credentials(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: crate::external_backend::AgentBackendTarget,
) -> ApiResponse<fox_agent_backend::onboarding::CredentialProbe> {
    external.credential_probe(&request)
}

/// Whitelisted login files / key names found in the real HOME (no values).
#[tauri::command]
pub fn agent_backend_import_candidates(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: crate::external_backend::AgentBackendTarget,
) -> ApiResponse<Vec<fox_agent_backend::onboarding::ImportItem>> {
    external.import_candidates(&request)
}

/// Copy the confirmed items into the private HOME(s); audited by name.
#[tauri::command]
pub fn agent_backend_import_credentials(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: AgentBackendImportRequest,
) -> ApiResponse<fox_agent_backend::onboarding::ImportReport> {
    external.import_credentials(&request.target, &request.selected, request.all_projects, request.overwrite)
}

/// Open the private HOME in the system file manager.
#[tauri::command]
pub fn agent_backend_open_private_home(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: crate::external_backend::AgentBackendTarget,
) -> ApiResponse<String> {
    external.open_private_home(&request)
}

/// Permission mode / lock / FORBIDDEN for the picker and the chip.
#[tauri::command]
pub fn agent_backend_permission_status(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: AgentBackendPermissionStatusRequest,
) -> ApiResponse<serde_json::Value> {
    ApiResponse::success(external.permission_status(request.conversation_id.as_deref()))
}

/// Revoke full access mid-session (restarts the backend in the sandbox).
#[tauri::command]
pub async fn agent_backend_revoke_full_access(
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: AgentBackendConversationRequest,
) -> Result<ApiResponse<serde_json::Value>, ()> {
    Ok(external.revoke_full_access(&request.conversation_id).await)
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
        let gated: Result<Route, ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, &conversation.id, None, None);
        assert!(gated.is_ok(), "fox conversation routes to the existing path");
        let gated: Result<Route, ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, "missing", None, None);
        assert!(gated.is_ok(), "unknown conversation is left to the existing path");

        let storage_error: Result<Route, ApiResponse<()>> =
            gate_run_identity(HostEntry::Cancel, Err("database is locked".to_owned()), None);
        assert!(storage_error.is_ok(), "storage errors never change the existing path");
        let missing_column: Result<Route, ApiResponse<()>> =
            gate_run_identity(HostEntry::Approve, Err("no such column: c.agent_backend".to_owned()), None);
        assert!(missing_column.is_ok());
        let unknown: Result<Route, ApiResponse<()>> = gate_run_identity(HostEntry::Cancel, Ok(None), None);
        // A request that names an external backend is refused when the read fails.
        let named: Result<Route, ApiResponse<()>> =
            gate_run_identity(HostEntry::Cancel, Err("database is locked".to_owned()), Some("pi"));
        assert_eq!(named.unwrap_err().error.unwrap().code, "agent_backend_lookup_failed");
        assert!(unknown.is_ok());
    }

    #[test]
    fn only_a_confirmed_non_fox_backend_is_refused() {
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let pi = database
            .create_conversation_with_backend(database.default_agent_id(), Some("pi"), None, None, "pi")
            .expect("pi conversation");
        let gated: Result<Route, ApiResponse<()>> =
            gate_conversation(&database, HostEntry::Start, &pi.id, None, None);
        let failure = gated.expect_err("pi is refused before any runtime");
        let code = failure.error.as_ref().map(|error| error.code.clone());
        assert!(
            matches!(code.as_deref(), Some("agent_backend_disabled")),
            "{code:?}"
        );
        let identity = crate::database::RunBackendIdentity {
            conversation_id: "c".to_owned(),
            run_id: "r".to_owned(),
            agent_backend: "fox".to_owned(),
            engine_id: None,
        };
        let fox: Result<Route, ApiResponse<()>> = gate_run_identity(HostEntry::Cancel, Ok(Some(identity)), None);
        assert!(fox.is_ok());
    }

    /// 80b1622 acceptance: the login card must show for a NEW conversation
    /// (no row yet), and "覆盖已有登录" must be an explicit, default-off choice.
    #[test]
    fn credential_requests_accept_a_draft_target_and_overwrite_defaults_off() {
        let old: AgentBackendImportRequest = serde_json::from_value(serde_json::json!({ "conversationId": "c1", "selected": ["file:.codex/auth.json"], "allProjects": true })).unwrap();
        assert_eq!(old.target.conversation_id.as_deref(), Some("c1"));
        assert!(old.all_projects && !old.overwrite, "overwrite is off unless asked");
        let draft: AgentBackendImportRequest = serde_json::from_value(serde_json::json!({ "backend": "codex", "projectId": "p1", "projectRoot": "D:/proj", "fullAccess": true, "selected": [], "overwrite": true })).unwrap();
        assert!(draft.target.conversation_id.is_none());
        assert_eq!(draft.target.backend.as_deref(), Some("codex"));
        assert_eq!(draft.target.project_id.as_deref(), Some("p1"));
        assert!(draft.target.full_access && draft.overwrite && !draft.all_projects);
        let probe: crate::external_backend::AgentBackendTarget = serde_json::from_value(serde_json::json!({ "backend": "deepseek", "projectRoot": "D:/proj" })).unwrap();
        assert_eq!(probe.project_root.as_deref(), Some("D:/proj"));
        assert!(!probe.full_access);
        let conversation: crate::external_backend::AgentBackendTarget = serde_json::from_value(serde_json::json!({ "conversationId": "c2" })).unwrap();
        assert_eq!(conversation.conversation_id.as_deref(), Some("c2"));
    }

    #[test]
    fn agent_backend_draft_probe_finds_the_project_by_id_or_root_and_labels_projects_by_name() {
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        database
            .with_connection(|connection| {
                connection.execute("INSERT INTO projects(id,name,root_path,permission_mode,status,created_at,updated_at) VALUES ('p2','第二个项目','D:/two','read_only','active',1,1)", [])?;
                connection.execute("INSERT INTO projects(id,name,root_path,permission_mode,status,created_at,updated_at) VALUES ('p1','主项目','D:/main','read_only','active',1,1)", [])
            })
            .expect("projects");
        assert_eq!(database.known_projects().unwrap(), vec![("p1".to_owned(), "主项目".to_owned()), ("p2".to_owned(), "第二个项目".to_owned())]);
        let by_id = database.project_by_id_or_root(Some("p2"), Some("D:/main")).unwrap();
        assert_eq!(by_id, Some(("p2".to_owned(), "D:/two".to_owned())), "the id wins");
        let by_root = database.project_by_id_or_root(Some("gone"), Some("D:/main")).unwrap();
        assert_eq!(by_root, Some(("p1".to_owned(), "D:/main".to_owned())));
        assert_eq!(database.project_by_id_or_root(None, None).unwrap(), None);
        assert_eq!(database.project_by_id_or_root(Some(""), Some("D:/nowhere")).unwrap(), None);
    }
}
