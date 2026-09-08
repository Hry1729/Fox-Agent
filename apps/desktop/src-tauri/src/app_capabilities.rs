use crate::{
    app_state::AppState,
    database::{
        ApiResponse, AppNotificationRecord, AppNotificationUpsert, ConversationDetail,
        GlobalSearchRecord, MessageFeedbackRecord, NotificationPreferencesRecord,
        ProjectManagementRecord,
    },
    local_knowledge::LocalKnowledgeDocumentsRequest,
};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationListRequest {
    #[serde(default)]
    pub unread_only: bool,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationReadRequest {
    pub id: String,
    pub read: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppNotificationPublishRequest {
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub merge_key: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub workspace_view: Option<String>,
    pub entity_id: Option<String>,
    pub status: Option<String>,
    pub action: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackRequest {
    pub message_id: String,
    pub sentiment: String,
    pub category: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManagementIdRequest {
    pub project_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPathUpdateRequest {
    pub project_id: String,
    pub root_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectArchiveRequest {
    pub project_id: String,
    pub archived: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueRunDetailRequest {
    pub run_id: String,
}

fn failure<T: serde::Serialize>(
    code: &str,
    error: impl ToString,
    retryable: bool,
) -> ApiResponse<T> {
    ApiResponse::failure(code, error.to_string(), retryable)
}

fn validate_notification_value(
    field: &str,
    value: &str,
    max_chars: usize,
    required: bool,
) -> Result<String, String> {
    let value = value.trim();
    if required && value.is_empty() {
        return Err(format!("{field} 不能为空"));
    }
    if value.chars().count() > max_chars {
        return Err(format!("{field} 不能超过 {max_chars} 个字符"));
    }
    Ok(value.to_owned())
}

fn sync_local_knowledge_notifications(state: &AppState) -> Result<(), String> {
    let preferences = state.database.notification_preferences()?;
    let jobs = state
        .local_knowledge
        .list_jobs(None)
        .map_err(|error| error.to_string())?;
    for job in jobs.into_iter().take(100) {
        let (kind, severity) = match job.status.as_str() {
            "queued" | "running" | "paused" => (
                "progress",
                if preferences.quiet_progress {
                    "quiet"
                } else {
                    "normal"
                },
            ),
            "failed" | "interrupted" => ("failed", "high"),
            "completed" | "cancelled" => ("completed", "normal"),
            _ => continue,
        };
        let title = match job.job_type.as_str() {
            "import" => "知识库文档导入",
            "parse" => "知识库文档解析",
            "index" => "知识库向量索引",
            "rebuild" => "知识库索引重建",
            "delete" => "知识库文档删除",
            _ => "知识库后台任务",
        };
        let body = job.error_message.clone().unwrap_or_else(|| {
            format!(
                "{} · {}%",
                job.stage.as_deref().unwrap_or(&job.status),
                job.progress
            )
        });
        state
            .database
            .upsert_app_notification(AppNotificationUpsert {
                merge_key: format!("knowledge-job:{}", job.id),
                kind: kind.to_owned(),
                severity: severity.to_owned(),
                title: title.to_owned(),
                body,
                source_type: "knowledge_job".to_owned(),
                source_id: job.id,
                workspace_view: Some("local-knowledge-jobs".to_owned()),
                entity_id: Some(job.knowledge_base_id),
                progress: Some(job.progress),
                status: job.status,
                action: json!({"open": "knowledge_job"}),
            })?;
    }
    Ok(())
}

#[tauri::command]
pub fn app_notifications_list(
    state: State<'_, AppState>,
    request: NotificationListRequest,
) -> ApiResponse<Vec<AppNotificationRecord>> {
    if let Err(error) = state
        .database
        .sync_app_notifications()
        .and_then(|_| sync_local_knowledge_notifications(&state))
    {
        return failure("notifications.sync_failed", error, true);
    }
    match state
        .database
        .list_app_notifications(request.unread_only, request.limit.unwrap_or(100))
    {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.list_failed", error, true),
    }
}

#[tauri::command]
pub fn app_notification_read(
    state: State<'_, AppState>,
    request: NotificationReadRequest,
) -> ApiResponse<bool> {
    match state
        .database
        .set_app_notification_read(&request.id, request.read)
    {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.update_failed", error, true),
    }
}

#[tauri::command]
pub fn app_notification_publish(
    state: State<'_, AppState>,
    request: AppNotificationPublishRequest,
) -> ApiResponse<bool> {
    if !matches!(
        request.kind.as_str(),
        "approval" | "question" | "progress" | "completed" | "failed"
    ) {
        return failure(
            "notifications.kind_invalid",
            "通知类型必须是 approval、question、progress、completed 或 failed",
            false,
        );
    }
    if !matches!(request.severity.as_str(), "quiet" | "normal" | "high") {
        return failure(
            "notifications.severity_invalid",
            "通知级别必须是 quiet、normal 或 high",
            false,
        );
    }
    let title = match validate_notification_value("通知标题", &request.title, 160, true) {
        Ok(value) => value,
        Err(error) => return failure("notifications.title_invalid", error, false),
    };
    let body = match validate_notification_value("通知内容", &request.body, 16_000, true) {
        Ok(value) => value,
        Err(error) => return failure("notifications.body_invalid", error, false),
    };
    let notification_id = Uuid::new_v4().to_string();
    let source_type = request
        .source_type
        .as_deref()
        .map(|value| validate_notification_value("通知来源", value, 80, false))
        .transpose();
    let source_type = match source_type {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => "ui".to_owned(),
        Err(error) => return failure("notifications.source_invalid", error, false),
    };
    let source_id = request
        .source_id
        .as_deref()
        .map(|value| validate_notification_value("来源 ID", value, 256, false))
        .transpose();
    let source_id = match source_id {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => notification_id.clone(),
        Err(error) => return failure("notifications.source_invalid", error, false),
    };
    let merge_key = request
        .merge_key
        .as_deref()
        .map(|value| validate_notification_value("合并键", value, 256, false))
        .transpose();
    let merge_key = match merge_key {
        Ok(Some(value)) if !value.is_empty() => value,
        Ok(_) => format!("ui:{source_type}:{notification_id}"),
        Err(error) => return failure("notifications.merge_key_invalid", error, false),
    };
    let workspace_view = match request.workspace_view.as_deref() {
        Some(value) => match validate_notification_value("页面标识", value, 80, false) {
            Ok(value) if !value.is_empty() => Some(value),
            Ok(_) => None,
            Err(error) => return failure("notifications.route_invalid", error, false),
        },
        None => None,
    };
    let entity_id = match request.entity_id.as_deref() {
        Some(value) => match validate_notification_value("页面对象 ID", value, 256, false) {
            Ok(value) if !value.is_empty() => Some(value),
            Ok(_) => None,
            Err(error) => return failure("notifications.route_invalid", error, false),
        },
        None => None,
    };
    let status = match request.status.as_deref() {
        Some(value) => match validate_notification_value("通知状态", value, 80, false) {
            Ok(value) if !value.is_empty() => value,
            Ok(_) => request.kind.clone(),
            Err(error) => return failure("notifications.status_invalid", error, false),
        },
        None => request.kind.clone(),
    };
    let action = request
        .action
        .unwrap_or_else(|| json!({"open": "workspace"}));
    if action.to_string().chars().count() > 4_096 {
        return failure(
            "notifications.action_invalid",
            "通知操作数据不能超过 4096 个字符",
            false,
        );
    }
    match state
        .database
        .upsert_app_notification(AppNotificationUpsert {
            merge_key,
            kind: request.kind,
            severity: request.severity,
            title,
            body,
            source_type,
            source_id,
            workspace_view,
            entity_id,
            progress: None,
            status,
            action,
        }) {
        Ok(()) => ApiResponse::success(true),
        Err(error) => failure("notifications.publish_failed", error, true),
    }
}

#[tauri::command]
pub fn app_notifications_mark_all_read(state: State<'_, AppState>) -> ApiResponse<usize> {
    match state.database.mark_all_app_notifications_read() {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.update_failed", error, true),
    }
}

#[tauri::command]
pub fn app_notifications_clear_read(state: State<'_, AppState>) -> ApiResponse<usize> {
    match state.database.clear_read_app_notifications() {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.clear_failed", error, true),
    }
}

#[tauri::command]
pub fn notification_preferences_get(
    state: State<'_, AppState>,
) -> ApiResponse<NotificationPreferencesRecord> {
    match state.database.notification_preferences() {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.preferences_failed", error, true),
    }
}

#[tauri::command]
pub fn notification_preferences_save(
    state: State<'_, AppState>,
    request: NotificationPreferencesRecord,
) -> ApiResponse<NotificationPreferencesRecord> {
    match state.database.save_notification_preferences(&request) {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("notifications.preferences_failed", error, true),
    }
}

#[tauri::command]
pub fn message_feedback_save(
    state: State<'_, AppState>,
    request: MessageFeedbackRequest,
) -> ApiResponse<MessageFeedbackRecord> {
    match state.database.save_message_feedback(
        &request.message_id,
        &request.sentiment,
        request.category.as_deref(),
        request.comment.as_deref(),
    ) {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("feedback.invalid", error, false),
    }
}

#[tauri::command]
pub fn app_global_search(
    state: State<'_, AppState>,
    request: GlobalSearchRequest,
) -> ApiResponse<Vec<GlobalSearchRecord>> {
    let limit = request.limit.unwrap_or(40).clamp(1, 100);
    let mut records = match state.database.global_search(&request.query, limit) {
        Ok(value) => value,
        Err(error) => return failure("search.failed", error, true),
    };
    let query = request.query.trim().to_lowercase();
    if !query.is_empty() {
        if let Ok(bases) = state.local_knowledge.list_bases() {
            for base in bases {
                if base.name.to_lowercase().contains(&query)
                    || base
                        .description
                        .as_deref()
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&query)
                {
                    records.push(GlobalSearchRecord {
                        id: base.id.clone(),
                        kind: "knowledge_base".to_owned(),
                        title: base.name.clone(),
                        subtitle: base.description.clone().unwrap_or_default(),
                        workspace_view: "local-knowledge-detail".to_owned(),
                        entity_id: Some(base.id.clone()),
                        document_id: None,
                        updated_at: base.updated_at,
                    });
                }
                if let Ok(documents) =
                    state
                        .local_knowledge
                        .list_documents(LocalKnowledgeDocumentsRequest {
                            knowledge_base_id: base.id.clone(),
                            query: Some(request.query.clone()),
                        })
                {
                    records.extend(documents.into_iter().take(10).map(|document| {
                        GlobalSearchRecord {
                            id: document.id.clone(),
                            kind: "knowledge_document".to_owned(),
                            title: document.display_name,
                            subtitle: format!("{} · {}", base.name, document.relative_path),
                            workspace_view: "local-knowledge-documents".to_owned(),
                            entity_id: Some(base.id.clone()),
                            document_id: Some(document.id),
                            updated_at: document.updated_at,
                        }
                    }));
                }
            }
        }
    }
    records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    records.truncate(limit);
    ApiResponse::success(records)
}

#[tauri::command]
pub async fn projects_management_list(
    state: State<'_, AppState>,
) -> Result<ApiResponse<Vec<ProjectManagementRecord>>, String> {
    let database = state.database.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || database.list_project_management()).await
        {
            Ok(Ok(value)) => ApiResponse::success(value),
            Ok(Err(error)) => failure("projects.inspect_failed", error, true),
            Err(error) => failure("projects.inspect_failed", error, true),
        },
    )
}

#[tauri::command]
pub async fn project_path_update(
    state: State<'_, AppState>,
    request: ProjectPathUpdateRequest,
) -> Result<ApiResponse<ProjectManagementRecord>, String> {
    let database = state.database.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || {
            database.update_project_path(&request.project_id, &request.root_path)
        })
        .await
        {
            Ok(Ok(value)) => ApiResponse::success(value),
            Ok(Err(error)) => failure("project.path_invalid", error, false),
            Err(error) => failure("project.path_update_failed", error, true),
        },
    )
}

#[tauri::command]
pub fn project_archive(
    state: State<'_, AppState>,
    request: ProjectArchiveRequest,
) -> ApiResponse<bool> {
    match state
        .database
        .set_project_archived(&request.project_id, request.archived)
    {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("project.archive_failed", error, true),
    }
}

#[tauri::command]
pub fn project_root_open(
    state: State<'_, AppState>,
    request: ProjectManagementIdRequest,
) -> ApiResponse<bool> {
    let project = match state.database.list_project_management() {
        Ok(items) => items.into_iter().find(|item| item.id == request.project_id),
        Err(error) => return failure("project.open_failed", error, true),
    };
    let Some(project) = project else {
        return failure("project.not_found", "未找到项目", false);
    };
    if !project.path_exists {
        return failure("project.path_missing", "项目目录已移动或删除", false);
    }
    match open_folder(Path::new(&project.root_path)) {
        Ok(()) => ApiResponse::success(true),
        Err(error) => failure("project.open_failed", error, true),
    }
}

#[tauri::command]
pub fn digital_colleague_run_detail(
    state: State<'_, AppState>,
    request: DigitalColleagueRunDetailRequest,
) -> ApiResponse<ConversationDetail> {
    let conversation_id = match state
        .database
        .digital_colleague_run_conversation_id(&request.run_id)
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            return failure(
                "digital_colleague.run_not_found",
                "未找到对应的数字同事运行",
                false,
            )
        }
        Err(error) => return failure("digital_colleague.run_detail_failed", error, true),
    };
    match state.database.load_conversation(&conversation_id) {
        Ok(value) => ApiResponse::success(value),
        Err(error) => failure("digital_colleague.run_detail_failed", error, true),
    }
}

#[cfg(target_os = "windows")]
fn open_folder(path: &Path) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
#[cfg(target_os = "macos")]
fn open_folder(path: &Path) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
#[cfg(all(unix, not(target_os = "macos")))]
fn open_folder(path: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(path)
        .spawn()
        .map(|_| ())
        .map_err(|error| error.to_string())
}
