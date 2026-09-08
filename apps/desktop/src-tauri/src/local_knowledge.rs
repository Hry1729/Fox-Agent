use crate::{
    app_state::AppState,
    database::{ApiResponse, Database},
    local_knowledge_import::{
        import_document_at_relative_path, parse_imported_document, ImportOptions, ImportedDocument,
        ParsedDocument, DEFAULT_CHUNK_MAX_BYTES,
    },
    local_knowledge_picker::{
        is_supported_file_name, mime_type_for_file_name, validate_path_component,
        MAX_PICKED_FILE_SIZE,
    },
    local_knowledge_storage::{
        cleanup_staging, commit_staging, copy_managed_directory, create_staging_path,
        rollback_committed_destination, validate_destination_storage_root,
        verify_managed_directory, StorageMigration, StorageMigrationError,
        StorageMigrationIdRequest, StorageMigrationStartRequest,
    },
};
use rusqlite::{
    params, params_from_iter, types::Value as SqlValue, Connection, OpenFlags, OptionalExtension,
    Row,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, UNIX_EPOCH},
};
use tauri::State;
use uuid::Uuid;

const KNOWLEDGE_SCHEMA_VERSION: i64 = 4;
pub(crate) const DEFAULT_CHUNK_SIZE: i64 = 512;
pub(crate) const DEFAULT_CHUNK_OVERLAP: i64 = 50;
const MAX_LOCAL_FILE_CATALOG_ITEMS: usize = 100_000;
const DEFAULT_LOCAL_FILE_LIST_LIMIT: usize = 500;
const MAX_LOCAL_FILE_LIST_LIMIT: usize = 2_000;
const MAX_LOCAL_QUERY_CHARS: usize = 512;
const MAX_LOCAL_SEARCH_RESULTS: usize = 20;
const MAX_LOCAL_SEARCH_CHARS: usize = 32 * 1024;
const MAX_LOCAL_SEARCH_CANDIDATES: usize = 2_000;
const DEFAULT_LOCAL_SEARCH_RESULTS: usize = 8;
const DEFAULT_LOCAL_SEARCH_CHARS: usize = 16 * 1024;
const MAX_LOCAL_READ_CHARS: usize = 64 * 1024;
const MAX_LOCAL_PREVIEW_RANGE_BYTES: u64 = 4 * 1024 * 1024;
const DEFAULT_FOX_GUIDE_ID: &str = "fox-user-guide";
const DEFAULT_FOX_GUIDE_NAME: &str = "Fox 使用指南";
const DEFAULT_FOX_GUIDE_DESCRIPTION: &str = "Fox Agent 页面、功能和常用操作的内置使用教程。";

struct DefaultFoxGuideDocument {
    relative_path: &'static str,
    content: &'static str,
}

const DEFAULT_FOX_GUIDE_DOCUMENTS: &[DefaultFoxGuideDocument] = &[
    DefaultFoxGuideDocument {
        relative_path: "00-开始使用/01-认识Fox.md",
        content: include_str!("../resources/default-knowledge/fox-guide/00-开始使用/01-认识Fox.md"),
    },
    DefaultFoxGuideDocument {
        relative_path: "00-开始使用/02-第一次对话.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/00-开始使用/02-第一次对话.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "01-助手与专家/01-助手和专家.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/01-助手与专家/01-助手和专家.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "01-助手与专家/02-如何写清楚需求.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/01-助手与专家/02-如何写清楚需求.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "02-本地知识/01-本地文件.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/02-本地知识/01-本地文件.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "02-本地知识/02-本地知识库.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/02-本地知识/02-本地知识库.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "02-本地知识/03-任务中心.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/02-本地知识/03-任务中心.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "03-插件中心/01-插件中心.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/03-插件中心/01-插件中心.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "03-插件中心/02-MCP-Skills-工具.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/03-插件中心/02-MCP-Skills-工具.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "04-工作区与设置/01-工作区和文档.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/04-工作区与设置/01-工作区和文档.md"
        ),
    },
    DefaultFoxGuideDocument {
        relative_path: "05-常见问题/01-常见问题.md",
        content: include_str!(
            "../resources/default-knowledge/fox-guide/05-常见问题/01-常见问题.md"
        ),
    },
];

#[derive(Debug)]
pub struct LocalKnowledgeError {
    code: &'static str,
    message: String,
    retryable: bool,
}

impl std::fmt::Display for LocalKnowledgeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for LocalKnowledgeError {}

impl LocalKnowledgeError {
    pub(crate) fn code(&self) -> &'static str {
        self.code
    }

    pub(crate) fn retryable(&self) -> bool {
        self.retryable
    }

    pub(crate) fn not_found(entity: &str) -> Self {
        Self {
            code: "local_knowledge.not_found",
            message: format!("{entity} was not found"),
            retryable: false,
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "local_knowledge.invalid_request",
            message: message.into(),
            retryable: false,
        }
    }

    pub(crate) fn storage(error: impl std::fmt::Display) -> Self {
        Self {
            code: "local_knowledge.storage_failed",
            message: error.to_string(),
            retryable: true,
        }
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            code: "local_knowledge.job_conflict",
            message: message.into(),
            retryable: false,
        }
    }

    fn storage_migration(error: StorageMigrationError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        }
    }

    fn import(error: crate::local_knowledge_import::ImportError) -> Self {
        let retryable = matches!(
            error.code,
            "local_knowledge.import_source_unreadable"
                | "local_knowledge.import_storage_failed"
                | "local_knowledge.import_copy_failed"
                | "local_knowledge.import_commit_failed"
        );
        Self {
            code: error.code,
            message: error.message,
            retryable,
        }
    }
}

impl From<rusqlite::Error> for LocalKnowledgeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::storage(error)
    }
}

impl From<std::io::Error> for LocalKnowledgeError {
    fn from(error: std::io::Error) -> Self {
        Self::storage(error)
    }
}

#[derive(Clone)]
pub struct LocalKnowledgeStore {
    pub(crate) connection: Arc<Mutex<Connection>>,
    pub(crate) root: Arc<PathBuf>,
    pub(crate) zvec_resource_dir: Arc<Option<PathBuf>>,
    database_path: Arc<PathBuf>,
    storage_migration_active: Arc<Mutex<Option<String>>>,
    pending_root_path: Arc<Mutex<Option<PathBuf>>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeEmbeddingModel {
    pub id: String,
    pub name: String,
    pub version: String,
    pub dimension: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeBase {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub active_index_generation: Option<String>,
    pub document_count: i64,
    pub active_job_status: Option<String>,
    pub text_index_ready: bool,
    pub vector_index_ready: bool,
    pub search_mode: String,
    pub configured_embedding_model_id: Option<String>,
    pub chunk_size: i64,
    pub chunk_overlap: i64,
    pub embedding_model: Option<LocalKnowledgeEmbeddingModel>,
    pub chunk_count: Option<i64>,
    pub vector_count: Option<i64>,
    pub last_indexed_at: Option<i64>,
    pub fallback_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalKnowledgeBaseRequest {
    pub name: String,
    pub description: Option<String>,
    pub embedding_model_id: Option<String>,
    pub chunk_size: Option<i64>,
    pub chunk_overlap: Option<i64>,
    pub search_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLocalKnowledgeBaseRequest {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub embedding_model_id: Option<String>,
    pub chunk_size: Option<i64>,
    pub chunk_overlap: Option<i64>,
    pub search_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeBaseIdRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeDocument {
    pub id: String,
    pub knowledge_base_id: String,
    pub display_name: String,
    pub relative_path: String,
    pub current_revision: i64,
    pub file_size: i64,
    pub mime_type: String,
    pub parse_status: String,
    pub index_status: String,
    pub chunk_count: i64,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeDocumentsRequest {
    pub knowledge_base_id: String,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFolder {
    pub knowledge_base_id: String,
    pub name: String,
    pub relative_path: String,
    pub document_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFoldersRequest {
    pub knowledge_base_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalKnowledgeFolderRequest {
    pub knowledge_base_id: String,
    pub parent_path: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeDocumentFileRangeRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeDocumentFileOpenRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub reveal: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFileSource {
    pub id: String,
    pub root_path: String,
    pub display_name: String,
    pub file_count: i64,
    pub total_size: i64,
    pub last_scanned_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeCatalogFile {
    pub id: String,
    pub source_id: String,
    pub source_name: String,
    pub source_path: String,
    pub absolute_path: String,
    pub relative_path: String,
    pub display_name: String,
    pub extension: String,
    pub mime_type: String,
    pub file_size: i64,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFileSourcePathRequest {
    pub path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFileSourceIdRequest {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeFileSourceUpdateRequest {
    pub id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeCatalogFilesRequest {
    pub source_id: Option<String>,
    pub query: Option<String>,
    pub category: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeCatalogFileRangeRequest {
    pub id: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeCatalogFileOpenRequest {
    pub id: String,
    pub reveal: bool,
}

struct CatalogFileCandidate {
    absolute_path: String,
    relative_path: String,
    display_name: String,
    extension: String,
    mime_type: String,
    file_size: i64,
    modified_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeImportFileRequest {
    pub source_path: String,
    pub relative_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeImportRequest {
    pub knowledge_base_id: String,
    pub files: Vec<LocalKnowledgeImportFileRequest>,
    pub parser_version: Option<String>,
    pub chunk_config_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeOperationAccepted {
    pub operation_id: String,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeJob {
    pub id: String,
    pub parent_operation_id: Option<String>,
    pub knowledge_base_id: String,
    pub job_type: String,
    pub status: String,
    pub stage: Option<String>,
    pub progress: i64,
    pub retry_count: i64,
    pub last_sequence: i64,
    pub outcome: Option<String>,
    pub generation_id: Option<String>,
    pub heartbeat_at: Option<i64>,
    pub checkpoint_json: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeJobsRequest {
    pub knowledge_base_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeJobIdRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LocalKnowledgeStorageStatus {
    pub root_path: String,
    pub database_path: String,
    pub writable: bool,
    pub schema_version: i64,
    pub pending_root_path: Option<String>,
    pub restart_required: bool,
    pub migration_id: Option<String>,
}

impl LocalKnowledgeStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, LocalKnowledgeError> {
        Self::open_internal(root.as_ref(), None)
    }

    pub fn open_with_zvec_resource_dir(
        root: impl AsRef<Path>,
        resource_dir: impl AsRef<Path>,
    ) -> Result<Self, LocalKnowledgeError> {
        Self::open_internal(root.as_ref(), Some(resource_dir.as_ref()))
    }

    pub(crate) fn open_retrieval_worker(
        root: impl AsRef<Path>,
        zvec_resource_dir: Option<&Path>,
    ) -> Result<Self, LocalKnowledgeError> {
        let root = root.as_ref().to_path_buf();
        let database_path = root.join("knowledge.db");
        let connection = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_FULL_MUTEX,
        )?;
        Ok(Self {
            connection: Arc::new(Mutex::new(connection)),
            root: Arc::new(root),
            zvec_resource_dir: Arc::new(zvec_resource_dir.map(Path::to_path_buf)),
            database_path: Arc::new(database_path),
            storage_migration_active: Arc::new(Mutex::new(None)),
            pending_root_path: Arc::new(Mutex::new(None)),
        })
    }

    fn open_internal(
        root: &Path,
        zvec_resource_dir: Option<&Path>,
    ) -> Result<Self, LocalKnowledgeError> {
        let root = root.to_path_buf();
        std::fs::create_dir_all(&root)?;
        for directory in ["models", "indexes", "knowledge-bases", "staging"] {
            std::fs::create_dir_all(root.join(directory))?;
        }
        let database_path = root.join("knowledge.db");
        let connection = Connection::open(&database_path)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        initialize_schema(&connection)?;
        let store = Self {
            connection: Arc::new(Mutex::new(connection)),
            root: Arc::new(root),
            zvec_resource_dir: Arc::new(zvec_resource_dir.map(Path::to_path_buf)),
            database_path: Arc::new(database_path),
            storage_migration_active: Arc::new(Mutex::new(None)),
            pending_root_path: Arc::new(Mutex::new(None)),
        };
        store.reconcile_storage_migration_on_open()?;
        store.recover_startup_jobs()?;
        store.recover_embedding_downloads()?;
        Ok(store)
    }

    pub fn with_zvec_resource_dir(mut self, resource_dir: impl AsRef<Path>) -> Self {
        self.zvec_resource_dir = Arc::new(Some(resource_dir.as_ref().to_path_buf()));
        self
    }

    pub fn ensure_default_fox_guide(&self) -> Result<(), LocalKnowledgeError> {
        let now = crate::database::now_ms();
        self.connection()?.execute(
            "INSERT INTO local_knowledge_bases(id, name, description, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, description = excluded.description",
            params![
                DEFAULT_FOX_GUIDE_ID,
                DEFAULT_FOX_GUIDE_NAME,
                DEFAULT_FOX_GUIDE_DESCRIPTION,
                now
            ],
        )?;

        let documents_root = self
            .root
            .join("knowledge-bases")
            .join(DEFAULT_FOX_GUIDE_ID)
            .join("documents");
        std::fs::create_dir_all(&documents_root)?;
        let mut changed = false;

        for document in DEFAULT_FOX_GUIDE_DOCUMENTS {
            let stored_path = documents_root.join(document.relative_path);
            if let Some(parent) = stored_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::read(&stored_path).ok().as_deref() != Some(document.content.as_bytes()) {
                std::fs::write(&stored_path, document.content.as_bytes())?;
            }

            let content_hash = hex::encode(Sha256::digest(document.content.as_bytes()));
            let display_name = Path::new(document.relative_path)
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| LocalKnowledgeError::invalid("default guide filename is invalid"))?
                .to_owned();
            let imported = ImportedDocument {
                display_name: display_name.clone(),
                relative_path: document.relative_path.to_owned(),
                content_hash: content_hash.clone(),
                file_size: u64::try_from(document.content.len()).unwrap_or(u64::MAX),
                mime_type: "text/markdown".to_owned(),
                stored_path: stored_path.clone(),
            };
            let chunks = match parse_imported_document(&imported, DEFAULT_CHUNK_MAX_BYTES)
                .map_err(LocalKnowledgeError::import)?
            {
                ParsedDocument::Text { chunks, .. } => chunks,
                ParsedDocument::PendingSpecializedParser { .. } => Vec::new(),
            };

            let mut connection = self.connection()?;
            let existing = connection
                .query_row(
                    "SELECT id, current_revision, content_hash
                     FROM local_kb_documents
                     WHERE knowledge_base_id = ?1 AND relative_path = ?2",
                    params![DEFAULT_FOX_GUIDE_ID, document.relative_path],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()?;
            if existing
                .as_ref()
                .is_some_and(|(_, _, existing_hash)| existing_hash == &content_hash)
            {
                continue;
            }

            let mut document_hasher = Sha256::new();
            document_hasher.update(document.relative_path.as_bytes());
            let generated_id = format!(
                "fox-guide-{}",
                &hex::encode(document_hasher.finalize())[..24]
            );
            let document_id = existing
                .as_ref()
                .map(|(id, _, _)| id.clone())
                .unwrap_or(generated_id);
            let revision = existing
                .as_ref()
                .map(|(_, revision, _)| revision.saturating_add(1))
                .unwrap_or(1);
            let transaction = connection.transaction()?;
            transaction.execute(
                "INSERT INTO local_kb_documents(
                    id, knowledge_base_id, display_name, relative_path, source_path,
                    current_revision, content_hash, file_size, mime_type, parse_status,
                    index_status, parser_version, chunk_config_hash, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'text/markdown', 'ready',
                           'ready', 'fox-guide-v1', 'utf8-4096-v1', ?9, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    display_name = excluded.display_name,
                    relative_path = excluded.relative_path,
                    source_path = excluded.source_path,
                    current_revision = excluded.current_revision,
                    content_hash = excluded.content_hash,
                    file_size = excluded.file_size,
                    mime_type = excluded.mime_type,
                    parse_status = excluded.parse_status,
                    index_status = excluded.index_status,
                    parser_version = excluded.parser_version,
                    chunk_config_hash = excluded.chunk_config_hash,
                    updated_at = excluded.updated_at",
                params![
                    document_id,
                    DEFAULT_FOX_GUIDE_ID,
                    display_name,
                    document.relative_path,
                    stored_path.to_string_lossy(),
                    revision,
                    content_hash,
                    i64::try_from(document.content.len()).unwrap_or(i64::MAX),
                    now,
                ],
            )?;
            transaction.execute(
                "DELETE FROM local_kb_chunks WHERE document_id = ?1",
                [&document_id],
            )?;
            for chunk in chunks {
                transaction.execute(
                    "INSERT INTO local_kb_chunks(
                        id, document_id, document_revision, chunk_index, text,
                        anchor, start_offset, end_offset, metadata_json
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, '{}')",
                    params![
                        Uuid::new_v4().to_string(),
                        document_id,
                        revision,
                        i64::try_from(chunk.chunk_index).unwrap_or(i64::MAX),
                        chunk.text,
                        chunk.anchor,
                        i64::try_from(chunk.start_offset).unwrap_or(i64::MAX),
                        i64::try_from(chunk.end_offset).unwrap_or(i64::MAX),
                    ],
                )?;
            }
            transaction.commit()?;
            changed = true;
        }

        if changed {
            self.connection()?.execute(
                "UPDATE local_knowledge_bases SET updated_at = ?2 WHERE id = ?1",
                params![DEFAULT_FOX_GUIDE_ID, now],
            )?;
        }
        Ok(())
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, LocalKnowledgeError> {
        self.connection
            .lock()
            .map_err(|_| LocalKnowledgeError::storage("knowledge database lock is poisoned"))
    }

    fn migration_active(&self) -> bool {
        self.storage_migration_active
            .lock()
            .ok()
            .and_then(|migration| migration.as_ref().map(|_| true))
            .unwrap_or(false)
    }

    fn reconcile_storage_migration_on_open(&self) -> Result<(), LocalKnowledgeError> {
        let current_root = self.root.to_string_lossy().into_owned();
        let connection = self.connection()?;
        connection.execute(
            "UPDATE local_kb_storage_migrations
             SET restart_required = 0, updated_at = ?2, last_sequence = last_sequence + 1
             WHERE status = 'completed' AND restart_required = 1
               AND lower(destination_path) = lower(?1)",
            params![current_root, crate::database::now_ms()],
        )?;
        let pending_root = connection
            .query_row(
                "SELECT destination_path
                 FROM local_kb_storage_migrations
                 WHERE status = 'completed' AND restart_required = 1
                 ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        *self
            .pending_root_path
            .lock()
            .map_err(|_| LocalKnowledgeError::storage("storage migration lock is poisoned"))? =
            pending_root.map(PathBuf::from);
        Ok(())
    }

    fn set_migration_active(
        &self,
        migration_id: Option<String>,
    ) -> Result<(), LocalKnowledgeError> {
        *self.storage_migration_active.lock().map_err(|_| {
            LocalKnowledgeError::storage("storage migration state lock is poisoned")
        })? = migration_id;
        Ok(())
    }

    fn claim_migration(&self, migration_id: &str) -> Result<(), LocalKnowledgeError> {
        let mut active = self.storage_migration_active.lock().map_err(|_| {
            LocalKnowledgeError::storage("storage migration state lock is poisoned")
        })?;
        if active.is_some() {
            return Err(LocalKnowledgeError::conflict("已有知识库存储迁移正在进行"));
        }
        *active = Some(migration_id.to_owned());
        Ok(())
    }

    fn update_storage_migration(
        &self,
        id: &str,
        status: &str,
        stage: &str,
        progress: i64,
        error: Option<&StorageMigrationError>,
        restart_required: bool,
    ) -> Result<(), LocalKnowledgeError> {
        let now = crate::database::now_ms();
        let (error_code, error_message) = error
            .map(|error| (Some(error.code), Some(error.message.as_str())))
            .unwrap_or((None, None));
        self.connection()?.execute(
            "UPDATE local_kb_storage_migrations
             SET status = ?2, stage = ?3, progress = ?4, last_sequence = last_sequence + 1,
                 error_code = ?5, error_message = ?6, restart_required = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                id,
                status,
                stage,
                progress,
                error_code,
                error_message,
                if restart_required { 1 } else { 0 },
                now
            ],
        )?;
        Ok(())
    }

    fn quiesce_jobs(&self) -> Result<(), LocalKnowledgeError> {
        self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'cancelled', error_code = 'local_knowledge.storage_migration_quiescing',
                 error_message = '知识库存储迁移开始，排队任务已取消',
                 updated_at = ?1, completed_at = ?1, last_sequence = last_sequence + 1
             WHERE status = 'queued'",
            [crate::database::now_ms()],
        )?;
        for _ in 0..300 {
            let active_jobs: i64 = self.connection()?.query_row(
                "SELECT COUNT(*) FROM local_kb_jobs WHERE status IN ('running', 'paused')",
                [],
                |row| row.get(0),
            )?;
            if active_jobs == 0 {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(LocalKnowledgeError::storage_migration(
            StorageMigrationError::new(
                "local_knowledge.storage_migration_quiesce_timeout",
                "等待知识库任务停止超时，已保持原存储目录",
                true,
            ),
        ))
    }

    pub fn start_storage_migration(
        &self,
        database: Database,
        request: StorageMigrationStartRequest,
    ) -> Result<StorageMigration, LocalKnowledgeError> {
        let destination = validate_destination_storage_root(
            self.root.as_ref(),
            Path::new(request.destination_path.trim()),
        )
        .map_err(LocalKnowledgeError::storage_migration)?;
        let migration_id = Uuid::new_v4().to_string();
        self.claim_migration(&migration_id)?;
        let now = crate::database::now_ms();
        let insert_result = self.connection()?.execute(
            "INSERT INTO local_kb_storage_migrations(
                 id, source_path, destination_path, status, stage, progress,
                 last_sequence, created_at, updated_at, restart_required
             ) VALUES (?1, ?2, ?3, 'queued', 'validating', 0, 1, ?4, ?4, 0)",
            params![
                migration_id,
                self.root.to_string_lossy().as_ref(),
                destination.to_string_lossy().as_ref(),
                now
            ],
        );
        if let Err(error) = insert_result {
            let _ = self.set_migration_active(None);
            return Err(LocalKnowledgeError::from(error));
        }
        let migration = self.get_storage_migration(&migration_id)?;
        let store = self.clone();
        let migration_id_for_thread = migration_id.clone();
        let destination_for_thread = destination.clone();
        let spawn_result = thread::Builder::new()
            .name(format!(
                "fox-kb-storage-migration-{migration_id_for_thread}"
            ))
            .spawn(move || {
                store.run_storage_migration(
                    &database,
                    &migration_id_for_thread,
                    &destination_for_thread,
                );
            });
        if let Err(error) = spawn_result {
            let storage_error = StorageMigrationError::new(
                "local_knowledge.storage_migration_worker_failed",
                format!("无法启动存储迁移任务：{error}"),
                true,
            );
            let _ = self.update_storage_migration(
                &migration_id,
                "failed",
                "validating",
                0,
                Some(&storage_error),
                false,
            );
            let _ = self.set_migration_active(None);
            return Err(LocalKnowledgeError::storage_migration(storage_error));
        }
        Ok(migration)
    }

    fn run_storage_migration(&self, database: &Database, migration_id: &str, destination: &Path) {
        let staging = match create_staging_path(destination, migration_id) {
            Ok(path) => path,
            Err(error) => {
                let _ = self.update_storage_migration(
                    migration_id,
                    "failed",
                    "validating",
                    0,
                    Some(&error),
                    false,
                );
                let _ = self.set_migration_active(None);
                return;
            }
        };
        let result = self.perform_storage_migration(database, migration_id, destination, &staging);
        match result {
            Ok(()) => {
                let _ = self.set_migration_active(None);
            }
            Err(error) => {
                let cleanup_error = cleanup_staging(&staging, destination).err();
                let final_error = cleanup_error.as_ref().unwrap_or(&error);
                let message = if cleanup_error.is_some() {
                    format!(
                        "{}；临时目录清理失败：{}",
                        error.message, final_error.message
                    )
                } else {
                    error.message.clone()
                };
                let recorded_error =
                    StorageMigrationError::new(error.code, message, error.retryable);
                let stage = if cleanup_error.is_some() {
                    "awaiting_cleanup"
                } else {
                    "rolling_back"
                };
                let _ = self.update_storage_migration(
                    migration_id,
                    "failed",
                    stage,
                    0,
                    Some(&recorded_error),
                    false,
                );
                let _ = self.set_migration_active(None);
            }
        }
    }

    fn perform_storage_migration(
        &self,
        database: &Database,
        migration_id: &str,
        destination: &Path,
        staging: &Path,
    ) -> Result<(), StorageMigrationError> {
        self.update_storage_migration(migration_id, "running", "validating", 5, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        self.quiesce_jobs()
            .map_err(|error| StorageMigrationError::new(error.code(), error.to_string(), true))?;
        self.update_storage_migration(migration_id, "running", "quiescing", 20, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        self.update_storage_migration(migration_id, "running", "flushing", 30, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        self.update_storage_migration(migration_id, "running", "closing_handles", 35, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        self.update_storage_migration(migration_id, "running", "copying", 45, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        let connection = self.connection().map_err(|error| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_lock_failed",
                error.to_string(),
                true,
            )
        })?;
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_flush_failed",
                    format!("无法完成 knowledge.db WAL checkpoint：{error}"),
                    true,
                )
            })?;
        copy_managed_directory(self.root.as_ref(), staging)?;
        drop(connection);
        self.update_storage_migration(migration_id, "running", "verifying", 75, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        verify_managed_directory(self.root.as_ref(), staging)?;
        self.update_storage_migration(migration_id, "running", "switching", 88, None, false)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        commit_staging(staging, destination)?;
        if let Err(error) = database.set_app_setting(
            crate::local_knowledge_storage::LOCAL_KNOWLEDGE_STORAGE_PATH_KEY,
            &destination.to_string_lossy(),
        ) {
            let rollback = rollback_committed_destination(destination, staging);
            if let Err(rollback_error) = rollback {
                return Err(StorageMigrationError::new(
                    "local_knowledge.storage_migration_rollback_failed",
                    format!(
                        "配置写入失败：{error}；回滚失败：{}",
                        rollback_error.message
                    ),
                    true,
                ));
            }
            return Err(StorageMigrationError::new(
                "local_knowledge.storage_migration_config_failed",
                format!("无法写入知识库存储路径配置：{error}"),
                true,
            ));
        }
        *self.pending_root_path.lock().map_err(|_| {
            StorageMigrationError::new(
                "local_knowledge.storage_migration_failed",
                "storage migration lock is poisoned",
                true,
            )
        })? = Some(destination.to_path_buf());
        self.update_storage_migration(migration_id, "completed", "self_checking", 100, None, true)
            .map_err(|error| {
                StorageMigrationError::new(
                    "local_knowledge.storage_migration_failed",
                    error.to_string(),
                    true,
                )
            })?;
        Ok(())
    }

    pub fn get_storage_migration(&self, id: &str) -> Result<StorageMigration, LocalKnowledgeError> {
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT id, source_path, destination_path, status, stage, progress,
                        last_sequence, error_code, error_message, created_at, updated_at,
                        restart_required
                 FROM local_kb_storage_migrations WHERE id = ?1",
                [id],
                map_storage_migration,
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("storage migration"))
    }

    fn recover_startup_jobs(&self) -> Result<(), LocalKnowledgeError> {
        let now = crate::database::now_ms();
        self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'interrupted', updated_at = ?1, completed_at = ?1,
                 error_code = 'local_knowledge.job_interrupted',
                 error_message = 'Fox exited before this job completed',
                 last_sequence = last_sequence + 1
             WHERE status = 'running'",
            [now],
        )?;
        let queued_jobs = self
            .list_jobs(None)?
            .into_iter()
            .filter(|job| job.status == "queued")
            .collect::<Vec<_>>();
        for job in queued_jobs {
            let resumed = match job.job_type.as_str() {
                "import" => self.resume_import_job(&job),
                "parse" => self.resume_reparse_job(&job),
                "index" | "rebuild" => self.resume_index_job(&job),
                _ => Ok(()),
            };
            if let Err(error) = resumed {
                self.fail_job(&job.id, &error)?;
            }
        }
        Ok(())
    }

    pub fn list_bases(&self) -> Result<Vec<LocalKnowledgeBase>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT b.id, b.name, b.description, b.active_index_generation,
                    COUNT(d.id) AS document_count,
                    (SELECT j.status FROM local_kb_jobs j
                     WHERE j.knowledge_base_id = b.id
                       AND j.status IN ('queued', 'running', 'paused')
                     ORDER BY j.updated_at DESC LIMIT 1) AS active_job_status,
                    b.created_at, b.updated_at
             FROM local_knowledge_bases b
             LEFT JOIN local_kb_documents d ON d.knowledge_base_id = b.id
             GROUP BY b.id
             ORDER BY b.updated_at DESC, b.name COLLATE NOCASE",
        )?;
        let items = statement
            .query_map([], map_knowledge_base)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LocalKnowledgeError::from)?;
        let mut items = items;
        for base in &mut items {
            self.populate_capability_status(&connection, base)?;
        }
        Ok(items)
    }

    fn populate_capability_status(
        &self,
        connection: &Connection,
        base: &mut LocalKnowledgeBase,
    ) -> Result<(), LocalKnowledgeError> {
        let (configured_model_id, chunk_size, chunk_overlap, configured_search_mode) = connection
            .query_row(
            "SELECT embedding_model_id, chunk_size, chunk_overlap, search_mode
                 FROM local_knowledge_bases WHERE id = ?1",
            [&base.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        base.configured_embedding_model_id = configured_model_id;
        base.chunk_size = chunk_size;
        base.chunk_overlap = chunk_overlap;
        let chunk_count: i64 = connection.query_row(
            "SELECT COUNT(c.id)
             FROM local_kb_chunks c
             JOIN local_kb_documents d ON d.id = c.document_id
             WHERE d.knowledge_base_id = ?1
               AND d.parse_status = 'ready'
               AND c.document_revision = d.current_revision",
            [&base.id],
            |row| row.get(0),
        )?;
        let ready_document_count: i64 = connection.query_row(
            "SELECT COUNT(d.id)
             FROM local_kb_documents d
             WHERE d.knowledge_base_id = ?1
               AND d.parse_status = 'ready'
               AND EXISTS (
                 SELECT 1 FROM local_kb_chunks c
                 WHERE c.document_id = d.id
                   AND c.document_revision = d.current_revision
               )",
            [&base.id],
            |row| row.get(0),
        )?;
        let last_indexed_at: Option<i64> = connection.query_row(
            "SELECT MAX(updated_at)
             FROM local_kb_documents
             WHERE knowledge_base_id = ?1
               AND parse_status = 'ready'",
            [&base.id],
            |row| row.get(0),
        )?;
        let embedding_model: Option<LocalKnowledgeEmbeddingModel> =
            match base.configured_embedding_model_id.as_deref() {
                Some(model_id) => connection
                    .query_row(
                        "SELECT model_id, name, version, dimension
                         FROM local_embedding_models
                         WHERE model_id = ?1 AND status = 'ready'
                           AND integrity_status = 'verified'
                         ORDER BY updated_at DESC LIMIT 1",
                        [model_id],
                        |row| {
                            Ok(LocalKnowledgeEmbeddingModel {
                                id: row.get(0)?,
                                name: row.get(1)?,
                                version: row.get(2)?,
                                dimension: row.get(3)?,
                            })
                        },
                    )
                    .optional()?,
                None => None,
            };
        let active_generation_matches_configuration = match (
            base.active_index_generation.as_deref(),
            base.configured_embedding_model_id.as_deref(),
            embedding_model.as_ref(),
        ) {
            (Some(generation_id), Some(model_id), Some(model)) => connection.query_row(
                "SELECT EXISTS(
                        SELECT 1 FROM local_kb_index_generations
                        WHERE id = ?1 AND knowledge_base_id = ?2 AND status = 'active'
                          AND vector_store_kind = 'zvec-hnsw-cosine-v1'
                          AND embedding_model_id = ?3 AND chunk_config_hash = ?4
                          AND embedding_model_version = ?5
                     )",
                params![
                    generation_id,
                    &base.id,
                    model_id,
                    format!("chars-{}-overlap-{}", base.chunk_size, base.chunk_overlap),
                    &model.version
                ],
                |row| row.get::<_, bool>(0),
            )?,
            _ => false,
        };

        let vector_generation = match base.active_index_generation.as_deref() {
            Some(generation_id) => connection
                .query_row(
                    "SELECT dimension, chunk_count FROM local_kb_index_generations
                 WHERE id = ?1 AND knowledge_base_id = ?2 AND status = 'active'
                   AND vector_store_kind = 'zvec-hnsw-cosine-v1'",
                    params![generation_id, &base.id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
                .optional()?,
            None => None,
        };
        #[cfg(feature = "zvec")]
        let zvec_generation_error =
            match (base.active_index_generation.as_deref(), vector_generation) {
                (Some(generation_id), Some((dimension, count))) if dimension > 0 && count >= 0 => {
                    self.validate_zvec_generation_readable(
                        &base.id,
                        generation_id,
                        dimension as usize,
                        count as usize,
                    )
                    .err()
                    .map(|error| error.to_string())
                }
                _ => None,
            };
        #[cfg(not(feature = "zvec"))]
        let zvec_generation_error = vector_generation
            .is_some()
            .then(|| "当前桌面构建未启用 Zvec".to_owned());
        let zvec_generation_readable =
            vector_generation.is_some() && zvec_generation_error.is_none();
        let vector_count = vector_generation
            .filter(|_| zvec_generation_readable)
            .map(|(_, count)| count);
        base.text_index_ready = base.document_count > 0
            && ready_document_count == base.document_count
            && chunk_count > 0;
        base.vector_index_ready = embedding_model.is_some()
            && active_generation_matches_configuration
            && zvec_generation_readable
            && vector_count.is_some_and(|count| count > 0 && count == chunk_count);
        base.search_mode =
            if configured_search_mode == "vector" || configured_search_mode == "hybrid" {
                if base.vector_index_ready {
                    configured_search_mode
                } else {
                    "keyword".to_owned()
                }
            } else {
                "keyword".to_owned()
            };
        base.embedding_model = embedding_model;
        base.chunk_count = Some(chunk_count.max(0));
        base.vector_count = vector_count;
        base.last_indexed_at = last_indexed_at;
        base.fallback_reason = if base.vector_index_ready {
            None
        } else {
            Some(if base.configured_embedding_model_id.is_none() {
                "未配置向量模型".to_owned()
            } else if base.embedding_model.is_none() {
                "向量模型未安装或未通过完整性校验".to_owned()
            } else if base.active_index_generation.is_some() && vector_generation.is_none() {
                "当前索引不是可用的 Zvec 索引，请重新构建".to_owned()
            } else if let Some(error) = zvec_generation_error {
                format!("Zvec 索引不可用：{error}")
            } else {
                "Zvec 向量索引尚未完成读写校验".to_owned()
            })
        };
        Ok(())
    }

    pub fn list_file_sources(&self) -> Result<Vec<LocalKnowledgeFileSource>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT id, root_path, display_name, file_count, total_size,
                    last_scanned_at, created_at, updated_at
             FROM local_file_sources
             ORDER BY display_name COLLATE NOCASE, root_path COLLATE NOCASE",
        )?;
        let items = statement
            .query_map([], map_file_source)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LocalKnowledgeError::from)?;
        Ok(items)
    }

    pub fn add_file_source(
        &self,
        request: LocalKnowledgeFileSourcePathRequest,
    ) -> Result<LocalKnowledgeFileSource, LocalKnowledgeError> {
        let root = canonical_file_source_root(Path::new(request.path.trim()), self.root.as_ref())?;
        let root_path = root.to_string_lossy().into_owned();
        let display_name = root
            .file_name()
            .and_then(|value| value.to_str())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(root_path.as_str())
            .to_owned();
        let source_id = self
            .connection()?
            .query_row(
                "SELECT id FROM local_file_sources WHERE lower(root_path) = lower(?1)",
                [&root_path],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let now = crate::database::now_ms();
        self.connection()?.execute(
            "INSERT INTO local_file_sources(
                 id, root_path, display_name, file_count, total_size,
                 last_scanned_at, created_at, updated_at
             ) VALUES (?1, ?2, ?3, 0, 0, NULL, ?4, ?4)
             ON CONFLICT(id) DO UPDATE SET
                 root_path = excluded.root_path,
                 display_name = excluded.display_name,
                 updated_at = excluded.updated_at",
            params![source_id, root_path, display_name, now],
        )?;
        self.scan_file_source(&source_id)
    }

    pub fn scan_file_source(
        &self,
        source_id: &str,
    ) -> Result<LocalKnowledgeFileSource, LocalKnowledgeError> {
        let root_path = self
            .connection()?
            .query_row(
                "SELECT root_path FROM local_file_sources WHERE id = ?1",
                [source_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("local file source"))?;
        let root = canonical_file_source_root(Path::new(&root_path), self.root.as_ref())?;
        let files = scan_catalog_files(&root, self.root.as_ref())?;
        let total_size = files.iter().map(|file| file.file_size).sum::<i64>();
        let now = crate::database::now_ms();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM local_file_catalog WHERE source_id = ?1",
            [source_id],
        )?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO local_file_catalog(
                     id, source_id, absolute_path, relative_path, display_name,
                     extension, mime_type, file_size, modified_at, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
            )?;
            for file in &files {
                statement.execute(params![
                    Uuid::new_v4().to_string(),
                    source_id,
                    file.absolute_path,
                    file.relative_path,
                    file.display_name,
                    file.extension,
                    file.mime_type,
                    file.file_size,
                    file.modified_at,
                    now,
                ])?;
            }
        }
        transaction.execute(
            "UPDATE local_file_sources
             SET file_count = ?2, total_size = ?3, last_scanned_at = ?4, updated_at = ?4
             WHERE id = ?1",
            params![
                source_id,
                i64::try_from(files.len()).unwrap_or(i64::MAX),
                total_size,
                now,
            ],
        )?;
        transaction.commit()?;
        drop(connection);
        self.get_file_source(source_id)
    }

    pub fn remove_file_source(&self, source_id: &str) -> Result<bool, LocalKnowledgeError> {
        Ok(self
            .connection()?
            .execute("DELETE FROM local_file_sources WHERE id = ?1", [source_id])?
            > 0)
    }

    pub fn update_file_source(
        &self,
        request: LocalKnowledgeFileSourceUpdateRequest,
    ) -> Result<LocalKnowledgeFileSource, LocalKnowledgeError> {
        let source_id = request.id.trim();
        let display_name = request.display_name.trim();
        if source_id.is_empty() || display_name.is_empty() {
            return Err(LocalKnowledgeError::invalid(
                "file source id and display name are required",
            ));
        }
        if display_name.chars().count() > 80 {
            return Err(LocalKnowledgeError::invalid(
                "file source display name is too long",
            ));
        }
        let updated_at = crate::database::now_ms();
        let changed = self.connection()?.execute(
            "UPDATE local_file_sources SET display_name = ?1, updated_at = ?2 WHERE id = ?3",
            params![display_name, updated_at, source_id],
        )?;
        if changed == 0 {
            return Err(LocalKnowledgeError::not_found("local file source"));
        }
        self.get_file_source(source_id)
    }

    pub fn list_catalog_files(
        &self,
        request: LocalKnowledgeCatalogFilesRequest,
    ) -> Result<Vec<LocalKnowledgeCatalogFile>, LocalKnowledgeError> {
        let mut clauses = Vec::new();
        let mut values = Vec::<SqlValue>::new();
        if let Some(source_id) = request
            .source_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            clauses.push(format!("f.source_id = ?{}", values.len() + 1));
            values.push(SqlValue::Text(source_id.to_owned()));
        }
        if let Some(query) = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            clauses.push(format!(
                "(f.display_name LIKE ?{} OR f.relative_path LIKE ?{})",
                values.len() + 1,
                values.len() + 1
            ));
            values.push(SqlValue::Text(format!("%{query}%")));
        }
        if let Some(extensions) = catalog_category_extensions(request.category.as_deref())? {
            let placeholders = extensions
                .iter()
                .map(|_| {
                    values.push(SqlValue::Text(String::new()));
                    format!("?{}", values.len())
                })
                .collect::<Vec<_>>();
            let start = values.len() - extensions.len();
            for (index, extension) in extensions.iter().enumerate() {
                values[start + index] = SqlValue::Text((*extension).to_owned());
            }
            clauses.push(format!("f.extension IN ({})", placeholders.join(", ")));
        }
        let limit = request
            .limit
            .unwrap_or(DEFAULT_LOCAL_FILE_LIST_LIMIT)
            .clamp(1, MAX_LOCAL_FILE_LIST_LIMIT);
        let offset = request
            .offset
            .unwrap_or_default()
            .min(MAX_LOCAL_FILE_CATALOG_ITEMS);
        values.push(SqlValue::Integer(i64::try_from(limit).unwrap_or(i64::MAX)));
        let limit_parameter = values.len();
        values.push(SqlValue::Integer(i64::try_from(offset).unwrap_or(i64::MAX)));
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        let sql = format!(
            "SELECT f.id, f.source_id, s.display_name, s.root_path, f.absolute_path,
                    f.relative_path, f.display_name, f.extension, f.mime_type,
                    f.file_size, f.modified_at
             FROM local_file_catalog f
             JOIN local_file_sources s ON s.id = f.source_id
             {where_clause}
             ORDER BY f.modified_at DESC, f.display_name COLLATE NOCASE
             LIMIT ?{limit_parameter} OFFSET ?{}",
            values.len(),
        );
        let connection = self.connection()?;
        let mut statement = connection.prepare(&sql)?;
        let items = statement
            .query_map(params_from_iter(values), map_catalog_file)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LocalKnowledgeError::from)?;
        Ok(items)
    }

    pub fn read_catalog_file_range(
        &self,
        request: LocalKnowledgeCatalogFileRangeRequest,
    ) -> Result<Vec<u8>, LocalKnowledgeError> {
        let path = self.validated_catalog_file_path(&request.id)?;
        read_file_range(path, request.start, request.end)
    }

    pub fn open_catalog_file(
        &self,
        request: LocalKnowledgeCatalogFileOpenRequest,
    ) -> Result<bool, LocalKnowledgeError> {
        let path = self.validated_catalog_file_path(&request.id)?;
        crate::commands::open_downloaded_file(&path, request.reveal)
            .map_err(LocalKnowledgeError::storage)?;
        Ok(true)
    }

    fn validated_catalog_file_path(&self, id: &str) -> Result<PathBuf, LocalKnowledgeError> {
        let id = id.trim();
        if id.is_empty() {
            return Err(LocalKnowledgeError::invalid("local file id is required"));
        }
        let connection = self.connection()?;
        let paths = connection
            .query_row(
                "SELECT f.absolute_path, s.root_path
                 FROM local_file_catalog f
                 JOIN local_file_sources s ON s.id = f.source_id
                 WHERE f.id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("local catalog file"))?;
        drop(connection);
        let root = PathBuf::from(paths.1).canonicalize()?;
        let path = PathBuf::from(paths.0);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(LocalKnowledgeError::invalid(
                "local catalog entry is no longer a normal file",
            ));
        }
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(root) {
            return Err(LocalKnowledgeError::invalid(
                "local catalog file is outside its registered source folder",
            ));
        }
        Ok(canonical)
    }

    pub fn read_document_file_range(
        &self,
        request: LocalKnowledgeDocumentFileRangeRequest,
    ) -> Result<Vec<u8>, LocalKnowledgeError> {
        let path =
            self.validated_document_file_path(&request.knowledge_base_id, &request.document_id)?;
        read_file_range(path, request.start, request.end)
    }

    pub fn open_document_file(
        &self,
        request: LocalKnowledgeDocumentFileOpenRequest,
    ) -> Result<bool, LocalKnowledgeError> {
        let path =
            self.validated_document_file_path(&request.knowledge_base_id, &request.document_id)?;
        crate::commands::open_downloaded_file(&path, request.reveal)
            .map_err(LocalKnowledgeError::storage)?;
        Ok(true)
    }

    fn validated_document_file_path(
        &self,
        knowledge_base_id: &str,
        document_id: &str,
    ) -> Result<PathBuf, LocalKnowledgeError> {
        let knowledge_base_id = knowledge_base_id.trim();
        let document_id = document_id.trim();
        if knowledge_base_id.is_empty() || document_id.is_empty() {
            return Err(LocalKnowledgeError::invalid(
                "knowledge base id and document id are required",
            ));
        }
        let relative_path = self
            .connection()?
            .query_row(
                "SELECT relative_path FROM local_kb_documents
                 WHERE knowledge_base_id = ?1 AND id = ?2",
                params![knowledge_base_id, document_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("knowledge document"))?;
        let documents_root = self
            .root
            .join("knowledge-bases")
            .join(knowledge_base_id)
            .join("documents");
        let canonical_root = documents_root.canonicalize()?;
        let path = documents_root.join(relative_path);
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(LocalKnowledgeError::invalid(
                "knowledge document is no longer a normal file",
            ));
        }
        let canonical = path.canonicalize()?;
        if !canonical.starts_with(canonical_root) {
            return Err(LocalKnowledgeError::invalid(
                "knowledge document is outside its managed folder",
            ));
        }
        Ok(canonical)
    }

    fn get_file_source(
        &self,
        source_id: &str,
    ) -> Result<LocalKnowledgeFileSource, LocalKnowledgeError> {
        self.connection()?
            .query_row(
                "SELECT id, root_path, display_name, file_count, total_size,
                        last_scanned_at, created_at, updated_at
                 FROM local_file_sources WHERE id = ?1",
                [source_id],
                map_file_source,
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("local file source"))
    }

    pub fn list_bases_for_ids(
        &self,
        knowledge_base_ids: &[String],
    ) -> Result<Vec<LocalKnowledgeBase>, LocalKnowledgeError> {
        let mut bases = Vec::with_capacity(knowledge_base_ids.len());
        for id in knowledge_base_ids {
            if id.trim().is_empty() {
                return Err(LocalKnowledgeError::invalid(
                    "knowledge base reference id is required",
                ));
            }
            bases.push(self.get_base(id)?);
        }
        Ok(bases)
    }

    pub fn search_lexical(
        &self,
        knowledge_base_id: &str,
        query: &str,
        requested_limit: Option<usize>,
        requested_max_chars: Option<usize>,
    ) -> Result<Value, LocalKnowledgeError> {
        self.get_base(knowledge_base_id)?;
        let query = query.trim();
        if query.is_empty() {
            return Err(LocalKnowledgeError::invalid("query is required"));
        }
        if query.chars().count() > MAX_LOCAL_QUERY_CHARS {
            return Err(LocalKnowledgeError::invalid(format!(
                "query cannot exceed {MAX_LOCAL_QUERY_CHARS} characters"
            )));
        }

        let terms = lexical_terms(query);
        let result_limit = requested_limit
            .unwrap_or(DEFAULT_LOCAL_SEARCH_RESULTS)
            .clamp(1, MAX_LOCAL_SEARCH_RESULTS);
        let max_chars = requested_max_chars
            .unwrap_or(DEFAULT_LOCAL_SEARCH_CHARS)
            .clamp(1, MAX_LOCAL_SEARCH_CHARS);
        let term_predicates = (0..terms.len())
            .map(|index| format!("INSTR(LOWER(c.text), LOWER(?{})) > 0", index + 2))
            .collect::<Vec<_>>();
        let sql = format!(
            "SELECT c.id, c.document_id, c.chunk_index, c.text, c.anchor,
                    c.start_offset, c.end_offset, c.metadata_json, c.page_number,
                    c.slide_number, d.display_name, d.relative_path, d.mime_type,
                    d.content_hash
             FROM local_kb_chunks c
             JOIN local_kb_documents d ON d.id = c.document_id
             WHERE d.knowledge_base_id = ?1
               AND d.parse_status = 'ready'
               AND c.document_revision = d.current_revision
               AND ({})
             ORDER BY d.updated_at DESC, c.chunk_index ASC
             LIMIT ?{}",
            term_predicates.join(" OR "),
            terms.len() + 2
        );
        let mut sql_params = Vec::with_capacity(terms.len() + 2);
        sql_params.push(rusqlite::types::Value::Text(knowledge_base_id.to_owned()));
        sql_params.extend(
            terms
                .iter()
                .map(|term| rusqlite::types::Value::Text(term.to_lowercase())),
        );
        sql_params.push(rusqlite::types::Value::Integer(
            i64::try_from(MAX_LOCAL_SEARCH_CANDIDATES).unwrap_or(i64::MAX),
        ));

        let connection = self.connection()?;
        let mut statement = connection.prepare(&sql)?;
        let mut candidates = statement
            .query_map(params_from_iter(sql_params), map_lexical_chunk)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LocalKnowledgeError::from)?;
        candidates.sort_by(|left, right| {
            lexical_score(&right.text, &terms)
                .total_cmp(&lexical_score(&left.text, &terms))
                .then_with(|| left.document_id.cmp(&right.document_id))
                .then_with(|| left.chunk_index.cmp(&right.chunk_index))
        });

        let candidate_count = candidates.len();
        let mut items = Vec::new();
        let mut used_chars = 0usize;
        let mut truncated = candidate_count > result_limit;
        for candidate in candidates.into_iter().take(result_limit) {
            if used_chars >= max_chars {
                truncated = true;
                break;
            }
            let remaining = max_chars - used_chars;
            let content = truncate_chars(&candidate.text, remaining);
            if content.len() < candidate.text.len() {
                truncated = true;
            }
            used_chars += content.chars().count();
            items.push(json!({
                "documentId": candidate.document_id,
                "chunkId": candidate.id,
                "anchor": candidate.anchor,
                "score": lexical_score(&candidate.text, &terms),
                "content": content,
                "source": "local",
                "sourceMetadata": source_metadata(knowledge_base_id, &candidate),
            }));
        }

        Ok(json!({
            "source": "local",
            "knowledgeBaseId": knowledge_base_id,
            "query": query,
            "items": items,
            "total": candidate_count,
            "truncated": truncated,
            "fallback": {
                "active": true,
                "kind": "sqlite_lexical",
                "reason": "vector_index_unavailable"
            }
        }))
    }

    pub fn read_document(
        &self,
        knowledge_base_id: &str,
        document_id: &str,
        chunk_id: Option<&str>,
        requested_max_chars: Option<usize>,
    ) -> Result<Value, LocalKnowledgeError> {
        self.get_base(knowledge_base_id)?;
        let max_chars = requested_max_chars
            .unwrap_or(DEFAULT_LOCAL_SEARCH_CHARS)
            .clamp(1, MAX_LOCAL_READ_CHARS);
        let connection = self.connection()?;
        let document = query_document(&connection, knowledge_base_id, document_id)?
            .ok_or_else(|| LocalKnowledgeError::not_found("knowledge document"))?;
        let mut sql = String::from(
            "SELECT c.id, c.document_id, c.chunk_index, c.text, c.anchor,
                    c.start_offset, c.end_offset, c.metadata_json, c.page_number,
                    c.slide_number, d.display_name, d.relative_path, d.mime_type,
                    d.content_hash
             FROM local_kb_chunks c
             JOIN local_kb_documents d ON d.id = c.document_id
             WHERE d.knowledge_base_id = ?1
               AND d.id = ?2
               AND c.document_revision = d.current_revision",
        );
        let mut sql_params = vec![
            rusqlite::types::Value::Text(knowledge_base_id.to_owned()),
            rusqlite::types::Value::Text(document_id.to_owned()),
        ];
        if let Some(chunk_id) = chunk_id.filter(|value| !value.trim().is_empty()) {
            sql.push_str(" AND c.id = ?3");
            sql_params.push(rusqlite::types::Value::Text(chunk_id.to_owned()));
        }
        sql.push_str(" ORDER BY c.chunk_index ASC");

        let mut statement = connection.prepare(&sql)?;
        let chunks = statement
            .query_map(params_from_iter(sql_params), map_lexical_chunk)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(LocalKnowledgeError::from)?;
        if chunk_id.is_some() && chunks.is_empty() {
            return Err(LocalKnowledgeError::not_found("knowledge chunk"));
        }

        let mut used_chars = 0usize;
        let mut truncated = false;
        let mut chunk_items = Vec::new();
        let mut content_parts = Vec::new();
        for chunk in chunks {
            if used_chars >= max_chars {
                truncated = true;
                break;
            }
            let content = truncate_chars(&chunk.text, max_chars - used_chars);
            if content.len() < chunk.text.len() {
                truncated = true;
            }
            used_chars += content.chars().count();
            content_parts.push(content.clone());
            chunk_items.push(json!({
                "chunkId": chunk.id,
                "documentId": chunk.document_id,
                "anchor": chunk.anchor,
                "content": content,
                "source": "local",
                "sourceMetadata": source_metadata(knowledge_base_id, &chunk),
            }));
        }

        Ok(json!({
            "source": "local",
            "knowledgeBaseId": knowledge_base_id,
            "documentId": document_id,
            "document": document,
            "chunkId": chunk_id.filter(|value| !value.trim().is_empty()),
            "content": content_parts.join("\n\n"),
            "chunks": chunk_items,
            "truncated": truncated,
        }))
    }

    pub fn get_base(&self, id: &str) -> Result<LocalKnowledgeBase, LocalKnowledgeError> {
        let connection = self.connection()?;
        let mut base = connection
            .query_row(
                "SELECT b.id, b.name, b.description, b.active_index_generation,
                        COUNT(d.id) AS document_count,
                        (SELECT j.status FROM local_kb_jobs j
                         WHERE j.knowledge_base_id = b.id
                           AND j.status IN ('queued', 'running', 'paused')
                         ORDER BY j.updated_at DESC LIMIT 1) AS active_job_status,
                        b.created_at, b.updated_at
                 FROM local_knowledge_bases b
                 LEFT JOIN local_kb_documents d ON d.knowledge_base_id = b.id
                 WHERE b.id = ?1
                 GROUP BY b.id",
                [id],
                map_knowledge_base,
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("knowledge base"))?;
        self.populate_capability_status(&connection, &mut base)?;
        Ok(base)
    }

    pub fn create_base(
        &self,
        request: CreateLocalKnowledgeBaseRequest,
    ) -> Result<LocalKnowledgeBase, LocalKnowledgeError> {
        let name = normalized_name(&request.name)?;
        let connection = self.connection()?;
        let configuration = normalized_base_configuration(
            &connection,
            request.embedding_model_id,
            request.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE),
            request.chunk_overlap.unwrap_or(DEFAULT_CHUNK_OVERLAP),
            request.search_mode.as_deref().unwrap_or("keyword"),
        )?;
        let id = Uuid::new_v4().to_string();
        let now = crate::database::now_ms();
        connection.execute(
            "INSERT INTO local_knowledge_bases(
                id, name, description, embedding_model_id, chunk_size,
                chunk_overlap, search_mode, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                id,
                name,
                normalized_description(request.description),
                configuration.0,
                configuration.1,
                configuration.2,
                configuration.3,
                now
            ],
        )?;
        drop(connection);
        self.get_base(&id)
    }

    pub fn update_base(
        &self,
        request: UpdateLocalKnowledgeBaseRequest,
    ) -> Result<LocalKnowledgeBase, LocalKnowledgeError> {
        if request.id == DEFAULT_FOX_GUIDE_ID {
            return Err(LocalKnowledgeError::conflict(
                "Fox 使用指南是内置知识库，不能修改",
            ));
        }
        let name = normalized_name(&request.name)?;
        let connection = self.connection()?;
        let existing = connection
            .query_row(
                "SELECT embedding_model_id, chunk_size, chunk_overlap, search_mode
                 FROM local_knowledge_bases WHERE id = ?1",
                [&request.id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| LocalKnowledgeError::not_found("knowledge base"))?;
        let requested_model = request.embedding_model_id.or_else(|| existing.0.clone());
        let configuration = normalized_base_configuration(
            &connection,
            requested_model,
            request.chunk_size.unwrap_or(existing.1),
            request.chunk_overlap.unwrap_or(existing.2),
            request.search_mode.as_deref().unwrap_or(&existing.3),
        )?;
        let index_configuration_changed = existing.0 != configuration.0
            || existing.1 != configuration.1
            || existing.2 != configuration.2;
        if index_configuration_changed {
            let active_index_jobs: i64 = connection.query_row(
                "SELECT COUNT(*) FROM local_kb_jobs
                 WHERE knowledge_base_id = ?1 AND job_type IN ('index', 'rebuild')
                   AND status IN ('queued', 'running', 'paused')",
                [&request.id],
                |row| row.get(0),
            )?;
            if active_index_jobs > 0 {
                return Err(LocalKnowledgeError::conflict(
                    "knowledge base index configuration cannot change while an index job is active",
                ));
            }
        }
        let changed = connection.execute(
            "UPDATE local_knowledge_bases
             SET name = ?2, description = ?3, embedding_model_id = ?4,
                 chunk_size = ?5, chunk_overlap = ?6, search_mode = ?7, updated_at = ?8
             WHERE id = ?1",
            params![
                request.id,
                name,
                normalized_description(request.description),
                configuration.0,
                configuration.1,
                configuration.2,
                configuration.3,
                crate::database::now_ms()
            ],
        )?;
        if changed == 0 {
            return Err(LocalKnowledgeError::not_found("knowledge base"));
        }
        let has_parsed_chunks: bool = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM local_kb_chunks c
                JOIN local_kb_documents d ON d.id = c.document_id
                WHERE d.knowledge_base_id = ?1 AND d.parse_status = 'ready'
                  AND c.document_revision = d.current_revision
             )",
            [&request.id],
            |row| row.get(0),
        )?;
        let should_rebuild =
            index_configuration_changed && configuration.0.is_some() && has_parsed_chunks;
        drop(connection);
        if should_rebuild {
            self.start_index_generation(&request.id, "rebuild")?;
        }
        self.get_base(&request.id)
    }

    pub fn delete_base(&self, id: &str) -> Result<bool, LocalKnowledgeError> {
        let id = id.trim();
        if id.is_empty()
            || !id.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
        {
            return Err(LocalKnowledgeError::invalid("knowledge base id is invalid"));
        }
        if id == DEFAULT_FOX_GUIDE_ID {
            return Err(LocalKnowledgeError::conflict(
                "Fox 使用指南是内置知识库，不能删除",
            ));
        }
        if self.migration_active() {
            return Err(LocalKnowledgeError::conflict(
                "知识库存储迁移进行中，暂不能删除知识库",
            ));
        }

        let source_directory = self.root.join("knowledge-bases").join(id);
        let staged_directory = self
            .root
            .join("staging")
            .join(format!("deleted-{id}-{}", Uuid::new_v4()));
        let source_index_directory = self.root.join("indexes").join(id);
        let staged_index_directory = self
            .root
            .join("staging")
            .join(format!("deleted-index-{id}-{}", Uuid::new_v4()));
        let connection = self.connection()?;
        let exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_knowledge_bases WHERE id = ?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            return Err(LocalKnowledgeError::not_found("knowledge base"));
        }
        let active_jobs = connection.query_row(
            "SELECT COUNT(*) FROM local_kb_jobs
             WHERE knowledge_base_id = ?1 AND status IN ('queued', 'running', 'paused')",
            [id],
            |row| row.get::<_, i64>(0),
        )?;
        if active_jobs > 0 {
            return Err(LocalKnowledgeError::conflict(
                "知识库仍有任务正在执行，请等待任务结束后再删除",
            ));
        }

        let moved = if source_directory.exists() {
            let metadata = std::fs::symlink_metadata(&source_directory)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(LocalKnowledgeError::invalid(
                    "knowledge base storage directory is invalid",
                ));
            }
            std::fs::rename(&source_directory, &staged_directory)?;
            true
        } else {
            false
        };
        let index_moved = if source_index_directory.exists() {
            let metadata = match std::fs::symlink_metadata(&source_index_directory) {
                Ok(metadata) => metadata,
                Err(error) => {
                    if moved {
                        let _ = std::fs::rename(&staged_directory, &source_directory);
                    }
                    return Err(LocalKnowledgeError::from(error));
                }
            };
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                if moved {
                    let _ = std::fs::rename(&staged_directory, &source_directory);
                }
                return Err(LocalKnowledgeError::invalid(
                    "knowledge base Zvec index directory is invalid",
                ));
            }
            if let Err(error) = std::fs::rename(&source_index_directory, &staged_index_directory) {
                if moved {
                    let _ = std::fs::rename(&staged_directory, &source_directory);
                }
                return Err(LocalKnowledgeError::from(error));
            }
            true
        } else {
            false
        };

        let deleted = connection.execute("DELETE FROM local_knowledge_bases WHERE id = ?1", [id]);
        match deleted {
            Ok(1) => {
                drop(connection);
                if moved {
                    let _ = std::fs::remove_dir_all(staged_directory);
                }
                if index_moved {
                    let _ = std::fs::remove_dir_all(staged_index_directory);
                }
                Ok(true)
            }
            Ok(_) => {
                drop(connection);
                if moved {
                    let _ = std::fs::rename(staged_directory, source_directory);
                }
                if index_moved {
                    let _ = std::fs::rename(staged_index_directory, source_index_directory);
                }
                Err(LocalKnowledgeError::not_found("knowledge base"))
            }
            Err(error) => {
                drop(connection);
                if moved {
                    let _ = std::fs::rename(staged_directory, source_directory);
                }
                if index_moved {
                    let _ = std::fs::rename(staged_index_directory, source_index_directory);
                }
                Err(LocalKnowledgeError::from(error))
            }
        }
    }

    pub fn list_jobs(
        &self,
        knowledge_base_id: Option<&str>,
    ) -> Result<Vec<LocalKnowledgeJob>, LocalKnowledgeError> {
        let connection = self.connection()?;
        let sql = "SELECT id, parent_operation_id, knowledge_base_id, job_type, status, stage,
                          progress, retry_count, last_sequence, outcome, generation_id,
                          heartbeat_at, checkpoint_json, error_code, error_message,
                          created_at, started_at, updated_at, completed_at
                   FROM local_kb_jobs";
        let mut jobs = Vec::new();
        if let Some(knowledge_base_id) = knowledge_base_id {
            let mut statement = connection.prepare(&format!(
                "{sql} WHERE knowledge_base_id = ?1 ORDER BY updated_at DESC"
            ))?;
            for job in statement.query_map([knowledge_base_id], map_job)? {
                jobs.push(job?);
            }
        } else {
            let mut statement = connection.prepare(&format!("{sql} ORDER BY updated_at DESC"))?;
            for job in statement.query_map([], map_job)? {
                jobs.push(job?);
            }
        }
        Ok(jobs)
    }

    pub fn start_import(
        &self,
        request: LocalKnowledgeImportRequest,
    ) -> Result<LocalKnowledgeOperationAccepted, LocalKnowledgeError> {
        if self.migration_active() {
            return Err(LocalKnowledgeError::conflict(
                "知识库存储迁移进行中，暂不能创建新的知识库任务",
            ));
        }
        self.get_base(&request.knowledge_base_id)?;
        if request.files.is_empty() {
            return Err(LocalKnowledgeError::invalid(
                "at least one source file is required",
            ));
        }

        let operation_id = Uuid::new_v4().to_string();
        let accepted_at = crate::database::now_ms();
        let parser_version = request
            .parser_version
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "fox-text-v1".to_owned());
        let chunk_config_hash = request
            .chunk_config_hash
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "utf8-4096-v1".to_owned());
        let mut work = Vec::with_capacity(request.files.len());
        {
            let mut connection = self.connection()?;
            let transaction = connection.transaction()?;
            for file in request.files {
                let source_path = file.source_path.trim().to_owned();
                if source_path.is_empty() {
                    return Err(LocalKnowledgeError::invalid(
                        "sourcePath is required for every import file",
                    ));
                }
                let relative_path = file
                    .relative_path
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned);
                let job_id = Uuid::new_v4().to_string();
                let checkpoint = json!({
                    "sourcePath": source_path,
                    "relativePath": relative_path,
                    "parserVersion": parser_version,
                    "chunkConfigHash": chunk_config_hash,
                })
                .to_string();
                transaction.execute(
                    "INSERT INTO local_kb_jobs(
                        id, parent_operation_id, knowledge_base_id, job_type, status,
                        stage, progress, last_sequence, data_json, checkpoint_json,
                        created_at, updated_at
                     ) VALUES (?1, ?2, ?3, 'import', 'queued', 'validating', 0, 1, ?4, ?4, ?5, ?5)",
                    params![
                        job_id,
                        operation_id,
                        request.knowledge_base_id,
                        checkpoint,
                        accepted_at
                    ],
                )?;
                work.push((job_id, source_path, relative_path));
            }
            transaction.commit()?;
        }

        let store = self.clone();
        let knowledge_base_id = request.knowledge_base_id;
        let operation_id_for_thread = operation_id.clone();
        let spawn_result = thread::Builder::new()
            .name(format!("fox-kb-import-{operation_id_for_thread}"))
            .spawn(move || {
                for (job_id, source_path, relative_path) in work {
                    store.run_import_job(
                        &job_id,
                        &knowledge_base_id,
                        &source_path,
                        relative_path.as_deref(),
                        &parser_version,
                        &chunk_config_hash,
                    );
                }
            });
        if let Err(error) = spawn_result {
            self.fail_operation_jobs(
                &operation_id,
                "local_knowledge.import_worker_failed",
                &error.to_string(),
            )?;
            return Err(LocalKnowledgeError::storage(error));
        }

        Ok(LocalKnowledgeOperationAccepted {
            operation_id,
            accepted_at,
        })
    }

    fn run_import_job(
        &self,
        job_id: &str,
        knowledge_base_id: &str,
        source_path: &str,
        relative_path: Option<&str>,
        parser_version: &str,
        chunk_config_hash: &str,
    ) {
        if let Err(error) = self.perform_import_job(
            job_id,
            knowledge_base_id,
            source_path,
            relative_path,
            parser_version,
            chunk_config_hash,
        ) {
            let _ = self.fail_job(job_id, &error);
        }
    }

    fn perform_import_job(
        &self,
        job_id: &str,
        knowledge_base_id: &str,
        source_path: &str,
        relative_path: Option<&str>,
        parser_version: &str,
        chunk_config_hash: &str,
    ) -> Result<(), LocalKnowledgeError> {
        if !self.advance_job(job_id, "running", "copying", 10)? {
            return Ok(());
        }
        let cancellation_store = self.clone();
        let cancellation_job_id = job_id.to_owned();
        let cancelled = move || cancellation_store.job_is_cancelled(&cancellation_job_id);
        let imported = import_document_at_relative_path(
            self.root.as_ref(),
            knowledge_base_id,
            source_path,
            relative_path,
            ImportOptions::default().with_cancellation(&cancelled),
        )
        .map_err(LocalKnowledgeError::import)?;

        let duplicate_document_id = self
            .connection()?
            .query_row(
                "SELECT id FROM local_kb_documents
             WHERE knowledge_base_id = ?1 AND content_hash = ?2 LIMIT 1",
                params![knowledge_base_id, imported.content_hash],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if let Some(existing_id) = duplicate_document_id {
            let _ = std::fs::remove_file(&imported.stored_path);
            let now = crate::database::now_ms();
            self.connection()?.execute(
                "UPDATE local_kb_jobs SET status = 'completed', stage = 'committing',
                    progress = 100, outcome = 'success', last_sequence = last_sequence + 1,
                    heartbeat_at = ?2, updated_at = ?2, completed_at = ?2,
                    data_json = json_set(COALESCE(data_json, '{}'), '$.resultEntityId', ?3,
                                         '$.deduplicated', json('true'))
                 WHERE id = ?1 AND status IN ('queued', 'running', 'paused')",
                params![job_id, now, existing_id],
            )?;
            return Ok(());
        }

        if !self.advance_job(job_id, "running", "parsing", 65)? {
            let _ = std::fs::remove_file(&imported.stored_path);
            return Ok(());
        }
        let parsed = parse_imported_document(&imported, DEFAULT_CHUNK_MAX_BYTES)
            .map_err(LocalKnowledgeError::import)?;
        if !self.advance_job(job_id, "running", "committing", 90)? {
            let _ = std::fs::remove_file(&imported.stored_path);
            return Ok(());
        }

        let document_id = Uuid::new_v4().to_string();
        let now = crate::database::now_ms();
        let (parse_status, chunks) = match parsed {
            ParsedDocument::Text { chunks, .. } => ("ready", chunks),
            ParsedDocument::PendingSpecializedParser { .. } => ("pending_parser", Vec::new()),
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let still_active: bool = transaction.query_row(
            "SELECT status IN ('queued', 'running', 'paused') FROM local_kb_jobs WHERE id = ?1",
            [job_id],
            |row| row.get(0),
        )?;
        if !still_active {
            drop(transaction);
            let _ = std::fs::remove_file(&imported.stored_path);
            return Ok(());
        }
        transaction.execute(
            "INSERT INTO local_kb_documents(
                id, knowledge_base_id, display_name, relative_path, source_path,
                current_revision, content_hash, file_size, mime_type, parse_status,
                index_status, parser_version, chunk_config_hash, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?9, 'pending', ?10, ?11, ?12, ?12)",
            params![
                document_id,
                knowledge_base_id,
                imported.display_name,
                imported.relative_path,
                source_path,
                imported.content_hash,
                i64::try_from(imported.file_size).unwrap_or(i64::MAX),
                imported.mime_type,
                parse_status,
                parser_version,
                chunk_config_hash,
                now,
            ],
        )?;
        for chunk in chunks {
            transaction.execute(
                "INSERT INTO local_kb_chunks(
                    id, document_id, document_revision, chunk_index, text,
                    anchor, start_offset, end_offset, metadata_json
                 ) VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6, ?7, '{}')",
                params![
                    Uuid::new_v4().to_string(),
                    document_id,
                    i64::try_from(chunk.chunk_index).unwrap_or(i64::MAX),
                    chunk.text,
                    chunk.anchor,
                    i64::try_from(chunk.start_offset).unwrap_or(i64::MAX),
                    i64::try_from(chunk.end_offset).unwrap_or(i64::MAX),
                ],
            )?;
        }
        transaction.execute(
            "UPDATE local_kb_jobs
             SET status = 'completed', stage = 'committing', progress = 100,
                 outcome = 'success', last_sequence = last_sequence + 1,
                 heartbeat_at = ?2, updated_at = ?2, completed_at = ?2,
                 data_json = json_set(COALESCE(data_json, '{}'), '$.resultEntityId', ?3)
             WHERE id = ?1",
            params![job_id, now, document_id],
        )?;
        transaction.execute(
            "UPDATE local_knowledge_bases SET updated_at = ?2 WHERE id = ?1",
            params![knowledge_base_id, now],
        )?;
        transaction.commit()?;
        drop(connection);
        if self
            .get_base(knowledge_base_id)?
            .configured_embedding_model_id
            .is_some()
        {
            let _ = self.start_index_generation(knowledge_base_id, "index");
        }
        Ok(())
    }

    pub(crate) fn advance_job(
        &self,
        job_id: &str,
        status: &str,
        stage: &str,
        progress: i64,
    ) -> Result<bool, LocalKnowledgeError> {
        let now = crate::database::now_ms();
        if self.migration_active() {
            self.connection()?.execute(
                "UPDATE local_kb_jobs
                 SET status = 'cancelled', error_code = 'local_knowledge.storage_migration_quiescing',
                     error_message = '知识库存储迁移开始，任务已取消',
                     updated_at = ?2, completed_at = ?2, last_sequence = last_sequence + 1
                 WHERE id = ?1 AND status IN ('queued', 'running', 'paused')",
                params![job_id, now],
            )?;
            return Ok(false);
        }
        Ok(self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = ?2, stage = ?3, progress = ?4,
                 started_at = COALESCE(started_at, ?5), heartbeat_at = ?5,
                 updated_at = ?5, last_sequence = last_sequence + 1
             WHERE id = ?1 AND status IN ('queued', 'running', 'paused')",
            params![job_id, status, stage, progress, now],
        )? > 0)
    }

    pub(crate) fn job_is_cancelled(&self, job_id: &str) -> bool {
        if self.migration_active() {
            return true;
        }
        self.connection()
            .and_then(|connection| {
                connection
                    .query_row(
                        "SELECT status = 'cancelled' FROM local_kb_jobs WHERE id = ?1",
                        [job_id],
                        |row| row.get(0),
                    )
                    .map_err(Into::into)
            })
            .unwrap_or(true)
    }

    pub(crate) fn fail_job(
        &self,
        job_id: &str,
        error: &LocalKnowledgeError,
    ) -> Result<(), LocalKnowledgeError> {
        let now = crate::database::now_ms();
        self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'failed', outcome = NULL, error_code = ?2, error_message = ?3,
                 heartbeat_at = ?4, updated_at = ?4, completed_at = ?4,
                 last_sequence = last_sequence + 1
             WHERE id = ?1 AND status != 'cancelled'",
            params![job_id, error.code, error.message, now],
        )?;
        Ok(())
    }

    fn fail_operation_jobs(
        &self,
        operation_id: &str,
        error_code: &str,
        error_message: &str,
    ) -> Result<(), LocalKnowledgeError> {
        let now = crate::database::now_ms();
        self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'failed', error_code = ?2, error_message = ?3,
                 updated_at = ?4, completed_at = ?4, last_sequence = last_sequence + 1
             WHERE parent_operation_id = ?1 AND status = 'queued'",
            params![operation_id, error_code, error_message, now],
        )?;
        Ok(())
    }

    pub fn list_documents(
        &self,
        request: LocalKnowledgeDocumentsRequest,
    ) -> Result<Vec<LocalKnowledgeDocument>, LocalKnowledgeError> {
        self.get_base(&request.knowledge_base_id)?;
        let connection = self.connection()?;
        let query = request
            .query
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{value}%"));
        let sql = "SELECT d.id, d.knowledge_base_id, d.display_name, d.relative_path,
                          d.current_revision, d.file_size, d.mime_type, d.parse_status,
                          d.index_status, COUNT(c.id) AS chunk_count,
                          d.last_error_code, d.last_error_message, d.created_at, d.updated_at
                   FROM local_kb_documents d
                   LEFT JOIN local_kb_chunks c ON c.document_id = d.id
                   WHERE d.knowledge_base_id = ?1";
        let mut documents = Vec::new();
        if let Some(query) = query {
            let mut statement = connection.prepare(&format!(
                "{sql} AND d.display_name LIKE ?2 ESCAPE '\\' GROUP BY d.id ORDER BY d.updated_at DESC"
            ))?;
            for document in
                statement.query_map(params![request.knowledge_base_id, query], map_document)?
            {
                documents.push(document?);
            }
        } else {
            let mut statement =
                connection.prepare(&format!("{sql} GROUP BY d.id ORDER BY d.updated_at DESC"))?;
            for document in statement.query_map([request.knowledge_base_id], map_document)? {
                documents.push(document?);
            }
        }
        Ok(documents)
    }

    pub fn list_folders(
        &self,
        request: LocalKnowledgeFoldersRequest,
    ) -> Result<Vec<LocalKnowledgeFolder>, LocalKnowledgeError> {
        self.get_base(&request.knowledge_base_id)?;
        let documents = self.list_documents(LocalKnowledgeDocumentsRequest {
            knowledge_base_id: request.knowledge_base_id.clone(),
            query: None,
        })?;
        let root = self
            .root
            .join("knowledge-bases")
            .join(&request.knowledge_base_id)
            .join("documents");
        if !root.exists() {
            return Ok(Vec::new());
        }
        let metadata = std::fs::symlink_metadata(&root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(LocalKnowledgeError::invalid(
                "knowledge base documents directory is invalid",
            ));
        }
        let mut paths = Vec::new();
        collect_knowledge_folder_paths(&root, &root, &mut paths)?;
        paths.sort();
        Ok(paths
            .into_iter()
            .map(|relative_path| {
                let prefix = format!("{relative_path}/");
                let document_count = documents
                    .iter()
                    .filter(|document| document.relative_path.starts_with(&prefix))
                    .count() as i64;
                let name = relative_path
                    .rsplit('/')
                    .next()
                    .unwrap_or(relative_path.as_str())
                    .to_owned();
                LocalKnowledgeFolder {
                    knowledge_base_id: request.knowledge_base_id.clone(),
                    name,
                    relative_path,
                    document_count,
                }
            })
            .collect())
    }

    pub fn create_folder(
        &self,
        request: CreateLocalKnowledgeFolderRequest,
    ) -> Result<LocalKnowledgeFolder, LocalKnowledgeError> {
        self.get_base(&request.knowledge_base_id)?;
        let name = request.name.trim();
        validate_path_component(name.as_ref()).map_err(LocalKnowledgeError::invalid)?;
        let parent_path =
            normalize_relative_folder_path(request.parent_path.as_deref().unwrap_or_default())?;
        let relative_path = if parent_path.is_empty() {
            name.to_owned()
        } else {
            format!("{parent_path}/{name}")
        };
        let root = self
            .root
            .join("knowledge-bases")
            .join(&request.knowledge_base_id)
            .join("documents");
        std::fs::create_dir_all(&root)?;
        let root_metadata = std::fs::symlink_metadata(&root)?;
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(LocalKnowledgeError::invalid(
                "knowledge base documents directory is invalid",
            ));
        }
        let mut current = root;
        for component in Path::new(&parent_path).components() {
            let Component::Normal(value) = component else {
                return Err(LocalKnowledgeError::invalid(
                    "folder path contains an unsafe component",
                ));
            };
            current.push(value);
            let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    LocalKnowledgeError::invalid("parent folder does not exist")
                } else {
                    LocalKnowledgeError::storage(error)
                }
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(LocalKnowledgeError::invalid(
                    "parent folder is not a safe directory",
                ));
            }
        }
        let destination = current.join(name);
        if destination.exists() {
            return Err(LocalKnowledgeError::conflict("folder already exists"));
        }
        std::fs::create_dir(&destination)?;
        Ok(LocalKnowledgeFolder {
            knowledge_base_id: request.knowledge_base_id,
            name: name.to_owned(),
            relative_path,
            document_count: 0,
        })
    }

    pub fn get_job(&self, id: &str) -> Result<LocalKnowledgeJob, LocalKnowledgeError> {
        let connection = self.connection()?;
        query_job(&connection, id)?.ok_or_else(|| LocalKnowledgeError::not_found("knowledge job"))
    }

    pub fn cancel_job(&self, id: &str) -> Result<LocalKnowledgeJob, LocalKnowledgeError> {
        let now = crate::database::now_ms();
        let changed = self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'cancelled', completed_at = ?2, updated_at = ?2,
                 last_sequence = last_sequence + 1
             WHERE id = ?1 AND status IN ('queued', 'running', 'paused')",
            params![id, now],
        )?;
        if changed == 0 {
            let existing = self.get_job(id)?;
            return Err(LocalKnowledgeError::conflict(format!(
                "job cannot be cancelled from status {}",
                existing.status
            )));
        }
        self.get_job(id)
    }

    pub fn retry_job(&self, id: &str) -> Result<LocalKnowledgeJob, LocalKnowledgeError> {
        let now = crate::database::now_ms();
        let result = self.connection()?.execute(
            "UPDATE local_kb_jobs
             SET status = 'queued', stage = NULL, progress = 0,
                 retry_count = retry_count + 1, last_sequence = last_sequence + 1,
                 outcome = NULL, heartbeat_at = NULL,
                 error_code = NULL, error_message = NULL, started_at = NULL,
                 completed_at = NULL, updated_at = ?2
             WHERE id = ?1 AND status IN ('failed', 'cancelled', 'interrupted')",
            params![id, now],
        );
        let changed = match result {
            Ok(changed) => changed,
            Err(rusqlite::Error::SqliteFailure(_, Some(message)))
                if message.contains("UNIQUE constraint failed") =>
            {
                return Err(LocalKnowledgeError::conflict(
                    "another index job is already active for this knowledge base",
                ));
            }
            Err(error) => return Err(error.into()),
        };
        if changed == 0 {
            let existing = self.get_job(id)?;
            return Err(LocalKnowledgeError::conflict(format!(
                "job cannot be retried from status {}",
                existing.status
            )));
        }
        let job = self.get_job(id)?;
        if job.job_type == "import" {
            self.resume_import_job(&job)?;
        } else if job.job_type == "parse" {
            self.resume_reparse_job(&job)?;
        } else if matches!(job.job_type.as_str(), "index" | "rebuild") {
            self.resume_index_job(&job)?;
        }
        Ok(job)
    }

    fn resume_import_job(&self, job: &LocalKnowledgeJob) -> Result<(), LocalKnowledgeError> {
        let checkpoint = job
            .checkpoint_json
            .as_deref()
            .ok_or_else(|| LocalKnowledgeError::invalid("import job checkpoint is missing"))?;
        let checkpoint: serde_json::Value = serde_json::from_str(checkpoint)
            .map_err(|error| LocalKnowledgeError::invalid(error.to_string()))?;
        let source_path = checkpoint
            .get("sourcePath")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| LocalKnowledgeError::invalid("import sourcePath is missing"))?
            .to_owned();
        let parser_version = checkpoint
            .get("parserVersion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("fox-text-v1")
            .to_owned();
        let relative_path = checkpoint
            .get("relativePath")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let chunk_config_hash = checkpoint
            .get("chunkConfigHash")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("utf8-4096-v1")
            .to_owned();
        let store = self.clone();
        let job_id = job.id.clone();
        let knowledge_base_id = job.knowledge_base_id.clone();
        thread::Builder::new()
            .name(format!("fox-kb-import-retry-{job_id}"))
            .spawn(move || {
                store.run_import_job(
                    &job_id,
                    &knowledge_base_id,
                    &source_path,
                    relative_path.as_deref(),
                    &parser_version,
                    &chunk_config_hash,
                );
            })
            .map_err(LocalKnowledgeError::storage)?;
        Ok(())
    }

    pub fn storage_status(&self) -> Result<LocalKnowledgeStorageStatus, LocalKnowledgeError> {
        let probe_path = self.root.join(".fox-write-probe");
        let writable = std::fs::write(&probe_path, b"fox")
            .and_then(|_| std::fs::remove_file(&probe_path))
            .is_ok();
        let connection = self.connection()?;
        let schema_version =
            connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        let persisted_pending = connection
            .query_row(
                "SELECT id, destination_path
                 FROM local_kb_storage_migrations
                 WHERE status = 'completed' AND restart_required = 1
                 ORDER BY updated_at DESC LIMIT 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let active_migration_id = self
            .storage_migration_active
            .lock()
            .map_err(|_| LocalKnowledgeError::storage("storage migration lock is poisoned"))?
            .clone();
        let pending_root = self
            .pending_root_path
            .lock()
            .map_err(|_| LocalKnowledgeError::storage("storage migration lock is poisoned"))?
            .clone()
            .map(|path| path.to_string_lossy().into_owned())
            .or_else(|| persisted_pending.as_ref().map(|(_, path)| path.clone()));
        let migration_id =
            active_migration_id.or_else(|| persisted_pending.as_ref().map(|(id, _)| id.clone()));
        Ok(LocalKnowledgeStorageStatus {
            root_path: self.root.to_string_lossy().into_owned(),
            database_path: self.database_path.to_string_lossy().into_owned(),
            writable,
            schema_version,
            restart_required: pending_root.is_some(),
            pending_root_path: pending_root,
            migration_id,
        })
    }

    #[cfg(test)]
    fn raw_connection(&self) -> Result<MutexGuard<'_, Connection>, LocalKnowledgeError> {
        self.connection()
    }
}

fn normalize_relative_folder_path(value: &str) -> Result<String, LocalKnowledgeError> {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty() {
        return Ok(String::new());
    }
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(LocalKnowledgeError::invalid(
            "folder path must be relative to the knowledge base",
        ));
    }
    let mut components = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(LocalKnowledgeError::invalid(
                "folder path contains an unsafe component",
            ));
        };
        validate_path_component(value).map_err(LocalKnowledgeError::invalid)?;
        components.push(value.to_string_lossy().into_owned());
    }
    Ok(components.join("/"))
}

fn collect_knowledge_folder_paths(
    root: &Path,
    current: &Path,
    paths: &mut Vec<String>,
) -> Result<(), LocalKnowledgeError> {
    let mut entries = std::fs::read_dir(current)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            continue;
        }
        let relative_path = path
            .strip_prefix(root)
            .map_err(LocalKnowledgeError::storage)?
            .to_string_lossy()
            .replace('\\', "/");
        paths.push(relative_path);
        collect_knowledge_folder_paths(root, &path, paths)?;
    }
    Ok(())
}

fn initialize_schema(connection: &Connection) -> Result<(), LocalKnowledgeError> {
    connection.execute_batch(&format!(
        r#"
        CREATE TABLE IF NOT EXISTS local_knowledge_bases (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            description TEXT,
            active_index_generation TEXT,
            embedding_model_id TEXT,
            chunk_size INTEGER NOT NULL DEFAULT 512 CHECK(chunk_size BETWEEN 128 AND 4096),
            chunk_overlap INTEGER NOT NULL DEFAULT 50 CHECK(chunk_overlap >= 0 AND chunk_overlap < chunk_size),
            search_mode TEXT NOT NULL DEFAULT 'keyword' CHECK(search_mode IN ('keyword', 'vector', 'hybrid')),
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_file_sources (
            id TEXT PRIMARY KEY,
            root_path TEXT NOT NULL COLLATE NOCASE UNIQUE,
            display_name TEXT NOT NULL,
            file_count INTEGER NOT NULL DEFAULT 0,
            total_size INTEGER NOT NULL DEFAULT 0,
            last_scanned_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS local_file_catalog (
            id TEXT PRIMARY KEY,
            source_id TEXT NOT NULL,
            absolute_path TEXT NOT NULL COLLATE NOCASE UNIQUE,
            relative_path TEXT NOT NULL,
            display_name TEXT NOT NULL,
            extension TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            modified_at INTEGER,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (source_id) REFERENCES local_file_sources(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_local_file_catalog_source
        ON local_file_catalog(source_id, modified_at DESC);

        CREATE INDEX IF NOT EXISTS idx_local_file_catalog_name
        ON local_file_catalog(display_name COLLATE NOCASE);

        CREATE TABLE IF NOT EXISTS local_kb_documents (
            id TEXT PRIMARY KEY,
            knowledge_base_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            source_path TEXT,
            current_revision INTEGER NOT NULL DEFAULT 1,
            content_hash TEXT NOT NULL,
            file_size INTEGER NOT NULL,
            mime_type TEXT NOT NULL,
            parse_status TEXT NOT NULL,
            index_status TEXT NOT NULL,
            parser_version TEXT,
            chunk_config_hash TEXT,
            embedding_model_id TEXT,
            last_error_code TEXT,
            last_error_message TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (knowledge_base_id) REFERENCES local_knowledge_bases(id) ON DELETE CASCADE,
            UNIQUE(knowledge_base_id, relative_path)
        );

        CREATE TABLE IF NOT EXISTS local_kb_chunks (
            row_id INTEGER PRIMARY KEY AUTOINCREMENT,
            id TEXT NOT NULL UNIQUE,
            document_id TEXT NOT NULL,
            document_revision INTEGER NOT NULL,
            chunk_index INTEGER NOT NULL,
            text TEXT NOT NULL,
            page_number INTEGER,
            slide_number INTEGER,
            anchor TEXT,
            start_offset INTEGER,
            end_offset INTEGER,
            metadata_json TEXT,
            FOREIGN KEY (document_id) REFERENCES local_kb_documents(id) ON DELETE CASCADE,
            UNIQUE(document_id, document_revision, chunk_index)
        );

        CREATE TABLE IF NOT EXISTS local_kb_index_generations (
            id TEXT PRIMARY KEY,
            knowledge_base_id TEXT NOT NULL,
            sequence INTEGER NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('staging', 'active', 'stale', 'deleting', 'failed')),
            vector_store_kind TEXT NOT NULL,
            vector_store_version TEXT,
            embedding_model_id TEXT NOT NULL,
            embedding_model_version TEXT,
            dimension INTEGER NOT NULL CHECK(dimension > 0),
            chunk_config_hash TEXT NOT NULL,
            parser_version TEXT NOT NULL,
            chunk_count INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            activated_at INTEGER,
            retire_after INTEGER,
            last_error_code TEXT,
            last_error_message TEXT,
            FOREIGN KEY (knowledge_base_id) REFERENCES local_knowledge_bases(id) ON DELETE CASCADE,
            UNIQUE(knowledge_base_id, sequence)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_local_kb_one_active_generation
        ON local_kb_index_generations(knowledge_base_id)
        WHERE status = 'active';

        CREATE TABLE IF NOT EXISTS local_kb_jobs (
            id TEXT PRIMARY KEY,
            parent_operation_id TEXT,
            knowledge_base_id TEXT NOT NULL,
            job_type TEXT NOT NULL CHECK(job_type IN ('import', 'parse', 'index', 'rebuild', 'delete')),
            status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'paused', 'completed', 'failed', 'cancelled', 'interrupted')),
            stage TEXT CHECK(stage IN ('validating', 'copying', 'hashing', 'parsing', 'chunking', 'embedding', 'vector_upsert', 'verifying', 'committing')),
            progress INTEGER NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 100),
            retry_count INTEGER NOT NULL DEFAULT 0,
            last_sequence INTEGER NOT NULL DEFAULT 0,
            outcome TEXT CHECK(outcome IN ('success', 'partial')),
            data_json TEXT,
            generation_id TEXT,
            heartbeat_at INTEGER,
            checkpoint_json TEXT,
            error_code TEXT,
            error_message TEXT,
            created_at INTEGER NOT NULL,
            started_at INTEGER,
            updated_at INTEGER NOT NULL,
            completed_at INTEGER,
            FOREIGN KEY (knowledge_base_id) REFERENCES local_knowledge_bases(id) ON DELETE CASCADE
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_local_kb_single_active_index_job
        ON local_kb_jobs(knowledge_base_id)
        WHERE job_type IN ('index', 'rebuild')
          AND status IN ('queued', 'running', 'paused');

        CREATE TABLE IF NOT EXISTS local_kb_storage_migrations (
            id TEXT PRIMARY KEY,
            source_path TEXT NOT NULL,
            destination_path TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'paused', 'completed', 'failed', 'cancelled')),
            stage TEXT NOT NULL CHECK(stage IN ('validating', 'quiescing', 'flushing', 'closing_handles', 'copying', 'verifying', 'switching', 'reopening', 'self_checking', 'awaiting_cleanup', 'rolling_back')),
            progress INTEGER NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 100),
            last_sequence INTEGER NOT NULL DEFAULT 0,
            error_code TEXT,
            error_message TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            restart_required INTEGER NOT NULL DEFAULT 0 CHECK(restart_required IN (0, 1))
        );

        CREATE TABLE IF NOT EXISTS local_embedding_models (
            model_id TEXT NOT NULL,
            name TEXT,
            version TEXT NOT NULL,
            dimension INTEGER NOT NULL CHECK(dimension > 0),
            status TEXT NOT NULL CHECK(status IN ('not_installed', 'installing', 'ready', 'error')),
            languages_json TEXT NOT NULL DEFAULT '[]',
            license TEXT,
            source_url TEXT,
            package_path TEXT,
            integrity_status TEXT NOT NULL DEFAULT 'pending' CHECK(integrity_status IN ('pending', 'verifying', 'verified', 'failed')),
            is_default INTEGER NOT NULL DEFAULT 0 CHECK(is_default IN (0, 1)),
            package_hash TEXT,
            size_bytes INTEGER,
            installed_at INTEGER,
            updated_at INTEGER NOT NULL,
            last_error_code TEXT,
            last_error_message TEXT,
            PRIMARY KEY(model_id, version)
        );

        CREATE TABLE IF NOT EXISTS local_embedding_model_downloads (
            id TEXT PRIMARY KEY,
            model_id TEXT NOT NULL,
            version TEXT NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('queued', 'running', 'paused', 'completed', 'failed', 'cancelled')),
            progress INTEGER NOT NULL DEFAULT 0 CHECK(progress BETWEEN 0 AND 100),
            downloaded_bytes INTEGER NOT NULL DEFAULT 0,
            total_bytes INTEGER NOT NULL DEFAULT 0,
            current_file TEXT,
            checkpoint_json TEXT,
            error_code TEXT,
            error_message TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            completed_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS local_kb_vectors (
            generation_id TEXT NOT NULL,
            chunk_id TEXT NOT NULL,
            dimension INTEGER NOT NULL CHECK(dimension > 0),
            vector_json TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            PRIMARY KEY(generation_id, chunk_id),
            FOREIGN KEY (generation_id) REFERENCES local_kb_index_generations(id) ON DELETE CASCADE,
            FOREIGN KEY (chunk_id) REFERENCES local_kb_chunks(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_local_kb_vectors_generation
        ON local_kb_vectors(generation_id);

        CREATE TABLE IF NOT EXISTS local_kb_retrieval_cases (
            id TEXT PRIMARY KEY,
            knowledge_base_id TEXT NOT NULL,
            question TEXT NOT NULL,
            expected_document_ids_json TEXT NOT NULL DEFAULT '[]',
            expected_keywords_json TEXT NOT NULL DEFAULT '[]',
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (knowledge_base_id) REFERENCES local_knowledge_bases(id) ON DELETE CASCADE
        );

        PRAGMA user_version = {KNOWLEDGE_SCHEMA_VERSION};
        "#
    ))?;
    ensure_schema_column(
        connection,
        "local_kb_storage_migrations",
        "restart_required",
        "ALTER TABLE local_kb_storage_migrations ADD COLUMN restart_required INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_schema_column(
        connection,
        "local_knowledge_bases",
        "embedding_model_id",
        "ALTER TABLE local_knowledge_bases ADD COLUMN embedding_model_id TEXT",
    )?;
    ensure_schema_column(
        connection,
        "local_knowledge_bases",
        "chunk_size",
        "ALTER TABLE local_knowledge_bases ADD COLUMN chunk_size INTEGER NOT NULL DEFAULT 512",
    )?;
    ensure_schema_column(
        connection,
        "local_knowledge_bases",
        "chunk_overlap",
        "ALTER TABLE local_knowledge_bases ADD COLUMN chunk_overlap INTEGER NOT NULL DEFAULT 50",
    )?;
    ensure_schema_column(
        connection,
        "local_knowledge_bases",
        "search_mode",
        "ALTER TABLE local_knowledge_bases ADD COLUMN search_mode TEXT NOT NULL DEFAULT 'keyword'",
    )?;
    ensure_schema_column(
        connection,
        "local_kb_index_generations",
        "embedding_model_version",
        "ALTER TABLE local_kb_index_generations ADD COLUMN embedding_model_version TEXT",
    )?;
    for (column, statement) in [
        ("name", "ALTER TABLE local_embedding_models ADD COLUMN name TEXT"),
        ("languages_json", "ALTER TABLE local_embedding_models ADD COLUMN languages_json TEXT NOT NULL DEFAULT '[]'"),
        ("license", "ALTER TABLE local_embedding_models ADD COLUMN license TEXT"),
        ("source_url", "ALTER TABLE local_embedding_models ADD COLUMN source_url TEXT"),
        ("package_path", "ALTER TABLE local_embedding_models ADD COLUMN package_path TEXT"),
        ("integrity_status", "ALTER TABLE local_embedding_models ADD COLUMN integrity_status TEXT NOT NULL DEFAULT 'pending'"),
        ("is_default", "ALTER TABLE local_embedding_models ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0"),
    ] {
        ensure_schema_column(connection, "local_embedding_models", column, statement)?;
    }
    connection.execute(
        "UPDATE local_embedding_models SET is_default = 0
         WHERE model_id = 'all-MiniLM-L6-v2' AND is_default = 1",
        [],
    )?;
    connection.execute(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_local_embedding_one_default
         ON local_embedding_models(is_default) WHERE is_default = 1",
        [],
    )?;
    connection.pragma_update(None, "user_version", KNOWLEDGE_SCHEMA_VERSION)?;
    Ok(())
}

fn ensure_schema_column(
    connection: &Connection,
    table: &str,
    column: &str,
    alter_statement: &str,
) -> Result<(), LocalKnowledgeError> {
    let columns = connection
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    if !columns.iter().any(|name| name == column) {
        connection.execute(alter_statement, [])?;
    }
    Ok(())
}

fn canonical_file_source_root(
    path: &Path,
    knowledge_root: &Path,
) -> Result<PathBuf, LocalKnowledgeError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(LocalKnowledgeError::invalid(
            "local file source must be an absolute directory",
        ));
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(LocalKnowledgeError::invalid(
            "local file source must be a normal directory",
        ));
    }
    let canonical = path.canonicalize()?;
    let canonical_knowledge_root = knowledge_root
        .canonicalize()
        .unwrap_or_else(|_| knowledge_root.to_path_buf());
    if canonical.starts_with(&canonical_knowledge_root) {
        return Err(LocalKnowledgeError::invalid(
            "Fox knowledge storage cannot be added as a local file source",
        ));
    }
    Ok(canonical)
}

fn scan_catalog_files(
    root: &Path,
    knowledge_root: &Path,
) -> Result<Vec<CatalogFileCandidate>, LocalKnowledgeError> {
    let excluded_root = knowledge_root
        .canonicalize()
        .unwrap_or_else(|_| knowledge_root.to_path_buf());
    let mut directories = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = directories.pop() {
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if directory != root => {
                eprintln!(
                    "[local-knowledge] skip unreadable catalog directory {}: {error}",
                    directory.display()
                );
                continue;
            }
            Err(error) => return Err(LocalKnowledgeError::from(error)),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.starts_with(&excluded_root) {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                directories.push(path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let display_name = entry.file_name().to_string_lossy().into_owned();
            if !is_supported_file_name(&display_name) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if metadata.len() > MAX_PICKED_FILE_SIZE {
                continue;
            }
            if files.len() >= MAX_LOCAL_FILE_CATALOG_ITEMS {
                return Err(LocalKnowledgeError::invalid(format!(
                    "local file source exceeds {MAX_LOCAL_FILE_CATALOG_ITEMS} supported files"
                )));
            }
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();
            let relative_path = path
                .strip_prefix(root)
                .unwrap_or(path.as_path())
                .to_string_lossy()
                .into_owned();
            let modified_at = metadata
                .modified()
                .ok()
                .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
                .and_then(|value| i64::try_from(value.as_millis()).ok());
            files.push(CatalogFileCandidate {
                absolute_path: path.to_string_lossy().into_owned(),
                relative_path,
                display_name: display_name.clone(),
                extension,
                mime_type: mime_type_for_file_name(&display_name)
                    .unwrap_or("application/octet-stream")
                    .to_owned(),
                file_size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                modified_at,
            });
        }
    }
    Ok(files)
}

fn catalog_category_extensions(
    category: Option<&str>,
) -> Result<Option<&'static [&'static str]>, LocalKnowledgeError> {
    match category.map(str::trim).filter(|value| !value.is_empty()) {
        None | Some("all") => Ok(None),
        Some("text") => Ok(Some(&["txt", "md"])),
        Some("document") => Ok(Some(&["docx"])),
        Some("pdf") => Ok(Some(&["pdf"])),
        Some("presentation") => Ok(Some(&["pptx"])),
        Some("spreadsheet") => Ok(Some(&["xlsx"])),
        Some(_) => Err(LocalKnowledgeError::invalid(
            "unsupported local file category",
        )),
    }
}

fn normalized_name(value: &str) -> Result<String, LocalKnowledgeError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(LocalKnowledgeError::invalid(
            "knowledge base name is required",
        ));
    }
    if value.chars().count() > 120 {
        return Err(LocalKnowledgeError::invalid(
            "knowledge base name cannot exceed 120 characters",
        ));
    }
    Ok(value.to_owned())
}

fn normalized_description(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn normalized_base_configuration(
    connection: &Connection,
    embedding_model_id: Option<String>,
    chunk_size: i64,
    chunk_overlap: i64,
    search_mode: &str,
) -> Result<(Option<String>, i64, i64, String), LocalKnowledgeError> {
    if !(128..=4096).contains(&chunk_size) {
        return Err(LocalKnowledgeError::invalid(
            "chunkSize must be between 128 and 4096 characters",
        ));
    }
    if chunk_overlap < 0 || chunk_overlap >= chunk_size {
        return Err(LocalKnowledgeError::invalid(
            "chunkOverlap must be non-negative and smaller than chunkSize",
        ));
    }
    let search_mode = search_mode.trim().to_ascii_lowercase();
    if !matches!(search_mode.as_str(), "keyword" | "vector" | "hybrid") {
        return Err(LocalKnowledgeError::invalid(
            "searchMode must be keyword, vector, or hybrid",
        ));
    }
    let embedding_model_id = embedding_model_id
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if let Some(model_id) = embedding_model_id.as_deref() {
        let ready = connection.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM local_embedding_models
                WHERE model_id = ?1 AND status = 'ready' AND integrity_status = 'verified'
             )",
            [model_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !ready {
            return Err(LocalKnowledgeError::conflict(
                "selected embedding model is not installed or has not passed integrity verification",
            ));
        }
    }
    if search_mode != "keyword" && embedding_model_id.is_none() {
        return Err(LocalKnowledgeError::invalid(
            "vector and hybrid search require an installed embedding model",
        ));
    }
    Ok((embedding_model_id, chunk_size, chunk_overlap, search_mode))
}

fn map_knowledge_base(row: &Row<'_>) -> rusqlite::Result<LocalKnowledgeBase> {
    Ok(LocalKnowledgeBase {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        active_index_generation: row.get(3)?,
        document_count: row.get(4)?,
        active_job_status: row.get(5)?,
        text_index_ready: false,
        vector_index_ready: false,
        search_mode: "keyword".to_owned(),
        configured_embedding_model_id: None,
        chunk_size: DEFAULT_CHUNK_SIZE,
        chunk_overlap: DEFAULT_CHUNK_OVERLAP,
        embedding_model: None,
        chunk_count: None,
        vector_count: None,
        last_indexed_at: None,
        fallback_reason: Some("未配置向量模型".to_owned()),
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_file_source(row: &Row<'_>) -> rusqlite::Result<LocalKnowledgeFileSource> {
    Ok(LocalKnowledgeFileSource {
        id: row.get(0)?,
        root_path: row.get(1)?,
        display_name: row.get(2)?,
        file_count: row.get(3)?,
        total_size: row.get(4)?,
        last_scanned_at: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

fn map_catalog_file(row: &Row<'_>) -> rusqlite::Result<LocalKnowledgeCatalogFile> {
    Ok(LocalKnowledgeCatalogFile {
        id: row.get(0)?,
        source_id: row.get(1)?,
        source_name: row.get(2)?,
        source_path: row.get(3)?,
        absolute_path: row.get(4)?,
        relative_path: row.get(5)?,
        display_name: row.get(6)?,
        extension: row.get(7)?,
        mime_type: row.get(8)?,
        file_size: row.get(9)?,
        modified_at: row.get(10)?,
    })
}

fn read_file_range(path: PathBuf, start: u64, end: u64) -> Result<Vec<u8>, LocalKnowledgeError> {
    if end < start {
        return Err(LocalKnowledgeError::invalid(
            "preview range end must be greater than or equal to start",
        ));
    }
    let requested_len = end
        .checked_sub(start)
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| LocalKnowledgeError::invalid("preview range overflowed"))?;
    if requested_len > MAX_LOCAL_PREVIEW_RANGE_BYTES {
        return Err(LocalKnowledgeError::invalid(format!(
            "preview range cannot exceed {MAX_LOCAL_PREVIEW_RANGE_BYTES} bytes"
        )));
    }
    let file_size = std::fs::metadata(&path)?.len();
    if start >= file_size || end >= file_size {
        return Err(LocalKnowledgeError::invalid(
            "preview range is outside the current file size",
        ));
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![
        0;
        usize::try_from(requested_len).map_err(|_| {
            LocalKnowledgeError::invalid("preview range is too large for this platform")
        })?
    ];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn map_job(row: &Row<'_>) -> rusqlite::Result<LocalKnowledgeJob> {
    Ok(LocalKnowledgeJob {
        id: row.get(0)?,
        parent_operation_id: row.get(1)?,
        knowledge_base_id: row.get(2)?,
        job_type: row.get(3)?,
        status: row.get(4)?,
        stage: row.get(5)?,
        progress: row.get(6)?,
        retry_count: row.get(7)?,
        last_sequence: row.get(8)?,
        outcome: row.get(9)?,
        generation_id: row.get(10)?,
        heartbeat_at: row.get(11)?,
        checkpoint_json: row.get(12)?,
        error_code: row.get(13)?,
        error_message: row.get(14)?,
        created_at: row.get(15)?,
        started_at: row.get(16)?,
        updated_at: row.get(17)?,
        completed_at: row.get(18)?,
    })
}

fn map_storage_migration(row: &Row<'_>) -> rusqlite::Result<StorageMigration> {
    Ok(StorageMigration {
        id: row.get(0)?,
        source_path: row.get(1)?,
        destination_path: row.get(2)?,
        status: row.get(3)?,
        stage: row.get(4)?,
        progress: row.get(5)?,
        last_sequence: row.get(6)?,
        error_code: row.get(7)?,
        error_message: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
        restart_required: row.get::<_, i64>(11)? != 0,
    })
}

fn map_document(row: &Row<'_>) -> rusqlite::Result<LocalKnowledgeDocument> {
    Ok(LocalKnowledgeDocument {
        id: row.get(0)?,
        knowledge_base_id: row.get(1)?,
        display_name: row.get(2)?,
        relative_path: row.get(3)?,
        current_revision: row.get(4)?,
        file_size: row.get(5)?,
        mime_type: row.get(6)?,
        parse_status: row.get(7)?,
        index_status: row.get(8)?,
        chunk_count: row.get(9)?,
        last_error_code: row.get(10)?,
        last_error_message: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

#[derive(Debug)]
struct LexicalChunk {
    id: String,
    document_id: String,
    chunk_index: i64,
    text: String,
    anchor: Option<String>,
    start_offset: Option<i64>,
    end_offset: Option<i64>,
    metadata_json: Option<String>,
    page_number: Option<i64>,
    slide_number: Option<i64>,
    display_name: String,
    relative_path: String,
    mime_type: String,
    content_hash: String,
}

fn map_lexical_chunk(row: &Row<'_>) -> rusqlite::Result<LexicalChunk> {
    Ok(LexicalChunk {
        id: row.get(0)?,
        document_id: row.get(1)?,
        chunk_index: row.get(2)?,
        text: row.get(3)?,
        anchor: row.get(4)?,
        start_offset: row.get(5)?,
        end_offset: row.get(6)?,
        metadata_json: row.get(7)?,
        page_number: row.get(8)?,
        slide_number: row.get(9)?,
        display_name: row.get(10)?,
        relative_path: row.get(11)?,
        mime_type: row.get(12)?,
        content_hash: row.get(13)?,
    })
}

fn query_document(
    connection: &Connection,
    knowledge_base_id: &str,
    document_id: &str,
) -> Result<Option<LocalKnowledgeDocument>, LocalKnowledgeError> {
    connection
        .query_row(
            "SELECT d.id, d.knowledge_base_id, d.display_name, d.relative_path,
                    d.current_revision, d.file_size, d.mime_type, d.parse_status,
                    d.index_status, COUNT(c.id) AS chunk_count,
                    d.last_error_code, d.last_error_message, d.created_at, d.updated_at
             FROM local_kb_documents d
             LEFT JOIN local_kb_chunks c
               ON c.document_id = d.id AND c.document_revision = d.current_revision
             WHERE d.knowledge_base_id = ?1 AND d.id = ?2
             GROUP BY d.id",
            params![knowledge_base_id, document_id],
            map_document,
        )
        .optional()
        .map_err(Into::into)
}

fn lexical_terms(query: &str) -> Vec<String> {
    let mut terms = query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if terms.is_empty() {
        terms.push(query.to_owned());
    } else if terms.len() > 1 {
        terms.push(query.to_owned());
    }
    let mut unique = Vec::with_capacity(terms.len());
    for term in terms {
        if !unique
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&term))
        {
            unique.push(term);
        }
    }
    unique
}

fn lexical_score(text: &str, terms: &[String]) -> f64 {
    let normalized_text = text.to_lowercase();
    let mut matched_terms = 0usize;
    let mut occurrences = 0usize;
    for term in terms {
        let normalized_term = term.to_lowercase();
        if normalized_term.is_empty() {
            continue;
        }
        let count = normalized_text.match_indices(&normalized_term).count();
        if count > 0 {
            matched_terms += 1;
            occurrences += count.min(3);
        }
    }
    if terms.is_empty() {
        return 0.0;
    }
    let coverage = matched_terms as f64 / terms.len() as f64;
    let density = (occurrences as f64 / (terms.len() * 3) as f64).min(1.0);
    (coverage * 0.7 + density * 0.3).min(1.0)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn source_metadata(knowledge_base_id: &str, chunk: &LexicalChunk) -> Value {
    let stored_metadata = chunk
        .metadata_json
        .as_deref()
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    json!({
        "source": "local",
        "knowledgeBaseId": knowledge_base_id,
        "documentId": chunk.document_id,
        "chunkId": chunk.id,
        "displayName": chunk.display_name,
        "relativePath": chunk.relative_path,
        "mimeType": chunk.mime_type,
        "contentHash": chunk.content_hash,
        "chunkIndex": chunk.chunk_index,
        "anchor": chunk.anchor,
        "startOffset": chunk.start_offset,
        "endOffset": chunk.end_offset,
        "pageNumber": chunk.page_number,
        "slideNumber": chunk.slide_number,
        "metadata": stored_metadata,
    })
}

fn query_job(
    connection: &Connection,
    id: &str,
) -> Result<Option<LocalKnowledgeJob>, LocalKnowledgeError> {
    connection
        .query_row(
            "SELECT id, parent_operation_id, knowledge_base_id, job_type, status, stage,
                    progress, retry_count, last_sequence, outcome, generation_id,
                    heartbeat_at, checkpoint_json, error_code, error_message,
                    created_at, started_at, updated_at, completed_at
             FROM local_kb_jobs WHERE id = ?1",
            [id],
            map_job,
        )
        .optional()
        .map_err(Into::into)
}

fn local_response<T: Serialize>(result: Result<T, LocalKnowledgeError>) -> ApiResponse<T> {
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::failure(error.code, error.message, error.retryable),
    }
}

#[tauri::command]
pub fn local_knowledge_bases_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<LocalKnowledgeBase>> {
    local_response(state.local_knowledge.list_bases())
}

#[tauri::command]
pub fn local_knowledge_file_sources_list(
    state: State<'_, AppState>,
) -> ApiResponse<Vec<LocalKnowledgeFileSource>> {
    local_response(state.local_knowledge.list_file_sources())
}

#[tauri::command]
pub async fn local_knowledge_file_source_add(
    state: State<'_, AppState>,
    request: LocalKnowledgeFileSourcePathRequest,
) -> Result<ApiResponse<LocalKnowledgeFileSource>, String> {
    let store = state.local_knowledge.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || store.add_file_source(request)).await {
            Ok(result) => local_response(result),
            Err(error) => {
                ApiResponse::failure("local_knowledge.file_scan_failed", error.to_string(), true)
            }
        },
    )
}

#[tauri::command]
pub async fn local_knowledge_file_source_rescan(
    state: State<'_, AppState>,
    request: LocalKnowledgeFileSourceIdRequest,
) -> Result<ApiResponse<LocalKnowledgeFileSource>, String> {
    let store = state.local_knowledge.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || store.scan_file_source(&request.id))
            .await
        {
            Ok(result) => local_response(result),
            Err(error) => {
                ApiResponse::failure("local_knowledge.file_scan_failed", error.to_string(), true)
            }
        },
    )
}

#[tauri::command]
pub fn local_knowledge_file_source_update(
    state: State<'_, AppState>,
    request: LocalKnowledgeFileSourceUpdateRequest,
) -> ApiResponse<LocalKnowledgeFileSource> {
    local_response(state.local_knowledge.update_file_source(request))
}

#[tauri::command]
pub fn local_knowledge_file_source_remove(
    state: State<'_, AppState>,
    request: LocalKnowledgeFileSourceIdRequest,
) -> ApiResponse<bool> {
    local_response(state.local_knowledge.remove_file_source(&request.id))
}

#[tauri::command]
pub fn local_knowledge_local_files_list(
    state: State<'_, AppState>,
    request: LocalKnowledgeCatalogFilesRequest,
) -> ApiResponse<Vec<LocalKnowledgeCatalogFile>> {
    local_response(state.local_knowledge.list_catalog_files(request))
}

#[tauri::command]
pub fn local_knowledge_local_file_read(
    state: State<'_, AppState>,
    request: LocalKnowledgeCatalogFileRangeRequest,
) -> ApiResponse<Vec<u8>> {
    local_response(state.local_knowledge.read_catalog_file_range(request))
}

#[tauri::command]
pub fn local_knowledge_local_file_open(
    state: State<'_, AppState>,
    request: LocalKnowledgeCatalogFileOpenRequest,
) -> ApiResponse<bool> {
    local_response(state.local_knowledge.open_catalog_file(request))
}

#[tauri::command]
pub fn local_knowledge_base_get(
    state: State<'_, AppState>,
    request: LocalKnowledgeBaseIdRequest,
) -> ApiResponse<LocalKnowledgeBase> {
    local_response(state.local_knowledge.get_base(&request.id))
}

#[tauri::command]
pub fn local_knowledge_base_create(
    state: State<'_, AppState>,
    request: CreateLocalKnowledgeBaseRequest,
) -> ApiResponse<LocalKnowledgeBase> {
    local_response(state.local_knowledge.create_base(request))
}

#[tauri::command]
pub fn local_knowledge_base_update(
    state: State<'_, AppState>,
    request: UpdateLocalKnowledgeBaseRequest,
) -> ApiResponse<LocalKnowledgeBase> {
    local_response(state.local_knowledge.update_base(request))
}

#[tauri::command]
pub fn local_knowledge_base_delete(
    state: State<'_, AppState>,
    request: LocalKnowledgeBaseIdRequest,
) -> ApiResponse<bool> {
    local_response(state.local_knowledge.delete_base(&request.id))
}

#[tauri::command]
pub fn local_knowledge_documents_list(
    state: State<'_, AppState>,
    request: LocalKnowledgeDocumentsRequest,
) -> ApiResponse<Vec<LocalKnowledgeDocument>> {
    local_response(state.local_knowledge.list_documents(request))
}

#[tauri::command]
pub fn local_knowledge_folders_list(
    state: State<'_, AppState>,
    request: LocalKnowledgeFoldersRequest,
) -> ApiResponse<Vec<LocalKnowledgeFolder>> {
    local_response(state.local_knowledge.list_folders(request))
}

#[tauri::command]
pub fn local_knowledge_folder_create(
    state: State<'_, AppState>,
    request: CreateLocalKnowledgeFolderRequest,
) -> ApiResponse<LocalKnowledgeFolder> {
    local_response(state.local_knowledge.create_folder(request))
}

#[tauri::command]
pub fn local_knowledge_document_file_read(
    state: State<'_, AppState>,
    request: LocalKnowledgeDocumentFileRangeRequest,
) -> ApiResponse<Vec<u8>> {
    local_response(state.local_knowledge.read_document_file_range(request))
}

#[tauri::command]
pub fn local_knowledge_document_file_open(
    state: State<'_, AppState>,
    request: LocalKnowledgeDocumentFileOpenRequest,
) -> ApiResponse<bool> {
    local_response(state.local_knowledge.open_document_file(request))
}

#[tauri::command]
pub fn local_knowledge_documents_import_start(
    state: State<'_, AppState>,
    request: LocalKnowledgeImportRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    local_response(state.local_knowledge.start_import(request))
}

#[tauri::command]
pub fn local_knowledge_jobs_list(
    state: State<'_, AppState>,
    request: Option<LocalKnowledgeJobsRequest>,
) -> ApiResponse<Vec<LocalKnowledgeJob>> {
    local_response(
        state.local_knowledge.list_jobs(
            request
                .as_ref()
                .and_then(|request| request.knowledge_base_id.as_deref()),
        ),
    )
}

#[tauri::command]
pub fn local_knowledge_job_get(
    state: State<'_, AppState>,
    request: LocalKnowledgeJobIdRequest,
) -> ApiResponse<LocalKnowledgeJob> {
    local_response(state.local_knowledge.get_job(&request.id))
}

#[tauri::command]
pub fn local_knowledge_job_cancel(
    state: State<'_, AppState>,
    request: LocalKnowledgeJobIdRequest,
) -> ApiResponse<LocalKnowledgeJob> {
    local_response(state.local_knowledge.cancel_job(&request.id))
}

#[tauri::command]
pub fn local_knowledge_job_retry(
    state: State<'_, AppState>,
    request: LocalKnowledgeJobIdRequest,
) -> ApiResponse<LocalKnowledgeJob> {
    local_response(state.local_knowledge.retry_job(&request.id))
}

#[tauri::command]
pub fn local_knowledge_storage_status(
    state: State<'_, AppState>,
) -> ApiResponse<LocalKnowledgeStorageStatus> {
    local_response(state.local_knowledge.storage_status())
}

#[tauri::command]
pub fn local_knowledge_storage_migrate_start(
    state: State<'_, AppState>,
    request: StorageMigrationStartRequest,
) -> ApiResponse<LocalKnowledgeOperationAccepted> {
    local_response(
        state
            .local_knowledge
            .start_storage_migration(state.database.clone(), request)
            .map(|migration| LocalKnowledgeOperationAccepted {
                operation_id: migration.id,
                accepted_at: migration.created_at,
            }),
    )
}

#[tauri::command]
pub fn local_knowledge_storage_migration_get(
    state: State<'_, AppState>,
    request: StorageMigrationIdRequest,
) -> ApiResponse<StorageMigration> {
    local_response(state.local_knowledge.get_storage_migration(&request.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn test_store() -> (LocalKnowledgeStore, PathBuf) {
        let root = std::env::temp_dir().join(format!("fox-local-knowledge-{}", Uuid::new_v4()));
        (LocalKnowledgeStore::open(&root).unwrap(), root)
    }

    fn test_database() -> (Database, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("fox-local-knowledge-db-{}.db", Uuid::new_v4()));
        (Database::open(path.clone()).unwrap(), path)
    }

    fn wait_for_storage_migration(store: &LocalKnowledgeStore, id: &str) -> StorageMigration {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let migration = store.get_storage_migration(id).unwrap();
            if matches!(
                migration.status.as_str(),
                "completed" | "failed" | "cancelled"
            ) {
                return migration;
            }
            assert!(
                Instant::now() < deadline,
                "storage migration did not finish in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn creates_updates_and_deletes_knowledge_bases() {
        let (store, root) = test_store();
        let created = store
            .create_base(CreateLocalKnowledgeBaseRequest {
                name: " 项目资料 ".to_owned(),
                description: Some(" 本地文档 ".to_owned()),
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        assert_eq!(created.name, "项目资料");
        assert_eq!(created.description.as_deref(), Some("本地文档"));
        assert!(!created.text_index_ready);
        assert!(!created.vector_index_ready);
        assert_eq!(created.search_mode, "keyword");
        assert_eq!(created.embedding_model, None);
        assert_eq!(created.chunk_count, Some(0));
        assert_eq!(created.vector_count, None);
        assert_eq!(created.fallback_reason.as_deref(), Some("未配置向量模型"));
        assert_eq!(store.list_bases().unwrap().len(), 1);

        let updated = store
            .update_base(UpdateLocalKnowledgeBaseRequest {
                id: created.id,
                name: "产品资料".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        assert_eq!(updated.name, "产品资料");
        assert_eq!(updated.description, None);
        let base_directory = root
            .join("knowledge-bases")
            .join(&updated.id)
            .join("documents");
        std::fs::create_dir_all(&base_directory).unwrap();
        std::fs::write(base_directory.join("sample.txt"), b"sample").unwrap();
        let index_directory = root.join("indexes").join(&updated.id).join("generation-1");
        std::fs::create_dir_all(&index_directory).unwrap();
        std::fs::write(index_directory.join("index.bin"), b"zvec").unwrap();

        assert!(store.delete_base(&updated.id).unwrap());
        assert!(store.list_bases().unwrap().is_empty());
        assert!(!root.join("knowledge-bases").join(&updated.id).exists());
        assert!(!root.join("indexes").join(&updated.id).exists());
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn enforces_one_active_generation_and_index_job() {
        let (store, root) = test_store();
        let base = store
            .create_base(CreateLocalKnowledgeBaseRequest {
                name: "基准库".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        let connection = store.raw_connection().unwrap();
        let now = crate::database::now_ms();
        connection
            .execute(
                "INSERT INTO local_kb_index_generations(
                    id, knowledge_base_id, sequence, status, vector_store_kind,
                    embedding_model_id, dimension, chunk_config_hash, parser_version,
                    created_at
                 ) VALUES ('g1', ?1, 1, 'active', 'memory', 'test', 3, 'chunk', 'parser', ?2)",
                params![base.id, now],
            )
            .unwrap();
        assert!(connection
            .execute(
                "INSERT INTO local_kb_index_generations(
                    id, knowledge_base_id, sequence, status, vector_store_kind,
                    embedding_model_id, dimension, chunk_config_hash, parser_version,
                    created_at
                 ) VALUES ('g2', ?1, 2, 'active', 'memory', 'test', 3, 'chunk', 'parser', ?2)",
                params![base.id, now],
            )
            .is_err());
        connection
            .execute(
                "INSERT INTO local_kb_jobs(
                    id, knowledge_base_id, job_type, status, progress, created_at, updated_at
                 ) VALUES ('j1', ?1, 'index', 'queued', 0, ?2, ?2)",
                params![base.id, now],
            )
            .unwrap();
        assert!(connection
            .execute(
                "INSERT INTO local_kb_jobs(
                    id, knowledge_base_id, job_type, status, progress, created_at, updated_at
                 ) VALUES ('j2', ?1, 'rebuild', 'running', 1, ?2, ?2)",
                params![base.id, now],
            )
            .is_err());
        drop(connection);
        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn opens_idempotently_and_reports_storage() {
        let (store, root) = test_store();
        let status = store.storage_status().unwrap();
        assert!(status.writable);
        assert_eq!(status.schema_version, KNOWLEDGE_SCHEMA_VERSION);
        drop(store);
        let reopened = LocalKnowledgeStore::open(&root).unwrap();
        assert_eq!(reopened.list_bases().unwrap(), Vec::new());
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn seeds_default_fox_guide_idempotently() {
        let (store, root) = test_store();
        store.ensure_default_fox_guide().unwrap();
        store.ensure_default_fox_guide().unwrap();

        let guide = store.get_base(DEFAULT_FOX_GUIDE_ID).unwrap();
        assert_eq!(guide.name, DEFAULT_FOX_GUIDE_NAME);
        assert_eq!(
            guide.document_count,
            DEFAULT_FOX_GUIDE_DOCUMENTS.len() as i64
        );
        let documents = store
            .list_documents(LocalKnowledgeDocumentsRequest {
                knowledge_base_id: DEFAULT_FOX_GUIDE_ID.to_owned(),
                query: None,
            })
            .unwrap();
        assert_eq!(documents.len(), DEFAULT_FOX_GUIDE_DOCUMENTS.len());
        assert!(documents
            .iter()
            .all(|document| document.parse_status == "ready" && document.chunk_count > 0));
        assert!(documents
            .iter()
            .any(|document| document.relative_path == "03-插件中心/01-插件中心.md"));

        drop(store);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn catalogs_selected_folders_without_deleting_source_files() {
        let (store, root) = test_store();
        let source_root =
            std::env::temp_dir().join(format!("fox-local-files-source-{}", Uuid::new_v4()));
        let nested = source_root.join("reports");
        std::fs::create_dir_all(&nested).unwrap();
        let notes_path = source_root.join("notes.md");
        let report_path = nested.join("report.pdf");
        std::fs::write(&notes_path, "本地文件目录测试").unwrap();
        std::fs::write(&report_path, b"pdf-placeholder").unwrap();
        std::fs::write(source_root.join("archive.zip"), b"ignored").unwrap();

        let source = store
            .add_file_source(LocalKnowledgeFileSourcePathRequest {
                path: source_root.to_string_lossy().into_owned(),
            })
            .unwrap();
        assert_eq!(source.file_count, 2);
        assert_eq!(store.list_file_sources().unwrap().len(), 1);

        let all_files = store
            .list_catalog_files(LocalKnowledgeCatalogFilesRequest {
                source_id: Some(source.id.clone()),
                query: None,
                category: Some("all".to_owned()),
                limit: None,
                offset: None,
            })
            .unwrap();
        assert_eq!(all_files.len(), 2);
        assert!(all_files.iter().any(|file| file.display_name == "notes.md"));
        assert!(all_files
            .iter()
            .any(|file| file.relative_path.contains("report.pdf")));
        let first_page = store
            .list_catalog_files(LocalKnowledgeCatalogFilesRequest {
                source_id: Some(source.id.clone()),
                query: None,
                category: None,
                limit: Some(1),
                offset: Some(0),
            })
            .unwrap();
        let second_page = store
            .list_catalog_files(LocalKnowledgeCatalogFilesRequest {
                source_id: Some(source.id.clone()),
                query: None,
                category: None,
                limit: Some(1),
                offset: Some(1),
            })
            .unwrap();
        assert_eq!(first_page.len(), 1);
        assert_eq!(second_page.len(), 1);
        assert_ne!(first_page[0].id, second_page[0].id);
        let notes = all_files
            .iter()
            .find(|file| file.display_name == "notes.md")
            .unwrap();
        let expected = "本地文件目录测试".as_bytes();
        let bytes = store
            .read_catalog_file_range(LocalKnowledgeCatalogFileRangeRequest {
                id: notes.id.clone(),
                start: 0,
                end: u64::try_from(expected.len() - 1).unwrap(),
            })
            .unwrap();
        assert_eq!(bytes, expected);

        let pdf_files = store
            .list_catalog_files(LocalKnowledgeCatalogFilesRequest {
                source_id: None,
                query: Some("report".to_owned()),
                category: Some("pdf".to_owned()),
                limit: Some(10),
                offset: None,
            })
            .unwrap();
        assert_eq!(pdf_files.len(), 1);
        assert_eq!(pdf_files[0].extension, "pdf");

        assert!(store.remove_file_source(&source.id).unwrap());
        assert!(store.list_file_sources().unwrap().is_empty());
        assert!(store
            .list_catalog_files(LocalKnowledgeCatalogFilesRequest {
                source_id: None,
                query: None,
                category: None,
                limit: None,
                offset: None,
            })
            .unwrap()
            .is_empty());
        assert!(notes_path.exists());
        assert!(report_path.exists());

        drop(store);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }

    #[test]
    fn imports_text_into_documents_and_chunks() {
        let (store, root) = test_store();
        let base = store
            .create_base(CreateLocalKnowledgeBaseRequest {
                name: "导入测试".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        let source_root = std::env::temp_dir().join(format!("fox-kb-source-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&source_root).unwrap();
        let source_path = source_root.join("notes.md");
        std::fs::write(&source_path, "第一段知识。\n第二段知识。").unwrap();

        let accepted = store
            .start_import(LocalKnowledgeImportRequest {
                knowledge_base_id: base.id.clone(),
                files: vec![LocalKnowledgeImportFileRequest {
                    source_path: source_path.to_string_lossy().into_owned(),
                    relative_path: None,
                }],
                parser_version: None,
                chunk_config_hash: None,
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let jobs = store.list_jobs(Some(&base.id)).unwrap();
            if jobs.iter().all(|job| {
                matches!(
                    job.status.as_str(),
                    "completed" | "failed" | "cancelled" | "interrupted"
                )
            }) {
                assert_eq!(jobs.len(), 1);
                assert_eq!(
                    jobs[0].parent_operation_id.as_deref(),
                    Some(accepted.operation_id.as_str())
                );
                assert_eq!(jobs[0].status, "completed");
                break;
            }
            assert!(
                Instant::now() < deadline,
                "import job did not finish in time"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let documents = store
            .list_documents(LocalKnowledgeDocumentsRequest {
                knowledge_base_id: base.id.clone(),
                query: None,
            })
            .unwrap();
        assert_eq!(documents.len(), 1);
        assert_eq!(documents[0].display_name, "notes.md");
        assert_eq!(documents[0].parse_status, "ready");
        assert_eq!(documents[0].chunk_count, 1);
        let expected_document = "第一段知识。\n第二段知识。".as_bytes();
        let document_bytes = store
            .read_document_file_range(LocalKnowledgeDocumentFileRangeRequest {
                knowledge_base_id: base.id.clone(),
                document_id: documents[0].id.clone(),
                start: 0,
                end: u64::try_from(expected_document.len() - 1).unwrap(),
            })
            .unwrap();
        assert_eq!(document_bytes, expected_document);

        let listed = store
            .list_bases_for_ids(std::slice::from_ref(&base.id))
            .unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, base.id);
        assert_eq!(listed[0].document_count, 1);
        assert!(listed[0].text_index_ready);
        assert!(!listed[0].vector_index_ready);
        assert_eq!(listed[0].search_mode, "keyword");
        assert_eq!(listed[0].chunk_count, Some(1));
        assert_eq!(listed[0].vector_count, None);
        assert_eq!(listed[0].fallback_reason.as_deref(), Some("未配置向量模型"));

        let search = store
            .search_lexical(&base.id, "第二段", Some(3), Some(1_000))
            .unwrap();
        assert_eq!(search["source"], "local");
        assert_eq!(search["fallback"]["kind"], "sqlite_lexical");
        assert_eq!(search["items"].as_array().map(Vec::len), Some(1));
        let item = &search["items"][0];
        assert_eq!(item["documentId"], documents[0].id);
        assert_eq!(item["sourceMetadata"]["relativePath"], "notes.md");
        let chunk_id = item["chunkId"].as_str().unwrap().to_owned();

        let read = store
            .read_document(&base.id, &documents[0].id, Some(&chunk_id), Some(1_000))
            .unwrap();
        assert_eq!(read["source"], "local");
        assert_eq!(read["documentId"], documents[0].id);
        assert_eq!(read["chunkId"], chunk_id);
        assert_eq!(read["chunks"].as_array().map(Vec::len), Some(1));
        assert!(read["content"].as_str().unwrap().contains("第二段知识"));

        drop(store);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(source_root);
    }

    #[test]
    fn migrates_storage_safely_and_reopens_from_configured_path() {
        let (store, root) = test_store();
        let (database, database_path) = test_database();
        let base = store
            .create_base(CreateLocalKnowledgeBaseRequest {
                name: "迁移测试".to_owned(),
                description: None,
                embedding_model_id: None,
                chunk_size: None,
                chunk_overlap: None,
                search_mode: None,
            })
            .unwrap();
        let destination = root.parent().unwrap().join(format!(
            "fox-local-knowledge-destination-{}",
            Uuid::new_v4()
        ));
        let accepted = store
            .start_storage_migration(
                database.clone(),
                StorageMigrationStartRequest {
                    destination_path: destination.to_string_lossy().into_owned(),
                },
            )
            .unwrap();
        let migration = wait_for_storage_migration(&store, &accepted.id);
        assert_eq!(migration.status, "completed");
        assert!(migration.restart_required);
        let normalized_destination =
            crate::local_knowledge_storage::normalize_storage_path(&destination, None)
                .unwrap()
                .to_string_lossy()
                .into_owned();
        assert_eq!(
            database
                .app_setting(crate::local_knowledge_storage::LOCAL_KNOWLEDGE_STORAGE_PATH_KEY)
                .unwrap()
                .as_deref(),
            Some(normalized_destination.as_str())
        );
        let status = store.storage_status().unwrap();
        assert!(status.restart_required);
        assert_eq!(status.migration_id.as_deref(), Some(accepted.id.as_str()));
        assert_eq!(
            status.pending_root_path.as_deref(),
            Some(normalized_destination.as_str())
        );

        drop(store);
        let reopened = LocalKnowledgeStore::open(&destination).unwrap();
        assert_eq!(reopened.list_bases().unwrap()[0].id, base.id);
        let reopened_status = reopened.storage_status().unwrap();
        assert!(!reopened_status.restart_required);
        assert_eq!(reopened_status.pending_root_path, None);
        assert_eq!(reopened_status.migration_id, None);
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(destination);
        let _ = std::fs::remove_file(database_path);
    }

    #[test]
    fn rejects_failed_storage_destination_without_changing_current_path() {
        let (store, root) = test_store();
        let (database, database_path) = test_database();
        let destination = root
            .parent()
            .unwrap()
            .join(format!("fox-local-knowledge-non-empty-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(destination.join("keep.txt"), b"do not overwrite").unwrap();
        let error = store
            .start_storage_migration(
                database.clone(),
                StorageMigrationStartRequest {
                    destination_path: destination.to_string_lossy().into_owned(),
                },
            )
            .unwrap_err();
        assert_eq!(
            error.code(),
            "local_knowledge.storage_migration_destination_exists"
        );
        let status = store.storage_status().unwrap();
        assert!(!status.restart_required);
        assert_eq!(status.root_path, root.to_string_lossy());
        assert!(database
            .app_setting(crate::local_knowledge_storage::LOCAL_KNOWLEDGE_STORAGE_PATH_KEY)
            .unwrap()
            .is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(destination);
        let _ = std::fs::remove_file(database_path);
    }
}
