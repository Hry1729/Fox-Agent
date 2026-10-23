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

// ---------------------------------------------------------------------------
// Settings batch, items 4 / 5 / 6: 设置 →「新会话使用的 Agent」and
// 「新外部会话使用完全访问」, persisted like the other app settings
// (`app_settings`), plus the cached Responses probe that decides whether
// Codex can use 模型服务's default provider.
//
// Precedence of the backend of a NEW conversation: FOX_EXTERNAL_BACKENDS
// (administrator, can only force off) > this setting (user) > Fox
// (default). Existing conversations keep their frozen backend.

/// `app_settings` key: backend id of new conversations (`fox` when unset).
pub(crate) const DEFAULT_BACKEND_KEY: &str = "agent_backend.default_backend";
/// `app_settings` key: `1` = new external conversations get full access.
pub(crate) const NEW_EXTERNAL_FULL_ACCESS_KEY: &str = "agent_backend.new_external_full_access";
/// `app_settings` key: last Responses probe of 模型服务's base URL (JSON).
pub(crate) const CODEX_RESPONSES_PROBE_KEY: &str = "agent_backend.codex_responses_probe";

/// Result of probing `POST {base}/responses` (no key, no body content).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexResponsesProbe {
    pub base_url: String,
    /// `Some(false)`: 404 / 405 (no Responses API); `Some(true)`: the route
    /// exists; `None`: undetermined (auth, rate limit, server, network).
    pub supported: Option<bool>,
    pub status: Option<u16>,
    pub checked_at: i64,
}

/// What an HTTP status of `POST {base}/responses` (empty JSON body) says.
pub(crate) fn responses_support_from_status(status: u16) -> Option<bool> {
    match status {
        404 | 405 => Some(false),
        200..=299 => Some(true),
        _ => None,
    }
}

/// The cached probe, when it is about `base_url`.
pub(crate) fn cached_codex_probe(database: &crate::database::Database, base_url: &str) -> Option<CodexResponsesProbe> {
    database
        .app_setting(CODEX_RESPONSES_PROBE_KEY)
        .ok()
        .flatten()
        .and_then(|text| serde_json::from_str::<CodexResponsesProbe>(&text).ok())
        .filter(|probe| probe.base_url == base_url)
}

/// One choice of「新会话使用的 Agent」.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendPreferenceOption {
    pub backend: String,
    pub enabled: bool,
    pub experimental: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendPreferences {
    /// What the user saved (`fox` when never set or unreadable).
    pub default_backend: String,
    /// What a new conversation uses now: the saved backend unless the
    /// administrator forced it off, then Fox.
    pub effective_backend: String,
    pub new_external_full_access: bool,
    /// The saved full-access choice unless FORBIDDEN.
    pub effective_full_access: bool,
    pub full_access_forbidden: bool,
    pub forbidden_reason: Option<String>,
    pub switch_env: String,
    /// Every external backend is forced off by the administrator.
    pub forced_off: bool,
    pub options: Vec<AgentBackendPreferenceOption>,
    /// Name of 模型服务's default provider used for the unified login.
    pub unified_login_provider: Option<String>,
    pub codex_responses: Option<CodexResponsesProbe>,
}

/// The saved backend (`fox` for unset / garbage).
pub(crate) fn stored_default_backend(database: &crate::database::Database) -> AgentBackend {
    database
        .app_setting(DEFAULT_BACKEND_KEY)
        .ok()
        .flatten()
        .and_then(|value| AgentBackend::parse(value.trim()))
        .unwrap_or(AgentBackend::Fox)
}

/// The saved full-access choice (`false` for unset / garbage).
pub(crate) fn stored_new_external_full_access(database: &crate::database::Database) -> bool {
    database.app_setting(NEW_EXTERNAL_FULL_ACCESS_KEY).ok().flatten().is_some_and(|value| value.trim() == "1")
}

/// Build the preferences view (pure: no environment, no keyring).
pub(crate) fn preferences_view(
    stored: AgentBackend,
    full_access: bool,
    switch: &BackendSwitch,
    forbidden: bool,
    provider: Option<String>,
    codex: Option<CodexResponsesProbe>,
) -> AgentBackendPreferences {
    let options: Vec<AgentBackendPreferenceOption> = host_routing::backend_options(switch)
        .into_iter()
        .map(|option| {
            let codex_unsupported = option.backend == AgentBackend::Codex && option.enabled && codex.as_ref().is_some_and(|probe| probe.supported == Some(false));
            AgentBackendPreferenceOption {
                backend: option.backend.as_str().to_owned(),
                enabled: option.enabled && !codex_unsupported,
                experimental: option.experimental,
                reason: if codex_unsupported {
                    Some(format!(
                        "模型服务的默认 provider「{}」的地址不支持 Responses 接口（POST /responses 返回 404 / 405），Codex 无法用它登录",
                        provider.as_deref().unwrap_or("-")
                    ))
                } else {
                    option.reason
                },
            }
        })
        .collect();
    // The administrator's switch wins; a Codex whose provider lacks the
    // Responses API stays effective if it was saved (the user may have their
    // own Codex login in the private HOME), it just cannot be newly picked.
    let effective = if switch.is_enabled(stored) { stored } else { AgentBackend::Fox };
    AgentBackendPreferences {
        default_backend: stored.as_str().to_owned(),
        effective_backend: effective.as_str().to_owned(),
        new_external_full_access: full_access,
        effective_full_access: full_access && !forbidden,
        full_access_forbidden: forbidden,
        forbidden_reason: forbidden.then(|| fox_agent_backend::external_launch::FULL_ACCESS_FORBIDDEN_REASON.to_owned()),
        switch_env: host_routing::EXTERNAL_BACKENDS_ENV.to_owned(),
        forced_off: !switch.any_external(),
        options,
        unified_login_provider: provider,
        codex_responses: codex,
    }
}

fn current_preferences(state: &AppState) -> AgentBackendPreferences {
    let service = state.database.get_model_service().ok().flatten();
    let provider = service.as_ref().map(|service| service.name.clone());
    let codex = None; // Protocol selection replaces URL-based enablement guesses.
    preferences_view(
        stored_default_backend(&state.database),
        stored_new_external_full_access(&state.database),
        &BackendSwitch::from_env(),
        fox_agent_backend::external_launch::full_access_forbidden_from_env(),
        provider,
        codex,
    )
}

#[tauri::command]
pub fn agent_backend_preferences_get(state: State<'_, AppState>) -> ApiResponse<AgentBackendPreferences> {
    ApiResponse::success(current_preferences(&state))
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendPreferencesRequest {
    #[serde(default)]
    pub default_backend: Option<String>,
    #[serde(default)]
    pub new_external_full_access: Option<bool>,
}

/// Validate a change of the two settings against the administrator's
/// switches (`Err` = refusal code + message). Pure.
pub(crate) fn validate_preferences_change(
    request: &AgentBackendPreferencesRequest,
    current: &AgentBackendPreferences,
) -> Result<(Option<AgentBackend>, Option<bool>), (&'static str, String)> {
    let backend = match request.default_backend.as_deref() {
        None => None,
        Some(raw) => {
            let backend = AgentBackend::parse(raw.trim()).ok_or_else(|| (RouteError::CODE_INVALID, format!("不支持的 Agent 后端「{raw}」")))?;
            let option = current.options.iter().find(|option| option.backend == backend.as_str());
            if backend != AgentBackend::Fox && !option.is_some_and(|option| option.enabled) {
                let reason = option.and_then(|option| option.reason.clone()).unwrap_or_default();
                return Err((RouteError::CODE_DISABLED, format!("不能选择「{}」：{reason}", backend.as_str())));
            }
            Some(backend)
        }
    };
    let full_access = match request.new_external_full_access {
        Some(true) if current.full_access_forbidden => {
            return Err((host_routing::CODE_FULL_ACCESS_FORBIDDEN, fox_agent_backend::external_launch::FULL_ACCESS_FORBIDDEN_REASON.to_owned()))
        }
        other => other,
    };
    Ok((backend, full_access))
}

#[tauri::command]
pub fn agent_backend_preferences_save(state: State<'_, AppState>, request: AgentBackendPreferencesRequest) -> ApiResponse<AgentBackendPreferences> {
    let current = current_preferences(&state);
    let (backend, full_access) = match validate_preferences_change(&request, &current) {
        Ok(change) => change,
        Err((code, message)) => return ApiResponse::failure(code, message, false),
    };
    if let Some(backend) = backend {
        if let Err(error) = state.database.set_app_setting(DEFAULT_BACKEND_KEY, backend.as_str()) {
            return storage_error(error);
        }
    }
    if let Some(full_access) = full_access {
        if let Err(error) = state.database.set_app_setting(NEW_EXTERNAL_FULL_ACCESS_KEY, if full_access { "1" } else { "0" }) {
            return storage_error(error);
        }
    }
    ApiResponse::success(current_preferences(&state))
}

/// Explicit connection check for the selected Codex protocol. Settings no
/// longer run this automatically or use a failed HTTP request as support proof.
#[tauri::command]
pub async fn agent_backend_probe_codex_responses(state: State<'_, AppState>) -> Result<ApiResponse<Option<CodexResponsesProbe>>, ()> {
    let Some(service) = crate::model_service::external_login(&state.database, AgentBackend::Codex) else {
        return Ok(ApiResponse::success(None));
    };
    let result = state.model_service_client.test(&service.base_url, Some(service.api_key.expose()), &service.api_type, &service.model).await;
    Ok(ApiResponse::success(Some(CodexResponsesProbe {
        base_url: service.base_url,
        supported: result.is_ok().then_some(true),
        status: None,
        checked_at: chrono::Utc::now().timestamp_millis(),
    })))
}

// ---------------------------------------------------------------------------
// Per-agent native permission list (权限按 Agent 原生列出). The list, the
// names and the rules live in `fox_agent_backend::native_permission`; this
// is storage (`app_settings`) and the two commands for the backend bar.
//
// * `agent_backend.native_mode.conversation.<id>`: the conversation's current
//   sandboxed choice (the full-access option comes from the grant).
// * `agent_backend.native_mode.last.<backend>`: each Agent's last sandboxed
//   choice, the starting point of its next new conversation.
// * `agent_backend.native_mode.draft_full.<backend>`: `1` / `0` when the
//   user picked (or left) the full-access option in a new conversation's
//   list; read once at creation and cleared. Absent: 设置 →「新外部会话使用
//   完全访问」is the default.

pub(crate) const NATIVE_MODE_CONVERSATION_PREFIX: &str = "agent_backend.native_mode.conversation.";
pub(crate) const NATIVE_MODE_LAST_PREFIX: &str = "agent_backend.native_mode.last.";
pub(crate) const NATIVE_MODE_DRAFT_FULL_PREFIX: &str = "agent_backend.native_mode.draft_full.";

fn non_empty_setting(database: &crate::database::Database, key: &str) -> Option<String> {
    database.app_setting(key).ok().flatten().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// The conversation's stored choice; `None` = the Agent's default.
pub(crate) fn stored_conversation_native_mode(database: &crate::database::Database, conversation_id: &str) -> Option<String> {
    non_empty_setting(database, &format!("{NATIVE_MODE_CONVERSATION_PREFIX}{conversation_id}"))
}

/// The Agent's last sandboxed choice.
pub(crate) fn stored_last_native_mode(database: &crate::database::Database, backend: AgentBackend) -> Option<String> {
    non_empty_setting(database, &format!("{NATIVE_MODE_LAST_PREFIX}{}", backend.as_str()))
}

/// The new conversation's explicit full-access pick, if any.
pub(crate) fn draft_full_access_choice(database: &crate::database::Database, backend: AgentBackend) -> Option<bool> {
    non_empty_setting(database, &format!("{NATIVE_MODE_DRAFT_FULL_PREFIX}{}", backend.as_str())).map(|value| value == "1")
}

/// Creation consumed the draft pick.
pub(crate) fn clear_draft_full_access_choice(database: &crate::database::Database, backend: AgentBackend) {
    let _ = database.set_app_setting(&format!("{NATIVE_MODE_DRAFT_FULL_PREFIX}{}", backend.as_str()), "");
}

/// Record a new conversation's starting mode (the Agent's last sandboxed
/// choice, when it has one).
pub(crate) fn seed_conversation_native_mode(database: &crate::database::Database, conversation_id: &str, backend: AgentBackend) {
    if let Some(mode) = stored_last_native_mode(database, backend) {
        if fox_agent_backend::native_permission::is_known(backend, &mode) && !fox_agent_backend::native_permission::is_full_access(backend, &mode) {
            let _ = database.set_app_setting(&format!("{NATIVE_MODE_CONVERSATION_PREFIX}{conversation_id}"), &mode);
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativePermissionRequest {
    #[serde(default)]
    pub conversation_id: Option<String>,
    /// Backend of a new conversation (ignored when `conversation_id` is set).
    #[serde(default)]
    pub backend: Option<String>,
    /// The option to switch to (set only).
    #[serde(default)]
    pub mode: Option<String>,
}

fn native_context(state: &AppState, backend: AgentBackend, forbidden: bool) -> fox_agent_backend::native_permission::CatalogContext {
    fox_agent_backend::native_permission::CatalogContext {
        provider_base_url: crate::external_model::external_login(&state.database, backend).map(|login| login.base_url),
        full_access_forbidden: forbidden,
    }
}

/// Which Agent, whether its session runs in full access, and whether that
/// comes from a removable grant. `Err` = refusal for the UI.
fn native_target(
    state: &AppState,
    external: &crate::external_backend::ExternalBackendHost,
    request: &NativePermissionRequest,
) -> Result<(AgentBackend, bool, bool), ApiResponse<serde_json::Value>> {
    match request.conversation_id.as_deref() {
        Some(id) => {
            let backend = match state.database.conversation_agent_backend(id) {
                Ok(Some(stored)) => AgentBackend::parse(&stored).unwrap_or(AgentBackend::Fox),
                Ok(None) => return Err(ApiResponse::failure("conversation.not_found", "会话不存在", false)),
                Err(error) => return Err(ApiResponse::failure("agent_backend_lookup_failed", error, true)),
            };
            let status = external.permission_status(Some(id));
            let full = status.get("mode").and_then(serde_json::Value::as_str) == Some("full_access");
            let granted = status.get("granted").and_then(serde_json::Value::as_bool).unwrap_or(false);
            Ok((backend, full, granted))
        }
        None => {
            let backend = request.backend.as_deref().and_then(AgentBackend::parse).unwrap_or(AgentBackend::Fox);
            let forbidden = fox_agent_backend::external_launch::full_access_forbidden_from_env();
            let full = !forbidden
                && draft_full_access_choice(&state.database, backend).unwrap_or_else(|| stored_new_external_full_access(&state.database));
            Ok((backend, full, full))
        }
    }
}

fn native_view(state: &AppState, external: &crate::external_backend::ExternalBackendHost, request: &NativePermissionRequest) -> ApiResponse<serde_json::Value> {
    let (backend, full, granted) = match native_target(state, external, request) {
        Ok(target) => target,
        Err(refusal) => return refusal,
    };
    let forbidden = fox_agent_backend::external_launch::full_access_forbidden_from_env();
    let context = native_context(state, backend, forbidden);
    let catalog = fox_agent_backend::native_permission::catalog(backend, &context);
    let stored = match request.conversation_id.as_deref() {
        Some(id) => stored_conversation_native_mode(&state.database, id),
        None => stored_last_native_mode(&state.database, backend),
    };
    let current = fox_agent_backend::native_permission::effective_mode(backend, stored.as_deref(), full, &context);
    let draft = request.conversation_id.is_none();
    ApiResponse::success(serde_json::json!({
        "catalog": catalog,
        "current": current,
        "fullAccess": full,
        "fullAccessGranted": granted,
        "fullAccessForbidden": forbidden,
        "draft": draft,
        // Full access is granted only when a conversation is created.
        "fullAccessLockedReason": if draft || full { None } else { Some("完全访问只能在新建会话时选择（会话中只能撤销）") },
    }))
}

/// The open conversation's (or a new conversation's) permission list in its
/// Agent's own names, and the current choice.
#[tauri::command]
pub fn agent_backend_native_permissions(
    state: State<'_, AppState>,
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: NativePermissionRequest,
) -> ApiResponse<serde_json::Value> {
    native_view(&state, &external, &request)
}

/// Switch the mode. A new conversation: remembered for its Agent (the
/// full-access option is a one-shot pick, granted and audited at creation).
/// An open conversation: Codex switches live (`session/set_mode`), dsh at
/// the next turn (restart), pi at the next request; leaving the full-access
/// option revokes full access through the existing revoke path (audited,
/// backend restarted in the sandbox); entering it mid-session is refused.
#[tauri::command]
pub async fn agent_backend_native_permission_set(
    state: State<'_, AppState>,
    external: tauri::State<'_, crate::external_backend::ExternalBackendHost>,
    request: NativePermissionRequest,
) -> Result<ApiResponse<serde_json::Value>, ()> {
    use fox_agent_backend::native_permission as native;
    let (backend, full, granted) = match native_target(&state, &external, &request) {
        Ok(target) => target,
        Err(refusal) => return Ok(refusal),
    };
    if backend == AgentBackend::Fox {
        return Ok(ApiResponse::failure(RouteError::CODE_INVALID, "Fox 会话沿用原来的执行方式", false));
    }
    let forbidden = fox_agent_backend::external_launch::full_access_forbidden_from_env();
    let context = native_context(&state, backend, forbidden);
    let mode = request.mode.as_deref().map(str::trim).unwrap_or_default().to_owned();
    let catalog = native::catalog(backend, &context);
    let Some(option) = catalog.options.iter().find(|option| option.id == mode) else {
        return Ok(ApiResponse::failure(RouteError::CODE_INVALID, format!("「{}」没有权限选项「{mode}」", backend.as_str()), false));
    };
    if let Some(reason) = &option.disabled_reason {
        let code = if option.full_access { host_routing::CODE_FULL_ACCESS_FORBIDDEN } else { "agent_backend_permission_unavailable" };
        return Ok(ApiResponse::failure(code, reason.clone(), false));
    }
    let set = |key: String, value: &str| state.database.set_app_setting(&key, value);
    let Some(conversation_id) = request.conversation_id.clone() else {
        // New conversation.
        let result = if option.full_access {
            set(format!("{NATIVE_MODE_DRAFT_FULL_PREFIX}{}", backend.as_str()), "1")
        } else {
            set(format!("{NATIVE_MODE_LAST_PREFIX}{}", backend.as_str()), &mode)
                .and_then(|_| set(format!("{NATIVE_MODE_DRAFT_FULL_PREFIX}{}", backend.as_str()), "0"))
        };
        if let Err(error) = result {
            return Ok(storage_error(error));
        }
        return Ok(native_view(&state, &external, &request));
    };
    if option.full_access && !full {
        return Ok(ApiResponse::failure("agent_backend_full_access_locked", "完全访问只能在新建会话时选择；会话中只能撤销，不能开启", false));
    }
    let previous = native::effective_mode(backend, stored_conversation_native_mode(&state.database, &conversation_id).as_deref(), full, &context);
    let mut revoked = None;
    if !option.full_access && full {
        if !granted {
            return Ok(ApiResponse::failure(
                "agent_backend_full_access_engineering",
                "本会话的完全访问来自工程开关（FOX_EXTERNAL_FULL_ACCESS），不能在界面撤销",
                false,
            ));
        }
        // Leaving full access = the existing revoke (grant removed,
        // audited, the backend restarts in the sandbox).
        let response = external.revoke_full_access(&conversation_id).await;
        if !response.ok {
            return Ok(response);
        }
        revoked = response.data;
    }
    if !option.full_access {
        let stored = set(format!("{NATIVE_MODE_CONVERSATION_PREFIX}{conversation_id}"), &mode)
            .and_then(|_| set(format!("{NATIVE_MODE_LAST_PREFIX}{}", backend.as_str()), &mode));
        if let Err(error) = stored {
            return Ok(storage_error(error));
        }
    }
    let outcome = external.apply_native_mode(&conversation_id, &mode).await;
    let details = serde_json::json!({ "from": previous, "to": mode, "applied": outcome.as_str(), "fullAccessRevoked": revoked.is_some() });
    let _ = state.database.append_agent_backend_audit(Some(&conversation_id), backend.as_str(), "agent_backend.permission_mode_changed", &details);
    let mut view = native_view(&state, &external, &request);
    if let Some(data) = view.data.as_mut() {
        data["applied"] = serde_json::json!(outcome.as_str());
        if let fox_agent_backend::external::NativeModeOutcome::Failed(message) = &outcome {
            data["appliedError"] = serde_json::json!(message);
        }
        if let Some(revoked) = revoked {
            data["revoke"] = revoked;
        }
    }
    Ok(view)
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

        // Settings batch: with FOX_EXTERNAL_BACKENDS unset nothing is forced
        // off, so a lookup that fails is refused (an external conversation
        // must never fall back to Fox); only a forced-off switch keeps the
        // old fallback.
        let fallback = !BackendSwitch::from_env().any_external();
        let storage_error: Result<Route, ApiResponse<()>> =
            gate_run_identity(HostEntry::Cancel, Err("database is locked".to_owned()), None);
        assert_eq!(storage_error.is_ok(), fallback);
        let missing_column: Result<Route, ApiResponse<()>> =
            gate_run_identity(HostEntry::Approve, Err("no such column: c.agent_backend".to_owned()), None);
        assert_eq!(missing_column.is_ok(), fallback);
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
        if BackendSwitch::from_env().is_enabled(AgentBackend::Pi) {
            // FOX_EXTERNAL_BACKENDS unset (settings batch): pi is routed to
            // its external runtime, never to Fox.
            assert!(matches!(gated, Ok(Route::External(AgentBackend::Pi))));
        } else {
            let failure = gated.expect_err("pi is refused before any runtime");
            let code = failure.error.as_ref().map(|error| error.code.clone());
            assert!(matches!(code.as_deref(), Some("agent_backend_disabled")), "{code:?}");
        }
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

    /// Settings batch, item 4: FOX_EXTERNAL_BACKENDS only forces off; the
    /// setting decides otherwise; Fox is the default and the fallback.
    #[test]
    fn agent_backend_preferences_follow_the_setting_under_the_administrator_switch() {
        let on = BackendSwitch::from_env_value(None);
        let view = preferences_view(AgentBackend::Pi, false, &on, false, Some("DeepSeek".into()), None);
        assert_eq!(view.default_backend, "pi");
        assert_eq!(view.effective_backend, "pi");
        assert!(!view.forced_off && view.options.iter().all(|option| option.enabled));
        let off = BackendSwitch::from_env_value(Some("0"));
        let forced = preferences_view(AgentBackend::Pi, true, &off, false, None, None);
        assert_eq!(forced.effective_backend, "fox", "forced off: new conversations use Fox");
        assert!(forced.forced_off);
        assert!(forced.options.iter().filter(|option| option.backend != "fox").all(|option| !option.enabled && option.reason.as_deref().unwrap().contains("FOX_EXTERNAL_BACKENDS")));
        let only_codex = BackendSwitch::from_env_value(Some("codex"));
        assert_eq!(preferences_view(AgentBackend::Deepseek, false, &only_codex, false, None, None).effective_backend, "fox");
        // Default when nothing was saved: Fox.
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        assert_eq!(stored_default_backend(&database), AgentBackend::Fox);
        assert!(!stored_new_external_full_access(&database));
        database.set_app_setting(DEFAULT_BACKEND_KEY, "codex").unwrap();
        database.set_app_setting(NEW_EXTERNAL_FULL_ACCESS_KEY, "1").unwrap();
        assert_eq!(stored_default_backend(&database), AgentBackend::Codex);
        assert!(stored_new_external_full_access(&database));
        database.set_app_setting(DEFAULT_BACKEND_KEY, "deepseek_harness").unwrap();
        assert_eq!(stored_default_backend(&database), AgentBackend::Fox, "garbage reads as Fox");
    }

    /// Settings batch, items 4 / 5: a forced-off backend and full access under
    /// FORBIDDEN cannot be saved; Codex without the Responses API is greyed.
    #[test]
    fn agent_backend_preference_changes_are_validated_against_the_switches() {
        let off = preferences_view(AgentBackend::Fox, false, &BackendSwitch::from_env_value(Some("off")), true, None, None);
        let request = |backend: Option<&str>, full: Option<bool>| AgentBackendPreferencesRequest { default_backend: backend.map(str::to_owned), new_external_full_access: full };
        assert_eq!(validate_preferences_change(&request(Some("pi"), None), &off).unwrap_err().0, "agent_backend_disabled");
        assert_eq!(validate_preferences_change(&request(Some("fox"), None), &off).unwrap(), (Some(AgentBackend::Fox), None));
        assert_eq!(validate_preferences_change(&request(None, Some(true)), &off).unwrap_err().0, "agent_backend_full_access_forbidden");
        assert_eq!(validate_preferences_change(&request(None, Some(false)), &off).unwrap(), (None, Some(false)));
        assert_eq!(validate_preferences_change(&request(Some("claude"), None), &off).unwrap_err().0, "agent_backend_invalid");
        assert!(off.forbidden_reason.is_some() && !off.effective_full_access);
        let probe = CodexResponsesProbe { base_url: "https://api.example.com/v1".into(), supported: Some(false), status: Some(404), checked_at: 1 };
        let on = preferences_view(AgentBackend::Fox, true, &BackendSwitch::from_env_value(None), false, Some("Example".into()), Some(probe));
        let codex = on.options.iter().find(|option| option.backend == "codex").unwrap();
        assert!(!codex.enabled && codex.reason.as_deref().unwrap().contains("Responses"));
        assert_eq!(validate_preferences_change(&request(Some("codex"), None), &on).unwrap_err().0, "agent_backend_disabled");
        assert!(on.effective_full_access);
        assert_eq!(responses_support_from_status(404), Some(false));
        assert_eq!(responses_support_from_status(405), Some(false));
        assert_eq!(responses_support_from_status(400), None);
        assert_eq!(responses_support_from_status(422), None);
        assert_eq!(responses_support_from_status(200), Some(true));
        assert_eq!(responses_support_from_status(401), None);
        assert_eq!(responses_support_from_status(503), None);
    }
}
