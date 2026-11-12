use crate::{
    app_state::AppState,
    companion_workspace,
    database::{
        companions::{
            CompanionError, CompanionProfileEdit, CompanionView,
            CompanionsList,
        },
        ApiResponse, Database,
    },
};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionId {
    companion_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddExpert {
    expert_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditProfile {
    companion_id: String,
    #[serde(flatten)]
    edit: CompanionProfileEdit,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetCompanionModel {
    companion_id: String,
    provider_id: Option<String>,
    model_ref: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WebCapability {
    tool: String,
    granted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorTool {
    name: String,
    granted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectorCapability {
    server_id: String,
    name: String,
    transport: String,
    enabled: bool,
    status: String,
    last_error: Option<String>,
    tools: Vec<ConnectorTool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionCapabilities {
    web: Vec<WebCapability>,
    connectors: Vec<ConnectorCapability>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetCompanionCapability {
    companion_id: String,
    server_id: String,
    tool: String,
    granted: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PinCompanion {
    companion_id: String,
    pinned: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarkRead {
    companion_id: String,
    delivery_order: i64,
    observed_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MarkUnread {
    companion_id: String,
    bubble_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetReminderMode {
    companion_id: String,
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BubbleTarget {
    companion_id: String,
    bubble_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetBubbleFavorite {
    companion_id: String,
    bubble_id: String,
    favorited: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetBubbleReaction {
    companion_id: String,
    bubble_id: String,
    reaction: String,
}

fn response<T: Serialize>(result: Result<T, CompanionError>) -> ApiResponse<T> {
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => {
            let (code, message) = error.response();
            ApiResponse::failure(code, message, matches!(error, CompanionError::Storage(_)))
        }
    }
}

fn prepare_new_workspace(
    database: &Database,
    root: &Path,
    view: CompanionView,
) -> Result<CompanionView, CompanionError> {
    if view.profile.workspace_state != "pending" {
        return Ok(view);
    }
    let id = &view.profile.id;
    let result = companion_workspace::repair(database, root, id);
    let refreshed = database.companion_profile(id)?;
    if let Err(error) = result {
        if refreshed.profile.workspace_state != "failed" {
            return Err(CompanionError::Storage(error));
        }
    }
    Ok(refreshed)
}

pub(crate) fn ensure_default(database: &Database, root: &Path) -> Result<CompanionView, String> {
    let record = database
        .seed_default_companion(root)
        .and_then(|view| prepare_new_workspace(database, root, view));
    record.map_err(|error| error.response().1.to_owned())
}

#[tauri::command]
pub(crate) fn companions_list(state: State<'_, AppState>) -> ApiResponse<CompanionsList> {
    response(state.database.list_companions())
}

#[tauri::command]
pub(crate) fn companion_ensure_default(state: State<'_, AppState>) -> ApiResponse<CompanionView> {
    match ensure_default(&state.database, &state.data_dir) {
        Ok(view) => ApiResponse::success(view),
        Err(message) => ApiResponse::failure("storage.error", message, true),
    }
}

#[tauri::command]
pub(crate) fn companion_add_from_expert(
    state: State<'_, AppState>,
    request: AddExpert,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .add_companion(&state.data_dir, &request.expert_id)
            .and_then(|view| prepare_new_workspace(&state.database, &state.data_dir, view)),
    )
}

#[tauri::command]
pub(crate) fn companion_create_blank(state: State<'_, AppState>) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .create_blank_companion(&state.data_dir)
            .and_then(|view| prepare_new_workspace(&state.database, &state.data_dir, view)),
    )
}

#[tauri::command]
pub(crate) fn companion_open(
    state: State<'_, AppState>,
    request: CompanionId,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .companion_profile(&request.companion_id)
            .and_then(|view| {
                if view.profile.archived_at.is_some() {
                    Err(CompanionError::Archived)
                } else {
                    Ok(view)
                }
            }),
    )
}

#[tauri::command]
pub(crate) fn companion_update_profile(
    state: State<'_, AppState>,
    request: EditProfile,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .edit_companion(&request.companion_id, &request.edit),
    )
}

#[tauri::command]
pub(crate) fn companion_set_model(
    state: State<'_, AppState>,
    request: SetCompanionModel,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .set_companion_model(&request.companion_id, request.provider_id.as_deref(), request.model_ref.as_deref()),
    )
}

/// C02: the profile's capability list. Grants are the user's choice; the tool
/// list is the connector's live reality. A disabled or unreachable connector
/// shows its reason instead of pretending its tools are callable.
#[tauri::command]
pub(crate) fn companion_capabilities_get(
    state: State<'_, AppState>,
    request: CompanionId,
) -> ApiResponse<CompanionCapabilities> {
    let grants = match state.database.companion_tool_grants(&request.companion_id) {
        Ok(grants) => grants,
        Err(error) => return ApiResponse::failure("storage.error", error, true),
    };
    let granted = |server: &str, tool: &str| grants.iter().any(|(s, t)| s == server && t == tool);
    let web = crate::database::WEB_TOOLS.iter()
        .map(|tool| WebCapability { tool: (*tool).to_owned(), granted: granted(crate::database::WEB_SERVER_ID, tool) })
        .collect();
    let servers = match state.database.list_mcp_servers() {
        Ok(servers) => servers,
        Err(error) => return ApiResponse::failure("storage.error", error, true),
    };
    let connectors = servers.into_iter().map(|server| {
        let (tools, last_error) = if server.enabled {
            match crate::mcp::list_tools(&server) {
                Ok(list) => (list.into_iter()
                    .filter_map(|item| item.get("name").and_then(|value| value.as_str()).map(str::to_owned))
                    .map(|name| ConnectorTool { granted: granted(&server.id, &name), name })
                    .collect::<Vec<_>>(), None),
                Err(error) => (Vec::new(), Some(error)),
            }
        } else {
            (Vec::new(), server.last_error.clone())
        };
        ConnectorCapability {
            server_id: server.id,
            name: server.name,
            transport: server.transport,
            enabled: server.enabled,
            status: server.status,
            last_error,
            tools,
        }
    }).collect();
    ApiResponse::success(CompanionCapabilities { web, connectors })
}

#[tauri::command]
pub(crate) fn companion_capability_set(
    state: State<'_, AppState>,
    request: SetCompanionCapability,
) -> ApiResponse<()> {
    match state.database.set_companion_tool_grant(&request.companion_id, &request.server_id, &request.tool, request.granted) {
        Ok(affected) => {
            // D1-2: best-effort Host refresh of the invalidated cards/waiters. The
            // persistent barrier committed above is authoritative even if this
            // notification fails.
            if !affected.is_empty() {
                state.runtime_host.refresh_invalidated_approvals(&affected);
            }
            ApiResponse::success(())
        }
        Err(error) => ApiResponse::failure("companion.capability_invalid", error, false),
    }
}

#[tauri::command]
pub(crate) fn companion_set_pinned(
    state: State<'_, AppState>,
    request: PinCompanion,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .pin_companion(&request.companion_id, request.pinned),
    )
}

#[tauri::command]
pub(crate) fn companion_mark_read(
    state: State<'_, AppState>,
    request: MarkRead,
) -> ApiResponse<CompanionView> {
    response(state.database.read_companion_through(
        &request.companion_id,
        request.delivery_order,
        request.observed_at,
    ))
}

/// K6: mark a contact-sent visible bubble unread from its position onward.
#[tauri::command]
pub(crate) fn companion_mark_unread(
    state: State<'_, AppState>,
    request: MarkUnread,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .mark_companion_unread(&request.companion_id, &request.bubble_id),
    )
}

/// K6: per-contact reminder mode (follow_global / all / action_only / muted).
#[tauri::command]
pub(crate) fn companion_set_reminder_mode(
    state: State<'_, AppState>,
    request: SetReminderMode,
) -> ApiResponse<CompanionView> {
    response(
        state
            .database
            .set_companion_reminder_mode(&request.companion_id, &request.mode),
    )
}

/// C07: the composer's quote draft. Only a committed bubble of this contact's
/// private chat resolves; a live draft or preview is refused here already.
#[tauri::command]
pub(crate) fn companion_quote_preview(
    state: State<'_, AppState>,
    request: BubbleTarget,
) -> ApiResponse<crate::database::ContactQuotePreview> {
    response(
        state
            .database
            .quote_companion_bubble(&request.companion_id, &request.bubble_id),
    )
}

/// C07: the user's own favorite on a user message or a contact reply.
#[tauri::command]
pub(crate) fn companion_bubble_favorite(
    state: State<'_, AppState>,
    request: SetBubbleFavorite,
) -> ApiResponse<crate::database::ContactBubbleMarkRecord> {
    response(state.database.favorite_companion_bubble(
        &request.companion_id,
        &request.bubble_id,
        request.favorited,
    ))
}

/// C07: one reaction per bubble — the stored one clears it, another replaces it.
#[tauri::command]
pub(crate) fn companion_bubble_reaction(
    state: State<'_, AppState>,
    request: SetBubbleReaction,
) -> ApiResponse<crate::database::ContactBubbleMarkRecord> {
    response(state.database.react_companion_bubble(
        &request.companion_id,
        &request.bubble_id,
        &request.reaction,
    ))
}

/// C07: the contact's favorites (summary + time), newest marked first.
#[tauri::command]
pub(crate) fn companion_favorites_list(
    state: State<'_, AppState>,
    request: CompanionId,
) -> ApiResponse<Vec<crate::database::ContactFavoriteEntry>> {
    response(state.database.companion_favorites(&request.companion_id, 200))
}

#[tauri::command]
pub(crate) fn companion_archive(
    state: State<'_, AppState>,
    request: CompanionId,
) -> ApiResponse<CompanionView> {
    response(state.runtime_host.archive_companion(&request.companion_id))
}

#[tauri::command]
pub(crate) fn companion_restore(
    state: State<'_, AppState>,
    request: CompanionId,
) -> ApiResponse<CompanionView> {
    response(state.database.restore_companion(&request.companion_id))
}
