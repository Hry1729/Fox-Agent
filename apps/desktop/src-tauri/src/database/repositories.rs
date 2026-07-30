use super::{
    migrations, AgentRecord, AgentResourceRecord, AgentResourcesRecord, ArtifactRecord,
    AttachmentRecord, ConversationDetail, ConversationRuntimeRecord, ConversationSummary,
    ExternalRunRecord, KnowledgeBindingRecord, KnowledgeDocumentActivity,
    KnowledgeDocumentAnnotation, KnowledgeDocumentBookmark, KnowledgeDocumentReadingState,
    KnowledgePreviewCacheEntry, MessageRecord, ModelConnectionTest, ModelProviderRecord,
    ModelServiceRecord, ProviderModelRecord, RecoverableExternalRunRecord, RunEventRecord,
    RunRecord, RuntimePromptMessage, RuntimeSessionRecord, StartRunResult, ToolCallRecord,
    YuxiAgentRecord, YuxiConnectionTest, YuxiServiceRecord,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod work_events;
mod work_graph;

pub use work_events::WORK_EVENT_TYPES;
#[allow(unused_imports)]
pub use work_graph::{
    AddEvidenceInput, CreateGoalInput, CreateTaskInput, GoalRepository, RepositoryError,
    TaskEvidenceRepository, WorkTaskRepository,
};

const DEFAULT_AGENT_ID: &str = "fox-general";
const INITIAL_HISTORY_MESSAGES: usize = 120;
const MAX_STORED_TOOL_RESULT_BYTES: usize = 128 * 1024;
const KNOWLEDGE_PREVIEW_CACHE_LIMIT_KEY: &str = "knowledge_preview_cache_limit_bytes";
const USER_PROFILE_KEY: &str = "user_profile";

#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
}

impl Database {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        migrations::run(&mut connection, now_ms()).map_err(|error| error.to_string())?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
        };
        database.seed_default_agent()?;
        Ok(database)
    }

    pub fn default_agent_id(&self) -> &'static str {
        DEFAULT_AGENT_ID
    }

    pub fn backup_to(&self, path: &std::path::Path) -> Result<(), String> {
        if path.exists() {
            std::fs::remove_file(path).map_err(|error| error.to_string())?;
        }
        self.with_connection(|connection| {
            connection.execute("VACUUM INTO ?1", [path.to_string_lossy().as_ref()])?;
            Ok(())
        })
    }

    pub fn schema_version(&self) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
        })
    }

    #[allow(dead_code)] // Consumed by the FOX-3 Host protocol in the next milestone.
    pub fn goals(&self) -> GoalRepository {
        GoalRepository::new(self.clone())
    }

    #[allow(dead_code)] // Consumed by the FOX-3 Host protocol in the next milestone.
    pub fn work_tasks(&self) -> WorkTaskRepository {
        WorkTaskRepository::new(self.clone())
    }

    #[allow(dead_code)] // Consumed by the FOX-3 Host protocol in the next milestone.
    pub fn task_evidence(&self) -> TaskEvidenceRepository {
        TaskEvidenceRepository::new(self.clone())
    }

    pub fn known_attachment_paths(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement = connection
                .prepare("SELECT storage_path FROM attachments WHERE status = 'ready'")?;
            let paths = statement.query_map([], |row| row.get(0))?.collect();
            paths
        })
    }

    pub fn remove_failed_attachment_records(&self) -> Result<usize, String> {
        self.with_connection(|connection| {
            connection.execute("DELETE FROM attachments WHERE status != 'ready'", [])
        })
    }

    pub fn knowledge_preview_cache_usage(&self) -> Result<u64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(SUM(byte_size), 0) FROM knowledge_preview_cache",
                [],
                |row| row.get::<_, i64>(0).map(|value| value.max(0) as u64),
            )
        })
    }

    pub fn knowledge_preview_cache_limit(&self, default_limit: u64) -> Result<u64, String> {
        self.with_connection(|connection| {
            let value = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    [KNOWLEDGE_PREVIEW_CACHE_LIMIT_KEY],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Ok(value
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(default_limit))
        })
    }

    pub fn set_knowledge_preview_cache_limit(&self, limit_bytes: u64) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                     updated_at = excluded.updated_at",
                params![
                    KNOWLEDGE_PREVIEW_CACHE_LIMIT_KEY,
                    limit_bytes.to_string(),
                    now_ms()
                ],
            )?;
            Ok(())
        })
    }

    pub fn user_profile(&self) -> Result<Option<super::UserProfileRecord>, String> {
        self.with_connection(|connection| {
            let value = connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    [USER_PROFILE_KEY],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            value
                .map(|value| {
                    serde_json::from_str(&value).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })
                })
                .transpose()
        })
    }

    pub fn save_user_profile(
        &self,
        profile: &super::UserProfileRecord,
    ) -> Result<super::UserProfileRecord, String> {
        let value = serde_json::to_string(profile).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                     updated_at = excluded.updated_at",
                params![USER_PROFILE_KEY, value, now_ms()],
            )?;
            Ok(profile.clone())
        })
    }

    pub fn knowledge_document_activity(
        &self,
        knowledge_base_id: &str,
        document_id: &str,
    ) -> Result<KnowledgeDocumentActivity, String> {
        self.with_connection(|connection| {
            let reading_state = connection
                .query_row(
                    "SELECT knowledge_base_id, document_id, source_revision, page,
                            scroll_offset, zoom, updated_at
                     FROM knowledge_document_reading_state
                     WHERE knowledge_base_id = ?1 AND document_id = ?2",
                    params![knowledge_base_id, document_id],
                    |row| {
                        Ok(KnowledgeDocumentReadingState {
                            knowledge_base_id: row.get(0)?,
                            document_id: row.get(1)?,
                            source_revision: row.get(2)?,
                            page: row.get(3)?,
                            scroll_offset: row.get(4)?,
                            zoom: row.get(5)?,
                            updated_at: row.get(6)?,
                        })
                    },
                )
                .optional()?;
            let mut bookmark_statement = connection.prepare(
                "SELECT id, knowledge_base_id, document_id, source_revision, page,
                        anchor, excerpt, label, created_at, updated_at
                 FROM knowledge_document_bookmarks
                 WHERE knowledge_base_id = ?1 AND document_id = ?2
                 ORDER BY page, created_at",
            )?;
            let bookmarks = bookmark_statement
                .query_map(params![knowledge_base_id, document_id], |row| {
                    Ok(KnowledgeDocumentBookmark {
                        id: row.get(0)?,
                        knowledge_base_id: row.get(1)?,
                        document_id: row.get(2)?,
                        source_revision: row.get(3)?,
                        page: row.get(4)?,
                        anchor: row.get(5)?,
                        excerpt: row.get(6)?,
                        label: row.get(7)?,
                        created_at: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut annotation_statement = connection.prepare(
                "SELECT id, knowledge_base_id, document_id, source_revision,
                        annotation_type, page, anchor, excerpt, note, color,
                        created_at, updated_at
                 FROM knowledge_document_annotations
                 WHERE knowledge_base_id = ?1 AND document_id = ?2
                 ORDER BY page, created_at",
            )?;
            let annotations = annotation_statement
                .query_map(params![knowledge_base_id, document_id], |row| {
                    Ok(KnowledgeDocumentAnnotation {
                        id: row.get(0)?,
                        knowledge_base_id: row.get(1)?,
                        document_id: row.get(2)?,
                        source_revision: row.get(3)?,
                        annotation_type: row.get(4)?,
                        page: row.get(5)?,
                        anchor: row.get(6)?,
                        excerpt: row.get(7)?,
                        note: row.get(8)?,
                        color: row.get(9)?,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(KnowledgeDocumentActivity {
                reading_state,
                bookmarks,
                annotations,
            })
        })
    }

    pub fn save_knowledge_document_reading_state(
        &self,
        state: &KnowledgeDocumentReadingState,
    ) -> Result<KnowledgeDocumentReadingState, String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_document_reading_state(
                    knowledge_base_id, document_id, source_revision, page,
                    scroll_offset, zoom, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(knowledge_base_id, document_id) DO UPDATE SET
                    source_revision = excluded.source_revision,
                    page = excluded.page,
                    scroll_offset = excluded.scroll_offset,
                    zoom = excluded.zoom,
                    updated_at = excluded.updated_at",
                params![
                    state.knowledge_base_id,
                    state.document_id,
                    state.source_revision,
                    state.page,
                    state.scroll_offset,
                    state.zoom,
                    state.updated_at,
                ],
            )?;
            Ok(state.clone())
        })
    }

    pub fn save_knowledge_document_bookmark(
        &self,
        bookmark: &KnowledgeDocumentBookmark,
    ) -> Result<KnowledgeDocumentBookmark, String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_document_bookmarks(
                    id, knowledge_base_id, document_id, source_revision, page,
                    anchor, excerpt, label, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    source_revision = excluded.source_revision,
                    page = excluded.page,
                    anchor = excluded.anchor,
                    excerpt = excluded.excerpt,
                    label = excluded.label,
                    updated_at = excluded.updated_at
                 WHERE knowledge_document_bookmarks.knowledge_base_id = excluded.knowledge_base_id
                   AND knowledge_document_bookmarks.document_id = excluded.document_id",
                params![
                    bookmark.id,
                    bookmark.knowledge_base_id,
                    bookmark.document_id,
                    bookmark.source_revision,
                    bookmark.page,
                    bookmark.anchor,
                    bookmark.excerpt,
                    bookmark.label,
                    bookmark.created_at,
                    bookmark.updated_at,
                ],
            )?;
            Ok(bookmark.clone())
        })
    }

    pub fn save_knowledge_document_annotation(
        &self,
        annotation: &KnowledgeDocumentAnnotation,
    ) -> Result<KnowledgeDocumentAnnotation, String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_document_annotations(
                    id, knowledge_base_id, document_id, source_revision,
                    annotation_type, page, anchor, excerpt, note, color,
                    created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                    source_revision = excluded.source_revision,
                    annotation_type = excluded.annotation_type,
                    page = excluded.page,
                    anchor = excluded.anchor,
                    excerpt = excluded.excerpt,
                    note = excluded.note,
                    color = excluded.color,
                    updated_at = excluded.updated_at
                 WHERE knowledge_document_annotations.knowledge_base_id = excluded.knowledge_base_id
                   AND knowledge_document_annotations.document_id = excluded.document_id",
                params![
                    annotation.id,
                    annotation.knowledge_base_id,
                    annotation.document_id,
                    annotation.source_revision,
                    annotation.annotation_type,
                    annotation.page,
                    annotation.anchor,
                    annotation.excerpt,
                    annotation.note,
                    annotation.color,
                    annotation.created_at,
                    annotation.updated_at,
                ],
            )?;
            Ok(annotation.clone())
        })
    }

    pub fn delete_knowledge_document_bookmark(
        &self,
        knowledge_base_id: &str,
        document_id: &str,
        id: &str,
    ) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "DELETE FROM knowledge_document_bookmarks
                     WHERE id = ?1 AND knowledge_base_id = ?2 AND document_id = ?3",
                    params![id, knowledge_base_id, document_id],
                )
                .map(|count| count > 0)
        })
    }

    pub fn delete_knowledge_document_annotation(
        &self,
        knowledge_base_id: &str,
        document_id: &str,
        id: &str,
    ) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection
                .execute(
                    "DELETE FROM knowledge_document_annotations
                     WHERE id = ?1 AND knowledge_base_id = ?2 AND document_id = ?3",
                    params![id, knowledge_base_id, document_id],
                )
                .map(|count| count > 0)
        })
    }

    pub fn knowledge_preview_cache_statistics(
        &self,
        limit_bytes: u64,
    ) -> Result<super::KnowledgePreviewCacheStatistics, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COUNT(*),
                        COALESCE(SUM(CASE WHEN lease_count > 0 THEN 1 ELSE 0 END), 0),
                        COALESCE(SUM(byte_size), 0),
                        COALESCE(SUM(CASE WHEN lease_count > 0 THEN byte_size ELSE 0 END), 0)
                 FROM knowledge_preview_cache",
                [],
                |row| {
                    Ok(super::KnowledgePreviewCacheStatistics {
                        total_files: row.get::<_, i64>(0)?.max(0) as u64,
                        active_files: row.get::<_, i64>(1)?.max(0) as u64,
                        total_bytes: row.get::<_, i64>(2)?.max(0) as u64,
                        active_bytes: row.get::<_, i64>(3)?.max(0) as u64,
                        limit_bytes,
                    })
                },
            )
        })
    }

    pub fn knowledge_preview_cache_lru_candidates(
        &self,
    ) -> Result<Vec<(String, String, u64)>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT cache_key, storage_path, byte_size
                 FROM knowledge_preview_cache
                 WHERE lease_count = 0
                 ORDER BY last_accessed_at ASC",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get::<_, i64>(2)?.max(0) as u64,
                    ))
                })?
                .collect();
            rows
        })
    }

    pub fn knowledge_preview_cache_storage_paths(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement =
                connection.prepare("SELECT storage_path FROM knowledge_preview_cache")?;
            let paths = statement.query_map([], |row| row.get(0))?.collect();
            paths
        })
    }

    pub fn remove_knowledge_preview_cache_entry(&self, cache_key: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "DELETE FROM knowledge_preview_cache WHERE cache_key = ?1 AND lease_count = 0",
                [cache_key],
            )? > 0)
        })
    }

    #[allow(dead_code)]
    pub fn knowledge_preview_cache_entry(
        &self,
        cache_key: &str,
    ) -> Result<Option<KnowledgePreviewCacheEntry>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT cache_key, knowledge_base_id, document_id, source_revision, variant,
                            storage_path, media_type, byte_size, lease_count, created_at,
                            last_accessed_at
                     FROM knowledge_preview_cache WHERE cache_key = ?1",
                    [cache_key],
                    |row| {
                        Ok(KnowledgePreviewCacheEntry {
                            cache_key: row.get(0)?,
                            knowledge_base_id: row.get(1)?,
                            document_id: row.get(2)?,
                            source_revision: row.get(3)?,
                            variant: row.get(4)?,
                            storage_path: row.get(5)?,
                            media_type: row.get(6)?,
                            byte_size: row.get::<_, i64>(7)?.max(0) as u64,
                            lease_count: row.get::<_, i64>(8)?.max(0) as u32,
                            created_at: row.get(9)?,
                            last_accessed_at: row.get(10)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn reset_knowledge_preview_cache_leases(&self) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute("UPDATE knowledge_preview_cache SET lease_count = 0", [])?;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub fn upsert_knowledge_preview_cache_entry(
        &self,
        entry: &KnowledgePreviewCacheEntry,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO knowledge_preview_cache(
                    cache_key, knowledge_base_id, document_id, source_revision, variant,
                    storage_path, media_type, byte_size, lease_count, created_at, last_accessed_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                 ON CONFLICT(cache_key) DO UPDATE SET
                    knowledge_base_id = excluded.knowledge_base_id,
                    document_id = excluded.document_id,
                    source_revision = excluded.source_revision,
                    variant = excluded.variant,
                    storage_path = excluded.storage_path,
                    media_type = excluded.media_type,
                    byte_size = excluded.byte_size,
                    lease_count = knowledge_preview_cache.lease_count,
                    last_accessed_at = excluded.last_accessed_at",
                params![
                    entry.cache_key,
                    entry.knowledge_base_id,
                    entry.document_id,
                    entry.source_revision,
                    entry.variant,
                    entry.storage_path,
                    entry.media_type,
                    entry.byte_size.min(i64::MAX as u64) as i64,
                    i64::from(entry.lease_count),
                    entry.created_at,
                    entry.last_accessed_at,
                ],
            )?;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub fn touch_knowledge_preview_cache_entry(
        &self,
        cache_key: &str,
        accessed_at: i64,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE knowledge_preview_cache SET last_accessed_at = ?2 WHERE cache_key = ?1",
                params![cache_key, accessed_at],
            )?;
            Ok(())
        })
    }

    #[allow(dead_code)]
    pub fn acquire_knowledge_preview_cache_lease(&self, cache_key: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE knowledge_preview_cache
                 SET lease_count = lease_count + 1, last_accessed_at = ?2
                 WHERE cache_key = ?1",
                params![cache_key, now_ms()],
            )? > 0)
        })
    }

    #[allow(dead_code)]
    pub fn release_knowledge_preview_cache_lease(&self, cache_key: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "UPDATE knowledge_preview_cache
                 SET lease_count = CASE WHEN lease_count > 0 THEN lease_count - 1 ELSE 0 END
                 WHERE cache_key = ?1",
                [cache_key],
            )? > 0)
        })
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        operation(&mut connection).map_err(|error| error.to_string())
    }

    fn seed_default_agent(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT OR IGNORE INTO agents(
                    id, name, description, runtime_type, system_prompt, default_model, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![
                    DEFAULT_AGENT_ID,
                    "Fox 通用助手",
                    "Fox 默认通用智能体",
                    "pi",
                    "You are Fox, a careful general-purpose desktop assistant.",
                    "configured-model",
                    now,
                ],
            )?;
            Ok(())
        })
    }

    pub fn repair_interrupted_runs(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs
                 SET status = 'interrupted', finished_at = ?1,
                     error_code = 'runtime.application_restarted',
                     error_message = 'Fox closed before the run reached a terminal state.'
                 WHERE status IN ('queued', 'running', 'cancelling')
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations c JOIN agents a ON a.id = c.agent_id
                       WHERE c.id = runs.conversation_id AND a.runtime_type = 'yuxi'
                         AND runs.external_run_id IS NOT NULL
                   )",
                [now],
            )?;
            connection.execute(
                "UPDATE messages SET status = 'interrupted', updated_at = ?1
                 WHERE status = 'streaming'
                   AND NOT EXISTS (
                       SELECT 1 FROM runs r
                       JOIN conversations c ON c.id = r.conversation_id
                       JOIN agents a ON a.id = c.agent_id
                       WHERE r.id = messages.run_id AND a.runtime_type = 'yuxi'
                         AND r.external_run_id IS NOT NULL
                         AND r.status IN ('queued', 'running', 'cancelling', 'interrupted')
                   )",
                [now],
            )?;
            connection.execute(
                "UPDATE tool_calls SET status = 'interrupted', completed_at = ?1, updated_at = ?1
                 WHERE status IN ('pending', 'running')
                   AND NOT EXISTS (
                       SELECT 1 FROM runs r
                       JOIN conversations c ON c.id = r.conversation_id
                       JOIN agents a ON a.id = c.agent_id
                       WHERE r.id = tool_calls.run_id AND a.runtime_type = 'yuxi'
                         AND r.external_run_id IS NOT NULL
                         AND r.status IN ('queued', 'running', 'cancelling', 'interrupted')
                   )",
                [now],
            )?;
            connection.execute(
                "UPDATE approvals
                 SET status = 'cancelled', decision_json = '{\"reason\":\"application_restarted\"}',
                     resolved_at = ?1
                 WHERE status = 'pending'
                   AND NOT EXISTS (
                       SELECT 1 FROM tool_calls t
                       JOIN runs r ON r.id = t.run_id
                       JOIN conversations c ON c.id = r.conversation_id
                       JOIN agents a ON a.id = c.agent_id
                       WHERE t.id = approvals.tool_call_id AND a.runtime_type = 'yuxi'
                         AND r.external_run_id IS NOT NULL
                         AND r.status IN ('queued', 'running', 'cancelling', 'interrupted')
                   )",
                [now],
            )?;
            Ok(())
        })
    }

    pub fn list_agents(&self) -> Result<Vec<AgentRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, description, runtime_type, default_model, system_prompt,
                        CASE WHEN runtime_type = 'yuxi' THEN
                            COALESCE((SELECT last_status FROM yuxi_service WHERE singleton_id = 1), 'unknown')
                        ELSE 'connected' END
                 FROM agents ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, created_at ASC",
            )?;
            let records = statement
                .query_map([DEFAULT_AGENT_ID], |row| {
                    let id: String = row.get(0)?;
                    let runtime_type: String = row.get(3)?;
                    let metadata = parse_json(&row.get::<_, String>(5)?);
                    Ok(AgentRecord {
                        id: id.clone(),
                        name: row.get(1)?,
                        description: row.get(2)?,
                        runtime_type: runtime_type.clone(),
                        default_model: row.get(4)?,
                        icon: metadata.get("icon").and_then(Value::as_str).map(str::to_owned),
                        capabilities: metadata.get("capabilities").cloned().unwrap_or_else(|| {
                            if runtime_type == "pi" { json!(["files", "tools", "reasoning"]) } else { json!([]) }
                        }),
                        resources: metadata
                            .get("resources")
                            .cloned()
                            .and_then(|value| serde_json::from_value(value).ok())
                            .unwrap_or_else(|| {
                                if runtime_type == "pi" {
                                    AgentResourcesRecord {
                                        tools: vec![
                                            AgentResourceRecord { id: "files".to_owned(), name: "文件读写".to_owned(), description: "读取和处理已授权项目文件".to_owned() },
                                            AgentResourceRecord { id: "tools".to_owned(), name: "本地工具".to_owned(), description: "调用 Fox Runtime 提供的本地工具".to_owned() },
                                            AgentResourceRecord { id: "reasoning".to_owned(), name: "任务推理".to_owned(), description: "规划并执行多步骤任务".to_owned() },
                                        ],
                                        ..AgentResourcesRecord::default()
                                    }
                                } else {
                                    AgentResourcesRecord::default()
                                }
                            }),
                        configurable_items: metadata.get("configurableItems").cloned().unwrap_or_else(|| json!({})),
                        is_default: id == DEFAULT_AGENT_ID || metadata.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
                        available: runtime_type != "yuxi" || (
                            row.get::<_, String>(6)? == "connected"
                                && metadata.get("remoteAvailable").and_then(Value::as_bool).unwrap_or(true)
                        ),
                    })
                })?
                .collect();
            records
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                        COALESCE(p.root_path, c.project_root), c.status,
                        c.created_at, c.updated_at, c.last_message_at
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 ORDER BY COALESCE(c.last_message_at, c.created_at) DESC",
            )?;
            let records = statement.query_map([], map_conversation)?.collect();
            records
        })
    }

    pub fn usage_statistics(&self) -> Result<super::UsageStatistics, String> {
        self.with_connection(|connection| {
            let conversation_count = connection.query_row(
                "SELECT COUNT(*) FROM conversations",
                [],
                |row| row.get(0),
            )?;
            let completed_run_count = connection.query_row(
                "SELECT COUNT(*) FROM runs WHERE status = 'completed'",
                [],
                |row| row.get(0),
            )?;
            let user_message_count = connection.query_row(
                "SELECT COUNT(*) FROM messages WHERE role = 'user'",
                [],
                |row| row.get(0),
            )?;
            let active_day_count = connection.query_row(
                "SELECT COUNT(DISTINCT date(created_at / 1000, 'unixepoch', 'localtime'))
                 FROM messages WHERE role = 'user'",
                [],
                |row| row.get(0),
            )?;
            let (input_tokens, output_tokens, total_tokens) = connection.query_row(
                "WITH latest_usage AS (
                    SELECT e.event_json,
                           ROW_NUMBER() OVER (PARTITION BY e.run_id ORDER BY e.seq DESC) AS position
                    FROM run_events e WHERE e.event_type = 'usage.updated'
                 )
                 SELECT
                    COALESCE(SUM(CAST(json_extract(event_json, '$.inputTokens') AS INTEGER)), 0),
                    COALESCE(SUM(CAST(json_extract(event_json, '$.outputTokens') AS INTEGER)), 0),
                    COALESCE(SUM(MAX(
                        CAST(COALESCE(json_extract(event_json, '$.totalTokens'), 0) AS INTEGER),
                        CAST(COALESCE(json_extract(event_json, '$.inputTokens'), 0) AS INTEGER) +
                        CAST(COALESCE(json_extract(event_json, '$.outputTokens'), 0) AS INTEGER)
                    )), 0)
                 FROM latest_usage WHERE position = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;

            let mut agent_statement = connection.prepare(
                "WITH latest_usage AS (
                    SELECT c.agent_id, e.event_json,
                           ROW_NUMBER() OVER (PARTITION BY e.run_id ORDER BY e.seq DESC) AS position
                    FROM run_events e
                    JOIN runs r ON r.id = e.run_id
                    JOIN conversations c ON c.id = r.conversation_id
                    WHERE e.event_type = 'usage.updated'
                 ), conversation_counts AS (
                    SELECT agent_id, COUNT(*) AS conversation_count
                    FROM conversations GROUP BY agent_id
                 ), run_counts AS (
                    SELECT c.agent_id, COUNT(*) AS run_count
                    FROM runs r JOIN conversations c ON c.id = r.conversation_id
                    GROUP BY c.agent_id
                 ), usage_by_agent AS (
                    SELECT agent_id,
                           SUM(MAX(
                             CAST(COALESCE(json_extract(event_json, '$.totalTokens'), 0) AS INTEGER),
                             CAST(COALESCE(json_extract(event_json, '$.inputTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(event_json, '$.outputTokens'), 0) AS INTEGER)
                           )) AS total_tokens
                    FROM latest_usage WHERE position = 1 GROUP BY agent_id
                 )
                 SELECT a.id, a.name, COALESCE(c.conversation_count, 0),
                        COALESCE(r.run_count, 0), COALESCE(u.total_tokens, 0)
                 FROM agents a
                 LEFT JOIN conversation_counts c ON c.agent_id = a.id
                 LEFT JOIN run_counts r ON r.agent_id = a.id
                 LEFT JOIN usage_by_agent u ON u.agent_id = a.id
                 WHERE COALESCE(c.conversation_count, 0) > 0
                 ORDER BY COALESCE(r.run_count, 0) DESC, a.name COLLATE NOCASE
                 LIMIT 12",
            )?;
            let agents = agent_statement
                .query_map([], |row| {
                    Ok(super::UsageAgentStat {
                        agent_id: row.get(0)?,
                        agent_name: row.get(1)?,
                        conversation_count: row.get(2)?,
                        run_count: row.get(3)?,
                        total_tokens: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            let mut day_statement = connection.prepare(
                "WITH RECURSIVE days(day) AS (
                    SELECT date('now', 'localtime', '-363 days')
                    UNION ALL SELECT date(day, '+1 day') FROM days WHERE day < date('now', 'localtime')
                 ), conversation_days AS (
                    SELECT date(created_at / 1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS count
                    FROM conversations GROUP BY day
                 ), run_days AS (
                    SELECT date(created_at / 1000, 'unixepoch', 'localtime') AS day, COUNT(*) AS count
                    FROM runs GROUP BY day
                 ), usage_days AS (
                    SELECT date(e.created_at / 1000, 'unixepoch', 'localtime') AS day,
                           SUM(MAX(
                             CAST(COALESCE(json_extract(e.event_json, '$.totalTokens'), 0) AS INTEGER),
                             CAST(COALESCE(json_extract(e.event_json, '$.inputTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(e.event_json, '$.outputTokens'), 0) AS INTEGER)
                           )) AS total_tokens
                    FROM run_events e
                    WHERE e.event_type = 'usage.updated'
                      AND e.seq = (SELECT MAX(e2.seq) FROM run_events e2
                                   WHERE e2.run_id = e.run_id AND e2.event_type = 'usage.updated')
                    GROUP BY day
                 )
                 SELECT days.day, COALESCE(c.count, 0), COALESCE(r.count, 0),
                        COALESCE(u.total_tokens, 0)
                 FROM days
                 LEFT JOIN conversation_days c ON c.day = days.day
                 LEFT JOIN run_days r ON r.day = days.day
                 LEFT JOIN usage_days u ON u.day = days.day
                 ORDER BY days.day",
            )?;
            let days = day_statement
                .query_map([], |row| {
                    Ok(super::UsageDayStat {
                        date: row.get(0)?,
                        conversation_count: row.get(1)?,
                        run_count: row.get(2)?,
                        total_tokens: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(super::UsageStatistics {
                conversation_count,
                completed_run_count,
                user_message_count,
                active_day_count,
                input_tokens,
                output_tokens,
                total_tokens,
                agents,
                days,
            })
        })
    }

    pub fn search_conversations(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ConversationSummary>, String> {
        let query = query.trim();
        if query.is_empty() {
            return self.list_conversations();
        }
        let limit = limit.clamp(1, 100);
        let like = format!(
            "%{}%",
            query
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let fts_query = query
            .split_whitespace()
            .filter(|part| !part.is_empty())
            .map(|part| format!("\"{}\"", part.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" AND ");
        self.with_connection(|connection| {
            if fts_query.is_empty() {
                return Ok(Vec::new());
            }
            let mut statement = connection.prepare(
                "SELECT DISTINCT c.id, c.agent_id, a.name, c.title, c.project_id,
                        COALESCE(p.root_path, c.project_root), c.status,
                        c.created_at, c.updated_at, c.last_message_at
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 WHERE c.title LIKE ?1 ESCAPE '\\'
                    OR COALESCE(p.root_path, c.project_root, '') LIKE ?1 ESCAPE '\\'
                    OR a.name LIKE ?1 ESCAPE '\\'
                    OR EXISTS(SELECT 1 FROM messages m
                              WHERE m.conversation_id = c.id AND m.content LIKE ?1 ESCAPE '\\')
                    OR c.id IN (SELECT conversation_id FROM conversation_fts
                                WHERE conversation_fts MATCH ?2)
                    OR c.id IN (SELECT conversation_id FROM message_fts
                                WHERE message_fts MATCH ?2)
                 ORDER BY COALESCE(c.last_message_at, c.created_at) DESC
                 LIMIT ?3",
            )?;
            let records = statement
                .query_map(params![like, fts_query, limit as i64], map_conversation)?
                .collect();
            records
        })
    }

    pub fn conversation_internal_paths(&self, id: &str) -> Result<Vec<PathBuf>, String> {
        self.with_connection(|connection| {
            let mut paths = Vec::new();
            let mut attachments = connection
                .prepare("SELECT storage_path FROM attachments WHERE conversation_id = ?1")?;
            paths.extend(
                attachments
                    .query_map([id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .into_iter()
                    .map(PathBuf::from),
            );
            let mut artifacts = connection
                .prepare("SELECT storage_path FROM artifacts WHERE conversation_id = ?1")?;
            paths.extend(
                artifacts
                    .query_map([id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?
                    .into_iter()
                    .map(PathBuf::from),
            );
            Ok(paths)
        })
    }

    pub fn list_projects(&self) -> Result<Vec<super::ProjectRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, root_path, permission_mode, status,
                        created_at, updated_at, last_opened_at
                 FROM projects
                 ORDER BY COALESCE(last_opened_at, updated_at) DESC, name ASC",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok(super::ProjectRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        root_path: row.get(2)?,
                        permission_mode: row.get(3)?,
                        status: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
                        last_opened_at: row.get(7)?,
                    })
                })?
                .collect();
            records
        })
    }

    pub fn update_project_permission_mode(
        &self,
        project_id: &str,
        permission_mode: &str,
    ) -> Result<Option<super::ProjectRecord>, String> {
        if !matches!(permission_mode, "read_only" | "ask" | "allow") {
            return Err("invalid project permission mode".to_owned());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE projects SET permission_mode = ?2, updated_at = ?3 WHERE id = ?1",
                params![project_id, permission_mode, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            connection
                .query_row(
                    "SELECT id, name, root_path, permission_mode, status,
                            created_at, updated_at, last_opened_at
                     FROM projects WHERE id = ?1",
                    [project_id],
                    |row| {
                        Ok(super::ProjectRecord {
                            id: row.get(0)?,
                            name: row.get(1)?,
                            root_path: row.get(2)?,
                            permission_mode: row.get(3)?,
                            status: row.get(4)?,
                            created_at: row.get(5)?,
                            updated_at: row.get(6)?,
                            last_opened_at: row.get(7)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn create_conversation(
        &self,
        agent_id: &str,
        title: Option<&str>,
        project_root: Option<&str>,
        permission_mode: Option<&str>,
    ) -> Result<ConversationSummary, String> {
        let id = Uuid::new_v4().to_string();
        let now = now_ms();
        let title = title
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("新对话");
        let permission_mode = permission_mode.unwrap_or("read_only");
        if !matches!(permission_mode, "read_only" | "ask" | "allow") {
            return Err("invalid project permission mode".to_owned());
        }

        self.with_connection(|connection| {
            let agent_exists = connection
                .query_row("SELECT 1 FROM agents WHERE id = ?1", [agent_id], |_| Ok(()))
                .optional()?
                .is_some();
            if !agent_exists {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }

            let project_id = project_root
                .map(|root| upsert_project(connection, root, permission_mode, now))
                .transpose()?;

            connection.execute(
                "INSERT INTO conversations(
                    id, agent_id, title, project_id, project_root, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'active', ?6, ?6)",
                params![id, agent_id, title, project_id, project_root, now],
            )?;
            query_conversation(connection, &id)
        })
    }

    pub fn load_conversation(&self, id: &str) -> Result<ConversationDetail, String> {
        self.with_connection(|connection| {
            let conversation = query_conversation(connection, id)?;
            let messages = query_message_page(connection, id, None, INITIAL_HISTORY_MESSAGES)?;
            let oldest_ordinal = messages.first().map(|message| message.ordinal).unwrap_or(0);
            let runtime_events = query_runtime_events_window(connection, id, oldest_ordinal, None)?;
            let tool_calls = query_tool_calls_window(connection, id, oldest_ordinal, None)?;
            let approvals = query_approvals_window(connection, id, oldest_ordinal, None)?;
            let attachments = query_attachments_window(connection, id, oldest_ordinal, None)?;
            let artifacts = query_artifacts_window(connection, id, oldest_ordinal, None)?;
            let knowledge_bindings = query_knowledge_bindings(connection, id)?;
            let last_run = query_last_run(connection, id)?;
            let has_earlier_messages = oldest_ordinal > 0 && connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation_id = ?1 AND ordinal < ?2)",
                params![id, oldest_ordinal],
                |row| row.get::<_, bool>(0),
            )?;
            Ok(ConversationDetail {
                conversation,
                messages,
                runtime_events,
                tool_calls,
                approvals,
                attachments,
                artifacts,
                knowledge_bindings,
                last_run,
                has_earlier_messages,
            })
        })
    }

    pub fn conversation_knowledge_bindings(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<KnowledgeBindingRecord>, String> {
        self.with_connection(|connection| query_knowledge_bindings(connection, conversation_id))
    }

    pub fn conversation_agent_knowledge_scope(
        &self,
        conversation_id: &str,
    ) -> Result<Option<Vec<String>>, String> {
        self.with_connection(|connection| {
            let (runtime_type, prompt): (String, String) = connection.query_row(
                "SELECT a.runtime_type, a.system_prompt
                 FROM conversations c JOIN agents a ON a.id = c.agent_id
                 WHERE c.id = ?1",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if runtime_type != "yuxi" {
                return Ok(None);
            }

            let metadata = parse_json(&prompt);
            if metadata
                .get("configurableItems")
                .and_then(|items| items.get("knowledges"))
                .and_then(|item| item.get("options"))
                .and_then(Value::as_array)
                .is_none()
            {
                return Ok(None);
            }
            let Some(knowledges) = metadata
                .get("resources")
                .and_then(|resources| resources.get("knowledges"))
                .and_then(Value::as_array)
            else {
                return Ok(None);
            };
            Ok(Some(
                knowledges
                    .iter()
                    .filter_map(|item| item.get("id").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect(),
            ))
        })
    }

    pub fn load_conversation_history(
        &self,
        id: &str,
        before_ordinal: i64,
        limit: usize,
    ) -> Result<super::ConversationHistoryPage, String> {
        let limit = limit.clamp(20, 200);
        self.with_connection(|connection| {
            query_conversation(connection, id)?;
            let messages = query_message_page(connection, id, Some(before_ordinal), limit)?;
            let oldest_ordinal = messages
                .first()
                .map(|message| message.ordinal)
                .unwrap_or(before_ordinal);
            let runtime_events =
                query_runtime_events_window(connection, id, oldest_ordinal, Some(before_ordinal))?;
            let tool_calls =
                query_tool_calls_window(connection, id, oldest_ordinal, Some(before_ordinal))?;
            let approvals =
                query_approvals_window(connection, id, oldest_ordinal, Some(before_ordinal))?;
            let attachments =
                query_attachments_window(connection, id, oldest_ordinal, Some(before_ordinal))?;
            let artifacts =
                query_artifacts_window(connection, id, oldest_ordinal, Some(before_ordinal))?;
            let has_earlier_messages = !messages.is_empty() && connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation_id = ?1 AND ordinal < ?2)",
                params![id, oldest_ordinal],
                |row| row.get::<_, bool>(0),
            )?;
            Ok(super::ConversationHistoryPage {
                messages,
                runtime_events,
                tool_calls,
                approvals,
                attachments,
                artifacts,
                has_earlier_messages,
            })
        })
    }

    pub fn upsert_yuxi_agents(&self, agents: &[YuxiAgentRecord]) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let remote_ids = agents
                .iter()
                .map(|agent| format!("yuxi:{}", agent.slug))
                .collect::<Vec<_>>();
            for agent in agents {
                let id = format!("yuxi:{}", agent.slug);
                let system_prompt = serde_json::to_string(&json!({
                    "remoteAgentId": agent.slug,
                    "backendId": agent.backend_id,
                    "icon": agent.icon,
                    "capabilities": agent.capabilities,
                    "resources": agent.resources,
                    "configurableItems": agent.configurable_items,
                    "isDefault": agent.is_default,
                    "remoteAvailable": agent.available,
                }))
                .unwrap_or_else(|_| "{}".to_owned());
                transaction.execute(
                    "INSERT INTO agents(id, name, description, runtime_type, system_prompt,
                                        default_model, created_at, updated_at)
                     VALUES (?1, ?2, ?3, 'yuxi', ?4, ?5, ?6, ?6)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                        description = excluded.description, system_prompt = excluded.system_prompt,
                        default_model = excluded.default_model, updated_at = excluded.updated_at",
                    params![
                        id,
                        agent.name,
                        agent.description,
                        system_prompt,
                        agent.default_model,
                        now
                    ],
                )?;
            }
            let mut statement = transaction
                .prepare("SELECT id, system_prompt FROM agents WHERE runtime_type = 'yuxi'")?;
            let existing = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (id, system_prompt) in existing {
                if !remote_ids.contains(&id) {
                    let mut metadata = parse_json(&system_prompt);
                    if let Some(object) = metadata.as_object_mut() {
                        object.insert("remoteAvailable".to_owned(), Value::Bool(false));
                    }
                    transaction.execute(
                        "UPDATE agents SET system_prompt = ?2, updated_at = ?3 WHERE id = ?1",
                        params![id, metadata.to_string(), now],
                    )?;
                }
            }
            transaction.commit()
        })
    }

    pub fn conversation_runtime(&self, id: &str) -> Result<ConversationRuntimeRecord, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT c.id, c.agent_id, a.runtime_type, a.system_prompt,
                        (SELECT runtime_session_id FROM runtime_sessions
                         WHERE conversation_id = c.id AND runtime_type = a.runtime_type)
                 FROM conversations c JOIN agents a ON a.id = c.agent_id WHERE c.id = ?1",
                [id],
                |row| {
                    let prompt: String = row.get(3)?;
                    let metadata = parse_json(&prompt);
                    Ok(ConversationRuntimeRecord {
                        runtime_type: row.get(2)?,
                        remote_agent_id: metadata
                            .get("remoteAgentId")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        remote_thread_id: row.get(4)?,
                    })
                },
            )
        })
    }

    pub fn agent_runtime(
        &self,
        agent_id: &str,
    ) -> Result<Option<(String, Option<String>)>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT runtime_type, system_prompt FROM agents WHERE id = ?1",
                    [agent_id],
                    |row| {
                        let runtime_type: String = row.get(0)?;
                        let metadata = parse_json(&row.get::<_, String>(1)?);
                        Ok((
                            runtime_type,
                            metadata
                                .get("remoteAgentId")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        ))
                    },
                )
                .optional()
        })
    }

    pub fn rename_conversation(
        &self,
        id: &str,
        title: &str,
    ) -> Result<Option<ConversationSummary>, String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("conversation title cannot be empty".to_owned());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations SET title = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, title, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn ensure_external_runtime_session(
        &self,
        conversation_id: &str,
        runtime_type: &str,
        external_id: &str,
        runtime_version: Option<&str>,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO runtime_sessions(id, conversation_id, runtime_type, runtime_session_id,
                                              runtime_version, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?1, ?4, 'ready', ?5, ?5)
                 ON CONFLICT(conversation_id, runtime_type) DO UPDATE SET
                    runtime_session_id = excluded.runtime_session_id,
                    runtime_version = excluded.runtime_version,
                    status = 'ready', updated_at = excluded.updated_at",
                params![external_id, conversation_id, runtime_type, runtime_version, now],
            )?;
            Ok(())
        })
    }

    pub fn set_external_run(
        &self,
        local_run_id: &str,
        external_run_id: &str,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs SET external_run_id = ?2 WHERE id = ?1",
                params![local_run_id, external_run_id],
            )?;
            Ok(())
        })
    }

    pub fn set_external_cursor(&self, local_run_id: &str, cursor: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs SET external_cursor = ?2 WHERE id = ?1",
                params![local_run_id, cursor],
            )?;
            Ok(())
        })
    }

    pub fn external_run(&self, local_run_id: &str) -> Result<Option<ExternalRunRecord>, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT r.id, r.conversation_id, r.external_run_id, r.external_cursor, a.runtime_type
                 FROM runs r JOIN conversations c ON c.id = r.conversation_id
                 JOIN agents a ON a.id = c.agent_id WHERE r.id = ?1",
                [local_run_id],
                |row| Ok(ExternalRunRecord {
                    conversation_id: row.get(1)?, external_run_id: row.get(2)?, external_cursor: row.get(3)?, runtime_type: row.get(4)?,
                }),
            ).optional()
        })
    }

    pub fn recoverable_yuxi_runs(&self) -> Result<Vec<RecoverableExternalRunRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT r.id, r.conversation_id, r.external_run_id, r.external_cursor,
                        rs.runtime_session_id
                 FROM runs r
                 JOIN conversations c ON c.id = r.conversation_id
                 JOIN agents a ON a.id = c.agent_id AND a.runtime_type = 'yuxi'
                 JOIN runtime_sessions rs ON rs.conversation_id = c.id AND rs.runtime_type = 'yuxi'
                 WHERE r.external_run_id IS NOT NULL
                   AND (
                       r.status IN ('queued', 'running', 'cancelling')
                       OR (
                           r.status = 'interrupted'
                           AND r.error_code IN ('yuxi.connection_interrupted', 'yuxi.recovery_failed')
                       )
                   )
                 ORDER BY COALESCE(r.started_at, r.created_at) ASC",
            )?;
            let records = statement
                .query_map([], |row| {
                    Ok(RecoverableExternalRunRecord {
                        local_run_id: row.get(0)?,
                        conversation_id: row.get(1)?,
                        external_run_id: row.get(2)?,
                        external_cursor: row.get(3)?,
                        remote_thread_id: row.get(4)?,
                    })
                })?
                .collect();
            records
        })
    }

    pub fn add_attachments(&self, records: &[AttachmentRecord]) -> Result<(), String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            for item in records {
                transaction.execute(
                    "INSERT INTO attachments(id, conversation_id, message_id, display_name, storage_path,
                                             media_type, byte_size, sha256, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![item.id, item.conversation_id, item.message_id, item.display_name,
                            item.storage_path, item.media_type, item.byte_size, item.sha256,
                            item.status, item.created_at],
                )?;
            }
            transaction.commit()
        })
    }

    pub fn bind_attachments_to_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        attachment_ids: &[String],
    ) -> Result<Vec<AttachmentRecord>, String> {
        if attachment_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut records = Vec::with_capacity(attachment_ids.len());
            for attachment_id in attachment_ids {
                let record = transaction
                    .query_row(
                        "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                                byte_size, sha256, status, created_at
                         FROM attachments
                         WHERE id = ?1 AND conversation_id = ?2 AND status = 'ready'
                           AND (message_id IS NULL OR message_id = ?3)",
                        params![attachment_id, conversation_id, message_id],
                        map_attachment,
                    )
                    .optional()?;
                let Some(mut record) = record else {
                    return Err(rusqlite::Error::InvalidQuery);
                };
                transaction.execute(
                    "UPDATE attachments SET message_id = ?2 WHERE id = ?1",
                    params![attachment_id, message_id],
                )?;
                record.message_id = Some(message_id.to_owned());
                records.push(record);
            }
            transaction.commit()?;
            Ok(records)
        })
    }

    pub fn attachment_for_conversation(
        &self,
        conversation_id: &str,
        attachment_id: &str,
    ) -> Result<Option<AttachmentRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                            byte_size, sha256, status, created_at
                     FROM attachments
                     WHERE id = ?1 AND conversation_id = ?2 AND status = 'ready'",
                    params![attachment_id, conversation_id],
                    map_attachment,
                )
                .optional()
        })
    }

    pub fn set_knowledge_bindings(
        &self,
        conversation_id: &str,
        knowledge_bases: &[(String, String)],
    ) -> Result<Vec<KnowledgeBindingRecord>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "DELETE FROM knowledge_bindings WHERE conversation_id = ?1",
                [conversation_id],
            )?;
            for (id, name) in knowledge_bases {
                transaction.execute(
                    "INSERT INTO knowledge_bindings(conversation_id, service_connection_id,
                                                    knowledge_base_id, knowledge_base_name,
                                                    enabled, created_at, updated_at)
                     VALUES (?1, 'yuxi-primary', ?2, ?3, 1, ?4, ?4)",
                    params![conversation_id, id, name, now],
                )?;
            }
            let records = query_knowledge_bindings(&transaction, conversation_id)?;
            transaction.commit()?;
            Ok(records)
        })
    }

    pub fn conversation_project_root(&self, id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(p.root_path, c.project_root)
                 FROM conversations c
                 LEFT JOIN projects p ON p.id = c.project_id
                 WHERE c.id = ?1",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
        })
    }

    pub fn enabled_agent_skills(&self, agent_id: &str) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let value = connection
                .query_row(
                    "SELECT value_json FROM agent_runtime_config
                     WHERE agent_id = ?1 AND scope_type = 'agent' AND scope_id = ?1
                       AND config_key = 'skills.enabled'",
                    [agent_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            Ok(value
                .and_then(|json| serde_json::from_str::<Vec<String>>(&json).ok())
                .unwrap_or_default())
        })
    }

    pub fn set_agent_skill_enabled(
        &self,
        agent_id: &str,
        skill_id: &str,
        enabled: bool,
    ) -> Result<Vec<String>, String> {
        let mut skills = self.enabled_agent_skills(agent_id)?;
        if enabled {
            if !skills.iter().any(|item| item == skill_id) {
                skills.push(skill_id.to_owned());
            }
        } else {
            skills.retain(|item| item != skill_id);
        }
        skills.sort();
        skills.dedup();
        let value = serde_json::to_string(&skills).map_err(|error| error.to_string())?;
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO agent_runtime_config(
                    agent_id, scope_type, scope_id, config_key, value_json, source,
                    created_at, updated_at
                 ) VALUES (?1, 'agent', ?1, 'skills.enabled', ?2, 'user', ?3, ?3)
                 ON CONFLICT(agent_id, scope_type, scope_id, config_key) DO UPDATE SET
                    value_json = excluded.value_json,
                    source = 'user',
                    updated_at = excluded.updated_at",
                params![agent_id, value, now],
            )?;
            Ok(())
        })?;
        Ok(skills)
    }

    pub fn conversation_agent_id(&self, conversation_id: &str) -> Result<String, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT agent_id FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get(0),
            )
        })
    }

    pub fn list_mcp_servers(&self) -> Result<Vec<super::McpServerRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, command, args_json, enabled, status, last_error,
                        last_checked_at, created_at, updated_at
                 FROM mcp_servers ORDER BY name COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], |row| {
                let args_json: String = row.get(3)?;
                Ok(super::McpServerRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    command: row.get(2)?,
                    args: serde_json::from_str(&args_json).unwrap_or_default(),
                    enabled: row.get::<_, i64>(4)? != 0,
                    status: row.get(5)?,
                    credential_configured: false,
                    last_error: row.get(6)?,
                    last_checked_at: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn get_mcp_server(&self, id: &str) -> Result<Option<super::McpServerRecord>, String> {
        Ok(self
            .list_mcp_servers()?
            .into_iter()
            .find(|item| item.id == id))
    }

    pub fn save_mcp_server(
        &self,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
    ) -> Result<super::McpServerRecord, String> {
        let now = now_ms();
        let args_json = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO mcp_servers(id, name, command, args_json, enabled, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 1, 'unknown', ?5, ?5)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, command = excluded.command,
                    args_json = excluded.args_json, enabled = 1, status = 'unknown',
                    last_error = NULL, updated_at = excluded.updated_at",
                params![id, name, command, args_json, now],
            )?;
            Ok(())
        })?;
        self.get_mcp_server(id)?
            .ok_or_else(|| "saved MCP server could not be loaded".to_owned())
    }

    pub fn record_mcp_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE mcp_servers SET status = ?2, last_error = ?3,
                         last_checked_at = ?4, updated_at = ?4 WHERE id = ?1",
                params![id, status, error, now],
            )?;
            Ok(())
        })
    }

    pub fn set_mcp_server_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<super::McpServerRecord>, String> {
        let now = now_ms();
        let changed = self.with_connection(|connection| {
            connection.execute(
                "UPDATE mcp_servers SET enabled = ?2, status = CASE WHEN ?2 = 1 THEN 'unknown' ELSE 'disabled' END,
                         last_error = NULL, updated_at = ?3 WHERE id = ?1",
                params![id, enabled, now],
            )
        })?;
        if changed == 0 {
            return Ok(None);
        }
        self.get_mcp_server(id)
    }

    pub fn delete_mcp_server(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM mcp_servers WHERE id = ?1", [id])? > 0)
        })
    }

    pub fn conversation_project_access(
        &self,
        id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT COALESCE(p.root_path, c.project_root),
                            COALESCE(p.permission_mode, 'read_only')
                     FROM conversations c
                     LEFT JOIN projects p ON p.id = c.project_id
                     WHERE c.id = ?1
                       AND COALESCE(p.root_path, c.project_root) IS NOT NULL",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
    }

    pub fn conversation_system_prompt(&self, id: &str) -> Result<String, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT a.system_prompt FROM conversations c
                 JOIN agents a ON a.id = c.agent_id WHERE c.id = ?1",
                [id],
                |row| row.get(0),
            )
        })
    }

    pub fn next_run_seq(&self, run_id: &str) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT last_seq + 1 FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )
        })
    }

    pub fn create_host_tool_call(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        tool_name: &str,
        input: &Value,
        status: &str,
        requires_approval: bool,
    ) -> Result<super::ToolCallRecord, String> {
        let now = now_ms();
        let input_json = serde_json::to_string(input).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let conversation_id: String = connection.query_row(
                "SELECT conversation_id FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )?;
            let id = Uuid::new_v4().to_string();
            connection.execute(
                "INSERT INTO tool_calls(
                    id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                    status, execution_location, requires_approval, started_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'host', ?8, ?9, ?9)
                 ON CONFLICT(run_id, runtime_tool_call_id) DO UPDATE SET
                    input_json = excluded.input_json,
                    status = excluded.status,
                    execution_location = 'host',
                    requires_approval = excluded.requires_approval,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    runtime_tool_call_id,
                    run_id,
                    conversation_id,
                    tool_name,
                    input_json,
                    status,
                    requires_approval as i64,
                    now
                ],
            )?;
            query_tool_call(connection, run_id, runtime_tool_call_id)
        })
    }

    pub fn complete_host_tool_call(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        result: Option<&Value>,
        error_message: Option<&str>,
    ) -> Result<(), String> {
        let now = now_ms();
        let result_json = result
            .map(compact_tool_result)
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE tool_calls
                 SET status = ?3, result_json = ?4, error_message = ?5,
                     completed_at = ?6, updated_at = ?6
                 WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                params![
                    run_id,
                    runtime_tool_call_id,
                    if error_message.is_some() {
                        "failed"
                    } else {
                        "completed"
                    },
                    result_json,
                    error_message,
                    now
                ],
            )?;
            if error_message.is_none() {
                if let Some(result) = result {
                    let tool_name: Option<String> = transaction.query_row(
                        "SELECT tool_name FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                        params![run_id, runtime_tool_call_id],
                        |row| row.get(0),
                    ).optional()?;
                    if matches!(tool_name.as_deref(), Some("write_file") | Some("edit_file")) {
                        if let Some(path) = result.get("details").and_then(|details| details.get("path")).and_then(Value::as_str) {
                            let display_name = PathBuf::from(path).file_name().and_then(|value| value.to_str()).unwrap_or(path).to_owned();
                            let byte_size = result.get("details").and_then(|details| details.get("bytes")).and_then(Value::as_i64).unwrap_or(0);
                            transaction.execute(
                                "INSERT INTO artifacts(id, conversation_id, run_id, display_name,
                                                       artifact_type, storage_path, byte_size,
                                                       status, created_at, updated_at)
                                 SELECT ?1, conversation_id, run_id, ?2, 'file', ?3, ?4,
                                        'ready', ?5, ?5 FROM tool_calls
                                 WHERE run_id = ?6 AND runtime_tool_call_id = ?7",
                                params![Uuid::new_v4().to_string(), display_name, path, byte_size, now, run_id, runtime_tool_call_id],
                            )?;
                        }
                    }
                }
            }
            transaction.commit()
        })
    }

    pub fn create_approval(
        &self,
        tool_call_id: &str,
        requested_action: &str,
        request: &Value,
    ) -> Result<super::ApprovalRecord, String> {
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let request_json = serde_json::to_string(request).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO approvals(
                    id, tool_call_id, status, requested_action, request_json, requested_at
                 ) VALUES (?1, ?2, 'pending', ?3, ?4, ?5)
                 ON CONFLICT(tool_call_id) DO UPDATE SET
                    status = 'pending', requested_action = excluded.requested_action,
                    request_json = excluded.request_json, decision_json = NULL,
                    requested_at = excluded.requested_at, resolved_at = NULL",
                params![id, tool_call_id, requested_action, request_json, now],
            )?;
            query_approval_by_tool_call(connection, tool_call_id)
        })
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        approved: bool,
    ) -> Result<Option<super::ApprovalRecord>, String> {
        let now = now_ms();
        let decision = serde_json::to_string(&json!({ "approved": approved }))
            .map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE approvals
                 SET status = ?2, decision_json = ?3, resolved_at = ?4
                 WHERE id = ?1 AND status = 'pending'",
                params![
                    approval_id,
                    if approved { "approved" } else { "denied" },
                    decision,
                    now
                ],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_approval(connection, approval_id).optional()
        })
    }

    pub fn conversation_has_active_run(&self, conversation_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT 1 FROM runs
                     WHERE conversation_id = ?1 AND status IN ('queued', 'running', 'cancelling')
                     LIMIT 1",
                    [conversation_id],
                    |_| Ok(()),
                )
                .optional()
                .map(|value| value.is_some())
        })
    }

    pub fn delete_conversation(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM conversations WHERE id = ?1", [id])? > 0)
        })
    }

    pub fn create_run(
        &self,
        conversation_id: &str,
        text: &str,
        requested_model: Option<&str>,
    ) -> Result<StartRunResult, String> {
        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err("message text cannot be empty".to_owned());
        }

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let started = create_run_in_transaction(
                &transaction,
                conversation_id,
                clean_text,
                requested_model,
            )?;
            transaction.commit()?;
            Ok(started)
        })
    }

    pub fn create_resumed_run(
        &self,
        conversation_id: &str,
        parent_run_id: &str,
        text: &str,
    ) -> Result<StartRunResult, String> {
        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err("message text cannot be empty".to_owned());
        }

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let parent_exists = transaction
                .query_row(
                    "SELECT 1 FROM runs
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![parent_run_id, conversation_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if !parent_exists {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }

            let (latest_requested_seq, latest_responded_seq): (Option<i64>, Option<i64>) =
                transaction.query_row(
                    "SELECT
                       MAX(CASE WHEN event_type = 'user.question.requested' THEN seq END),
                       MAX(CASE WHEN event_type = 'user.question.responded' THEN seq END)
                     FROM run_events WHERE run_id = ?1",
                    [parent_run_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            if latest_requested_seq.is_none()
                || latest_responded_seq.is_some_and(|seq| seq >= latest_requested_seq.unwrap())
            {
                return Err(rusqlite::Error::InvalidQuery);
            }

            let started =
                create_run_in_transaction(&transaction, conversation_id, clean_text, None)?;
            let seq: i64 = transaction.query_row(
                "SELECT last_seq + 1 FROM runs WHERE id = ?1",
                [parent_run_id],
                |row| row.get(0),
            )?;
            let payload = json!({
                "type": "user.question.responded",
                "childRunId": started.run.id.clone(),
            });
            transaction.execute(
                "INSERT INTO run_events(id, run_id, seq, event_type, event_json, created_at)
                 VALUES (?1, ?2, ?3, 'user.question.responded', ?4, ?5)",
                params![
                    Uuid::new_v4().to_string(),
                    parent_run_id,
                    seq,
                    serde_json::to_string(&payload).map_err(|_| rusqlite::Error::InvalidQuery)?,
                    now_ms()
                ],
            )?;
            transaction.execute(
                "UPDATE runs SET last_seq = ?2 WHERE id = ?1",
                params![parent_run_id, seq],
            )?;
            transaction.commit()?;
            Ok(started)
        })
    }

    pub fn runtime_prompt_context(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<RuntimePromptMessage>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT role, content FROM (
                    SELECT role, content, ordinal FROM messages
                    WHERE conversation_id = ?1
                      AND role IN ('user', 'assistant')
                      AND kind = 'text'
                      AND status IN ('completed', 'streaming')
                    ORDER BY ordinal DESC LIMIT ?2
                 ) ORDER BY ordinal ASC",
            )?;
            let messages = statement
                .query_map(params![conversation_id, limit as i64], |row| {
                    Ok(RuntimePromptMessage {
                        role: row.get(0)?,
                        content: row.get(1)?,
                    })
                })?
                .collect();
            messages
        })
    }

    pub fn runtime_prompt_context_budget(
        &self,
        conversation_id: &str,
        max_messages: usize,
        max_chars: usize,
    ) -> Result<Vec<RuntimePromptMessage>, String> {
        let candidates = self.runtime_prompt_context(conversation_id, max_messages.max(1))?;
        let mut selected = Vec::new();
        let mut used = 0usize;
        for message in candidates.into_iter().rev() {
            let cost = message.content.chars().count() + message.role.chars().count() + 16;
            if !selected.is_empty() && used.saturating_add(cost) > max_chars {
                break;
            }
            used = used.saturating_add(cost);
            selected.push(message);
        }
        selected.reverse();
        Ok(selected)
    }

    pub fn ensure_runtime_session(
        &self,
        conversation_id: &str,
        runtime_session_id: &str,
        runtime_version: Option<&str>,
        session_path: Option<&str>,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO runtime_sessions(
                    id, conversation_id, runtime_type, runtime_session_id, runtime_version,
                    session_path, status, created_at, updated_at
                 ) VALUES (?1, ?2, 'pi', ?1, ?3, ?4, 'ready', ?5, ?5)
                 ON CONFLICT(conversation_id, runtime_type) DO UPDATE SET
                    runtime_session_id = excluded.runtime_session_id,
                    runtime_version = excluded.runtime_version,
                    session_path = excluded.session_path,
                    status = 'ready',
                    updated_at = excluded.updated_at",
                params![
                    runtime_session_id,
                    conversation_id,
                    runtime_version,
                    session_path,
                    now
                ],
            )?;
            connection.execute(
                "UPDATE runs SET runtime_session_id = ?2 WHERE conversation_id = ?1 AND status = 'queued'",
                params![conversation_id, runtime_session_id],
            )?;
            Ok(())
        })
    }

    pub fn get_runtime_session(
        &self,
        conversation_id: &str,
    ) -> Result<Option<RuntimeSessionRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT runtime_session_id, runtime_version, session_path, status
                     FROM runtime_sessions
                     WHERE conversation_id = ?1 AND runtime_type = 'pi'",
                    [conversation_id],
                    |row| {
                        Ok(RuntimeSessionRecord {
                            runtime_session_id: row.get(0)?,
                            runtime_version: row.get(1)?,
                            session_path: row.get(2)?,
                            status: row.get(3)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn mark_runtime_session_unavailable(&self, conversation_id: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runtime_sessions SET status = 'unavailable', updated_at = ?2
                 WHERE conversation_id = ?1 AND runtime_type = 'pi'",
                params![conversation_id, now],
            )?;
            Ok(())
        })
    }

    pub fn mark_run_cancelling(&self, run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            let conversation_id = connection
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1 AND status IN ('queued', 'running')",
                    [run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if conversation_id.is_some() {
                let updated = connection.execute(
                    "UPDATE runs SET status = 'cancelling'
                     WHERE id = ?1 AND status IN ('queued', 'running')",
                    [run_id],
                )?;
                if updated == 0 {
                    return Ok(None);
                }
            }
            Ok(conversation_id)
        })
    }

    pub fn mark_run_failed(&self, run_id: &str, code: &str, message: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs SET status = 'failed', finished_at = ?2, error_code = ?3, error_message = ?4
                 WHERE id = ?1",
                params![run_id, now, code, message],
            )?;
            connection.execute(
                "UPDATE tool_calls
                 SET status = 'interrupted', error_message = ?2, completed_at = ?3, updated_at = ?3
                 WHERE run_id = ?1 AND status IN ('pending', 'running')",
                params![run_id, message, now],
            )?;
            Ok(())
        })
    }

    pub fn mark_run_interrupted(
        &self,
        run_id: &str,
        code: &str,
        message: &str,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs SET status = 'interrupted', finished_at = ?2,
                                 error_code = ?3, error_message = ?4
                 WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                params![run_id, now, code, message],
            )?;
            connection.execute(
                "UPDATE messages SET status = 'interrupted', updated_at = ?2
                 WHERE run_id = ?1 AND status = 'streaming'",
                params![run_id, now],
            )?;
            connection.execute(
                "UPDATE tool_calls
                 SET status = 'interrupted', error_message = ?2, completed_at = ?3, updated_at = ?3
                 WHERE run_id = ?1 AND status IN ('pending', 'running')",
                params![run_id, message, now],
            )?;
            Ok(())
        })
    }

    pub fn get_yuxi_service(&self) -> Result<Option<YuxiServiceRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT name, base_url, enabled, last_status, last_version, last_latency_ms,
                            last_checked_at, created_at, updated_at
                     FROM yuxi_service WHERE singleton_id = 1",
                    [],
                    |row| {
                        let base_url: String = row.get(1)?;
                        Ok(YuxiServiceRecord {
                            name: row.get(0)?,
                            connection_type: connection_type(&base_url),
                            base_url,
                            enabled: row.get::<_, i64>(2)? != 0,
                            credential_configured: false,
                            last_status: row.get(3)?,
                            last_version: row.get(4)?,
                            last_latency_ms: row.get(5)?,
                            last_checked_at: row.get(6)?,
                            created_at: row.get(7)?,
                            updated_at: row.get(8)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn save_yuxi_service(
        &self,
        name: &str,
        base_url: &str,
    ) -> Result<YuxiServiceRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO yuxi_service(singleton_id, name, base_url, enabled, created_at, updated_at)
                 VALUES (1, ?1, ?2, 1, ?3, ?3)
                 ON CONFLICT(singleton_id) DO UPDATE SET
                    name = excluded.name,
                    base_url = excluded.base_url,
                    enabled = 1,
                    last_status = CASE WHEN yuxi_service.base_url = excluded.base_url THEN yuxi_service.last_status ELSE 'unknown' END,
                    last_version = CASE WHEN yuxi_service.base_url = excluded.base_url THEN yuxi_service.last_version ELSE NULL END,
                    last_latency_ms = CASE WHEN yuxi_service.base_url = excluded.base_url THEN yuxi_service.last_latency_ms ELSE NULL END,
                    last_checked_at = CASE WHEN yuxi_service.base_url = excluded.base_url THEN yuxi_service.last_checked_at ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![name, base_url, now],
            )?;
            transaction.execute(
                "INSERT INTO service_connections(
                    id, service_type, name, base_url, enabled, last_status, created_at, updated_at
                 ) VALUES ('yuxi-primary', 'yuxi', ?1, ?2, 1, 'unknown', ?3, ?3)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    base_url = excluded.base_url,
                    enabled = 1,
                    last_status = CASE
                        WHEN service_connections.base_url = excluded.base_url
                        THEN service_connections.last_status ELSE 'unknown' END,
                    last_version = CASE
                        WHEN service_connections.base_url = excluded.base_url
                        THEN service_connections.last_version ELSE NULL END,
                    last_latency_ms = CASE
                        WHEN service_connections.base_url = excluded.base_url
                        THEN service_connections.last_latency_ms ELSE NULL END,
                    last_checked_at = CASE
                        WHEN service_connections.base_url = excluded.base_url
                        THEN service_connections.last_checked_at ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![name, base_url, now],
            )?;
            transaction.commit()?;
            Ok(())
        })?;
        self.get_yuxi_service()?
            .ok_or_else(|| "saved Yuxi service could not be loaded".to_owned())
    }

    pub fn record_yuxi_connection_test(&self, result: &YuxiConnectionTest) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE yuxi_service SET last_status = ?1, last_version = ?2,
                         last_latency_ms = ?3, last_checked_at = ?4, updated_at = ?4
                 WHERE singleton_id = 1 AND base_url = ?5",
                params![
                    result.status,
                    result.version,
                    result.latency_ms,
                    now,
                    result.base_url
                ],
            )?;
            transaction.execute(
                "UPDATE service_connections
                 SET last_status = ?1, last_version = ?2, last_latency_ms = ?3,
                     last_checked_at = ?4, updated_at = ?4
                 WHERE id = 'yuxi-primary' AND base_url = ?5",
                params![
                    result.status,
                    result.version,
                    result.latency_ms,
                    now,
                    result.base_url
                ],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn record_yuxi_connection_failure(&self, base_url: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE yuxi_service SET last_status = 'unavailable', last_version = NULL,
                         last_latency_ms = NULL, last_checked_at = ?1, updated_at = ?1
                 WHERE singleton_id = 1 AND base_url = ?2",
                params![now, base_url],
            )?;
            transaction.execute(
                "UPDATE service_connections
                 SET last_status = 'unavailable', last_version = NULL, last_latency_ms = NULL,
                     last_checked_at = ?1, updated_at = ?1
                 WHERE id = 'yuxi-primary' AND base_url = ?2",
                params![now, base_url],
            )?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn get_model_service(&self) -> Result<Option<ModelServiceRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT name, base_url, model_id, api_type, context_window,
                            max_output_tokens, supports_image_input, enabled, last_status, last_latency_ms,
                            last_checked_at, created_at, updated_at
                     FROM model_service WHERE singleton_id = 1",
                    [],
                    |row| {
                        let base_url: String = row.get(1)?;
                        Ok(ModelServiceRecord {
                            name: row.get(0)?,
                            connection_type: connection_type(&base_url),
                            base_url,
                            model_id: row.get(2)?,
                            api_type: row.get(3)?,
                            context_window: row.get(4)?,
                            max_output_tokens: row.get(5)?,
                            supports_image_input: row.get::<_, i64>(6)? != 0,
                            enabled: row.get::<_, i64>(7)? != 0,
                            credential_configured: false,
                            last_status: row.get(8)?,
                            last_latency_ms: row.get(9)?,
                            last_checked_at: row.get(10)?,
                            created_at: row.get(11)?,
                            updated_at: row.get(12)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn save_model_service(
        &self,
        service: &ModelServiceRecord,
    ) -> Result<ModelServiceRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO model_service(
                    singleton_id, name, base_url, model_id, api_type, context_window,
                    max_output_tokens, supports_image_input, enabled, created_at, updated_at
                 ) VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)
                 ON CONFLICT(singleton_id) DO UPDATE SET
                    name = excluded.name,
                    base_url = excluded.base_url,
                    model_id = excluded.model_id,
                    api_type = excluded.api_type,
                    context_window = excluded.context_window,
                    max_output_tokens = excluded.max_output_tokens,
                    supports_image_input = excluded.supports_image_input,
                    enabled = 1,
                    last_status = CASE WHEN model_service.base_url = excluded.base_url THEN model_service.last_status ELSE 'unknown' END,
                    last_latency_ms = CASE WHEN model_service.base_url = excluded.base_url THEN model_service.last_latency_ms ELSE NULL END,
                    last_checked_at = CASE WHEN model_service.base_url = excluded.base_url THEN model_service.last_checked_at ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![
                    service.name,
                    service.base_url,
                    service.model_id,
                    service.api_type,
                    service.context_window,
                    service.max_output_tokens,
                    service.supports_image_input as i64,
                    now,
                ],
            )?;
            Ok(())
        })?;
        self.get_model_service()?
            .ok_or_else(|| "saved model service could not be loaded".to_owned())
    }

    pub fn list_model_providers(&self) -> Result<Vec<ModelProviderRecord>, String> {
        self.with_connection(|connection| {
            let mut provider_statement = connection.prepare(
                "SELECT id, name, base_url, api_type, enabled, is_default, last_status,
                        last_latency_ms, last_checked_at, created_at, updated_at
                 FROM model_providers ORDER BY is_default DESC, created_at ASC",
            )?;
            let providers = provider_statement
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let base_url: String = row.get(2)?;
                    let mut model_statement = connection.prepare(
                        "SELECT id, model_id, display_name, context_window, max_output_tokens,
                                supports_image_input, is_default
                         FROM provider_models WHERE provider_id = ?1
                         ORDER BY is_default DESC, created_at ASC",
                    )?;
                    let models = model_statement
                        .query_map([&id], |model| {
                            Ok(ProviderModelRecord {
                                id: model.get(0)?,
                                model_id: model.get(1)?,
                                display_name: model.get(2)?,
                                context_window: model.get(3)?,
                                max_output_tokens: model.get(4)?,
                                supports_image_input: model.get::<_, i64>(5)? != 0,
                                is_default: model.get::<_, i64>(6)? != 0,
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(ModelProviderRecord {
                        id,
                        name: row.get(1)?,
                        connection_type: connection_type(&base_url),
                        base_url,
                        api_type: row.get(3)?,
                        enabled: row.get::<_, i64>(4)? != 0,
                        is_default: row.get::<_, i64>(5)? != 0,
                        credential_configured: false,
                        last_status: row.get(6)?,
                        last_latency_ms: row.get(7)?,
                        last_checked_at: row.get(8)?,
                        models,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                })?
                .collect();
            providers
        })
    }

    pub fn save_model_provider(
        &self,
        provider: &ModelProviderRecord,
    ) -> Result<ModelProviderRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if provider.is_default {
                transaction.execute("UPDATE model_providers SET is_default = 0", [])?;
            }
            transaction.execute(
                "INSERT INTO model_providers(
                    id, name, base_url, api_type, enabled, is_default, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name, base_url = excluded.base_url,
                    api_type = excluded.api_type, enabled = 1,
                    is_default = excluded.is_default,
                    last_status = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_status ELSE 'unknown' END,
                    last_latency_ms = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_latency_ms ELSE NULL END,
                    last_checked_at = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_checked_at ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![provider.id, provider.name, provider.base_url, provider.api_type, provider.is_default as i64, now],
            )?;
            transaction.execute("DELETE FROM provider_models WHERE provider_id = ?1", [&provider.id])?;
            for model in &provider.models {
                transaction.execute(
                    "INSERT INTO provider_models(
                        id, provider_id, model_id, display_name, context_window,
                        max_output_tokens, supports_image_input, is_default, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![model.id, provider.id, model.model_id, model.display_name,
                        model.context_window, model.max_output_tokens,
                        model.supports_image_input as i64, model.is_default as i64, now],
                )?;
            }
            transaction.commit()?;
            Ok(())
        })?;
        self.list_model_providers()?
            .into_iter()
            .find(|item| item.id == provider.id)
            .ok_or_else(|| "saved model provider could not be loaded".to_owned())
    }

    pub fn delete_model_provider(&self, provider_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM model_providers WHERE id = ?1", [provider_id])? > 0)
        })
    }

    pub fn record_model_connection_test(&self, result: &ModelConnectionTest) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE model_service SET last_status = ?1, last_latency_ms = ?2,
                         last_checked_at = ?3, updated_at = ?3
                 WHERE singleton_id = 1 AND base_url = ?4",
                params![result.status, result.latency_ms, now, result.base_url],
            )?;
            connection.execute(
                "UPDATE model_providers SET last_status = ?1, last_latency_ms = ?2,
                         last_checked_at = ?3, updated_at = ?3 WHERE base_url = ?4",
                params![result.status, result.latency_ms, now, result.base_url],
            )?;
            Ok(())
        })
    }

    pub fn record_model_connection_failure(&self, base_url: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE model_service SET last_status = 'unavailable', last_latency_ms = NULL,
                         last_checked_at = ?1, updated_at = ?1
                 WHERE singleton_id = 1 AND base_url = ?2",
                params![now, base_url],
            )?;
            connection.execute(
                "UPDATE model_providers SET last_status = 'unavailable', last_latency_ms = NULL,
                         last_checked_at = ?1, updated_at = ?1 WHERE base_url = ?2",
                params![now, base_url],
            )?;
            Ok(())
        })
    }

    pub fn apply_runtime_event(
        &self,
        run_id: &str,
        seq: i64,
        payload: &Value,
    ) -> Result<bool, String> {
        let event_type = payload
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| "runtime event type is missing".to_owned())?;
        let event_json = serde_json::to_string(payload).map_err(|error| error.to_string())?;
        let now = now_ms();

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let last_seq: i64 = transaction.query_row(
                "SELECT last_seq FROM runs WHERE id = ?1",
                [run_id],
                |row| row.get(0),
            )?;
            if seq <= last_seq {
                transaction.rollback()?;
                return Ok(false);
            }
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO run_events(id, run_id, seq, event_type, event_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![Uuid::new_v4().to_string(), run_id, seq, event_type, event_json, now],
            )?;
            if inserted == 0 {
                transaction.rollback()?;
                return Ok(false);
            }

            let assistant_message_id = format!("assistant-{run_id}");
            match event_type {
                "run.started" => {
                    transaction.execute(
                        "UPDATE runs SET status = 'running', started_at = COALESCE(started_at, ?2),
                                         finished_at = NULL, error_code = NULL, error_message = NULL,
                                         last_seq = ?3
                         WHERE id = ?1",
                        params![run_id, now, seq],
                    )?;
                }
                "message.started" => {
                    let (conversation_id, ordinal): (String, i64) = transaction.query_row(
                        "SELECT conversation_id,
                                (SELECT COALESCE(MAX(ordinal), 0) + 1 FROM messages WHERE conversation_id = runs.conversation_id)
                         FROM runs WHERE id = ?1",
                        [run_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    transaction.execute(
                        "INSERT OR IGNORE INTO messages(
                            id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, 'assistant', 'text', '', 'streaming', ?4, ?5, ?5)",
                        params![assistant_message_id, conversation_id, run_id, ordinal, now],
                    )?;
                    transaction.execute(
                        "UPDATE messages SET status = 'streaming', updated_at = ?2 WHERE id = ?1",
                        params![assistant_message_id, now],
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "message.delta" => {
                    let delta = payload.get("delta").and_then(Value::as_str).unwrap_or_default();
                    transaction.execute(
                        "UPDATE messages SET content = content || ?2, updated_at = ?3 WHERE id = ?1",
                        params![assistant_message_id, delta, now],
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "message.completed" => {
                    transaction.execute(
                        "UPDATE messages SET status = 'completed', updated_at = ?2 WHERE id = ?1",
                        params![assistant_message_id, now],
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.started" => {
                    project_tool_started(&transaction, run_id, payload, now)?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.updated" => {
                    project_tool_updated(&transaction, run_id, payload, now)?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.completed" => {
                    project_tool_completed(&transaction, run_id, payload, now)?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "run.completed" => {
                    transaction.execute(
                        "UPDATE runs SET status = 'completed', finished_at = ?2,
                                         error_code = NULL, error_message = NULL, last_seq = ?3
                         WHERE id = ?1",
                        params![run_id, now, seq],
                    )?;
                    transaction.execute(
                        "UPDATE messages SET status = 'completed', updated_at = ?2
                         WHERE id = ?1 AND status IN ('streaming', 'interrupted')",
                        params![assistant_message_id, now],
                    )?;
                }
                "run.cancelled" => {
                    transaction.execute(
                        "UPDATE runs SET status = 'cancelled', finished_at = ?2, last_seq = ?3 WHERE id = ?1",
                        params![run_id, now, seq],
                    )?;
                    transaction.execute(
                        "UPDATE messages SET status = 'interrupted', updated_at = ?2
                         WHERE id = ?1 AND status = 'streaming'",
                        params![assistant_message_id, now],
                    )?;
                    transaction.execute(
                        "UPDATE tool_calls
                         SET status = 'cancelled', completed_at = ?2, updated_at = ?2
                         WHERE run_id = ?1 AND status IN ('pending', 'running')",
                        params![run_id, now],
                    )?;
                }
                "run.interrupted" => {
                    let code = payload.get("code").and_then(Value::as_str);
                    let message = payload.get("message").and_then(Value::as_str);
                    transaction.execute(
                        "UPDATE runs SET status = 'interrupted', finished_at = ?2,
                                         error_code = ?3, error_message = ?4, last_seq = ?5
                         WHERE id = ?1",
                        params![run_id, now, code, message, seq],
                    )?;
                    transaction.execute(
                        "UPDATE messages SET status = 'interrupted', updated_at = ?2
                         WHERE id = ?1 AND status = 'streaming'",
                        params![assistant_message_id, now],
                    )?;
                    transaction.execute(
                        "UPDATE tool_calls SET status = 'interrupted', error_message = ?2,
                                               completed_at = ?3, updated_at = ?3
                         WHERE run_id = ?1 AND status IN ('pending', 'running')",
                        params![run_id, message, now],
                    )?;
                }
                "run.failed" => {
                    let code = payload.get("code").and_then(Value::as_str);
                    let message = payload.get("message").and_then(Value::as_str);
                    transaction.execute(
                        "UPDATE runs SET status = 'failed', finished_at = ?2, error_code = ?3,
                                         error_message = ?4, last_seq = ?5 WHERE id = ?1",
                        params![run_id, now, code, message, seq],
                    )?;
                    transaction.execute(
                        "UPDATE messages SET status = 'interrupted', updated_at = ?2
                         WHERE id = ?1 AND status = 'streaming'",
                        params![assistant_message_id, now],
                    )?;
                    transaction.execute(
                        "UPDATE tool_calls
                         SET status = 'interrupted', error_message = ?2,
                             completed_at = ?3, updated_at = ?3
                         WHERE run_id = ?1 AND status IN ('pending', 'running')",
                        params![run_id, message, now],
                    )?;
                }
                _ => update_last_seq(&transaction, run_id, seq)?,
            }

            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn record_external_event(
        &self,
        run_id: &str,
        payload: &Value,
    ) -> Result<Option<i64>, String> {
        let seq = self.next_run_seq(run_id)?;
        if self.apply_runtime_event(run_id, seq, payload)? {
            Ok(Some(seq))
        } else {
            Ok(None)
        }
    }
}

fn create_run_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    clean_text: &str,
    requested_model: Option<&str>,
) -> rusqlite::Result<StartRunResult> {
    let run_id = Uuid::new_v4().to_string();
    let message_id = Uuid::new_v4().to_string();
    let now = now_ms();
    let (model, current_title): (String, String) = transaction.query_row(
        "SELECT CASE WHEN a.runtime_type = 'yuxi'
                    THEN COALESCE(?2, '')
                    ELSE COALESCE(?2,
                                  (SELECT model_id FROM model_service WHERE singleton_id = 1 AND enabled = 1),
                                  a.default_model)
                END,
                c.title
         FROM conversations c JOIN agents a ON a.id = c.agent_id
         WHERE c.id = ?1",
        params![conversation_id, requested_model],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let active_run = transaction
        .query_row(
            "SELECT id FROM runs
             WHERE conversation_id = ?1 AND status IN ('queued', 'running', 'cancelling')
             LIMIT 1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if active_run.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }

    let ordinal: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(ordinal), 0) + 1 FROM messages WHERE conversation_id = ?1",
        [conversation_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO runs(id, conversation_id, status, model, created_at)
         VALUES (?1, ?2, 'queued', ?3, ?4)",
        params![run_id, conversation_id, model, now],
    )?;
    transaction.execute(
        "INSERT INTO messages(
            id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at
         ) VALUES (?1, ?2, ?3, 'user', 'text', ?4, 'completed', ?5, ?6, ?6)",
        params![message_id, conversation_id, run_id, clean_text, ordinal, now],
    )?;
    let next_title = if current_title == "新对话" {
        truncate_title(clean_text)
    } else {
        current_title
    };
    transaction.execute(
        "UPDATE conversations
         SET title = ?2, updated_at = ?3, last_message_at = ?3
         WHERE id = ?1",
        params![conversation_id, next_title, now],
    )?;

    Ok(StartRunResult {
        run: RunRecord {
            id: run_id.clone(),
            conversation_id: conversation_id.to_owned(),
            runtime_session_id: None,
            status: "queued".to_owned(),
            model,
            started_at: None,
            finished_at: None,
            error_code: None,
            error_message: None,
            last_seq: 0,
            trace_id: None,
            root_span_id: None,
        },
        user_message: MessageRecord {
            id: message_id,
            conversation_id: conversation_id.to_owned(),
            run_id: Some(run_id),
            role: "user".to_owned(),
            kind: "text".to_owned(),
            content: clean_text.to_owned(),
            status: "completed".to_owned(),
            ordinal,
            created_at: now,
            updated_at: now,
        },
        attachments: Vec::new(),
    })
}

fn map_conversation(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConversationSummary> {
    Ok(ConversationSummary {
        id: row.get(0)?,
        agent_id: row.get(1)?,
        agent_name: row.get(2)?,
        title: row.get(3)?,
        project_id: row.get(4)?,
        project_root: row.get(5)?,
        status: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        last_message_at: row.get(9)?,
    })
}

fn query_conversation(connection: &Connection, id: &str) -> rusqlite::Result<ConversationSummary> {
    connection.query_row(
        "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                COALESCE(p.root_path, c.project_root), c.status,
                c.created_at, c.updated_at, c.last_message_at
         FROM conversations c
         JOIN agents a ON a.id = c.agent_id
         LEFT JOIN projects p ON p.id = c.project_id
         WHERE c.id = ?1",
        [id],
        map_conversation,
    )
}

fn query_message_page(
    connection: &Connection,
    id: &str,
    before_ordinal: Option<i64>,
    limit: usize,
) -> rusqlite::Result<Vec<MessageRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at
         FROM (
           SELECT id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at
           FROM messages
           WHERE conversation_id = ?1 AND (?2 IS NULL OR ordinal < ?2)
           ORDER BY ordinal DESC LIMIT ?3
         ) ORDER BY ordinal ASC",
    )?;
    let messages = statement
        .query_map(params![id, before_ordinal, limit as i64], |row| {
            Ok(MessageRecord {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                run_id: row.get(2)?,
                role: row.get(3)?,
                kind: row.get(4)?,
                content: row.get(5)?,
                status: row.get(6)?,
                ordinal: row.get(7)?,
                created_at: row.get(8)?,
                updated_at: row.get(9)?,
            })
        })?
        .collect();
    messages
}

fn query_runtime_events_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<RunEventRecord>> {
    let mut statement = connection.prepare(
        "SELECT e.run_id, e.seq, e.event_type, e.event_json, e.created_at,
                e.trace_id, e.span_id
         FROM run_events e
         JOIN runs r ON r.id = e.run_id
         WHERE r.conversation_id = ?1
           AND EXISTS(SELECT 1 FROM messages m WHERE m.run_id = r.id AND m.role = 'user'
                      AND m.ordinal >= ?2 AND (?3 IS NULL OR m.ordinal < ?3))
           AND e.event_type IN (
               'run.started', 'message.started', 'reasoning.delta',
               'tool.started', 'tool.updated', 'tool.completed',
               'user.question.requested', 'user.question.responded',
               'source.added', 'usage.updated', 'message.completed',
               'run.completed', 'run.cancelled', 'run.failed', 'run.interrupted'
           )
         ORDER BY r.created_at ASC, e.seq ASC",
    )?;
    let events = statement
        .query_map(
            params![conversation_id, from_ordinal, before_ordinal],
            |row| {
                let event_json: String = row.get(3)?;
                Ok(RunEventRecord {
                    run_id: row.get(0)?,
                    seq: row.get(1)?,
                    event_type: row.get(2)?,
                    event: serde_json::from_str(&event_json).unwrap_or_else(|_| {
                        serde_json::json!({
                            "type": "runtime.invalid_persisted_event"
                        })
                    }),
                    created_at: row.get(4)?,
                    trace_id: row.get(5)?,
                    span_id: row.get(6)?,
                })
            },
        )?
        .collect();
    events
}

fn query_tool_calls_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<ToolCallRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                status, result_json, error_message, execution_location, requires_approval,
                started_at, completed_at, updated_at, trace_id, span_id
         FROM tool_calls
         WHERE conversation_id = ?1
           AND EXISTS(SELECT 1 FROM messages m WHERE m.run_id = tool_calls.run_id AND m.role = 'user'
                      AND m.ordinal >= ?2 AND (?3 IS NULL OR m.ordinal < ?3))
         ORDER BY started_at ASC, rowid ASC",
    )?;
    let records = statement
        .query_map(
            params![conversation_id, from_ordinal, before_ordinal],
            |row| {
                let input_json: String = row.get(5)?;
                let result_json: Option<String> = row.get(7)?;
                Ok(ToolCallRecord {
                    id: row.get(0)?,
                    runtime_tool_call_id: row.get(1)?,
                    run_id: row.get(2)?,
                    conversation_id: row.get(3)?,
                    tool_name: row.get(4)?,
                    input: parse_json(&input_json),
                    status: row.get(6)?,
                    result: result_json.as_deref().map(parse_json),
                    error_message: row.get(8)?,
                    execution_location: row.get(9)?,
                    requires_approval: row.get::<_, i64>(10)? != 0,
                    started_at: row.get(11)?,
                    completed_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    trace_id: row.get(14)?,
                    span_id: row.get(15)?,
                })
            },
        )?
        .collect();
    records
}

fn query_tool_call(
    connection: &Connection,
    run_id: &str,
    runtime_tool_call_id: &str,
) -> rusqlite::Result<ToolCallRecord> {
    connection.query_row(
        "SELECT id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                status, result_json, error_message, execution_location, requires_approval,
                started_at, completed_at, updated_at, trace_id, span_id
         FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
        params![run_id, runtime_tool_call_id],
        map_tool_call,
    )
}

fn map_tool_call(row: &rusqlite::Row<'_>) -> rusqlite::Result<ToolCallRecord> {
    let input_json: String = row.get(5)?;
    let result_json: Option<String> = row.get(7)?;
    Ok(ToolCallRecord {
        id: row.get(0)?,
        runtime_tool_call_id: row.get(1)?,
        run_id: row.get(2)?,
        conversation_id: row.get(3)?,
        tool_name: row.get(4)?,
        input: parse_json(&input_json),
        status: row.get(6)?,
        result: result_json.as_deref().map(parse_json),
        error_message: row.get(8)?,
        execution_location: row.get(9)?,
        requires_approval: row.get::<_, i64>(10)? != 0,
        started_at: row.get(11)?,
        completed_at: row.get(12)?,
        updated_at: row.get(13)?,
        trace_id: row.get(14)?,
        span_id: row.get(15)?,
    })
}

fn query_approvals_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<super::ApprovalRecord>> {
    let mut statement = connection.prepare(
        "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                a.status, a.requested_action, a.request_json, a.decision_json,
                a.requested_at, a.resolved_at
         FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
         WHERE t.conversation_id = ?1
           AND EXISTS(SELECT 1 FROM messages m WHERE m.run_id = t.run_id AND m.role = 'user'
                      AND m.ordinal >= ?2 AND (?3 IS NULL OR m.ordinal < ?3))
         ORDER BY a.requested_at ASC",
    )?;
    let records = statement
        .query_map(
            params![conversation_id, from_ordinal, before_ordinal],
            map_approval,
        )?
        .collect();
    records
}

fn query_attachments_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<AttachmentRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                byte_size, sha256, status, created_at
         FROM attachments
         WHERE conversation_id = ?1
           AND (message_id IS NULL OR EXISTS(
             SELECT 1 FROM messages m WHERE m.id = attachments.message_id
               AND m.ordinal >= ?2 AND (?3 IS NULL OR m.ordinal < ?3)
           ))
         ORDER BY created_at ASC",
    )?;
    let records = statement
        .query_map(
            params![conversation_id, from_ordinal, before_ordinal],
            map_attachment,
        )?
        .collect();
    records
}

fn map_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttachmentRecord> {
    Ok(AttachmentRecord {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        message_id: row.get(2)?,
        display_name: row.get(3)?,
        storage_path: row.get(4)?,
        media_type: row.get(5)?,
        byte_size: row.get(6)?,
        sha256: row.get(7)?,
        status: row.get(8)?,
        created_at: row.get(9)?,
    })
}

fn query_artifacts_window(
    connection: &Connection,
    conversation_id: &str,
    from_ordinal: i64,
    before_ordinal: Option<i64>,
) -> rusqlite::Result<Vec<ArtifactRecord>> {
    let mut statement = connection.prepare(
        "SELECT id, conversation_id, run_id, display_name, artifact_type, storage_path,
                media_type, byte_size, sha256, status, created_at, updated_at
         FROM artifacts WHERE conversation_id = ?1
           AND (run_id IS NULL OR EXISTS(
             SELECT 1 FROM messages m WHERE m.run_id = artifacts.run_id AND m.role = 'user'
               AND m.ordinal >= ?2 AND (?3 IS NULL OR m.ordinal < ?3)
           ))
         ORDER BY created_at ASC",
    )?;
    let records = statement
        .query_map(
            params![conversation_id, from_ordinal, before_ordinal],
            |row| {
                Ok(ArtifactRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    run_id: row.get(2)?,
                    display_name: row.get(3)?,
                    artifact_type: row.get(4)?,
                    storage_path: row.get(5)?,
                    media_type: row.get(6)?,
                    byte_size: row.get(7)?,
                    sha256: row.get(8)?,
                    status: row.get(9)?,
                    created_at: row.get(10)?,
                    updated_at: row.get(11)?,
                })
            },
        )?
        .collect();
    records
}

fn query_knowledge_bindings(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<KnowledgeBindingRecord>> {
    let mut statement = connection.prepare(
        "SELECT conversation_id, service_connection_id, knowledge_base_id,
                knowledge_base_name, enabled, created_at, updated_at
         FROM knowledge_bindings WHERE conversation_id = ?1 AND enabled = 1
         ORDER BY knowledge_base_name ASC",
    )?;
    let records = statement
        .query_map([conversation_id], |row| {
            Ok(KnowledgeBindingRecord {
                conversation_id: row.get(0)?,
                service_connection_id: row.get(1)?,
                knowledge_base_id: row.get(2)?,
                knowledge_base_name: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect();
    records
}

fn query_approval_by_tool_call(
    connection: &Connection,
    tool_call_id: &str,
) -> rusqlite::Result<super::ApprovalRecord> {
    connection.query_row(
        "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                a.status, a.requested_action, a.request_json, a.decision_json,
                a.requested_at, a.resolved_at
         FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
         WHERE a.tool_call_id = ?1",
        [tool_call_id],
        map_approval,
    )
}

fn query_approval(
    connection: &Connection,
    approval_id: &str,
) -> rusqlite::Result<super::ApprovalRecord> {
    connection.query_row(
        "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                a.status, a.requested_action, a.request_json, a.decision_json,
                a.requested_at, a.resolved_at
         FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
         WHERE a.id = ?1",
        [approval_id],
        map_approval,
    )
}

fn map_approval(row: &rusqlite::Row<'_>) -> rusqlite::Result<super::ApprovalRecord> {
    let request_json: String = row.get(7)?;
    let decision_json: Option<String> = row.get(8)?;
    Ok(super::ApprovalRecord {
        id: row.get(0)?,
        tool_call_id: row.get(1)?,
        run_id: row.get(2)?,
        conversation_id: row.get(3)?,
        tool_name: row.get(4)?,
        status: row.get(5)?,
        requested_action: row.get(6)?,
        request: parse_json(&request_json),
        decision: decision_json.as_deref().map(parse_json),
        requested_at: row.get(9)?,
        resolved_at: row.get(10)?,
    })
}

fn upsert_project(
    connection: &Connection,
    root_path: &str,
    permission_mode: &str,
    now: i64,
) -> rusqlite::Result<String> {
    if let Some(id) = connection
        .query_row(
            "SELECT id FROM projects WHERE root_path = ?1",
            [root_path],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        connection.execute(
            "UPDATE projects SET permission_mode = ?2, status = 'active',
                                 updated_at = ?3, last_opened_at = ?3
             WHERE id = ?1",
            params![id, permission_mode, now],
        )?;
        return Ok(id);
    }

    let id = Uuid::new_v4().to_string();
    let root = PathBuf::from(root_path);
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(root_path);
    connection.execute(
        "INSERT INTO projects(
            id, name, root_path, permission_mode, status, created_at, updated_at, last_opened_at
         ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?5, ?5)",
        params![id, name, root_path, permission_mode, now],
    )?;
    Ok(id)
}

fn project_tool_started(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let Some(runtime_tool_call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return Ok(());
    };
    let tool_name = payload
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let input_json = serde_json::to_string(payload.get("input").unwrap_or(&Value::Null))
        .unwrap_or_else(|_| "null".to_owned());
    let conversation_id: String = transaction.query_row(
        "SELECT conversation_id FROM runs WHERE id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO tool_calls(
            id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
            status, execution_location, requires_approval, started_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', 'runtime', 0, ?7, ?7)
         ON CONFLICT(run_id, runtime_tool_call_id) DO UPDATE SET
            tool_name = excluded.tool_name,
            input_json = excluded.input_json,
            status = 'running',
            updated_at = excluded.updated_at",
        params![
            Uuid::new_v4().to_string(),
            runtime_tool_call_id,
            run_id,
            conversation_id,
            tool_name,
            input_json,
            now
        ],
    )?;
    Ok(())
}

fn project_tool_updated(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let Some(runtime_tool_call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return Ok(());
    };
    let result_json = serde_json::to_string(&compact_tool_result(
        payload.get("update").unwrap_or(&Value::Null),
    ))
    .unwrap_or_else(|_| "null".to_owned());
    transaction.execute(
        "UPDATE tool_calls SET result_json = ?3, updated_at = ?4
         WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
        params![run_id, runtime_tool_call_id, result_json, now],
    )?;
    Ok(())
}

fn project_tool_completed(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    payload: &Value,
    now: i64,
) -> rusqlite::Result<()> {
    let Some(runtime_tool_call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return Ok(());
    };
    let result_json = serde_json::to_string(&compact_tool_result(
        payload.get("result").unwrap_or(&Value::Null),
    ))
    .unwrap_or_else(|_| "null".to_owned());
    let is_error = payload
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    transaction.execute(
        "UPDATE tool_calls
         SET status = ?3, result_json = ?4, completed_at = ?5, updated_at = ?5
         WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
        params![
            run_id,
            runtime_tool_call_id,
            if is_error { "failed" } else { "completed" },
            result_json,
            now
        ],
    )?;
    Ok(())
}

fn parse_json(value: &str) -> Value {
    serde_json::from_str(value).unwrap_or(Value::Null)
}

fn compact_tool_result(value: &Value) -> Value {
    let Ok(serialized) = serde_json::to_string(value) else {
        return Value::Null;
    };
    if serialized.len() <= MAX_STORED_TOOL_RESULT_BYTES {
        return value.clone();
    }
    let preview = serialized.chars().take(8_000).collect::<String>();
    json!({
        "truncated": true,
        "originalBytes": serialized.len(),
        "preview": preview,
        "message": "Large tool result was summarized by Fox. Use the referenced artifact or source path for the complete data."
    })
}

fn query_last_run(connection: &Connection, id: &str) -> rusqlite::Result<Option<RunRecord>> {
    connection
        .query_row(
            "SELECT id, conversation_id, runtime_session_id, status, model, started_at,
                    finished_at, error_code, error_message, last_seq, trace_id, root_span_id
             FROM runs WHERE conversation_id = ?1 ORDER BY created_at DESC LIMIT 1",
            [id],
            |row| {
                Ok(RunRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    runtime_session_id: row.get(2)?,
                    status: row.get(3)?,
                    model: row.get(4)?,
                    started_at: row.get(5)?,
                    finished_at: row.get(6)?,
                    error_code: row.get(7)?,
                    error_message: row.get(8)?,
                    last_seq: row.get(9)?,
                    trace_id: row.get(10)?,
                    root_span_id: row.get(11)?,
                })
            },
        )
        .optional()
}

fn update_last_seq(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    seq: i64,
) -> rusqlite::Result<()> {
    transaction.execute(
        "UPDATE runs SET last_seq = MAX(last_seq, ?2) WHERE id = ?1",
        params![run_id, seq],
    )?;
    Ok(())
}

fn truncate_title(text: &str) -> String {
    const LIMIT: usize = 36;
    let mut title: String = text.chars().take(LIMIT).collect();
    if text.chars().count() > LIMIT {
        title.push('…');
    }
    title
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn connection_type(base_url: &str) -> String {
    url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .map(|host| {
            if host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
            {
                "local"
            } else if host.ends_with(".local")
                || !host.contains('.')
                || host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
                    std::net::IpAddr::V4(ip) => ip.is_private() || ip.is_link_local(),
                    std::net::IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local(),
                })
            {
                "lan"
            } else {
                "remote"
            }
        })
        .unwrap_or("remote")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_database() -> (Database, PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-test-{}.db", Uuid::new_v4()));
        (
            Database::open(path.clone()).expect("open test database"),
            path,
        )
    }

    fn preview_cache_entry(
        cache_key: &str,
        storage_path: &str,
        byte_size: u64,
        lease_count: u32,
        last_accessed_at: i64,
    ) -> KnowledgePreviewCacheEntry {
        KnowledgePreviewCacheEntry {
            cache_key: cache_key.to_owned(),
            knowledge_base_id: "kb-1".to_owned(),
            document_id: format!("document-{cache_key}"),
            source_revision: "sha256:test".to_owned(),
            variant: "original".to_owned(),
            storage_path: storage_path.to_owned(),
            media_type: Some("application/octet-stream".to_owned()),
            byte_size,
            lease_count,
            created_at: 1,
            last_accessed_at,
        }
    }

    #[test]
    fn preview_cache_lru_excludes_active_leases_and_tracks_usage() {
        let (database, path) = test_database();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "old", "old.bin", 100, 0, 10,
            ))
            .unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "leased",
                "leased.bin",
                200,
                1,
                5,
            ))
            .unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "new", "new.bin", 300, 0, 20,
            ))
            .unwrap();

        assert_eq!(database.knowledge_preview_cache_usage().unwrap(), 600);
        let candidates = database.knowledge_preview_cache_lru_candidates().unwrap();
        assert_eq!(
            candidates
                .iter()
                .map(|(key, _, _)| key.as_str())
                .collect::<Vec<_>>(),
            vec!["old", "new"]
        );
        assert!(!database
            .remove_knowledge_preview_cache_entry("leased")
            .unwrap());
        assert_eq!(database.knowledge_preview_cache_usage().unwrap(), 600);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_cache_leases_do_not_underflow() {
        let (database, path) = test_database();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "entry",
                "entry.bin",
                100,
                0,
                10,
            ))
            .unwrap();
        assert!(database
            .acquire_knowledge_preview_cache_lease("entry")
            .unwrap());
        assert!(database
            .release_knowledge_preview_cache_lease("entry")
            .unwrap());
        assert!(database
            .release_knowledge_preview_cache_lease("entry")
            .unwrap());
        assert_eq!(
            database.knowledge_preview_cache_lru_candidates().unwrap()[0].0,
            "entry"
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_cache_upsert_preserves_active_leases() {
        let (database, path) = test_database();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "entry",
                "entry.bin",
                100,
                0,
                10,
            ))
            .unwrap();
        assert!(database
            .acquire_knowledge_preview_cache_lease("entry")
            .unwrap());

        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "entry",
                "entry.bin",
                200,
                0,
                20,
            ))
            .unwrap();

        let entry = database
            .knowledge_preview_cache_entry("entry")
            .unwrap()
            .expect("cache entry");
        assert_eq!(entry.lease_count, 1);
        assert_eq!(entry.byte_size, 200);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_cache_statistics_separate_active_usage() {
        let (database, path) = test_database();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "idle", "idle.bin", 100, 0, 10,
            ))
            .unwrap();
        database
            .upsert_knowledge_preview_cache_entry(&preview_cache_entry(
                "active",
                "active.bin",
                250,
                2,
                20,
            ))
            .unwrap();

        let statistics = database.knowledge_preview_cache_statistics(500).unwrap();
        assert_eq!(statistics.total_files, 2);
        assert_eq!(statistics.active_files, 1);
        assert_eq!(statistics.total_bytes, 350);
        assert_eq!(statistics.active_bytes, 250);
        assert_eq!(statistics.limit_bytes, 500);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preview_cache_limit_uses_default_and_persists_updates() {
        let (database, path) = test_database();
        assert_eq!(database.knowledge_preview_cache_limit(500).unwrap(), 500);

        database.set_knowledge_preview_cache_limit(2_048).unwrap();
        assert_eq!(database.knowledge_preview_cache_limit(500).unwrap(), 2_048);

        drop(database);
        let reopened = Database::open(path.clone()).unwrap();
        assert_eq!(reopened.knowledge_preview_cache_limit(500).unwrap(), 2_048);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn creates_and_restores_a_conversation() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "hello Fox", None)
            .expect("create run");

        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .expect("apply run start");
        database
            .apply_runtime_event(
                &started.run.id,
                2,
                &serde_json::json!({"type": "message.started"}),
            )
            .expect("apply message start");
        database
            .apply_runtime_event(
                &started.run.id,
                3,
                &serde_json::json!({"type": "message.delta", "delta": "hello back"}),
            )
            .expect("apply delta");

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(detail.messages.len(), 2);
        assert_eq!(detail.messages[1].content, "hello back");
        assert_eq!(
            detail
                .runtime_events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec!["run.started", "message.started"]
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restores_reasoning_and_tool_events_in_sequence() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "inspect files", None)
            .expect("create run");
        for (seq, event) in [
            serde_json::json!({"type": "run.started"}),
            serde_json::json!({"type": "reasoning.delta", "delta": "Check the project."}),
            serde_json::json!({"type": "tool.started", "toolCallId": "tool-1", "tool": "ls", "input": {"path": "."}}),
            serde_json::json!({"type": "tool.completed", "toolCallId": "tool-1", "tool": "ls", "result": {"content": []}, "isError": false}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&started.run.id, seq as i64 + 1, &event)
                .expect("apply runtime event");
        }

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        let event_types = detail
            .runtime_events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            event_types,
            vec![
                "run.started",
                "reasoning.delta",
                "tool.started",
                "tool.completed"
            ]
        );
        assert_eq!(detail.runtime_events[3].event["tool"], "ls");
        assert_eq!(detail.tool_calls.len(), 1);
        assert_eq!(detail.tool_calls[0].runtime_tool_call_id, "tool-1");
        assert_eq!(detail.tool_calls[0].tool_name, "ls");
        assert_eq!(detail.tool_calls[0].status, "completed");
        assert_eq!(detail.tool_calls[0].input["path"], ".");
        assert_eq!(
            detail.tool_calls[0].result,
            Some(serde_json::json!({"content": []}))
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn restores_pending_user_question_events() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "ask me", None)
            .expect("create run");
        for (seq, event) in [
            json!({"type": "run.started"}),
            json!({
                "type": "user.question.requested",
                "source": "ask_user_question",
                "questions": [{
                    "question_id": "style",
                    "question": "选择界面风格",
                    "options": [{"label": "简洁", "value": "simple"}],
                    "multi_select": false,
                    "allow_other": true
                }]
            }),
            json!({"type": "run.completed", "completionReason": "awaiting_user"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&started.run.id, seq as i64 + 1, &event)
                .expect("apply runtime event");
        }

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(detail.last_run.as_ref().unwrap().status, "completed");
        assert_eq!(
            detail.runtime_events[1].event_type,
            "user.question.requested"
        );
        assert_eq!(
            detail.runtime_events[1].event["questions"][0]["question_id"],
            "style"
        );
        assert_eq!(
            detail.runtime_events[2].event["completionReason"],
            "awaiting_user"
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn atomically_creates_a_resumed_run_and_records_the_response() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let parent = database
            .create_run(&conversation.id, "ask me", None)
            .expect("create parent run");
        for (seq, event) in [
            json!({"type":"user.question.requested","questions":[{"question_id":"scope","question":"选择范围"}]}),
            json!({"type":"run.completed","completionReason":"awaiting_user"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&parent.run.id, seq as i64 + 1, &event)
                .expect("record parent event");
        }

        let resumed = database
            .create_resumed_run(&conversation.id, &parent.run.id, "范围：全部")
            .expect("resume run");
        let detail = database
            .load_conversation(&conversation.id)
            .expect("load resumed conversation");
        assert_eq!(detail.last_run.as_ref().unwrap().id, resumed.run.id);
        assert_eq!(detail.last_run.as_ref().unwrap().status, "queued");
        let responded = detail
            .runtime_events
            .iter()
            .find(|event| event.event_type == "user.question.responded")
            .expect("response lifecycle event");
        assert_eq!(responded.run_id, parent.run.id);
        assert_eq!(responded.event["childRunId"], resumed.run.id);

        database
            .mark_run_interrupted(&resumed.run.id, "test.cleanup", "cleanup")
            .expect("finish child run");
        assert!(database
            .create_resumed_run(&conversation.id, &parent.run.id, "范围：再次提交")
            .is_err());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_resume_rolls_back_without_creating_a_run_or_message() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let parent = database
            .create_run(&conversation.id, "ordinary request", None)
            .expect("create parent run");
        database
            .apply_runtime_event(&parent.run.id, 1, &json!({"type":"run.completed"}))
            .expect("complete parent run");
        let before = database
            .load_conversation(&conversation.id)
            .expect("load before invalid resume");

        assert!(database
            .create_resumed_run(&conversation.id, &parent.run.id, "should not persist")
            .is_err());
        let after = database
            .load_conversation(&conversation.id)
            .expect("load after invalid resume");
        assert_eq!(after.messages.len(), before.messages.len());
        assert_eq!(after.last_run.as_ref().unwrap().id, parent.run.id);
        assert!(!database
            .conversation_has_active_run(&conversation.id)
            .expect("check active run"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resume_targets_the_latest_unanswered_question_on_the_parent_run() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let parent = database
            .create_run(&conversation.id, "ask twice", None)
            .expect("create parent run");
        for (seq, event) in [
            json!({"type":"user.question.requested","questions":[{"question_id":"first","question":"第一个问题"}]}),
            json!({"type":"user.question.responded","childRunId":"old-child"}),
            json!({"type":"user.question.requested","questions":[{"question_id":"second","question":"第二个问题"}]}),
            json!({"type":"run.completed","completionReason":"awaiting_user"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&parent.run.id, seq as i64 + 1, &event)
                .expect("record lifecycle event");
        }

        let resumed = database
            .create_resumed_run(&conversation.id, &parent.run.id, "第二个答案")
            .expect("resume latest question");
        assert_eq!(resumed.run.status, "queued");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ignores_duplicate_runtime_events() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "deduplicate", None)
            .expect("create run");
        let event = serde_json::json!({"type": "run.started"});

        assert!(database
            .apply_runtime_event(&started.run.id, 1, &event)
            .unwrap());
        assert!(!database
            .apply_runtime_event(&started.run.id, 1, &event)
            .unwrap());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ignores_runtime_events_that_arrive_out_of_order() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "event order", None)
            .expect("create run");

        assert!(database
            .apply_runtime_event(
                &started.run.id,
                2,
                &serde_json::json!({"type": "run.started"}),
            )
            .unwrap());
        assert!(!database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "message.started"}),
            )
            .unwrap());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn yuxi_service_is_a_singleton() {
        let (database, path) = test_database();
        database
            .save_yuxi_service("Local", "http://127.0.0.1:5050")
            .expect("save local Yuxi");
        let remote = database
            .save_yuxi_service("Remote", "https://yuxi.example.com")
            .expect("replace Yuxi service");

        assert_eq!(remote.name, "Remote");
        assert_eq!(remote.base_url, "https://yuxi.example.com");
        assert_eq!(remote.connection_type, "remote");

        let lan = database
            .save_yuxi_service("LAN", "http://192.168.1.20:5050")
            .expect("replace with LAN Yuxi");
        assert_eq!(lan.connection_type, "lan");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn builds_runtime_context_in_message_order() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let first = database
            .create_run(&conversation.id, "first question", None)
            .expect("create first run");
        for (seq, event) in [
            serde_json::json!({"type": "run.started"}),
            serde_json::json!({"type": "message.started"}),
            serde_json::json!({"type": "message.delta", "delta": "first answer"}),
            serde_json::json!({"type": "message.completed"}),
            serde_json::json!({"type": "run.completed"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&first.run.id, seq as i64 + 1, &event)
                .expect("apply first run event");
        }
        database
            .create_run(&conversation.id, "second question", None)
            .expect("create second run");

        let context = database
            .runtime_prompt_context(&conversation.id, 20)
            .expect("build runtime context");
        assert_eq!(context.len(), 3);
        assert_eq!(context[0].role, "user");
        assert_eq!(context[0].content, "first question");
        assert_eq!(context[1].role, "assistant");
        assert_eq!(context[1].content, "first answer");
        assert_eq!(context[2].content, "second question");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_context_budget_keeps_recent_messages_in_original_order() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        for index in 0..4 {
            let started = database
                .create_run(&conversation.id, &format!("question-{index}"), None)
                .expect("create run");
            database
                .apply_runtime_event(&started.run.id, 1, &json!({"type":"message.started"}))
                .expect("start answer");
            database
                .apply_runtime_event(
                    &started.run.id,
                    2,
                    &json!({"type":"message.delta", "delta": format!("answer-{index}")}),
                )
                .expect("append answer");
            database
                .apply_runtime_event(&started.run.id, 3, &json!({"type":"run.completed"}))
                .expect("complete run");
        }

        let context = database
            .runtime_prompt_context_budget(&conversation.id, 20, 90)
            .expect("build budgeted context");
        assert!(!context.is_empty());
        assert_eq!(context.last().unwrap().content, "answer-3");
        let full = database
            .runtime_prompt_context(&conversation.id, 20)
            .expect("build complete context");
        let suffix = &full[full.len() - context.len()..];
        assert!(context
            .iter()
            .zip(suffix)
            .all(|(actual, expected)| actual.role == expected.role
                && actual.content == expected.content));

        let oversized = database
            .create_run(&conversation.id, &"x".repeat(200), None)
            .expect("create oversized run");
        let context = database
            .runtime_prompt_context_budget(&conversation.id, 20, 20)
            .expect("keep one oversized latest message");
        assert_eq!(context.last().unwrap().content, "x".repeat(200));
        assert_eq!(oversized.user_message.content, "x".repeat(200));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn paginates_large_history_without_loading_the_entire_conversation() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        for index in 0..80 {
            let started = database
                .create_run(&conversation.id, &format!("question {index}"), None)
                .expect("create run");
            for (seq, event) in [
                json!({"type":"message.started"}),
                json!({"type":"message.delta","delta":format!("answer {index}")}),
                json!({"type":"run.completed"}),
            ]
            .into_iter()
            .enumerate()
            {
                database
                    .apply_runtime_event(&started.run.id, seq as i64 + 1, &event)
                    .expect("complete run");
            }
        }
        let initial = database
            .load_conversation(&conversation.id)
            .expect("initial window");
        assert_eq!(initial.messages.len(), INITIAL_HISTORY_MESSAGES);
        assert!(initial.has_earlier_messages);
        let oldest = initial.messages[0].ordinal;
        let earlier = database
            .load_conversation_history(&conversation.id, oldest, 100)
            .expect("earlier window");
        assert_eq!(earlier.messages.len(), 40);
        assert!(!earlier.has_earlier_messages);
        assert!(earlier.messages.last().unwrap().ordinal < oldest);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn searches_conversation_titles_projects_agents_and_message_content() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(
                DEFAULT_AGENT_ID,
                Some("季度计划"),
                Some(path.parent().unwrap().to_string_lossy().as_ref()),
                Some("read_only"),
            )
            .expect("create searchable conversation");
        database
            .create_run(&conversation.id, "独特的全文检索标记", None)
            .expect("create searchable message");

        assert_eq!(
            database.search_conversations("季度计划", 20).unwrap().len(),
            1
        );
        assert_eq!(
            database
                .search_conversations("全文检索标记", 20)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(database.search_conversations("Fox", 20).unwrap().len(), 1);
        assert!(database
            .search_conversations("完全不存在", 20)
            .unwrap()
            .is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn summarizes_large_tool_results_before_persistence() {
        let value = json!({"content": "x".repeat(MAX_STORED_TOOL_RESULT_BYTES + 1)});
        let compact = compact_tool_result(&value);
        assert_eq!(compact["truncated"], true);
        assert!(compact["originalBytes"].as_u64().unwrap() > MAX_STORED_TOOL_RESULT_BYTES as u64);
        assert!(compact["preview"].as_str().unwrap().len() <= 8_000);
    }

    #[test]
    fn interrupts_an_active_run_on_shutdown() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "keep running", None)
            .expect("create run");
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &serde_json::json!({"type": "run.started"}),
            )
            .expect("start run");
        assert!(database
            .conversation_has_active_run(&conversation.id)
            .expect("check active run"));

        database
            .mark_run_interrupted(
                &started.run.id,
                "runtime.application_exit",
                "application closed",
            )
            .expect("interrupt run");
        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(detail.last_run.unwrap().status, "interrupted");
        assert!(!database
            .conversation_has_active_run(&conversation.id)
            .expect("check terminal run"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn preserves_recoverable_yuxi_runs_across_restart_repair() {
        let (database, path) = test_database();
        database
            .upsert_yuxi_agents(&[YuxiAgentRecord {
                id: "yuxi:assistant".to_owned(),
                slug: "assistant".to_owned(),
                name: "Yuxi Assistant".to_owned(),
                description: String::new(),
                icon: None,
                backend_id: "ChatbotAgent".to_owned(),
                default_model: "remote-model".to_owned(),
                capabilities: json!([]),
                resources: AgentResourcesRecord::default(),
                configurable_items: json!({}),
                is_default: true,
                available: true,
            }])
            .expect("upsert remote agent");
        let conversation = database
            .create_conversation("yuxi:assistant", None, None, None)
            .expect("create remote conversation");
        database
            .ensure_external_runtime_session(&conversation.id, "yuxi", "thread-1", None)
            .expect("save remote thread");
        let started = database
            .create_run(&conversation.id, "recover me", None)
            .expect("create remote run");
        database
            .set_external_run(&started.run.id, "remote-run-1")
            .expect("save remote run");
        database
            .apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"}))
            .expect("start remote run");
        database.repair_interrupted_runs().expect("repair startup");

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load remote conversation");
        assert_eq!(detail.last_run.unwrap().status, "running");
        let recoverable = database
            .recoverable_yuxi_runs()
            .expect("list recoverable runs");
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].external_run_id, "remote-run-1");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn marks_remote_agents_missing_from_refresh_as_unavailable() {
        let (database, path) = test_database();
        database
            .save_yuxi_service("Local", "http://127.0.0.1:5050")
            .expect("save knowledge service");
        database
            .record_yuxi_connection_test(&YuxiConnectionTest {
                ok: true,
                base_url: "http://127.0.0.1:5050".to_owned(),
                connection_type: "local".to_owned(),
                status: "connected".to_owned(),
                authenticated: true,
                auth_status: "authenticated".to_owned(),
                version: Some("test".to_owned()),
                message: "ok".to_owned(),
                latency_ms: 1,
            })
            .expect("record connected service");
        database
            .upsert_yuxi_agents(&[YuxiAgentRecord {
                id: "yuxi:assistant".to_owned(),
                slug: "assistant".to_owned(),
                name: "Knowledge Assistant".to_owned(),
                description: String::new(),
                icon: None,
                backend_id: "ChatbotAgent".to_owned(),
                default_model: "remote-model".to_owned(),
                capabilities: json!([]),
                resources: AgentResourcesRecord::default(),
                configurable_items: json!({}),
                is_default: true,
                available: true,
            }])
            .expect("upsert remote agent");
        assert!(
            database
                .list_agents()
                .expect("list agents")
                .into_iter()
                .find(|agent| agent.id == "yuxi:assistant")
                .expect("remote agent")
                .available
        );

        database
            .upsert_yuxi_agents(&[])
            .expect("refresh empty remote agents");
        assert!(
            !database
                .list_agents()
                .expect("list agents")
                .into_iter()
                .find(|agent| agent.id == "yuxi:assistant")
                .expect("stale remote agent remains for conversation history")
                .available
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn yuxi_run_uses_only_an_explicit_model_override() {
        let (database, path) = test_database();
        database
            .save_model_service(&ModelServiceRecord {
                name: "Fox Model".to_owned(),
                base_url: "https://model.example.com".to_owned(),
                model_id: "fox-local-model".to_owned(),
                api_type: "openai-completions".to_owned(),
                context_window: 128_000,
                max_output_tokens: 8_192,
                supports_image_input: false,
                enabled: true,
                credential_configured: false,
                connection_type: "remote".to_owned(),
                last_status: "unknown".to_owned(),
                last_latency_ms: None,
                last_checked_at: None,
                created_at: 0,
                updated_at: 0,
            })
            .expect("save local model");
        database
            .upsert_yuxi_agents(&[YuxiAgentRecord {
                id: "yuxi:assistant".to_owned(),
                slug: "assistant".to_owned(),
                name: "Yuxi Assistant".to_owned(),
                description: String::new(),
                icon: None,
                backend_id: "ChatbotAgent".to_owned(),
                default_model: "remote-default".to_owned(),
                capabilities: json!([]),
                resources: AgentResourcesRecord::default(),
                configurable_items: json!({}),
                is_default: true,
                available: true,
            }])
            .expect("upsert remote agent");
        let conversation = database
            .create_conversation("yuxi:assistant", None, None, None)
            .expect("create remote conversation");

        let inherited = database
            .create_run(&conversation.id, "use Yuxi default", None)
            .expect("create default remote run");
        assert_eq!(inherited.run.model, "");
        database
            .apply_runtime_event(&inherited.run.id, 1, &json!({"type":"run.completed"}))
            .expect("complete default remote run");

        let overridden = database
            .create_run(&conversation.id, "override model", Some("yuxi:model"))
            .expect("create overridden remote run");
        assert_eq!(overridden.run.model, "yuxi:model");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn failed_run_interrupts_the_streaming_assistant_message() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "trigger failure", None)
            .expect("create run");
        database
            .apply_runtime_event(&started.run.id, 1, &json!({"type":"message.started"}))
            .expect("start assistant message");
        database
            .apply_runtime_event(
                &started.run.id,
                2,
                &json!({"type":"run.failed","code":"test.failed","message":"boom"}),
            )
            .expect("fail run");

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(detail.last_run.unwrap().status, "failed");
        assert_eq!(detail.messages.last().unwrap().status, "interrupted");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persists_attachments_artifacts_bindings_and_approval_lifecycle() {
        let (database, path) = test_database();
        database
            .save_yuxi_service("Local", "http://127.0.0.1:5050")
            .expect("save Yuxi service");
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "phase two", None)
            .expect("create run");

        database
            .add_attachments(&[AttachmentRecord {
                id: "attachment-1".to_owned(),
                conversation_id: conversation.id.clone(),
                message_id: Some(started.user_message.id.clone()),
                display_name: "notes.md".to_owned(),
                storage_path: "D:\\data\\notes.md".to_owned(),
                media_type: Some("text/markdown".to_owned()),
                byte_size: 10,
                sha256: Some("hash".to_owned()),
                status: "ready".to_owned(),
                created_at: now_ms(),
            }])
            .expect("save attachment");
        database
            .set_knowledge_bindings(
                &conversation.id,
                &[("kb-1".to_owned(), "产品知识库".to_owned())],
            )
            .expect("bind knowledge");

        let tool = database
            .create_host_tool_call(
                &started.run.id,
                "write-1",
                "write_file",
                &json!({"path":"result.md"}),
                "pending",
                true,
            )
            .expect("create tool");
        let approval = database
            .create_approval(&tool.id, "创建 result.md", &json!({"target":"result.md"}))
            .expect("create approval");
        let resolved = database
            .resolve_approval(&approval.id, true)
            .expect("approve")
            .expect("resolved approval");
        assert_eq!(resolved.status, "approved");
        assert!(database
            .resolve_approval(&approval.id, false)
            .expect("repeat approval")
            .is_none());

        database
            .complete_host_tool_call(
                &started.run.id,
                "write-1",
                Some(&json!({
                    "content": [{"type":"text","text":"done"}],
                    "details": {"path":"D:\\data\\result.md","bytes":4}
                })),
                None,
            )
            .expect("complete tool");
        let detail = database
            .load_conversation(&conversation.id)
            .expect("load phase two data");
        assert_eq!(detail.attachments.len(), 1);
        let attachment = database
            .attachment_for_conversation(&conversation.id, "attachment-1")
            .expect("lookup attachment")
            .expect("attachment exists");
        assert_eq!(attachment.display_name, "notes.md");
        assert!(database
            .attachment_for_conversation("another-conversation", "attachment-1")
            .expect("scoped lookup")
            .is_none());
        assert_eq!(detail.artifacts.len(), 1);
        assert_eq!(detail.knowledge_bindings.len(), 1);
        assert_eq!(detail.approvals[0].status, "approved");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn binds_pending_attachments_to_the_created_user_message() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        database
            .add_attachments(&[AttachmentRecord {
                id: "pending-attachment".to_owned(),
                conversation_id: conversation.id.clone(),
                message_id: None,
                display_name: "notes.txt".to_owned(),
                storage_path: "D:\\data\\notes.txt".to_owned(),
                media_type: Some("text/plain".to_owned()),
                byte_size: 5,
                sha256: Some("hash".to_owned()),
                status: "ready".to_owned(),
                created_at: now_ms(),
            }])
            .expect("save pending attachment");
        let started = database
            .create_run(&conversation.id, "read attachment", None)
            .expect("create run");
        let attachments = database
            .bind_attachments_to_message(
                &conversation.id,
                &started.user_message.id,
                &["pending-attachment".to_owned()],
            )
            .expect("bind attachment");
        assert_eq!(attachments.len(), 1);
        assert_eq!(
            attachments[0].message_id.as_deref(),
            Some(started.user_message.id.as_str())
        );
        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(
            detail.attachments[0].message_id.as_deref(),
            Some(started.user_message.id.as_str())
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn only_active_runs_can_enter_cancelling() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "cancel once", None)
            .expect("create run");
        assert_eq!(
            database
                .mark_run_cancelling(&started.run.id)
                .expect("mark cancelling")
                .as_deref(),
            Some(conversation.id.as_str())
        );
        assert!(database
            .mark_run_cancelling(&started.run.id)
            .expect("reject repeated cancel")
            .is_none());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tracks_conversation_project_roots() {
        let (database, path) = test_database();
        let root = std::env::temp_dir();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, root.to_str(), Some("ask"))
            .expect("create project conversation");
        assert_eq!(
            database
                .conversation_project_root(&conversation.id)
                .expect("load project root")
                .as_deref(),
            root.to_str()
        );
        assert!(conversation.project_id.is_some());
        let project_id = conversation.project_id.as_deref().unwrap();
        let projects = database.list_projects().expect("list projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, project_id);
        assert_eq!(projects[0].permission_mode, "ask");
        let updated = database
            .update_project_permission_mode(project_id, "allow")
            .expect("update permission")
            .expect("project exists");
        assert_eq!(updated.permission_mode, "allow");
        assert!(database
            .update_project_permission_mode(project_id, "unrestricted")
            .is_err());

        let no_project = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create plain conversation");
        assert_eq!(
            database
                .conversation_project_root(&no_project.id)
                .expect("load empty project root"),
            None
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn usage_statistics_counts_latest_usage_once_per_run() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "test", Some("model"))
            .expect("start run");
        for (seq, total) in [(1, 12), (2, 25)] {
            database
                .apply_runtime_event(
                    &started.run.id,
                    seq,
                    &json!({"type":"usage.updated","inputTokens":10,"outputTokens":15,"totalTokens":total}),
                )
                .expect("append usage event");
        }
        let statistics = database.usage_statistics().expect("usage statistics");
        assert_eq!(statistics.conversation_count, 1);
        assert_eq!(statistics.input_tokens, 10);
        assert_eq!(statistics.output_tokens, 15);
        assert_eq!(statistics.total_tokens, 25);
        assert_eq!(statistics.agents[0].total_tokens, 25);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persists_document_reading_state_bookmarks_and_annotations_by_document() {
        let (database, path) = test_database();
        let now = now_ms();
        let reading_state = KnowledgeDocumentReadingState {
            knowledge_base_id: "kb-1".to_owned(),
            document_id: "doc-1".to_owned(),
            source_revision: Some("revision-a".to_owned()),
            page: 4,
            scroll_offset: 120.0,
            zoom: Some(1.25),
            updated_at: now,
        };
        database
            .save_knowledge_document_reading_state(&reading_state)
            .expect("save reading state");
        let bookmark = KnowledgeDocumentBookmark {
            id: Uuid::new_v4().to_string(),
            knowledge_base_id: "kb-1".to_owned(),
            document_id: "doc-1".to_owned(),
            source_revision: Some("revision-a".to_owned()),
            page: 4,
            anchor: String::new(),
            excerpt: String::new(),
            label: "关键页".to_owned(),
            created_at: now,
            updated_at: now,
        };
        database
            .save_knowledge_document_bookmark(&bookmark)
            .expect("save bookmark");
        let annotation = KnowledgeDocumentAnnotation {
            id: Uuid::new_v4().to_string(),
            knowledge_base_id: "kb-1".to_owned(),
            document_id: "doc-1".to_owned(),
            source_revision: Some("revision-a".to_owned()),
            annotation_type: "note".to_owned(),
            page: 5,
            anchor: String::new(),
            excerpt: String::new(),
            note: "检查这一节".to_owned(),
            color: "blue".to_owned(),
            created_at: now,
            updated_at: now,
        };
        database
            .save_knowledge_document_annotation(&annotation)
            .expect("save annotation");

        let activity = database
            .knowledge_document_activity("kb-1", "doc-1")
            .expect("load document activity");
        assert_eq!(activity.reading_state, Some(reading_state));
        assert_eq!(activity.bookmarks, vec![bookmark.clone()]);
        assert_eq!(activity.annotations, vec![annotation.clone()]);
        assert!(database
            .knowledge_document_activity("kb-1", "doc-2")
            .expect("load isolated document")
            .bookmarks
            .is_empty());
        assert!(database
            .delete_knowledge_document_bookmark("kb-1", "doc-1", &bookmark.id)
            .expect("delete bookmark"));
        assert!(database
            .delete_knowledge_document_annotation("kb-1", "doc-1", &annotation.id)
            .expect("delete annotation"));

        let activity = database
            .knowledge_document_activity("kb-1", "doc-1")
            .expect("load deleted activity");
        assert!(activity.bookmarks.is_empty());
        assert!(activity.annotations.is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
