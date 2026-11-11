use crate::{app_state::AppState, database::ApiResponse};
use serde::Serialize;
use serde_json::Value;
use tauri::State;

fn response<T: Serialize>(value: Result<T, String>) -> ApiResponse<T> {
    value.map(ApiResponse::success).unwrap_or_else(|error| {
        let code = error
            .split(|c: char| c.is_whitespace() || c == ':' || c == '：')
            .find(|s| s.starts_with("companion.") || s.starts_with("memory."))
            .unwrap_or("companion.notes_failed")
            .to_owned();
        ApiResponse::failure(&code, error, false)
    })
}

#[tauri::command]
pub(crate) fn companion_notes_get(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<Value> {
    response(state.database.companion_notes_get(&companion_id))
}

#[tauri::command]
pub(crate) fn companion_note_save_user(
    state: State<'_, AppState>,
    companion_id: String,
    memory_id: Option<String>,
    expected_version: Option<i64>,
    content: String,
    canonical_key: Option<String>,
    kind: Option<String>,
) -> ApiResponse<Value> {
    response(state.database.companion_note_save_user(
        &companion_id,
        memory_id.as_deref(),
        expected_version,
        &content,
        canonical_key.as_deref(),
        kind.as_deref(),
    ))
}

#[tauri::command]
pub(crate) fn companion_note_set_enabled(
    state: State<'_, AppState>,
    companion_id: String,
    memory_id: String,
    expected_version: i64,
    enabled: bool,
) -> ApiResponse<Value> {
    response(state.database.companion_note_set_enabled(
        &companion_id,
        &memory_id,
        expected_version,
        enabled,
    ))
}

#[tauri::command]
pub(crate) fn companion_note_draft_save(
    state: State<'_, AppState>,
    companion_id: String,
    content: String,
) -> ApiResponse<()> {
    response(
        state
            .database
            .companion_note_draft_save(&companion_id, &content),
    )
}

#[tauri::command]
pub(crate) fn companion_note_draft_to_topic(
    state: State<'_, AppState>,
    companion_id: String,
    title: String,
    aliases: Option<Vec<String>>,
) -> ApiResponse<Value> {
    response(state.database.companion_note_draft_to_topic(
        &companion_id,
        &title,
        aliases.as_deref().unwrap_or(&[]),
    ))
}

#[tauri::command]
pub(crate) fn companion_topic_read(
    state: State<'_, AppState>,
    companion_id: String,
    topic_id: String,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_topic_read(&companion_id, &topic_id),
    )
}

#[tauri::command]
pub(crate) fn companion_topic_save(
    state: State<'_, AppState>,
    companion_id: String,
    topic_id: Option<String>,
    title: String,
    aliases: Option<Vec<String>>,
    content: String,
    expected_sha: Option<String>,
) -> ApiResponse<Value> {
    response(state.database.companion_topic_save(
        &companion_id,
        topic_id.as_deref(),
        &title,
        aliases.as_deref().unwrap_or(&[]),
        &content,
        expected_sha.as_deref(),
    ))
}

#[tauri::command]
pub(crate) fn companion_topic_delete(
    state: State<'_, AppState>,
    companion_id: String,
    topic_id: String,
    expected_sha: String,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_topic_delete(&companion_id, &topic_id, &expected_sha),
    )
}

#[tauri::command]
pub(crate) fn companion_note_request_resolve(
    state: State<'_, AppState>,
    companion_id: String,
    request_id: String,
    approve: bool,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_note_request_resolve(&companion_id, &request_id, approve),
    )
}

#[tauri::command]
pub(crate) fn companion_shared_request_resolve(
    state: State<'_, AppState>,
    companion_id: String,
    request_id: String,
    decision: String,
    replace_memory_id: Option<String>,
) -> ApiResponse<Value> {
    response(state.database.companion_shared_request_resolve(
        &companion_id,
        &request_id,
        &decision,
        replace_memory_id.as_deref(),
    ))
}

#[tauri::command]
pub(crate) fn companion_memory_undo(
    state: State<'_, AppState>,
    companion_id: String,
    memory_id: String,
    run_id: String,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_memory_undo(&companion_id, &memory_id, &run_id),
    )
}

#[tauri::command]
pub(crate) fn companion_note_move_undo(
    state: State<'_, AppState>,
    companion_id: String,
    move_id: String,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_note_move_undo(&companion_id, &move_id),
    )
}

#[tauri::command]
pub(crate) fn companion_note_delete(
    state: State<'_, AppState>,
    companion_id: String,
    memory_id: String,
    expected_version: i64,
) -> ApiResponse<Value> {
    response(state.database.companion_note_delete(
        &companion_id,
        Some(&memory_id),
        Some(expected_version),
    ))
}

#[tauri::command]
pub(crate) fn companion_note_clear(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_note_delete(&companion_id, None, None),
    )
}

#[tauri::command]
pub(crate) fn companion_note_confirm(
    state: State<'_, AppState>,
    companion_id: String,
    memory_id: String,
    expected_version: i64,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_note_confirm(&companion_id, &memory_id, expected_version),
    )
}

#[tauri::command]
pub(crate) fn companion_note_conflict_resolve(
    state: State<'_, AppState>,
    companion_id: String,
    conflict_id: String,
    accept: bool,
) -> ApiResponse<Value> {
    response(
        state
            .database
            .companion_note_conflict_resolve(&companion_id, &conflict_id, accept),
    )
}

#[tauri::command]
pub(crate) fn companion_grants_list(
    state: State<'_, AppState>,
    companion_id: String,
) -> ApiResponse<Value> {
    response(state.database.companion_grants_list(&companion_id))
}

#[tauri::command]
pub(crate) fn companion_grant_project(
    state: State<'_, AppState>,
    companion_id: String,
    project_id: String,
) -> ApiResponse<()> {
    response(
        state
            .database
            .companion_grant_project(&companion_id, &project_id, true),
    )
}

#[tauri::command]
pub(crate) fn companion_revoke_project(
    state: State<'_, AppState>,
    companion_id: String,
    project_id: String,
) -> ApiResponse<()> {
    response(
        state
            .database
            .companion_grant_project(&companion_id, &project_id, false),
    )
}

#[tauri::command]
pub(crate) fn companion_project_note_reference_set(
    state: State<'_, AppState>,
    conversation_id: String,
    companion_id: Option<String>,
) -> ApiResponse<()> {
    response(set_project_note_reference(
        &state.database,
        &conversation_id,
        companion_id.as_deref(),
    ))
}

fn set_project_note_reference(
    database: &crate::database::Database,
    conversation_id: &str,
    companion_id: Option<&str>,
) -> Result<(), String> {
    database.companion_project_note_reference_set(conversation_id, companion_id)
}

#[cfg(test)]
mod project_note_reference_tests {
    use super::set_project_note_reference;
    use crate::database::Database;
    use uuid::Uuid;

    #[test]
    fn command_sets_and_clears_only_explicit_project_companion_references() {
        let root = std::env::temp_dir().join(format!("fox-note-reference-command-{}", Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let database = Database::open(root.join("fox.db")).unwrap();
        let contact = database.add_companion(&root, "fox-security").unwrap();
        database.with_connection(|connection| {
            connection.execute_batch(
                "INSERT INTO projects(id,name,root_path,created_at,updated_at)
                 VALUES ('project','project','project-root',1,1);
                 INSERT INTO conversations(id,agent_id,title,project_id,status,created_at,updated_at)
                 VALUES ('project-chat','fox-general','project','project','active',1,1);
                 INSERT INTO conversations(id,agent_id,title,status,created_at,updated_at)
                 VALUES ('ordinary-chat','fox-general','ordinary','active',1,1);",
            )?;
            Ok(())
        }).unwrap();
        database.companion_grant_project(&contact.profile.id, "project", true).unwrap();
        set_project_note_reference(&database, "project-chat", Some(&contact.profile.id)).unwrap();
        assert_eq!(database.companion_note_access("project-chat").unwrap(), Some((contact.profile.id.clone(), false)));
        assert!(set_project_note_reference(&database, "ordinary-chat", Some(&contact.profile.id)).is_err());
        set_project_note_reference(&database, "project-chat", None).unwrap();
        assert!(database.companion_note_access("project-chat").unwrap().is_none());
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }
}
