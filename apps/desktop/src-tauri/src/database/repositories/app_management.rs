use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppNotificationRecord {
    pub id: String,
    pub merge_key: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub source_type: String,
    pub source_id: String,
    pub workspace_view: Option<String>,
    pub entity_id: Option<String>,
    pub progress: Option<i64>,
    pub status: String,
    pub action: Value,
    pub read_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct AppNotificationUpsert {
    pub merge_key: String,
    pub kind: String,
    pub severity: String,
    pub title: String,
    pub body: String,
    pub source_type: String,
    pub source_id: String,
    pub workspace_view: Option<String>,
    pub entity_id: Option<String>,
    pub progress: Option<i64>,
    pub status: String,
    pub action: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotificationPreferencesRecord {
    pub system_popup: bool,
    pub sound: bool,
    pub sound_id: String,
    pub badge: bool,
    pub quiet_progress: bool,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageFeedbackRecord {
    pub id: String,
    pub message_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub sentiment: String,
    pub category: Option<String>,
    pub comment: Option<String>,
    pub model: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalSearchRecord {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub subtitle: String,
    pub workspace_view: String,
    pub entity_id: Option<String>,
    pub document_id: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectManagementRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub permission_mode: String,
    pub status: String,
    pub path_exists: bool,
    pub is_git_repository: bool,
    pub git_branch: Option<String>,
    pub disk_bytes: Option<u64>,
    pub conversation_count: i64,
    pub artifact_count: i64,
    pub recent_conversation_title: Option<String>,
    pub recent_artifact_name: Option<String>,
    pub last_opened_at: Option<i64>,
    pub archived_at: Option<i64>,
    pub updated_at: i64,
}

impl Database {
    pub fn sync_app_notifications(&self) -> Result<(), String> {
        let approval_items = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.status, a.requested_action, a.requested_at,
                        t.conversation_id, c.title
                 FROM approvals a
                 JOIN tool_calls t ON t.id = a.tool_call_id
                 JOIN conversations c ON c.id = t.conversation_id
                 ORDER BY a.requested_at DESC LIMIT 100",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })?;
        for (id, status, action, _requested_at, conversation_id, conversation_title) in
            approval_items
        {
            let pending = status == "pending";
            self.upsert_app_notification(AppNotificationUpsert {
                merge_key: format!("approval:{id}"),
                kind: if pending { "approval" } else { "completed" }.to_owned(),
                severity: if pending { "high" } else { "normal" }.to_owned(),
                title: if pending {
                    "操作等待审批"
                } else {
                    "审批已处理"
                }
                .to_owned(),
                body: format!("{conversation_title} · {action}"),
                source_type: "approval".to_owned(),
                source_id: id,
                workspace_view: Some("chat".to_owned()),
                entity_id: Some(conversation_id),
                progress: None,
                status,
                action: json!({"open": "conversation"}),
            })?;
        }

        let run_items = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT r.id, r.status, r.error_message, r.conversation_id, c.title,
                        COALESCE(r.finished_at, r.started_at, r.created_at)
                 FROM runs r JOIN conversations c ON c.id = r.conversation_id
                 WHERE r.run_kind = 'primary'
                 ORDER BY COALESCE(r.finished_at, r.started_at, r.created_at) DESC LIMIT 100",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })?;
        for (id, status, error, conversation_id, title, _updated_at) in run_items {
            let (kind, severity, progress, body) = match status.as_str() {
                "queued" => ("progress", "quiet", Some(0), "任务正在排队".to_owned()),
                "running" => ("progress", "quiet", None, "Agent 正在执行".to_owned()),
                "completed" => ("completed", "normal", Some(100), "任务已完成".to_owned()),
                "failed" | "interrupted" => (
                    "failed",
                    "high",
                    None,
                    error.unwrap_or_else(|| "任务执行失败".to_owned()),
                ),
                "cancelled" => ("completed", "normal", None, "任务已取消".to_owned()),
                _ => continue,
            };
            self.upsert_app_notification(AppNotificationUpsert {
                merge_key: format!("run:{id}"),
                kind: kind.to_owned(),
                severity: severity.to_owned(),
                title,
                body,
                source_type: "run".to_owned(),
                source_id: id,
                workspace_view: Some("chat".to_owned()),
                entity_id: Some(conversation_id),
                progress,
                status,
                action: json!({"open": "conversation"}),
            })?;
        }

        let colleague_items = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT t.id, t.status, t.error_message, t.colleague_id, d.name
                 FROM digital_colleague_triggers t
                 JOIN digital_colleagues d ON d.id = t.colleague_id
                 ORDER BY t.updated_at DESC LIMIT 100",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })?;
        for (id, status, error, colleague_id, name) in colleague_items {
            let (kind, severity, body) = match status.as_str() {
                "accepted" | "queued" | "running" => {
                    ("progress", "quiet", "数字同事正在执行".to_owned())
                }
                "completed" => ("completed", "normal", "数字同事已完成本次运行".to_owned()),
                "failed" | "rejected" => (
                    "failed",
                    "high",
                    error.unwrap_or_else(|| "数字同事运行失败".to_owned()),
                ),
                "cancelled" | "skipped" => ("completed", "normal", format!("运行已{status}")),
                _ => continue,
            };
            self.upsert_app_notification(AppNotificationUpsert {
                merge_key: format!("digital-colleague:{id}"),
                kind: kind.to_owned(),
                severity: severity.to_owned(),
                title: name,
                body,
                source_type: "digital_colleague".to_owned(),
                source_id: id,
                workspace_view: Some("agent-detail".to_owned()),
                entity_id: Some(colleague_id),
                progress: None,
                status,
                action: json!({"open": "digital_colleague"}),
            })?;
        }
        Ok(())
    }

    pub fn upsert_app_notification(&self, item: AppNotificationUpsert) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_notifications(
                    id, merge_key, kind, severity, title, body, source_type, source_id,
                    workspace_view, entity_id, progress, status, action_json, created_at, updated_at,
                    read_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14,
                    CASE WHEN ?7 = 'approval' AND ?3 = 'completed' THEN ?14 ELSE NULL END
                 )
                 ON CONFLICT(merge_key) DO UPDATE SET
                    kind = excluded.kind, severity = excluded.severity, title = excluded.title,
                    body = excluded.body, workspace_view = excluded.workspace_view,
                    entity_id = excluded.entity_id, progress = excluded.progress,
                    status = excluded.status, action_json = excluded.action_json,
                    read_at = CASE
                        WHEN excluded.source_type = 'approval' AND excluded.kind = 'completed'
                            THEN excluded.updated_at
                        WHEN app_notifications.status <> excluded.status
                          OR app_notifications.kind <> excluded.kind THEN NULL
                        ELSE app_notifications.read_at
                    END,
                    dismissed_at = CASE
                        WHEN app_notifications.status <> excluded.status
                          OR app_notifications.kind <> excluded.kind THEN NULL
                        ELSE app_notifications.dismissed_at
                    END,
                    updated_at = excluded.updated_at
                 WHERE app_notifications.kind IS NOT excluded.kind
                    OR app_notifications.severity IS NOT excluded.severity
                    OR app_notifications.title IS NOT excluded.title
                    OR app_notifications.body IS NOT excluded.body
                    OR app_notifications.workspace_view IS NOT excluded.workspace_view
                    OR app_notifications.entity_id IS NOT excluded.entity_id
                    OR app_notifications.progress IS NOT excluded.progress
                    OR app_notifications.status IS NOT excluded.status
                    OR app_notifications.action_json IS NOT excluded.action_json",
                params![
                    Uuid::new_v4().to_string(),
                    item.merge_key,
                    item.kind,
                    item.severity,
                    item.title,
                    item.body,
                    item.source_type,
                    item.source_id,
                    item.workspace_view,
                    item.entity_id,
                    item.progress,
                    item.status,
                    item.action.to_string(),
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn list_app_notifications(
        &self,
        unread_only: bool,
        limit: usize,
    ) -> Result<Vec<AppNotificationRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, merge_key, kind, severity, title, body, source_type, source_id,
                        workspace_view, entity_id, progress, status, action_json, read_at,
                        created_at, updated_at
                 FROM app_notifications
                 WHERE dismissed_at IS NULL AND (?1 = 0 OR read_at IS NULL)
                 ORDER BY CASE severity WHEN 'high' THEN 0 WHEN 'normal' THEN 1 ELSE 2 END,
                          updated_at DESC LIMIT ?2",
            )?;
            let records = statement
                .query_map(params![unread_only, limit.clamp(1, 500) as i64], |row| {
                    Ok(AppNotificationRecord {
                        id: row.get(0)?,
                        merge_key: row.get(1)?,
                        kind: row.get(2)?,
                        severity: row.get(3)?,
                        title: row.get(4)?,
                        body: row.get(5)?,
                        source_type: row.get(6)?,
                        source_id: row.get(7)?,
                        workspace_view: row.get(8)?,
                        entity_id: row.get(9)?,
                        progress: row.get(10)?,
                        status: row.get(11)?,
                        action: serde_json::from_str(&row.get::<_, String>(12)?)
                            .unwrap_or_else(|_| json!({})),
                        read_at: row.get(13)?,
                        created_at: row.get(14)?,
                        updated_at: row.get(15)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })
    }

    pub fn set_app_notification_read(&self, id: &str, read: bool) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE app_notifications SET read_at = ?2 WHERE id = ?1",
                params![id, if read { Some(now_ms()) } else { None::<i64> }],
            )? > 0)
        })
    }

    pub fn mark_all_app_notifications_read(&self) -> Result<usize, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE app_notifications SET read_at = ?1
                 WHERE dismissed_at IS NULL AND read_at IS NULL",
                [now_ms()],
            )?)
        })
    }

    pub fn clear_read_app_notifications(&self) -> Result<usize, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE app_notifications SET dismissed_at = ?1
                 WHERE dismissed_at IS NULL AND read_at IS NOT NULL",
                [now_ms()],
            )?)
        })
    }

    pub fn notification_preferences(&self) -> Result<NotificationPreferencesRecord, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT system_popup, sound, sound_id, badge, quiet_progress, updated_at
             FROM notification_preferences WHERE singleton_id = 1",
                    [],
                    |row| {
                        Ok(NotificationPreferencesRecord {
                            system_popup: row.get(0)?,
                            sound: row.get(1)?,
                            sound_id: row.get(2)?,
                            badge: row.get(3)?,
                            quiet_progress: row.get(4)?,
                            updated_at: row.get(5)?,
                        })
                    },
                )
                .map_err(Into::into)
        })
    }

    pub fn save_notification_preferences(
        &self,
        value: &NotificationPreferencesRecord,
    ) -> Result<NotificationPreferencesRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE notification_preferences SET system_popup = ?1, sound = ?2,
                    sound_id = ?3, badge = ?4, quiet_progress = ?5, updated_at = ?6
                 WHERE singleton_id = 1",
                params![
                    value.system_popup,
                    value.sound,
                    value.sound_id,
                    value.badge,
                    value.quiet_progress,
                    now
                ],
            )?;
            Ok(NotificationPreferencesRecord {
                updated_at: now,
                ..value.clone()
            })
        })
    }

    pub fn save_message_feedback(
        &self,
        message_id: &str,
        sentiment: &str,
        category: Option<&str>,
        comment: Option<&str>,
    ) -> Result<MessageFeedbackRecord, String> {
        if !matches!(sentiment, "positive" | "negative") {
            return Err("invalid feedback sentiment".to_owned());
        }
        if category.is_some_and(|value| {
            !matches!(
                value,
                "irrelevant" | "code_error" | "misunderstanding" | "other"
            )
        }) {
            return Err("invalid feedback category".to_owned());
        }
        if sentiment == "negative" && category.is_none() {
            return Err("negative feedback requires a category".to_owned());
        }
        let comment = comment
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.chars().take(1000).collect::<String>());
        let now = now_ms();
        self.with_connection(|connection| {
            let source = connection.query_row(
                "SELECT m.conversation_id, m.run_id, r.model, c.project_id, p.permission_mode
                 FROM messages m
                 JOIN conversations c ON c.id = m.conversation_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN runs r ON r.id = m.run_id
                 WHERE m.id = ?1 AND m.role = 'assistant'",
                [message_id], |row| Ok((
                    row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?, row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                )),
            ).optional()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            let tools = if let Some(run_id) = source.1.as_deref() {
                let mut statement = connection.prepare(
                    "SELECT tool_name, status FROM tool_calls WHERE run_id = ?1
                     ORDER BY started_at ASC LIMIT 50",
                )?;
                let rows = statement
                    .query_map([run_id], |row| Ok(json!({
                        "name": row.get::<_, String>(0)?,
                        "status": row.get::<_, String>(1)?,
                    })))?
                    .collect::<Result<Vec<_>, _>>()?;
                rows
            } else {
                Vec::new()
            };
            let context = json!({
                "projectId": source.3,
                "projectPermissionMode": source.4,
                "tools": tools,
            })
            .to_string();
            let id = connection.query_row(
                "SELECT id FROM message_feedback WHERE message_id = ?1", [message_id],
                |row| row.get::<_, String>(0),
            ).optional()?.unwrap_or_else(|| Uuid::new_v4().to_string());
            connection.execute(
                "INSERT INTO message_feedback(id, message_id, conversation_id, run_id, sentiment,
                    category, comment, model, context_json, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
                 ON CONFLICT(message_id) DO UPDATE SET sentiment = excluded.sentiment,
                    category = excluded.category, comment = excluded.comment, model = excluded.model,
                    context_json = excluded.context_json, updated_at = excluded.updated_at",
                params![id, message_id, source.0, source.1, sentiment, category, comment, source.2, context, now],
            )?;
            Ok(MessageFeedbackRecord { id, message_id: message_id.to_owned(), conversation_id: source.0,
                run_id: source.1, sentiment: sentiment.to_owned(), category: category.map(str::to_owned),
                comment, model: source.2, created_at: now, updated_at: now })
        })
    }

    pub fn global_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<GlobalSearchRecord>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", query.replace('%', "\\%").replace('_', "\\_"));
        self.with_connection(|connection| {
            let mut results = Vec::new();
            {
                let mut statement = connection.prepare(
                    "SELECT c.id, c.title, COALESCE(p.name, '普通对话'), c.updated_at
                     FROM conversations c LEFT JOIN projects p ON p.id = c.project_id
                     WHERE c.archived = 0 AND c.trashed_at IS NULL AND c.conversation_kind = 'primary'
                       AND (c.title LIKE ?1 ESCAPE '\\' OR EXISTS(
                            SELECT 1 FROM messages m WHERE m.conversation_id = c.id
                              AND m.content LIKE ?1 ESCAPE '\\'))
                     ORDER BY c.updated_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "conversation".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                    workspace_view: "chat".to_owned(), entity_id: Some(row.get(0)?), document_id: None, updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            {
                let mut statement = connection.prepare(
                    "SELECT id, name, root_path, updated_at FROM projects
                     WHERE archived_at IS NULL AND status <> 'revoked'
                       AND (name LIKE ?1 ESCAPE '\\' OR root_path LIKE ?1 ESCAPE '\\')
                     ORDER BY updated_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "project".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                    workspace_view: "settings-projects".to_owned(), entity_id: Some(row.get(0)?), document_id: None, updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            {
                let mut statement = connection.prepare(
                    "SELECT id, name, description, updated_at FROM agents
                     WHERE visibility <> 'hidden' AND (name LIKE ?1 ESCAPE '\\' OR description LIKE ?1 ESCAPE '\\')
                     ORDER BY updated_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "agent".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                    workspace_view: "agent-detail".to_owned(), entity_id: Some(row.get(0)?), document_id: None, updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            {
                let mut statement = connection.prepare(
                    "SELECT id, name, objective, updated_at FROM digital_colleagues
                     WHERE status <> 'revoked' AND (name LIKE ?1 ESCAPE '\\' OR objective LIKE ?1 ESCAPE '\\')
                     ORDER BY updated_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "digital_colleague".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                    workspace_view: "agents".to_owned(), entity_id: Some(row.get(0)?), document_id: None, updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            {
                let mut statement = connection.prepare(
                    "SELECT a.id, a.display_name, COALESCE(c.title, '文件产物'), a.created_at, c.id
                     FROM artifacts a JOIN conversations c ON c.id = a.conversation_id
                     WHERE c.archived = 0 AND c.trashed_at IS NULL
                       AND a.display_name LIKE ?1 ESCAPE '\\'
                     ORDER BY a.created_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "artifact".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                     workspace_view: "chat".to_owned(), entity_id: Some(row.get(4)?), document_id: Some(row.get(0)?), updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            {
                let mut statement = connection.prepare(
                    "SELECT t.id, t.title, g.objective, t.updated_at, g.conversation_id
                     FROM work_tasks t
                     JOIN goals g ON g.id = t.goal_id
                     JOIN conversations c ON c.id = g.conversation_id
                     WHERE c.archived = 0 AND c.trashed_at IS NULL
                       AND (t.title LIKE ?1 ESCAPE '\\' OR t.detail LIKE ?1 ESCAPE '\\')
                     ORDER BY t.updated_at DESC LIMIT ?2")?;
                results.extend(statement.query_map(params![pattern, limit as i64], |row| Ok(GlobalSearchRecord {
                    id: row.get(0)?, kind: "task".to_owned(), title: row.get(1)?, subtitle: row.get(2)?,
                    workspace_view: "chat".to_owned(), entity_id: Some(row.get(4)?), document_id: None, updated_at: row.get(3)?,
                }))?.collect::<Result<Vec<_>, _>>()?);
            }
            results.sort_by(|left, right| right.updated_at.cmp(&left.updated_at).then_with(|| left.kind.cmp(&right.kind)));
            results.truncate(limit.clamp(1, 100));
            Ok(results)
        })
    }

    pub fn list_project_management(&self) -> Result<Vec<ProjectManagementRecord>, String> {
        let rows = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT p.id, p.name, p.root_path, p.permission_mode, p.status,
                        p.last_opened_at, p.archived_at, p.updated_at,
                        (SELECT COUNT(*) FROM conversations c WHERE c.project_id = p.id),
                        (SELECT COUNT(*) FROM artifacts a JOIN conversations c ON c.id = a.conversation_id WHERE c.project_id = p.id),
                        (SELECT c.title FROM conversations c WHERE c.project_id = p.id
                         ORDER BY COALESCE(c.last_message_at, c.updated_at) DESC LIMIT 1),
                        (SELECT a.display_name FROM artifacts a JOIN conversations c ON c.id = a.conversation_id
                         WHERE c.project_id = p.id ORDER BY a.created_at DESC LIMIT 1)
                 FROM projects p ORDER BY COALESCE(p.last_opened_at, p.updated_at) DESC")?;
            let records = statement.query_map([], |row| Ok((
                row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                row.get::<_, String>(3)?, row.get::<_, String>(4)?, row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?, row.get::<_, i64>(7)?, row.get::<_, i64>(8)?, row.get::<_, i64>(9)?,
                row.get::<_, Option<String>>(10)?, row.get::<_, Option<String>>(11)?,
            )))?.collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })?;
        let mut records = Vec::with_capacity(rows.len());
        for (
            id,
            name,
            root_path,
            permission_mode,
            persisted_status,
            last_opened_at,
            archived_at,
            updated_at,
            conversation_count,
            artifact_count,
            recent_conversation_title,
            recent_artifact_name,
        ) in rows
        {
            let path = PathBuf::from(&root_path);
            let path_exists = path.is_dir();
            let status = if archived_at.is_some() {
                "revoked"
            } else if path_exists {
                "active"
            } else {
                "missing"
            }
            .to_owned();
            if status != persisted_status {
                let _ = self.with_connection(|connection| {
                    connection
                        .execute(
                            "UPDATE projects SET status = ?2, updated_at = ?3 WHERE id = ?1",
                            params![&id, &status, now_ms()],
                        )
                        .map(|_| ())
                });
            }
            let is_git_repository = path_exists && path.join(".git").exists();
            let git_branch = if is_git_repository {
                read_git_branch(&path)
            } else {
                None
            };
            records.push(ProjectManagementRecord {
                id,
                name,
                root_path,
                permission_mode,
                status,
                path_exists,
                is_git_repository,
                git_branch,
                disk_bytes: if path_exists {
                    directory_size_bounded(&path, 200_000).ok()
                } else {
                    None
                },
                conversation_count,
                artifact_count,
                recent_conversation_title,
                recent_artifact_name,
                last_opened_at,
                archived_at,
                updated_at,
            });
        }
        Ok(records)
    }

    pub fn update_project_path(
        &self,
        project_id: &str,
        root_path: &str,
    ) -> Result<ProjectManagementRecord, String> {
        let path = PathBuf::from(root_path.trim())
            .canonicalize()
            .map_err(|error| error.to_string())?;
        if !path.is_dir() {
            return Err("project path is not a directory".to_owned());
        }
        let root = path.to_string_lossy().into_owned();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or(&root)
            .to_owned();
        self.with_connection(|connection| {
            let changed = connection.execute(
                "UPDATE projects SET root_path = ?2, name = ?3, status = 'active', archived_at = NULL,
                    updated_at = ?4 WHERE id = ?1", params![project_id, root, name, now_ms()],
            )?;
            if changed == 0 { return Err(rusqlite::Error::QueryReturnedNoRows); }
            Ok(())
        })?;
        self.list_project_management()?
            .into_iter()
            .find(|item| item.id == project_id)
            .ok_or_else(|| "project not found after path update".to_owned())
    }

    pub fn set_project_archived(&self, project_id: &str, archived: bool) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE projects SET archived_at = ?2, status = ?3, updated_at = ?4 WHERE id = ?1",
                params![
                    project_id,
                    if archived {
                        Some(now_ms())
                    } else {
                        None::<i64>
                    },
                    if archived { "revoked" } else { "active" },
                    now_ms()
                ],
            )? > 0)
        })
    }

    pub fn digital_colleague_run_conversation_id(
        &self,
        run_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT runs.conversation_id
                     FROM runs
                     JOIN digital_colleague_triggers AS trigger ON trigger.run_id = runs.id
                     WHERE runs.id = ?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }
}

fn read_git_branch(root: &Path) -> Option<String> {
    let head = fs::read_to_string(root.join(".git").join("HEAD")).ok()?;
    let head = head.trim();
    Some(
        head.strip_prefix("ref: refs/heads/")
            .unwrap_or(head)
            .to_owned(),
    )
}

fn directory_size_bounded(root: &Path, max_entries: usize) -> Result<u64, std::io::Error> {
    let mut total = 0u64;
    let mut visited = 0usize;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(path)? {
            visited += 1;
            if visited > max_entries {
                return Ok(total);
            }
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::super::DEFAULT_AGENT_ID;
    use super::*;

    fn test_database() -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-app-management-{}.db", Uuid::new_v4()));
        (
            Database::open(path.clone()).expect("open app management test database"),
            path,
        )
    }

    #[test]
    fn notification_merge_reopens_changed_status_and_preferences_persist() {
        let (database, path) = test_database();
        let input = |kind: &str, status: &str, progress| AppNotificationUpsert {
            merge_key: "run:one".to_owned(),
            kind: kind.to_owned(),
            severity: "normal".to_owned(),
            title: format!("Run {status}"),
            body: "runtime update".to_owned(),
            source_type: "run".to_owned(),
            source_id: "one".to_owned(),
            workspace_view: Some("chat".to_owned()),
            entity_id: Some("conversation-one".to_owned()),
            progress,
            status: status.to_owned(),
            action: json!({"open": "conversation"}),
        };

        database
            .upsert_app_notification(input("progress", "running", Some(30)))
            .expect("insert progress notification");
        let initial = database.list_app_notifications(true, 20).unwrap();
        assert_eq!(initial.len(), 1);
        database
            .set_app_notification_read(&initial[0].id, true)
            .expect("mark read");
        assert!(database
            .list_app_notifications(true, 20)
            .unwrap()
            .is_empty());

        database
            .upsert_app_notification(input("completed", "completed", Some(100)))
            .expect("merge completion notification");
        let completed = database.list_app_notifications(true, 20).unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].kind, "completed");
        assert_eq!(completed[0].status, "completed");
        assert_eq!(completed[0].progress, Some(100));

        database
            .set_app_notification_read(&completed[0].id, true)
            .expect("mark completion read");
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE app_notifications SET updated_at = 7 WHERE merge_key = 'run:one'",
                    [],
                )?;
                Ok(())
            })
            .expect("set deterministic notification timestamp");
        database
            .upsert_app_notification(input("completed", "completed", Some(100)))
            .expect("repeat identical completion notification");
        let repeated = database.list_app_notifications(false, 20).unwrap();
        assert_eq!(repeated.len(), 1);
        assert_eq!(repeated[0].updated_at, 7);
        assert!(repeated[0].read_at.is_some());
        assert!(database
            .list_app_notifications(true, 20)
            .unwrap()
            .is_empty());

        assert_eq!(database.clear_read_app_notifications().unwrap(), 1);
        assert!(database
            .list_app_notifications(false, 20)
            .unwrap()
            .is_empty());
        database
            .upsert_app_notification(input("completed", "completed", Some(100)))
            .expect("repeat dismissed completion notification");
        assert!(database
            .list_app_notifications(false, 20)
            .unwrap()
            .is_empty());

        database
            .upsert_app_notification(input("progress", "running", Some(10)))
            .expect("reopen notification after lifecycle transition");
        let reopened = database.list_app_notifications(true, 20).unwrap();
        assert_eq!(reopened.len(), 1);
        assert_eq!(reopened[0].status, "running");

        let saved = database
            .save_notification_preferences(&NotificationPreferencesRecord {
                system_popup: true,
                sound: true,
                sound_id: "chime".to_owned(),
                badge: false,
                quiet_progress: false,
                updated_at: 0,
            })
            .expect("save notification preferences");
        assert!(saved.system_popup && saved.sound);
        assert_eq!(saved.sound_id, "chime");
        assert!(!saved.badge && !saved.quiet_progress);
        assert_eq!(database.notification_preferences().unwrap().badge, false);

        drop(database);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn resolved_approval_notifications_are_read_instead_of_reopened() {
        let (database, path) = test_database();
        let approval = |merge_key: &str, kind: &str, status: &str| AppNotificationUpsert {
            merge_key: merge_key.to_owned(),
            kind: kind.to_owned(),
            severity: if kind == "approval" { "high" } else { "normal" }.to_owned(),
            title: if kind == "approval" {
                "操作等待审批"
            } else {
                "审批已处理"
            }
            .to_owned(),
            body: "测试对话 · 写入文件".to_owned(),
            source_type: "approval".to_owned(),
            source_id: merge_key.to_owned(),
            workspace_view: Some("chat".to_owned()),
            entity_id: Some("conversation-one".to_owned()),
            progress: None,
            status: status.to_owned(),
            action: json!({"open": "conversation"}),
        };

        database
            .upsert_app_notification(approval("approval:one", "approval", "pending"))
            .expect("insert pending approval");
        let pending = database.list_app_notifications(true, 20).unwrap();
        assert_eq!(pending.len(), 1);
        database
            .set_app_notification_read(&pending[0].id, true)
            .expect("mark visible approval read");

        database
            .upsert_app_notification(approval("approval:one", "completed", "approved"))
            .expect("resolve approval");
        assert!(database
            .list_app_notifications(true, 20)
            .unwrap()
            .is_empty());
        let resolved = database.list_app_notifications(false, 20).unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].status, "approved");
        assert!(resolved[0].read_at.is_some());

        database
            .upsert_app_notification(approval("approval:two", "completed", "denied"))
            .expect("insert already resolved approval");
        assert!(database
            .list_app_notifications(true, 20)
            .unwrap()
            .is_empty());

        drop(database);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn feedback_context_is_bounded_and_excludes_message_content() {
        let (database, path) = test_database();
        let project_root = std::env::temp_dir().join(format!("fox-feedback-{}", Uuid::new_v4()));
        fs::create_dir_all(&project_root).expect("create feedback project");
        let conversation = database
            .create_conversation(
                DEFAULT_AGENT_ID,
                Some("Feedback conversation"),
                Some(project_root.to_string_lossy().as_ref()),
                Some("read_only"),
            )
            .expect("create feedback conversation");
        let started = database
            .create_run(
                &conversation.id,
                "SECRET_PROMPT_MUST_NOT_BE_IN_FEEDBACK_CONTEXT",
                Some("test-model"),
            )
            .expect("create feedback run");
        for (sequence, event) in [
            json!({"type": "run.started"}),
            json!({"type": "message.started"}),
            json!({"type": "message.delta", "delta": "SECRET_ANSWER_MUST_NOT_BE_IN_FEEDBACK_CONTEXT"}),
            json!({"type": "run.completed"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&started.run.id, sequence as i64 + 1, &event)
                .expect("apply feedback run event");
        }
        let assistant_message_id = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT id FROM messages WHERE run_id = ?1 AND role = 'assistant'",
                    [&started.run.id],
                    |row| row.get::<_, String>(0),
                )
            })
            .expect("load assistant message");
        let feedback = database
            .save_message_feedback(
                &assistant_message_id,
                "negative",
                Some("misunderstanding"),
                Some(&format!("  {}  ", "x".repeat(1_100))),
            )
            .expect("save message feedback");
        assert_eq!(feedback.comment.as_ref().map(String::len), Some(1_000));
        let context = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT context_json FROM message_feedback WHERE message_id = ?1",
                    [&assistant_message_id],
                    |row| row.get::<_, String>(0),
                )
            })
            .expect("load feedback context");
        assert!(context.contains("read_only"));
        assert!(!context.contains("SECRET_PROMPT"));
        assert!(!context.contains("SECRET_ANSWER"));
        assert!(database
            .save_message_feedback(&assistant_message_id, "negative", None, None)
            .is_err());

        drop(database);
        let _ = fs::remove_file(path);
        let _ = fs::remove_dir_all(project_root);
    }

    #[test]
    fn archived_projects_leave_global_search_and_missing_paths_are_reported() {
        let (database, path) = test_database();
        let project_root = std::env::temp_dir().join(format!("fox-project-{}", Uuid::new_v4()));
        fs::create_dir_all(&project_root).expect("create managed project");
        let conversation = database
            .create_conversation(
                DEFAULT_AGENT_ID,
                Some("Unrelated conversation"),
                Some(project_root.to_string_lossy().as_ref()),
                Some("allow"),
            )
            .expect("create managed project conversation");
        let project_id = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT project_id FROM conversations WHERE id = ?1",
                    [&conversation.id],
                    |row| row.get::<_, String>(0),
                )
            })
            .expect("load project id");
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE projects SET name = 'UniqueArchivedProject' WHERE id = ?1",
                    [&project_id],
                )?;
                Ok(())
            })
            .expect("rename project for search");
        assert_eq!(
            database
                .global_search("UniqueArchivedProject", 20)
                .unwrap()
                .len(),
            1
        );
        assert!(database.set_project_archived(&project_id, true).unwrap());
        assert!(database
            .global_search("UniqueArchivedProject", 20)
            .unwrap()
            .is_empty());

        assert!(database.set_project_archived(&project_id, false).unwrap());
        fs::remove_dir_all(&project_root).expect("remove managed project directory");
        let record = database
            .list_project_management()
            .unwrap()
            .into_iter()
            .find(|item| item.id == project_id)
            .expect("managed project record");
        assert_eq!(record.status, "missing");
        assert!(!record.path_exists);

        drop(database);
        let _ = fs::remove_file(path);
    }
}
