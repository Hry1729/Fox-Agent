use super::{
    migrations, AgentRecord, AgentResourceRecord, AgentResourcesRecord, ArtifactRecord,
    AttachmentRecord, ConversationDetail, ConversationExpertBinding,
    ConversationExpertBindingError, ConversationRuntimeRecord, ConversationSummary,
    ExpertPackageVersionRecord, ExternalRunRecord, InstallExpertPackageVersionRequest,
    KnowledgeBindingRecord, KnowledgeDocumentActivity, KnowledgeDocumentAnnotation,
    KnowledgeDocumentBookmark, KnowledgeDocumentReadingState, KnowledgePreviewCacheEntry,
    KnowledgeReference, KnowledgeReferenceBindingInput, KnowledgeReferenceBindingRecord,
    McpPluginSourceRecord, MessageRecord, ModelConnectionTest, ModelProviderRecord,
    ModelServiceRecord, PendingWorkModeDispatch, PluginCatalogEntryRecord, PluginInstallStatus,
    PluginInstallationRecord, PluginKind, PluginOrigin, ProviderModelRecord,
    RecoverableExternalRunRecord, RunEventRecord, RunRecord, RuntimePromptMessage,
    RuntimeSessionRecord, SaveAgentRequest, StartRunResult, ToolCallRecord, ToolResultRange,
    YuxiAgentRecord,
    YuxiConnectionTest, YuxiServiceRecord,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod a1_workflow;
mod app_management;
mod bundled_capabilities;
mod computed_artifacts;
mod child_runs;
mod digital_colleagues;
mod expert_teams;
mod expert_workflows;
mod graph_lead;
mod kernel;
mod kernel_compaction;
pub(crate) mod kernel_authorization;
pub(crate) mod kernel_continuation;
pub(crate) mod kernel_execution_admission;
pub(crate) mod kernel_execution_policy;
pub(crate) mod kernel_jobs;
mod kernel_job_execution;
mod model_usage;
pub(crate) mod kernel_reconciliation;
pub(crate) use kernel_authorization::{
    AdditionalGrant, GrantRegistration, GrantScopeKind, GrantSkipReason,
};
pub(crate) use kernel_continuation::{run_budget_for_tier, BudgetTier, ContinuableRun, ContinuationRequest};
pub(crate) use kernel_continuation::{PreparedContinuation, VerifiedContinuationPermission};
pub(crate) use kernel_jobs::{JobSnapshot, JobStartOutcome, JobStartRequest, JobState};
mod run_control;
mod skill_activations;
mod delivery_checks;
mod run_steering;
mod managed_files;
pub(crate) use delivery_checks::{
    DeliveryArtifactRow, DeliveryChecklistItem, DeliveryChecklistSeed, DeliveryRequirement,
    RequirementKind,
};
pub(crate) use managed_files::{ManagedFileSource, ManagedFileVersion, ManagedFileVersionInput};
pub(crate) use run_steering::{
    adopt_received_steering_in_tx, apply_delivered_steering_in_tx, cancel_open_steering_in_tx,
    deliver_steering_in_tx, SteeringMessage, SteeringDecision, MAX_STEERING_FOLLOWUPS,
};
pub(crate) use skill_activations::SkillActivationRecord;
mod kernel_model_config;
mod kernel_initial_input;
mod kernel_host;
mod kernel_projection;
mod kernel_display;
pub(crate) use kernel_host::KernelHostScope;
mod memory;
mod observability;
mod work_events;
mod work_graph;

pub use app_management::{
    AppNotificationRecord, AppNotificationUpsert, GlobalSearchRecord, MessageFeedbackRecord,
    NotificationPreferencesRecord, ProjectManagementRecord,
};
pub(crate) use digital_colleagues::MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS;

pub(crate) use child_runs::CreateChildRunInput;
#[allow(unused_imports)]
pub use graph_lead::{
    ActivateReadOnlyGraphInput, ActiveGraphNodeCancelIntent, ActiveGraphNodeReviewRequest,
    CreateGraphNodeCancelIntentInput, CreateGraphNodeReviewRequestInput,
    CreateReadOnlyGraphAcceptanceInput, FinishReadOnlyGraphNodeInput, GraphAcceptanceIntentResult,
    GraphAcceptanceResult, GraphCriterionEvidenceInput, GraphLeadActivationResult,
    GraphLeadEdgeSnapshot, GraphLeadNodeFinishResult, GraphLeadNodeReadiness,
    GraphLeadNodeSnapshot, GraphLeadNodeStartResult, GraphLeadSnapshot,
    GraphNodeCancelActivationResult, GraphNodeCancelIntentResult, GraphNodeReviewActivationResult,
    GraphNodeReviewDecisionResult, GraphNodeReviewOutcome, GraphNodeReviewRequestResult,
    GraphReviewerDispatchResult, PendingGraphAcceptance, StartReadyReadOnlyGraphNodeInput,
};
pub use work_events::WORK_EVENT_TYPES;
#[allow(unused_imports)]
pub use work_graph::{
    AddEvidenceInput, CreateGoalInput, CreateTaskInput, FinishTaskAttemptInput, GoalRepository,
    PreflightTaskRepairOverrideInput, RepositoryError, StartTaskAttemptInput,
    StartTaskRepairOverrideInput, TaskEvidenceRepository, WorkTaskRepository,
};

const DEFAULT_AGENT_ID: &str = "fox-general";
pub(crate) const TOOL_EXECUTION_APPROVAL_CATEGORY: &str = "tool_execution";
pub(crate) const TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY: &str = "task_repair_budget_override";
const BUILTIN_AGENTS: &[(&str, &str, &str, &str)] = &[
    (
        DEFAULT_AGENT_ID,
        "Fox 通用助手",
        "Fox 默认通用智能体",
        "You are Fox, a careful general-purpose desktop assistant. Lead with the useful result, use evidence from tools, and make the smallest safe change that fully solves the user's request.",
    ),
    (
        "fox-debugger",
        "Fox 调试专家",
        "定位根因、修复缺陷并补充回归验证",
        r#"You are Fox's debugging specialist. Reproduce the problem when possible, collect concrete evidence, isolate the earliest root cause, and distinguish cause from symptom. Prefer the smallest complete fix, preserve unrelated behavior, and add or run focused regression checks. State uncertainty plainly. Do not claim a bug is fixed until the relevant verification succeeds."#,
    ),
    (
        "fox-frontend",
        "Fox 前端体验专家",
        "优化桌面端界面、交互、可访问性与视觉一致性",
        r#"You are Fox's frontend experience specialist. Work from the existing design system and product intent. Check layout, typography, spacing, responsive behavior, interaction states, accessibility, and visual consistency together. Reuse existing components and tokens before adding new abstractions. Keep changes scoped, protect working interactions, and verify both behavior and build output."#,
    ),
    (
        "fox-reviewer",
        "Fox 代码审查专家",
        "按风险审查正确性、回归、测试与可维护性",
        r#"You are Fox's code review specialist. When asked to review, inspect before proposing changes and do not edit files unless the user explicitly asks for a fix. Prioritize concrete correctness, security, data-loss, concurrency, compatibility, and regression risks over style preferences. Rank findings by severity, cite precise files or symbols, explain impact, and identify missing tests. Say clearly when no material finding is supported by evidence."#,
    ),
    (
        "fox-security",
        "Fox 安全审查专家",
        "检查权限边界、注入、凭证、依赖与数据安全",
        r#"You are Fox's application security specialist. Use a threat-informed, evidence-first review of trust boundaries, authorization, injection, secret handling, local file access, network calls, dependencies, and sensitive data. Stay within the authorized project and never perform destructive or exploitative actions merely to demonstrate risk. Recommend proportional mitigations and verification steps, and separate confirmed vulnerabilities from defense-in-depth suggestions."#,
    ),
    (
        "fox-architect",
        "Fox 架构规划专家",
        "分析约束、拆分方案并规划可验证的增量实施",
        r#"You are Fox's architecture and planning specialist. First understand the existing system, constraints, and acceptance criteria. Produce an incremental plan with explicit boundaries, dependencies, risks, migration strategy, and verification. Prefer adapting established project patterns over inventing parallel systems. Planning must remain actionable: when the user asks for implementation, proceed through the normal Fox tools and evidence workflow instead of stopping at a document."#,
    ),
];
const INITIAL_HISTORY_MESSAGES: usize = 120;
pub(crate) const MAX_STORED_TOOL_RESULT_BYTES: usize = 128 * 1024;
/// Largest single response of `Database::tool_result_range`. A range read is
/// paginated so a stored result can always be walked to its end without ever
/// returning an unbounded blob.
const MAX_TOOL_RESULT_RANGE_BYTES: usize = 64 * 1024;

/// The Host's trusted storage fact for one settled tool result, derived from the
/// durable row through the very same range reader the model would use.
///
/// Extracted as a free function over `&Connection` so that the admission-side
/// delivery replay (which runs inside a `Transaction`) and
/// `Database::tool_result_storage` derive it from ONE definition. Two copies
/// would let the model view and the admission decision disagree about whether
/// any byte was omitted — which is exactly the confusion A-F1 is about.
///
/// A non-terminal row, a row with no result, or a row the range reader can only
/// serve as a preview is `unknown`: no caller may read that as permission to
/// omit bytes, and the projection keeps every byte instead.
pub(crate) fn tool_result_storage_for(
    connection: &Connection,
    run_id: &str,
    tool_call_id: &str,
) -> Result<crate::kernel_compaction::ToolResultStorage, String> {
    use crate::kernel_compaction::ToolResultStorage;
    let record = query_tool_call(connection, run_id, tool_call_id)
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "tool result has not been stored".to_owned())?;
    if !matches!(record.status.as_str(), "completed" | "failed") || record.result.is_none() {
        return Ok(ToolResultStorage::unknown());
    }
    let reference = crate::kernel_compaction::tool_result_ref(run_id, tool_call_id)
        .ok_or_else(|| "invalid stored tool identity".to_owned())?;
    let (parsed_run, parsed_call) = crate::kernel_compaction::parse_tool_result_ref(&reference)
        .ok_or_else(|| "invalid tool result reference".to_owned())?;
    if parsed_run != record.run_id || parsed_call != record.runtime_tool_call_id {
        return Err("tool result reference does not name this record".to_owned());
    }
    // When the full result was spilled to a content-addressed blob, ranges are
    // served from it: the inline preview is only the model-facing copy. Blob
    // identity was verified at write time, so a present blob is by construction
    // the complete original.
    let blob_body: Option<Value> = connection
        .query_row(
            "SELECT b.body_json FROM tool_calls t
             JOIN tool_call_result_blobs b ON b.sha256 = t.result_blob_sha256
             WHERE t.run_id = ?1 AND t.runtime_tool_call_id = ?2
               AND t.result_blob_sha256 IS NOT NULL",
            params![record.run_id, record.runtime_tool_call_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .map(|body| parse_json(&body));
    let serving = blob_body.as_ref().or(record.result.as_ref());
    let (_text, truncated, original_bytes) = stored_result_text(serving);
    if truncated {
        return Ok(ToolResultStorage::unknown());
    }
    Ok(ToolResultStorage::whole(original_bytes))
}
const KNOWLEDGE_PREVIEW_CACHE_LIMIT_KEY: &str = "knowledge_preview_cache_limit_bytes";
const USER_PROFILE_KEY: &str = "user_profile";
const AGENT_RECORD_COLUMNS: &str =
    "id, name, description, runtime_type, agent_kind, invocation_mode, visibility, default_model, \
     system_prompt, icon, category, opening_suggestions_json, is_builtin, package_version, \
     package_manifest_json, package_source, package_id, package_hash";

#[derive(Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    kernel_changes: Arc<super::kernel_changes::KernelChanges>,
    /// Volatile worker-observed model output progress: run_id -> wall_ms of the
    /// last preview (text, thinking, or tool-parameter bytes). Never persisted:
    /// a restart measures idleness conservatively from the persisted model
    /// wall anchor until fresh progress arrives. Follows the KernelChanges
    /// precedent (process-local hub on the shared Database handle).
    model_progress: Arc<Mutex<BTreeMap<String, i64>>>,
}

fn require_legacy_run_writer(connection: &Connection, run_id: &str) -> rusqlite::Result<()> {
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
        [run_id], |row| row.get::<_,bool>(0))? {
        return Err(projection_violation("Kernel-owned read models cannot be changed by a Legacy writer".into()));
    }
    Ok(())
}

impl Database {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        migrations::run(&mut connection, now_ms()).map_err(|error| error.to_string())?;
        let database = Self {
            connection: Arc::new(Mutex::new(connection)),
            kernel_changes: Arc::new(super::kernel_changes::KernelChanges::default()),
            model_progress: Arc::new(Mutex::new(BTreeMap::new())),
        };
        database.seed_builtin_agents()?;
        database.seed_bundled_experts()?;
        database.seed_expert_avatars()?;
        Ok(database)
    }

    pub(crate) fn subscribe_kernel_changes(&self) -> std::sync::mpsc::Receiver<()> {
        self.kernel_changes.subscribe()
    }

    /// Record worker-observed model output for a Run (volatile timing signal).
    /// Called by the display preview path for every accepted preview, including
    /// tool-parameter-only progress that never appears as visible text. Entries
    /// are consumed once by the coordinator tick that advances the controller;
    /// terminal runs drop their entry. Best-effort and bounded: never blocks,
    /// never persists, never fails the caller.
    pub(crate) fn note_kernel_model_progress(&self, run_id: &str, wall_ms: i64) {
        if let Ok(mut guard) = self.model_progress.lock() {
            while guard.len() >= 8192 {
                // Fail-safe bound; progress is best-effort and runs are few.
                guard.pop_first();
            }
            guard.insert(run_id.to_owned(), wall_ms);
        }
    }

    /// Take the pending progress wall timestamp for a Run, if any. Consume-once
    /// semantics: every preview is folded into the controller exactly once, so
    /// repeated ticks without fresh output correctly grow idleness.
    pub(crate) fn consume_kernel_model_progress(&self, run_id: &str) -> Option<i64> {
        self.model_progress
            .lock()
            .ok()
            .and_then(|mut guard| guard.remove(run_id))
    }

    pub(crate) fn clear_kernel_model_progress(&self, run_id: &str) {
        if let Ok(mut guard) = self.model_progress.lock() {
            guard.remove(run_id);
        }
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

    pub fn app_setting(&self, key: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT value FROM app_settings WHERE key = ?1",
                    [key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
    }

    pub fn set_app_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO app_settings(key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value,
                     updated_at = excluded.updated_at",
                params![key, value, now_ms()],
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

    /// Raw connection access for repository code. Crate-visible so tests in
    /// sibling modules can seed durable facts without inventing a production
    /// entry point; it grants no authority the caller did not already have.
    pub(crate) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, rusqlite::Error>,
    ) -> Result<T, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        operation(&mut connection).map_err(|error| error.to_string())
    }

    /// Test-only: execute raw SQL (e.g. install a reject-trigger to simulate
    /// durable write failure). Not used by production code.
    #[cfg(test)]
    pub(crate) fn execute_raw_sql(&self, sql: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            connection.execute_batch(sql)?;
            Ok(())
        })
    }

    /// Test-only: read one integer with bound parameters. Lets a test assert on a
    /// durable fact (an uncertain execution, a pending dispatch) that no product
    /// accessor exposes, without reaching into the schema from production code.
    #[cfg(test)]
    pub(crate) fn query_count_raw(
        &self,
        sql: &str,
        params: &[&dyn rusqlite::ToSql],
    ) -> Result<i64, String> {
        self.with_connection(|connection| {
            connection.query_row(sql, params, |row| row.get::<_, i64>(0))
        })
    }

    fn seed_builtin_agents(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            for (index, (id, name, description, system_prompt)) in
                BUILTIN_AGENTS.iter().enumerate()
            {
                // These stable identities are now supplied by the versioned library.
                if matches!(*id, "fox-frontend" | "fox-reviewer" | "fox-architect") {
                    continue;
                }
                let created_at = now + index as i64;
                connection.execute(
                    "INSERT OR IGNORE INTO agents(
                        id, name, description, runtime_type, system_prompt, default_model, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, 'pi', ?4, 'configured-model', ?5, ?5)",
                    params![id, name, description, system_prompt, created_at],
                )?;
                let category = match *id {
                    "fox-debugger" => "engineering",
                    "fox-frontend" => "design",
                    "fox-reviewer" | "fox-security" => "review",
                    "fox-architect" => "planning",
                    _ => "general",
                };
                let manifest = json!({
                    "version": "1.0.0",
                    "prompt": system_prompt,
                    "skills": [],
                    "knowledge": [],
                    "mcpServers": [],
                    "allowedTools": ["read", "ls", "find", "grep", "read_attachment", "attachment_compute", "compute_job_start", "compute_job_status", "compute_job_cancel", "compute_job_result", "read_tool_result", "skill_load", "write_file", "edit_file", "run_command", "web_search", "web_read", "http_request", "system_info", "sqlite_read", "structured_data", "git_read", "test_run", "code_check", "format_code", "tabular_data", "child_agent_list", "child_run_start", "child_run_collect", "child_run_cancel", "memory_search", "memory_propose", "list_knowledge_bases", "search_knowledge", "read_knowledge_document", "query_knowledge_graph", "list_mcp_tools", "call_mcp_tool", "work_snapshot_get", "goal_propose", "goal_complete", "task_create_many", "task_update", "task_attempt_start", "task_attempt_finish", "task_repair_start", "task_repair_escalate_start", "task_evidence_add", "task_evidence_validate", "plan_revision_create", "review_finding_add", "review_finding_resolve", "acceptance_submit", "workflow_snapshot_get", "workflow_start", "workflow_stage_start", "workflow_stage_complete", "workflow_stage_fail", "workflow_cancel"]
                });
                let (agent_kind, invocation_mode, visibility) = if *id == DEFAULT_AGENT_ID {
                    ("assistant", "primary", "chat_selector")
                } else {
                    ("expert", "inline", "expert_center")
                };
                connection.execute(
                    "UPDATE agents SET is_builtin = 1, category = ?2,
                         package_version = '1.0.0', package_manifest_json = ?3,
                         agent_kind = ?4, invocation_mode = ?5, visibility = ?6,
                         package_source = 'builtin', package_id = NULL, package_hash = NULL
                     WHERE id = ?1",
                    params![
                        id,
                        category,
                        manifest.to_string(),
                        agent_kind,
                        invocation_mode,
                        visibility
                    ],
                )?;
            }
            Ok(())
        })
    }

    pub fn repair_interrupted_runs(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            child_runs::reconcile_terminal_graph_children_in_transaction(&transaction, now)?;
            transaction.execute(
                "UPDATE runs
                 SET status = 'interrupted', finished_at = ?1,
                     error_code = 'runtime.application_restarted',
                     error_message = 'Fox closed before the run reached a terminal state.'
                 WHERE status IN ('queued', 'running', 'cancelling')
                   AND NOT EXISTS (SELECT 1 FROM run_control_bindings b WHERE b.run_id=runs.id AND b.authority='authoritative')
                   AND NOT EXISTS (
                       SELECT 1 FROM conversations c JOIN agents a ON a.id = c.agent_id
                       WHERE c.id = runs.conversation_id AND a.runtime_type = 'yuxi'
                         AND runs.external_run_id IS NOT NULL
                   )",
                [now],
            )?;
            transaction.execute(
                "UPDATE messages SET status = 'interrupted', updated_at = ?1
                 WHERE status = 'streaming'
                   AND NOT EXISTS (SELECT 1 FROM run_control_bindings b WHERE b.run_id=messages.run_id AND b.authority='authoritative')
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
            transaction.execute(
                "UPDATE tool_calls SET status = 'interrupted', completed_at = ?1, updated_at = ?1
                 WHERE status IN ('pending', 'running')
                   AND NOT EXISTS (SELECT 1 FROM run_control_bindings b WHERE b.run_id=tool_calls.run_id AND b.authority='authoritative')
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
            transaction.execute(
                "UPDATE approvals
                 SET status = 'expired', decision_json = '{\"reason\":\"application_restarted\"}',
                      resolved_at = ?1
                 WHERE status = 'pending'
                   AND NOT EXISTS (SELECT 1 FROM tool_calls t JOIN run_control_bindings b ON b.run_id=t.run_id
                       WHERE t.id=approvals.tool_call_id AND b.authority='authoritative')
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
            child_runs::reconcile_terminal_graph_children_in_transaction(&transaction, now)?;
            interrupt_tasks_owned_by_terminal_runs(&transaction, &now.to_string())?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn audit_interrupted_tasks(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            child_runs::reconcile_terminal_graph_children_in_transaction(&transaction, now)?;
            interrupt_tasks_owned_by_terminal_runs(&transaction, &now.to_string())?;
            transaction.commit()?;
            Ok(())
        })
    }

    pub fn list_agents(&self) -> Result<Vec<AgentRecord>, String> {
        self.with_connection(|connection| {
            let sql = format!(
                "SELECT {AGENT_RECORD_COLUMNS}
                 FROM agents ORDER BY CASE WHEN id = ?1 THEN 0 ELSE 1 END, created_at ASC"
            );
            let mut statement = connection.prepare(&sql)?;
            let records = statement
                .query_map([DEFAULT_AGENT_ID], agent_record_from_row)?
                .collect();
            records
        })
    }

    pub fn get_agent(&self, agent_id: &str) -> Result<Option<AgentRecord>, String> {
        self.with_connection(|connection| query_agent_record(connection, agent_id))
    }

    pub fn save_agent(&self, request: &SaveAgentRequest) -> Result<AgentRecord, String> {
        let name = request.name.trim();
        let prompt = request.system_prompt.trim();
        if name.is_empty() || prompt.is_empty() {
            return Err("expert name and system prompt are required".to_owned());
        }
        let id = request
            .id
            .clone()
            .unwrap_or_else(|| format!("fox-user-{}", Uuid::new_v4().simple()));
        let now = now_ms();
        let suggestions = request
            .opening_suggestions
            .iter()
            .map(|item| item.trim())
            .filter(|item| !item.is_empty())
            .take(6)
            .collect::<Vec<_>>();
        let mut manifest = request.package_manifest.clone();
        if !manifest.is_object() {
            manifest = json!({});
        }
        let object = manifest.as_object_mut().expect("manifest object");
        object.insert("version".to_owned(), json!("1.0.0"));
        object.insert("prompt".to_owned(), json!(prompt));
        object
            .entry("skills".to_owned())
            .or_insert_with(|| json!([]));
        object
            .entry("knowledge".to_owned())
            .or_insert_with(|| json!([]));
        object
            .entry("mcpServers".to_owned())
            .or_insert_with(|| json!([]));
        object.entry("allowedTools".to_owned()).or_insert_with(|| {
            json!([
                "read",
                "ls",
                "find",
                "grep",
                "read_attachment",
                "read_tool_result",
                "skill_load",
                "write_file",
                "edit_file",
                "run_command",
                "web_search",
                "web_read",
                "http_request",
                "system_info",
                "sqlite_read",
                "structured_data",
                "git_read",
                "test_run",
                "code_check",
                "format_code",
                "tabular_data",
                "child_agent_list",
                "child_run_start",
                "child_run_collect",
                "child_run_cancel",
                "memory_search",
                "memory_propose",
                "list_knowledge_bases",
                "search_knowledge",
                "read_knowledge_document",
                "query_knowledge_graph",
                "list_mcp_tools",
                "call_mcp_tool",
                "work_snapshot_get",
                "goal_propose",
                "goal_complete",
                "task_create_many",
                "task_update",
                "task_attempt_start",
                "task_attempt_finish",
                "task_repair_start",
                "task_repair_escalate_start",
                "task_evidence_add",
                "task_evidence_validate",
                "plan_revision_create",
                "review_finding_add",
                "review_finding_resolve",
                "acceptance_submit",
                "workflow_snapshot_get",
                "workflow_start",
                "workflow_stage_start",
                "workflow_stage_complete",
                "workflow_stage_fail",
                "workflow_cancel"
            ])
        });
        let enabled_skills = object
            .get("skills")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let enabled_skills_json =
            serde_json::to_string(&enabled_skills).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if request.id.is_some() {
                let (builtin, package_source) = transaction.query_row(
                    "SELECT is_builtin, package_source FROM agents WHERE id = ?1 AND runtime_type = 'pi'",
                    [&id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )?;
                let (agent_kind, invocation_mode, visibility) = if id == DEFAULT_AGENT_ID {
                    if builtin == 0 || package_source != "builtin" {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    ("assistant", "primary", "chat_selector")
                } else {
                    if builtin != 0 || package_source != "local" {
                        return Err(rusqlite::Error::InvalidQuery);
                    }
                    ("expert", "inline", "expert_center")
                };
                transaction.execute(
                    "UPDATE agents SET name = ?2, description = ?3, icon = ?4, category = ?5,
                         system_prompt = ?6, default_model = ?7, opening_suggestions_json = ?8,
                         package_version = '1.0.0', package_manifest_json = ?9, updated_at = ?10,
                         agent_kind = ?11, invocation_mode = ?12, visibility = ?13
                     WHERE id = ?1",
                    params![id, name, request.description.trim(), request.icon.as_deref().filter(|value| !value.trim().is_empty()), request.category.trim(), prompt, request.default_model.trim(), serde_json::to_string(&suggestions).unwrap_or_else(|_| "[]".to_owned()), manifest.to_string(), now, agent_kind, invocation_mode, visibility],
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO agents(id, name, description, runtime_type, system_prompt, default_model,
                         created_at, updated_at, icon, category, opening_suggestions_json, is_builtin,
                         package_version, package_manifest_json, agent_kind, invocation_mode, visibility)
                     VALUES (?1, ?2, ?3, 'pi', ?4, ?5, ?6, ?6, ?7, ?8, ?9, 0, '1.0.0', ?10,
                             'expert', 'inline', 'expert_center')",
                    params![id, name, request.description.trim(), prompt, request.default_model.trim(), now, request.icon.as_deref().filter(|value| !value.trim().is_empty()), request.category.trim(), serde_json::to_string(&suggestions).unwrap_or_else(|_| "[]".to_owned()), manifest.to_string()],
                )?;
            }
            transaction.execute(
                "INSERT INTO agent_runtime_config(
                    agent_id, scope_type, scope_id, config_key, value_json, source,
                    created_at, updated_at
                 ) VALUES (?1, 'agent', ?1, 'skills.enabled', ?2, 'user', ?3, ?3)
                 ON CONFLICT(agent_id, scope_type, scope_id, config_key) DO UPDATE SET
                    value_json = excluded.value_json,
                    source = 'user',
                    updated_at = excluded.updated_at",
                params![id, enabled_skills_json, now],
            )?;
            transaction.commit()
        })?;
        self.get_agent(&id)?
            .ok_or_else(|| "saved expert was not found".to_owned())
    }

    pub fn get_agent_by_package_id(&self, package_id: &str) -> Result<Option<AgentRecord>, String> {
        self.with_connection(|connection| {
            let sql = format!("SELECT {AGENT_RECORD_COLUMNS} FROM agents WHERE package_id = ?1");
            connection
                .query_row(&sql, [package_id], agent_record_from_row)
                .optional()
        })
    }

    pub fn list_expert_package_versions(
        &self,
        expert_id: &str,
    ) -> Result<Vec<ExpertPackageVersionRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, expert_id, package_id, version, package_hash, package_json,
                        source, status, created_at, activated_at
                 FROM expert_package_versions
                 WHERE expert_id = ?1
                 ORDER BY activated_at DESC, created_at DESC, version DESC",
            )?;
            let records = statement
                .query_map([expert_id], |row| {
                    Ok(ExpertPackageVersionRecord {
                        id: row.get(0)?,
                        expert_id: row.get(1)?,
                        package_id: row.get(2)?,
                        version: row.get(3)?,
                        package_hash: row.get(4)?,
                        package: parse_json(&row.get::<_, String>(5)?),
                        source: row.get(6)?,
                        status: row.get(7)?,
                        created_at: row.get(8)?,
                        activated_at: row.get(9)?,
                    })
                })?
                .collect();
            records
        })
    }

    pub fn get_expert_package_version(
        &self,
        expert_id: &str,
        version: &str,
    ) -> Result<Option<ExpertPackageVersionRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, expert_id, package_id, version, package_hash, package_json,
                            source, status, created_at, activated_at
                     FROM expert_package_versions
                     WHERE expert_id = ?1 AND version = ?2",
                    params![expert_id, version],
                    |row| {
                        Ok(ExpertPackageVersionRecord {
                            id: row.get(0)?,
                            expert_id: row.get(1)?,
                            package_id: row.get(2)?,
                            version: row.get(3)?,
                            package_hash: row.get(4)?,
                            package: parse_json(&row.get::<_, String>(5)?),
                            source: row.get(6)?,
                            status: row.get(7)?,
                            created_at: row.get(8)?,
                            activated_at: row.get(9)?,
                        })
                    },
                )
                .optional()
        })
    }

    pub fn known_remote_knowledge_references(&self) -> Result<Vec<KnowledgeReference>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT DISTINCT provider_key, connection_id, knowledge_base_id
                 FROM knowledge_bindings_v2
                 WHERE source = 'remote' AND connection_id IS NOT NULL",
            )?;
            let references = statement
                .query_map([], |row| {
                    Ok(KnowledgeReference {
                        source: "remote".to_owned(),
                        provider_key: row.get(0)?,
                        connection_id: row.get(1)?,
                        id: row.get(2)?,
                    })
                })?
                .collect();
            references
        })
    }

    pub fn install_expert_package_version(
        &self,
        request: &InstallExpertPackageVersionRequest,
    ) -> Result<AgentRecord, String> {
        self.write_expert_package_version(request, false)
    }

    pub fn activate_expert_package_version(
        &self,
        request: &InstallExpertPackageVersionRequest,
    ) -> Result<AgentRecord, String> {
        self.write_expert_package_version(request, true)
    }

    fn write_expert_package_version(
        &self,
        request: &InstallExpertPackageVersionRequest,
        reactivate: bool,
    ) -> Result<AgentRecord, String> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| "database lock is poisoned".to_owned())?;
        let transaction = connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let current = transaction
            .query_row(
                "SELECT id, package_version, package_hash, package_source
                 FROM agents WHERE package_id = ?1",
                [&request.package_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((_, _, current_hash, source)) = &current {
            if source != "imported" {
                return Err("expert.package_source_conflict".to_owned());
            }
            if request.expected_current_hash.as_deref() != current_hash.as_deref() {
                return Err("expert.package_state_changed".to_owned());
            }
        } else if request.expected_current_hash.is_some() {
            return Err("expert.package_state_changed".to_owned());
        }

        let existing_version = transaction
            .query_row(
                "SELECT expert_id, package_hash FROM expert_package_versions
                 WHERE package_id = ?1 AND version = ?2",
                params![request.package_id, request.version],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some((version_expert_id, version_hash)) = &existing_version {
            if version_expert_id != &request.expert_id || version_hash != &request.package_hash {
                return Err("expert.package_version_conflict".to_owned());
            }
            if !reactivate {
                transaction.commit().map_err(|error| error.to_string())?;
                return query_agent_record(&connection, &request.expert_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "expert.package_not_found".to_owned());
            }
        } else if reactivate {
            return Err("expert.package_version_not_found".to_owned());
        }

        let now = now_ms();
        let suggestions_json = serde_json::to_string(&request.opening_suggestions)
            .map_err(|error| error.to_string())?;
        let manifest_json = request.package_manifest.to_string();
        if let Some((expert_id, _, _, _)) = current {
            if expert_id != request.expert_id {
                return Err("expert.package_identity_conflict".to_owned());
            }
            transaction
                .execute(
                    "UPDATE agents SET name = ?2, description = ?3, icon = ?4, category = ?5,
                         system_prompt = ?6, default_model = ?7, opening_suggestions_json = ?8,
                         package_version = ?9, package_manifest_json = ?10,
                         package_hash = ?11, updated_at = ?12
                     WHERE id = ?1 AND package_source = 'imported'",
                    params![
                        request.expert_id,
                        request.name,
                        request.description,
                        request.icon,
                        request.category,
                        request.system_prompt,
                        request.default_model,
                        suggestions_json,
                        request.version,
                        manifest_json,
                        request.package_hash,
                        now
                    ],
                )
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .execute(
                    "INSERT INTO agents(
                        id, name, description, runtime_type, system_prompt, default_model,
                        created_at, updated_at, icon, category, opening_suggestions_json,
                        is_builtin, package_version, package_manifest_json, agent_kind,
                        invocation_mode, visibility, package_source, package_id, package_hash
                     ) VALUES (
                        ?1, ?2, ?3, 'pi', ?4, ?5, ?6, ?6, ?7, ?8, ?9, 0, ?10, ?11,
                        'expert', 'inline', 'expert_center', 'imported', ?12, ?13
                     )",
                    params![
                        request.expert_id,
                        request.name,
                        request.description,
                        request.system_prompt,
                        request.default_model,
                        now,
                        request.icon,
                        request.category,
                        suggestions_json,
                        request.version,
                        manifest_json,
                        request.package_id,
                        request.package_hash
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute(
                "UPDATE expert_package_versions SET status = 'historical'
                 WHERE expert_id = ?1 AND status = 'active'",
                [&request.expert_id],
            )
            .map_err(|error| error.to_string())?;
        if reactivate {
            transaction
                .execute(
                    "UPDATE expert_package_versions
                     SET status = 'active', activated_at = ?3
                     WHERE expert_id = ?1 AND version = ?2",
                    params![request.expert_id, request.version, now],
                )
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .execute(
                    "INSERT INTO expert_package_versions(
                        id, expert_id, package_id, version, package_hash, package_json,
                        source, status, created_at, activated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'imported', 'active', ?7, ?7)",
                    params![
                        format!("expert-package-version-{}", Uuid::new_v4().simple()),
                        request.expert_id,
                        request.package_id,
                        request.version,
                        request.package_hash,
                        request.package.to_string(),
                        now
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute(
                "INSERT INTO agent_runtime_config(
                    agent_id, scope_type, scope_id, config_key, value_json, source,
                    created_at, updated_at
                 ) VALUES (?1, 'agent', ?1, 'skills.enabled', ?2, 'package', ?3, ?3)
                 ON CONFLICT(agent_id, scope_type, scope_id, config_key) DO UPDATE SET
                    value_json = excluded.value_json, source = 'package', updated_at = excluded.updated_at",
                params![
                    request.expert_id,
                    serde_json::to_string(&request.enabled_skills)
                        .map_err(|error| error.to_string())?,
                    now
                ],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        query_agent_record(&connection, &request.expert_id)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "expert.package_not_found".to_owned())
    }

    pub fn copy_agent(&self, agent_id: &str, name: Option<&str>) -> Result<AgentRecord, String> {
        let source = self
            .get_agent(agent_id)?
            .ok_or_else(|| "expert was not found".to_owned())?;
        if source.runtime_type != "pi" {
            return Err("only local experts can be copied".to_owned());
        }
        self.save_agent(&SaveAgentRequest {
            id: None,
            name: name
                .filter(|value| !value.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{} 副本", source.name)),
            description: source.description,
            icon: source.icon,
            category: source.category,
            system_prompt: source.system_prompt,
            default_model: source.default_model,
            opening_suggestions: source.opening_suggestions,
            package_manifest: source.package_manifest,
        })
    }

    pub fn delete_agent(&self, agent_id: &str) -> Result<bool, String> {
        if agent_id == DEFAULT_AGENT_ID {
            return Err("the default expert cannot be deleted".to_owned());
        }
        self.with_connection(|connection| {
            let builtin = connection
                .query_row(
                    "SELECT is_builtin FROM agents WHERE id = ?1 AND runtime_type = 'pi'",
                    [agent_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(builtin) = builtin else {
                return Ok(false);
            };
            if builtin != 0 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let in_use = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE agent_id = ?1)",
                [agent_id],
                |row| row.get::<_, bool>(0),
            )?;
            if in_use {
                return Err(rusqlite::Error::InvalidQuery);
            }
            Ok(connection.execute("DELETE FROM agents WHERE id = ?1", [agent_id])? > 0)
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                        COALESCE(p.root_path, c.project_root), c.status,
                        c.pinned, c.archived, c.archived_at, c.trashed_at,
                        c.parent_conversation_id, c.forked_from_message_id,
                        COALESCE(c.lineage_root_id, c.id),
                        c.created_at, c.updated_at, c.last_message_at,
                        COALESCE(kep.mode, c.permission_mode, 'ask')
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.conversation_kind = 'primary' AND c.archived = 0 AND c.trashed_at IS NULL
                 ORDER BY c.pinned DESC, COALESCE(c.last_message_at, c.created_at) DESC",
            )?;
            let records = statement.query_map([], map_conversation)?.collect();
            records
        })
    }

    pub fn list_archived_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                        COALESCE(p.root_path, c.project_root), c.status,
                        c.pinned, c.archived, c.archived_at, c.trashed_at,
                        c.parent_conversation_id, c.forked_from_message_id,
                        COALESCE(c.lineage_root_id, c.id),
                        c.created_at, c.updated_at, c.last_message_at,
                        COALESCE(kep.mode, c.permission_mode, 'ask')
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.conversation_kind = 'primary' AND c.archived = 1 AND c.trashed_at IS NULL
                 ORDER BY COALESCE(c.archived_at, c.updated_at) DESC",
            )?;
            let records = statement.query_map([], map_conversation)?.collect();
            records
        })
    }

    pub fn list_trashed_conversations(&self) -> Result<Vec<ConversationSummary>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                        COALESCE(p.root_path, c.project_root), c.status,
                        c.pinned, c.archived, c.archived_at, c.trashed_at,
                        c.parent_conversation_id, c.forked_from_message_id,
                        COALESCE(c.lineage_root_id, c.id),
                        c.created_at, c.updated_at, c.last_message_at,
                        COALESCE(kep.mode, c.permission_mode, 'ask')
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.conversation_kind = 'primary' AND c.trashed_at IS NOT NULL
                 ORDER BY c.trashed_at DESC",
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
            let (input_tokens, output_tokens, cache_read_tokens, cache_write_tokens, total_tokens) = connection.query_row(
                "WITH latest_usage AS (
                    SELECT e.event_json,
                           ROW_NUMBER() OVER (PARTITION BY e.run_id ORDER BY e.seq DESC) AS position
                    FROM run_events e WHERE e.event_type = 'usage.updated'
                 )
                 SELECT
                    COALESCE(SUM(CAST(json_extract(event_json, '$.inputTokens') AS INTEGER)), 0),
                    COALESCE(SUM(CAST(json_extract(event_json, '$.outputTokens') AS INTEGER)), 0),
                    COALESCE(SUM(CAST(COALESCE(json_extract(event_json, '$.cacheReadTokens'), 0) AS INTEGER)), 0),
                    COALESCE(SUM(CAST(COALESCE(json_extract(event_json, '$.cacheWriteTokens'), 0) AS INTEGER)), 0),
                    COALESCE(SUM(MAX(
                        CAST(COALESCE(json_extract(event_json, '$.totalTokens'), 0) AS INTEGER),
                        CAST(COALESCE(json_extract(event_json, '$.inputTokens'), 0) AS INTEGER) +
                        CAST(COALESCE(json_extract(event_json, '$.outputTokens'), 0) AS INTEGER) +
                        CAST(COALESCE(json_extract(event_json, '$.cacheReadTokens'), 0) AS INTEGER) +
                        CAST(COALESCE(json_extract(event_json, '$.cacheWriteTokens'), 0) AS INTEGER)
                    )), 0)
                 FROM latest_usage WHERE position = 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
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
                             CAST(COALESCE(json_extract(event_json, '$.outputTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(event_json, '$.cacheReadTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(event_json, '$.cacheWriteTokens'), 0) AS INTEGER)
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
                 ), raw_usage AS (
                    SELECT e.run_id, e.seq, e.created_at,
                           CAST(COALESCE(json_extract(e.event_json, '$.inputTokens'), 0) AS INTEGER) AS input_tokens,
                           CAST(COALESCE(json_extract(e.event_json, '$.outputTokens'), 0) AS INTEGER) AS output_tokens,
                           CAST(COALESCE(json_extract(e.event_json, '$.cacheReadTokens'), 0) AS INTEGER) AS cache_read_tokens,
                           CAST(COALESCE(json_extract(e.event_json, '$.cacheWriteTokens'), 0) AS INTEGER) AS cache_write_tokens,
                           MAX(
                             CAST(COALESCE(json_extract(e.event_json, '$.totalTokens'), 0) AS INTEGER),
                             CAST(COALESCE(json_extract(e.event_json, '$.inputTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(e.event_json, '$.outputTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(e.event_json, '$.cacheReadTokens'), 0) AS INTEGER) +
                             CAST(COALESCE(json_extract(e.event_json, '$.cacheWriteTokens'), 0) AS INTEGER)
                           ) AS total_tokens
                    FROM run_events e WHERE e.event_type = 'usage.updated'
                 ), usage_with_lag AS (
                    SELECT run_id, seq, created_at, input_tokens, output_tokens,
                           cache_read_tokens, cache_write_tokens, total_tokens,
                           LAG(input_tokens) OVER (PARTITION BY run_id ORDER BY seq) AS previous_input,
                           LAG(output_tokens) OVER (PARTITION BY run_id ORDER BY seq) AS previous_output,
                           LAG(cache_read_tokens) OVER (PARTITION BY run_id ORDER BY seq) AS previous_cache_read,
                           LAG(cache_write_tokens) OVER (PARTITION BY run_id ORDER BY seq) AS previous_cache_write,
                           LAG(total_tokens) OVER (PARTITION BY run_id ORDER BY seq) AS previous_total,
                           ROW_NUMBER() OVER (PARTITION BY run_id ORDER BY seq DESC) AS reverse_position
                    FROM raw_usage
                 ), usage_contract AS (
                    SELECT run_id,
                           MAX(CASE WHEN previous_total IS NOT NULL AND (
                             input_tokens < previous_input OR output_tokens < previous_output OR
                             cache_read_tokens < previous_cache_read OR
                             cache_write_tokens < previous_cache_write OR total_tokens < previous_total
                           ) THEN 1 ELSE 0 END) AS legacy_non_cumulative
                    FROM usage_with_lag GROUP BY run_id
                 ), usage_deltas AS (
                    SELECT usage.run_id, usage.created_at,
                           CASE WHEN contract.legacy_non_cumulative = 0
                             THEN usage.total_tokens - COALESCE(usage.previous_total, 0)
                             WHEN usage.reverse_position = 1 THEN usage.total_tokens
                             ELSE 0
                           END AS delta_tokens
                    FROM usage_with_lag usage
                    JOIN usage_contract contract ON contract.run_id = usage.run_id
                 ), usage_days AS (
                    SELECT date(created_at / 1000, 'unixepoch', 'localtime') AS day,
                           SUM(delta_tokens) AS total_tokens
                    FROM usage_deltas
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
                cache_read_tokens,
                cache_write_tokens,
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
                        c.pinned, c.archived, c.archived_at, c.trashed_at,
                        c.parent_conversation_id, c.forked_from_message_id,
                        COALESCE(c.lineage_root_id, c.id),
                        c.created_at, c.updated_at, c.last_message_at,
                        COALESCE(kep.mode, c.permission_mode, 'ask')
                 FROM conversations c
                 JOIN agents a ON a.id = c.agent_id
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.conversation_kind = 'primary' AND c.archived = 0 AND c.trashed_at IS NULL AND (
                       c.title LIKE ?1 ESCAPE '\\'
                    OR COALESCE(p.root_path, c.project_root, '') LIKE ?1 ESCAPE '\\'
                    OR a.name LIKE ?1 ESCAPE '\\'
                    OR EXISTS(SELECT 1 FROM messages m
                              WHERE m.conversation_id = c.id AND m.content LIKE ?1 ESCAPE '\\')
                    OR c.id IN (SELECT conversation_id FROM conversation_fts
                                WHERE conversation_fts MATCH ?2)
                    OR c.id IN (SELECT conversation_id FROM message_fts
                                WHERE message_fts MATCH ?2)
                 )
                 ORDER BY c.pinned DESC, COALESCE(c.last_message_at, c.created_at) DESC
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

    pub fn delete_project(&self, project_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "UPDATE conversations
                 SET permission_mode = COALESCE(
                         (SELECT permission_mode FROM projects WHERE id = ?1),
                         permission_mode
                     ),
                     project_root = COALESCE(
                         project_root,
                         (SELECT root_path FROM projects WHERE id = ?1)
                     ),
                     project_id = NULL
                 WHERE project_id = ?1",
                [project_id],
            )?;
            let deleted =
                transaction.execute("DELETE FROM projects WHERE id = ?1", [project_id])? > 0;
            transaction.commit()?;
            Ok(deleted)
        })
    }

    pub fn list_conversation_expert_bindings(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationExpertBinding>, String> {
        self.with_connection(|connection| {
            query_conversation(connection, conversation_id)?;
            query_conversation_expert_bindings(connection, conversation_id)
        })
    }

    pub fn current_conversation_expert_binding(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationExpertBinding>, String> {
        self.with_connection(|connection| {
            query_conversation_expert_binding_by_state(connection, conversation_id, "active")
                .optional()
        })
    }

    pub fn bind_conversation_expert(
        &self,
        conversation_id: &str,
        expert_id: &str,
        activation_source: &str,
    ) -> Result<ConversationExpertBinding, ConversationExpertBindingError> {
        let activation_source = activation_source.trim();
        if activation_source.is_empty() {
            return Err(ConversationExpertBindingError::InvalidActivationSource);
        }
        let mut connection = self.connection.lock().map_err(|_| {
            ConversationExpertBindingError::Storage("database lock is poisoned".to_owned())
        })?;
        let transaction = connection.transaction()?;
        let conversation_exists = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            [conversation_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !conversation_exists {
            return Err(ConversationExpertBindingError::ConversationNotFound);
        }
        if let Some(reason) =
            conversation_expert_binding_lock_reason(&transaction, conversation_id)?
        {
            return Err(reason);
        }

        let Some(expert) = query_agent_record(&transaction, expert_id)? else {
            return Err(ConversationExpertBindingError::ExpertNotFound);
        };
        if (
            expert.agent_kind.as_str(),
            expert.invocation_mode.as_str(),
            expert.visibility.as_str(),
        ) != ("expert", "inline", "expert_center")
        {
            return Err(ConversationExpertBindingError::InvalidRole);
        }
        if expert.runtime_type == "yuxi" && !expert.available {
            return Err(ConversationExpertBindingError::RemoteUnavailable);
        }

        let now = now_ms();
        transaction.execute(
            "UPDATE conversation_expert_bindings
             SET state = 'replaced', deactivated_at = ?2
             WHERE conversation_id = ?1 AND state = 'active'",
            params![conversation_id, now],
        )?;
        let id = Uuid::new_v4().to_string();
        let package_snapshot = agent_package_snapshot(&expert);
        let package_hash = package_snapshot_hash(&package_snapshot);
        let display_snapshot_json = json!({
            "id": expert_id,
            "name": expert.name,
            "description": expert.description,
            "icon": expert.icon,
            "category": expert.category,
            "agentKind": expert.agent_kind,
            "invocationMode": expert.invocation_mode,
            "visibility": expert.visibility,
            "packageVersion": expert.package_version,
        });
        transaction.execute(
            "INSERT INTO conversation_expert_bindings(
                id, conversation_id, expert_id, state, activation_source, expert_version,
                package_hash, package_snapshot_json, display_snapshot_json, activated_at,
                deactivated_at
             ) VALUES (?1, ?2, ?3, 'active', ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
            params![
                id,
                conversation_id,
                expert_id,
                activation_source,
                expert.package_version,
                package_hash,
                package_snapshot.to_string(),
                display_snapshot_json.to_string(),
                now
            ],
        )?;
        let binding = query_conversation_expert_binding(&transaction, &id)?;
        transaction.commit()?;
        Ok(binding)
    }

    pub fn remove_conversation_expert(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationExpertBinding>, ConversationExpertBindingError> {
        let mut connection = self.connection.lock().map_err(|_| {
            ConversationExpertBindingError::Storage("database lock is poisoned".to_owned())
        })?;
        let transaction = connection.transaction()?;
        let conversation_exists = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM conversations WHERE id = ?1)",
            [conversation_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !conversation_exists {
            return Err(ConversationExpertBindingError::ConversationNotFound);
        }
        if let Some(reason) =
            conversation_expert_binding_lock_reason(&transaction, conversation_id)?
        {
            return Err(reason);
        }
        let active_id = transaction
            .query_row(
                "SELECT id FROM conversation_expert_bindings
                 WHERE conversation_id = ?1 AND state = 'active'",
                [conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(active_id) = active_id else {
            transaction.commit()?;
            return Ok(None);
        };
        transaction.execute(
            "UPDATE conversation_expert_bindings
             SET state = 'removed', deactivated_at = ?2 WHERE id = ?1",
            params![active_id, now_ms()],
        )?;
        let binding = query_conversation_expert_binding(&transaction, &active_id)?;
        transaction.commit()?;
        Ok(Some(binding))
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
                .query_row(
                    "SELECT 1 FROM agents
                     WHERE id = ?1 AND agent_kind = 'assistant'
                       AND invocation_mode = 'primary' AND visibility = 'chat_selector'",
                    [agent_id],
                    |_| Ok(()),
                )
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
                    id, agent_id, title, project_id, project_root, permission_mode, status,
                    lineage_root_id, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'active', ?1, ?7, ?7)",
                params![
                    id,
                    agent_id,
                    title,
                    project_id,
                    project_root,
                    permission_mode,
                    now
                ],
            )?;
            query_conversation(connection, &id)
        })
    }

    pub fn load_conversation(&self, id: &str) -> Result<ConversationDetail, String> {
        let mut detail = self.with_connection(|connection| {
            let conversation = query_conversation(connection, id)?;
            let messages = query_message_page(connection, id, None, INITIAL_HISTORY_MESSAGES)?;
            let oldest_ordinal = messages.first().map(|message| message.ordinal).unwrap_or(0);
            let runtime_events = query_runtime_events_window(connection, id, oldest_ordinal, None)?;
            let tool_calls = query_tool_calls_window(connection, id, oldest_ordinal, None)?;
            let approvals = query_approvals_window(connection, id, oldest_ordinal, None)?;
            let attachments = query_attachments_window(connection, id, oldest_ordinal, None)?;
            let artifacts = query_artifacts_window(connection, id, oldest_ordinal, None)?;
            let knowledge_bindings = query_knowledge_bindings(connection, id)?;
            let expert_bindings = query_conversation_expert_bindings(connection, id)?;
            let last_run = query_last_run(connection, id)?;
            let run_ids = messages
                .iter()
                .filter_map(|message| message.run_id.clone())
                .collect::<Vec<_>>();
            let runs = query_runs_by_ids(connection, &run_ids)?;
            let has_earlier_messages = oldest_ordinal > 0 && connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE conversation_id = ?1 AND ordinal < ?2)",
                params![id, oldest_ordinal],
                |row| row.get::<_, bool>(0),
            )?;
            Ok(ConversationDetail {
                conversation,
                expert_bindings,
                messages,
                runtime_events,
                tool_calls,
                approvals,
                attachments,
                artifacts,
                knowledge_bindings,
                last_run,
                runs,
                has_earlier_messages,
                goals: Vec::new(),
                tasks: Vec::new(),
                evidence: Vec::new(),
                plan_revisions: Vec::new(),
                review_findings: Vec::new(),
                acceptances: Vec::new(),
                child_runs: Vec::new(),
                expert_workflow: None,
                expert_team: None,
            })
        })?;
        let (goals, tasks, evidence) = self
            .load_work_graph_snapshot(id)
            .map_err(|error| error.to_string())?;
        detail.goals = goals;
        detail.tasks = tasks;
        detail.evidence = evidence;
        let (plan_revisions, review_findings, acceptances) = self.load_a1_snapshot(id)?;
        detail.plan_revisions = plan_revisions;
        detail.review_findings = review_findings;
        detail.acceptances = acceptances;
        detail.child_runs = self.child_runs_for_conversation(id)?;
        detail.expert_workflow = self.active_expert_workflow(id)?;
        detail.expert_team = self.latest_expert_team(id)?;
        detail
            .approvals
            .extend(self.child_run_approvals_for_conversation(id)?);
        Ok(detail)
    }

    #[allow(clippy::type_complexity)]
    /// Resolve a `fox-result://<runId>/<toolCallId>` reference to one byte range
    /// of that settled tool call's stored result.
    ///
    /// Authorization is explicit and mandatory: the caller passes the
    /// conversation it is already allowed to read, and the tool call's own
    /// `conversation_id` must equal it, so a reference minted inside one
    /// conversation can never be used to read another conversation's tool
    /// output. This reads bytes Host already stored — it never executes a tool,
    /// never re-derives a result, and never replays a write.
    ///
    /// The stored copy is capped at `MAX_STORED_TOOL_RESULT_BYTES`. Above that
    /// the row holds a Fox preview, and the response reports `truncated: true`
    /// with the true `original_bytes` instead of implying the omitted bytes are
    /// obtainable from this reader.
    /// `tool_result_range` and then a banner saying those bytes are all Host
    /// kept, so the model can ask for them instead of guessing them.
    pub fn tool_result_range(
        &self,
        reference: &str,
        authorized_conversation_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<ToolResultRange, String> {
        let (run_id, tool_call_id) =
            crate::kernel_compaction::parse_tool_result_ref(reference)
                .ok_or_else(|| "invalid tool result reference".to_owned())?;
        let authorized = authorized_conversation_id.trim();
        if authorized.is_empty() {
            return Err("missing authorized conversation".to_owned());
        }
        let record = self.with_connection(|connection| {
            Ok(query_tool_call(connection, &run_id, &tool_call_id).optional()?)
        })?;
        let record = record.ok_or_else(|| "no stored result for this reference".to_owned())?;
        if record.conversation_id != authorized {
            return Err("tool result is outside the authorized conversation".to_owned());
        }
        // When the full result was spilled to a blob, serve ranges from it: the
        // inline preview is only the model-facing copy. Blob identity was
        // verified at write time (content-addressed sha256), so a present blob
        // is by construction the complete original.
        let blob_body: Option<Value> = self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT b.body_json FROM tool_calls t
                     JOIN tool_call_result_blobs b ON b.sha256 = t.result_blob_sha256
                     WHERE t.run_id = ?1 AND t.runtime_tool_call_id = ?2
                       AND t.result_blob_sha256 IS NOT NULL",
                    params![record.run_id, record.runtime_tool_call_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .map(|body| parse_json(&body)))
        })?;
        let serving = blob_body.as_ref().or(record.result.as_ref());
        let (text, truncated, original_bytes) = stored_result_text(serving);
        let (start, end, next_offset) = utf8_byte_range(&text, offset, limit)?;
        Ok(ToolResultRange {
            run_id: record.run_id,
            tool_call_id: record.runtime_tool_call_id,
            conversation_id: record.conversation_id,
            tool_name: record.tool_name,
            status: record.status,
            truncated,
            // True only when every byte of the original result is reachable from
            // here — either the inline row was small enough to keep everything,
            // or the full copy was spilled to the content-addressed blob table.
            // An inline preview without a blob is still not retrievable.
            retrievable: !truncated || blob_body.is_some(),
            original_bytes,
            offset: start,
            returned_bytes: end.saturating_sub(start),
            next_offset,
            content: text.get(start..end).unwrap_or_default().to_owned(),
        })
    }

    /// Derived exclusively from the persisted Host row and its range reader.
    /// Tool-supplied fields never authorize model-view omission.
    ///
    /// A-REQ-006 (EX-v1): this is a fact about *storage*, not about success. A
    /// call that settled as `failed` still had its complete result spilled to the
    /// content-addressed blob store by `complete_host_tool_call`, and the model
    /// has to be able to page through those diagnostics exactly like a
    /// successful result - otherwise a bounded view of a multi-megabyte failure
    /// would promise a retrieval that only completed rows were allowed to have.
    /// So both terminal states are in scope, while anything still in flight
    /// (`running`/`pending`), anything with no stored result, and anything the
    /// reader can only serve as a preview stay `unknown` - which no caller may
    /// read as permission to omit. Authorization still comes from the row's own
    /// conversation and from the same real range read, never from the status.
    pub(crate) fn tool_result_storage(
        &self, run_id: &str, tool_call_id: &str,
    ) -> Result<crate::kernel_compaction::ToolResultStorage, String> {
        let (fact, error) = self.with_connection(|connection| {
            Ok(match tool_result_storage_for(connection, run_id, tool_call_id) {
                Ok(fact) => (Some(fact), None),
                Err(error) => (None, Some(error)),
            })
        })?;
        match (fact, error) {
            (Some(fact), _) => Ok(fact),
            (None, Some(error)) => Err(error),
            (None, None) => Err("tool result storage fact is missing".to_owned()),
        }
    }

    pub fn load_conversation_trace_records(
        &self,
        conversation_id: &str,
    ) -> Result<(Vec<RunRecord>, Vec<RunEventRecord>, Vec<ToolCallRecord>), String> {
        self.with_connection(|connection| {
            query_conversation(connection, conversation_id)?;
            let runs = {
                let mut statement = connection.prepare(
                    "SELECT id, conversation_id, runtime_session_id, status, model, started_at,
                            finished_at, error_code, error_message, last_seq, trace_id, root_span_id
                     FROM runs WHERE conversation_id = ?1 ORDER BY created_at ASC, rowid ASC",
                )?;
                let records = statement
                    .query_map([conversation_id], |row| {
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
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                records
            };
            let runtime_events = {
                let mut statement = connection.prepare(
                    "SELECT e.run_id, e.seq, e.event_type, e.event_json, e.created_at,
                            e.trace_id, e.span_id
                     FROM run_events e JOIN runs r ON r.id = e.run_id
                     WHERE r.conversation_id = ?1
                     ORDER BY r.created_at ASC, e.seq ASC",
                )?;
                let records = statement
                    .query_map([conversation_id], |row| {
                        let event_json: String = row.get(3)?;
                        Ok(RunEventRecord {
                            run_id: row.get(0)?,
                            seq: row.get(1)?,
                            event_type: row.get(2)?,
                            event: parse_json(&event_json),
                            created_at: row.get(4)?,
                            trace_id: row.get(5)?,
                            span_id: row.get(6)?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                records
            };
            let tool_calls = {
                let mut statement = connection.prepare(
                    "SELECT id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                            status, result_json, error_message, execution_location, requires_approval,
                            started_at, completed_at, updated_at, trace_id, span_id
                     FROM tool_calls WHERE conversation_id = ?1 ORDER BY started_at ASC, rowid ASC",
                )?;
                let records = statement
                    .query_map([conversation_id], map_tool_call)?
                    .collect::<Result<Vec<_>, _>>()?;
                records
            };
            Ok((runs, runtime_events, tool_calls))
        })
    }

    pub fn conversation_knowledge_bindings(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<KnowledgeBindingRecord>, String> {
        self.with_connection(|connection| query_knowledge_bindings(connection, conversation_id))
    }

    pub fn conversation_knowledge_reference_bindings(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<KnowledgeReferenceBindingRecord>, String> {
        self.with_connection(|connection| {
            query_knowledge_reference_bindings(connection, conversation_id)
        })
    }

    pub fn conversation_knowledge_references(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<KnowledgeReference>, String> {
        self.conversation_knowledge_reference_bindings(conversation_id)
            .map(|bindings| {
                bindings
                    .into_iter()
                    .map(|binding| binding.reference)
                    .collect()
            })
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
                let is_subagent = agent.backend_id == "SubAgentBackend";
                let (agent_kind, invocation_mode, visibility) = if is_subagent {
                    ("worker", "child", "hidden")
                } else if agent.is_default {
                    ("assistant", "primary", "chat_selector")
                } else {
                    ("expert", "inline", "expert_center")
                };
                let system_prompt = serde_json::to_string(&json!({
                    "remoteAgentId": agent.slug,
                    "backendId": agent.backend_id,
                    "icon": agent.icon,
                    "capabilities": agent.capabilities,
                    "resources": agent.resources,
                    "configurableItems": agent.configurable_items,
                    "isSubagent": is_subagent,
                    "isDefault": agent.is_default,
                    "chatVisible": agent.is_default,
                    "remoteAvailable": agent.available,
                }))
                .unwrap_or_else(|_| "{}".to_owned());
                transaction.execute(
                    "INSERT INTO agents(id, name, description, runtime_type, system_prompt,
                                        default_model, created_at, updated_at, agent_kind,
                                        invocation_mode, visibility, package_source)
                     VALUES (?1, ?2, ?3, 'yuxi', ?4, ?5, ?6, ?6, ?7, ?8, ?9, 'remote')
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name,
                        description = excluded.description, system_prompt = excluded.system_prompt,
                        default_model = excluded.default_model, updated_at = excluded.updated_at,
                        agent_kind = excluded.agent_kind,
                        invocation_mode = excluded.invocation_mode,
                        visibility = excluded.visibility,
                        package_source = 'remote'",
                    params![
                        id,
                        agent.name,
                        agent.description,
                        system_prompt,
                        agent.default_model,
                        now,
                        agent_kind,
                        invocation_mode,
                        visibility
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

    pub fn mark_yuxi_agents_unavailable(&self) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut statement = transaction
                .prepare("SELECT id, system_prompt FROM agents WHERE runtime_type = 'yuxi'")?;
            let agents = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);

            for (id, system_prompt) in agents {
                let mut metadata = parse_json(&system_prompt);
                if !metadata.is_object() {
                    metadata = json!({});
                }
                if let Some(object) = metadata.as_object_mut() {
                    object.insert("remoteAvailable".to_owned(), Value::Bool(false));
                }
                transaction.execute(
                    "UPDATE agents SET system_prompt = ?2, updated_at = ?3 WHERE id = ?1",
                    params![id, metadata.to_string(), now],
                )?;
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

    pub fn agent_kind(&self, agent_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT agent_kind FROM agents WHERE id = ?1",
                    [agent_id],
                    |row| row.get(0),
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

    pub fn set_conversation_pinned(
        &self,
        id: &str,
        pinned: bool,
    ) -> Result<Option<ConversationSummary>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations SET pinned = ?2, updated_at = ?3
                 WHERE id = ?1 AND archived = 0 AND trashed_at IS NULL",
                params![id, pinned, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn archive_conversation(&self, id: &str) -> Result<Option<ConversationSummary>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations
                 SET archived = 1, archived_at = ?2, pinned = 0, updated_at = ?2
                 WHERE id = ?1 AND trashed_at IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM runs
                       WHERE conversation_id = ?1
                         AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
                   )",
                params![id, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn unarchive_conversation(&self, id: &str) -> Result<Option<ConversationSummary>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations
                 SET archived = 0, archived_at = NULL, updated_at = ?2
                 WHERE id = ?1 AND trashed_at IS NULL",
                params![id, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn trash_conversation(&self, id: &str) -> Result<Option<ConversationSummary>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations
                 SET trashed_at = ?2, archived = 0, archived_at = NULL,
                     pinned = 0, updated_at = ?2
                 WHERE id = ?1 AND trashed_at IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM runs
                       WHERE conversation_id = ?1
                         AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
                   )",
                params![id, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn restore_trashed_conversation(
        &self,
        id: &str,
    ) -> Result<Option<ConversationSummary>, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations
                 SET trashed_at = NULL, archived = 0, archived_at = NULL, updated_at = ?2
                 WHERE id = ?1 AND trashed_at IS NOT NULL",
                params![id, now],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            query_conversation(connection, id).optional()
        })
    }

    pub fn fork_conversation(
        &self,
        source_id: &str,
        message_id: &str,
        title: Option<&str>,
    ) -> Result<Option<ConversationSummary>, String> {
        let fork_id = Uuid::new_v4().to_string();
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let source = transaction
                .query_row(
                    "SELECT c.agent_id, c.title, c.project_id, c.project_root,
                            COALESCE(c.lineage_root_id, c.id), c.permission_mode
                     FROM conversations c
                     WHERE c.id = ?1 AND c.trashed_at IS NULL",
                    [source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                agent_id,
                source_title,
                project_id,
                project_root,
                lineage_root_id,
                permission_mode,
            )) = source
            else {
                return Ok(None);
            };
            let active_run = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM runs
                    WHERE conversation_id = ?1
                      AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
                 )",
                [source_id],
                |row| row.get::<_, bool>(0),
            )?;
            if active_run {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let target_ordinal = transaction
                .query_row(
                    "SELECT ordinal FROM messages WHERE id = ?1 AND conversation_id = ?2",
                    params![message_id, source_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(target_ordinal) = target_ordinal else {
                return Ok(None);
            };
            let fork_title = title
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("{source_title} · 分支"));
            let last_message_at = transaction.query_row(
                "SELECT MAX(created_at) FROM messages
                 WHERE conversation_id = ?1 AND ordinal <= ?2",
                params![source_id, target_ordinal],
                |row| row.get::<_, Option<i64>>(0),
            )?;
            transaction.execute(
                "INSERT INTO conversations(
                    id, agent_id, title, project_id, project_root, permission_mode,
                    status, pinned, archived,
                    parent_conversation_id, forked_from_message_id, lineage_root_id,
                    created_at, updated_at, last_message_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, 'active', 0, 0, ?7, ?8, ?9, ?10, ?10, ?11
                 )",
                params![
                    fork_id,
                    agent_id,
                    fork_title,
                    project_id,
                    project_root,
                    permission_mode,
                    source_id,
                    message_id,
                    lineage_root_id,
                    now,
                    last_message_at,
                ],
            )?;

            let messages = {
                let mut statement = transaction.prepare(
                    "SELECT role, kind, content, status, ordinal, created_at, updated_at
                     FROM messages
                     WHERE conversation_id = ?1 AND ordinal <= ?2
                     ORDER BY ordinal ASC",
                )?;
                let records = statement
                    .query_map(params![source_id, target_ordinal], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                records
            };
            for (role, kind, content, status, ordinal, created_at, updated_at) in messages {
                transaction.execute(
                    "INSERT INTO messages(
                        id, conversation_id, run_id, role, kind, content, status, ordinal,
                        runtime_message_id, metadata_json, created_at, updated_at
                     ) VALUES (?1, ?2, NULL, ?3, ?4, ?5, ?6, ?7, NULL, '{}', ?8, ?9)",
                    params![
                        Uuid::new_v4().to_string(),
                        fork_id,
                        role,
                        kind,
                        content,
                        status,
                        ordinal,
                        created_at,
                        updated_at,
                    ],
                )?;
            }

            let knowledge_bindings = {
                let mut statement = transaction.prepare(
                    "SELECT source, provider_key, connection_id, knowledge_base_id,
                            knowledge_base_name, enabled
                     FROM knowledge_bindings_v2
                     WHERE conversation_id = ?1 AND enabled = 1",
                )?;
                let records = statement
                    .query_map([source_id], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<String>>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, bool>(5)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                records
            };
            for (source, provider_key, connection_id, knowledge_base_id, name, enabled) in
                knowledge_bindings
            {
                transaction.execute(
                    "INSERT INTO knowledge_bindings_v2(
                        id, conversation_id, source, provider_key, connection_id,
                        knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
                    params![
                        Uuid::new_v4().to_string(),
                        fork_id,
                        source,
                        provider_key,
                        connection_id,
                        knowledge_base_id,
                        name,
                        enabled,
                        now,
                    ],
                )?;
            }

            let active_expert = transaction
                .query_row(
                    "SELECT expert_id, expert_version, package_hash, package_snapshot_json,
                            display_snapshot_json
                     FROM conversation_expert_bindings
                     WHERE conversation_id = ?1 AND state = 'active'
                     ORDER BY activated_at DESC, rowid DESC LIMIT 1",
                    [source_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()?;
            if let Some((expert_id, version, hash, package_snapshot, display_snapshot)) =
                active_expert
            {
                transaction.execute(
                    "INSERT INTO conversation_expert_bindings(
                        id, conversation_id, expert_id, state, activation_source,
                        expert_version, package_hash, package_snapshot_json,
                        display_snapshot_json, activated_at, deactivated_at
                     ) VALUES (?1, ?2, ?3, 'active', 'conversation_fork', ?4, ?5, ?6, ?7, ?8, NULL)",
                    params![
                        Uuid::new_v4().to_string(),
                        fork_id,
                        expert_id,
                        version,
                        hash,
                        package_snapshot,
                        display_snapshot,
                        now,
                    ],
                )?;
            }
            let fork = query_conversation(&transaction, &fork_id)?;
            transaction.commit()?;
            Ok(Some(fork))
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

    /// Load one artifact only when both the artifact id and conversation id
    /// match.  Artifact paths are untrusted data and must never be looked up
    /// by id alone; the host gateway performs the filesystem checks after this
    /// scoped query returns.
    pub fn artifact_for_conversation(
        &self,
        conversation_id: &str,
        artifact_id: &str,
    ) -> Result<Option<ArtifactRecord>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id, conversation_id, run_id, display_name, artifact_type,
                            artifact_class, artifact_origin, storage_path,
                            media_type, byte_size, sha256, status, created_at, updated_at
                     FROM artifacts
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![artifact_id, conversation_id],
                    |row| {
                        Ok(ArtifactRecord {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            run_id: row.get(2)?,
                            display_name: row.get(3)?,
                            artifact_type: row.get(4)?,
                            artifact_class: row.get(5)?,
                            artifact_origin: row.get(6)?,
                            storage_path: row.get(7)?,
                            media_type: row.get(8)?,
                            byte_size: row.get(9)?,
                            sha256: row.get(10)?,
                            status: row.get(11)?,
                            created_at: row.get(12)?,
                            updated_at: row.get(13)?,
                        })
                    },
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
                "DELETE FROM knowledge_bindings_v2 WHERE conversation_id = ?1",
                [conversation_id],
            )?;
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

    pub fn set_knowledge_reference_bindings(
        &self,
        conversation_id: &str,
        bindings: &[KnowledgeReferenceBindingInput],
    ) -> Result<Vec<KnowledgeReferenceBindingRecord>, String> {
        for binding in bindings {
            binding
                .reference
                .validate()
                .map_err(|error| format!("invalid knowledge reference: {error}"))?;
        }

        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            transaction.execute(
                "DELETE FROM knowledge_bindings_v2 WHERE conversation_id = ?1",
                [conversation_id],
            )?;
            transaction.execute(
                "DELETE FROM knowledge_bindings WHERE conversation_id = ?1",
                [conversation_id],
            )?;
            for binding in bindings {
                let reference = &binding.reference;
                transaction.execute(
                    "INSERT INTO knowledge_bindings_v2(
                        id, conversation_id, source, provider_key, connection_id,
                        knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?8)",
                    params![
                        knowledge_binding_v2_id(conversation_id, reference),
                        conversation_id,
                        reference.source,
                        reference.provider_key,
                        reference.connection_id,
                        reference.id,
                        binding.name,
                        now,
                    ],
                )?;
            }
            let records = query_knowledge_reference_bindings(&transaction, conversation_id)?;
            transaction.commit()?;
            Ok(records)
        })
    }

    pub fn set_knowledge_references(
        &self,
        conversation_id: &str,
        references: &[(KnowledgeReference, String)],
    ) -> Result<Vec<KnowledgeReferenceBindingRecord>, String> {
        let bindings = references
            .iter()
            .map(|(reference, name)| KnowledgeReferenceBindingInput {
                reference: reference.clone(),
                name: name.clone(),
            })
            .collect::<Vec<_>>();
        self.set_knowledge_reference_bindings(conversation_id, &bindings)
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

    pub fn enabled_agent_skill_count(&self, skill_id: &str) -> Result<usize, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT value_json FROM agent_runtime_config
                 WHERE scope_type = 'agent' AND config_key = 'skills.enabled'",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            let mut count = 0usize;
            for row in rows {
                let value = row?;
                let enabled = serde_json::from_str::<Vec<String>>(&value).unwrap_or_default();
                if enabled.iter().any(|item| item == skill_id) {
                    count += 1;
                }
            }
            Ok(count)
        })
    }

    pub fn list_plugin_catalog_entries(&self) -> Result<Vec<PluginCatalogEntryRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT plugin_id, version, kind, origin, manifest_json, package_hash,
                        signature, fetched_at
                 FROM plugin_catalog_cache
                 ORDER BY fetched_at DESC, plugin_id COLLATE NOCASE, version DESC",
            )?;
            let rows = statement.query_map([], |row| {
                let kind = plugin_kind_from_db(row.get::<_, String>(2)?)?;
                let origin = plugin_origin_from_db(row.get::<_, String>(3)?)?;
                let manifest_json: String = row.get(4)?;
                Ok(PluginCatalogEntryRecord {
                    plugin_id: row.get(0)?,
                    version: row.get(1)?,
                    kind,
                    origin,
                    manifest: serde_json::from_str(&manifest_json).unwrap_or_else(|_| json!({})),
                    package_hash: row.get(5)?,
                    signature: row.get(6)?,
                    fetched_at: row.get(7)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_plugin_installations(&self) -> Result<Vec<PluginInstallationRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT plugin_id, installed_version, origin, install_status, install_path,
                        package_hash, installed_at, updated_at, last_error_code,
                        last_error_message
                 FROM plugin_installations
                 ORDER BY updated_at DESC, plugin_id COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], |row| {
                let origin = plugin_origin_from_db(row.get::<_, String>(2)?)?;
                let install_status = plugin_install_status_from_db(row.get::<_, String>(3)?)?;
                Ok(PluginInstallationRecord {
                    plugin_id: row.get(0)?,
                    installed_version: row.get(1)?,
                    origin,
                    install_status,
                    install_path: row.get(4)?,
                    package_hash: row.get(5)?,
                    installed_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    last_error_code: row.get(8)?,
                    last_error_message: row.get(9)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn list_mcp_plugin_sources(&self) -> Result<Vec<McpPluginSourceRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, catalog_plugin_id, origin
                 FROM mcp_servers ORDER BY id COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(McpPluginSourceRecord {
                    server_id: row.get(0)?,
                    catalog_plugin_id: row.get(1)?,
                    origin: plugin_origin_from_db(row.get::<_, String>(2)?)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
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
                "SELECT id, name, command, args_json, transport, endpoint_url, definition,
                        enabled, status, last_error, last_checked_at, last_latency_ms,
                        tool_count, consecutive_failures, created_at, updated_at
                 FROM mcp_servers ORDER BY name COLLATE NOCASE",
            )?;
            let rows = statement.query_map([], |row| {
                let args_json: String = row.get(3)?;
                Ok(super::McpServerRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    command: row.get(2)?,
                    args: serde_json::from_str(&args_json).unwrap_or_default(),
                    transport: row.get(4)?,
                    endpoint_url: row.get(5)?,
                    definition: row.get(6)?,
                    enabled: row.get::<_, i64>(7)? != 0,
                    status: row.get(8)?,
                    credential_configured: false,
                    last_error: row.get(9)?,
                    last_checked_at: row.get(10)?,
                    last_latency_ms: row.get(11)?,
                    tool_count: row.get(12)?,
                    consecutive_failures: row.get(13)?,
                    created_at: row.get(14)?,
                    updated_at: row.get(15)?,
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
        transport: &str,
        endpoint_url: Option<&str>,
        definition: Option<&str>,
    ) -> Result<super::McpServerRecord, String> {
        if id == crate::office::SERVER_ID {
            return Err("Office 文档由 Fox 管理，请使用启用/停用操作。".to_owned());
        }
        let now = now_ms();
        let args_json = serde_json::to_string(args).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO mcp_servers(
                    id, name, command, args_json, transport, endpoint_url, definition,
                    enabled, status, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 'unknown', ?8, ?8)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, command = excluded.command,
                    args_json = excluded.args_json, transport = excluded.transport,
                    endpoint_url = excluded.endpoint_url, definition = excluded.definition,
                    enabled = 1, status = 'unknown', last_error = NULL,
                    last_latency_ms = NULL, tool_count = NULL, consecutive_failures = 0,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    name,
                    command,
                    args_json,
                    transport,
                    endpoint_url,
                    definition,
                    now
                ],
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
        self.record_mcp_health(id, status, error, None, None)
    }

    pub fn record_mcp_health(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        latency_ms: Option<i64>,
        tool_count: Option<i64>,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE mcp_servers SET status = ?2, last_error = ?3,
                         last_checked_at = ?4, last_latency_ms = ?5,
                         tool_count = COALESCE(?6, tool_count),
                         consecutive_failures = CASE WHEN ?2 = 'connected' THEN 0
                             ELSE consecutive_failures + 1 END,
                         updated_at = ?4 WHERE id = ?1",
                params![id, status, error, now, latency_ms, tool_count],
            )?;
            Ok(())
        })
    }

    pub fn list_lifecycle_hooks(&self) -> Result<Vec<super::LifecycleHookRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, name, event, matcher, action, reason, enabled, priority,
                        created_at, updated_at
                 FROM lifecycle_hooks ORDER BY priority, name COLLATE NOCASE, id",
            )?;
            let rows = statement.query_map([], |row| {
                Ok(super::LifecycleHookRecord {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    event: row.get(2)?,
                    matcher: row.get(3)?,
                    action: row.get(4)?,
                    reason: row.get(5)?,
                    enabled: row.get::<_, i64>(6)? != 0,
                    priority: row.get(7)?,
                    created_at: row.get(8)?,
                    updated_at: row.get(9)?,
                })
            })?;
            rows.collect::<Result<Vec<_>, _>>()
        })
    }

    pub fn save_lifecycle_hook(
        &self,
        id: &str,
        name: &str,
        event: &str,
        matcher: &str,
        action: &str,
        reason: &str,
        enabled: bool,
        priority: i64,
    ) -> Result<super::LifecycleHookRecord, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO lifecycle_hooks(
                    id, name, event, matcher, action, reason, enabled, priority, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(id) DO UPDATE SET name = excluded.name, event = excluded.event,
                    matcher = excluded.matcher, action = excluded.action, reason = excluded.reason,
                    enabled = excluded.enabled, priority = excluded.priority,
                    updated_at = excluded.updated_at",
                params![id, name, event, matcher, action, reason, enabled as i64, priority, now],
            )?;
            connection.query_row(
                "SELECT id, name, event, matcher, action, reason, enabled, priority,
                        created_at, updated_at FROM lifecycle_hooks WHERE id = ?1",
                [id],
                |row| {
                    Ok(super::LifecycleHookRecord {
                        id: row.get(0)?, name: row.get(1)?, event: row.get(2)?,
                        matcher: row.get(3)?, action: row.get(4)?, reason: row.get(5)?,
                        enabled: row.get::<_, i64>(6)? != 0, priority: row.get(7)?,
                        created_at: row.get(8)?, updated_at: row.get(9)?,
                    })
                },
            )
        })
    }

    pub fn set_lifecycle_hook_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<Option<super::LifecycleHookRecord>, String> {
        let now = now_ms();
        let changed = self.with_connection(|connection| {
            connection.execute(
                "UPDATE lifecycle_hooks SET enabled = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, enabled as i64, now],
            )
        })?;
        if changed == 0 {
            return Ok(None);
        }
        Ok(self
            .list_lifecycle_hooks()?
            .into_iter()
            .find(|item| item.id == id))
    }

    pub fn delete_lifecycle_hook(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM lifecycle_hooks WHERE id = ?1", [id])? > 0)
        })
    }

    pub fn record_lifecycle_hook_execution(
        &self,
        hook_id: &str,
        run_id: Option<&str>,
        tool_call_id: Option<&str>,
        event: &str,
        tool_name: Option<&str>,
        action: &str,
        outcome: &str,
        details: &Value,
    ) -> Result<(), String> {
        let now = now_ms();
        let details_json = serde_json::to_string(details).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "INSERT INTO lifecycle_hook_executions(
                    id, hook_id, run_id, tool_call_id, event, tool_name, action,
                    outcome, details_json, created_at
                 )
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10
                 WHERE NOT EXISTS (
                    SELECT 1 FROM lifecycle_hook_executions
                    WHERE hook_id = ?2 AND run_id IS ?3 AND tool_call_id IS ?4
                      AND event = ?5 AND action = ?7 AND outcome = ?8
                 )",
                params![
                    Uuid::new_v4().to_string(),
                    hook_id,
                    run_id,
                    tool_call_id,
                    event,
                    tool_name,
                    action,
                    outcome,
                    details_json,
                    now
                ],
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
        if id == crate::office::SERVER_ID {
            return Err("Office 文档为随 Fox 安装的连接器，可以停用。".to_owned());
        }
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
                            COALESCE(kep.mode, c.permission_mode, 'ask')
                     FROM conversations c
                     LEFT JOIN projects p ON p.id = c.project_id
                     LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                     WHERE c.id = ?1
                       AND COALESCE(p.root_path, c.project_root) IS NOT NULL",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
    }

    pub fn conversation_permission_mode(&self, id: &str) -> Result<String, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT COALESCE(kep.mode, c.permission_mode, 'ask')
                 FROM conversations c
                 LEFT JOIN projects p ON p.id = c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
                 WHERE c.id = ?1",
                [id],
                |row| row.get(0),
            )
        })
    }

    pub fn update_conversation_permission_mode(
        &self,
        id: &str,
        permission_mode: &str,
    ) -> Result<Option<ConversationSummary>, String> {
        if !matches!(permission_mode, "read_only" | "ask" | "allow") {
            return Err("invalid conversation permission mode".to_owned());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let updated = connection.execute(
                "UPDATE conversations
                 SET permission_mode = ?2, updated_at = ?3
                 WHERE id = ?1 AND project_id IS NULL",
                params![id, permission_mode, now],
            )?;
            if updated == 0 {
                return query_conversation(connection, id).optional();
            }
            query_conversation(connection, id).optional()
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

    pub(crate) fn get_runtime_tool_call(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
    ) -> Result<Option<super::ToolCallRecord>, String> {
        self.with_connection(|connection| {
            Ok(query_tool_call(connection, run_id, runtime_tool_call_id).optional()?)
        })
    }

    pub(crate) fn list_runtime_tool_calls_for_run(
        &self,
        run_id: &str,
    ) -> Result<Vec<super::ToolCallRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                        status, result_json, error_message, execution_location, requires_approval,
                        started_at, completed_at, updated_at, trace_id, span_id
                 FROM tool_calls WHERE run_id = ?1 ORDER BY started_at ASC, rowid ASC",
            )?;
            let records = statement
                .query_map([run_id], map_tool_call)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(records)
        })
    }

    #[cfg(test)]
    pub(crate) fn create_host_tool_call(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        tool_name: &str,
        input: &Value,
        status: &str,
        requires_approval: bool,
    ) -> Result<super::ToolCallRecord, String> {
        self.create_host_tool_call_once(
            run_id,
            runtime_tool_call_id,
            tool_name,
            input,
            status,
            requires_approval,
        )
        .map(|(record, _)| record)
    }

    pub(crate) fn create_host_tool_call_once(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        tool_name: &str,
        input: &Value,
        status: &str,
        requires_approval: bool,
    ) -> Result<(super::ToolCallRecord, super::HostToolCallDisposition), String> {
        if !matches!(status, "pending" | "running") {
            return Err("a Host ToolCall must start as pending or running".to_owned());
        }
        if requires_approval != (status == "pending") {
            return Err(
                "a Host ToolCall requiring approval must start pending; every other Host ToolCall must start running"
                    .to_owned(),
            );
        }
        let now = now_ms();
        let input_json = serde_json::to_string(input).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_legacy_run_writer(&transaction, run_id)?;
            let (conversation_id, run_status): (String, String) = transaction.query_row(
                "SELECT conversation_id, status FROM runs WHERE id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let existing = query_tool_call(&transaction, run_id, runtime_tool_call_id).optional()?;
            let inserted = if existing.is_none() {
                if run_status != "running" {
                    return Err(projection_violation(format!(
                        "Run '{run_id}' is '{run_status}' and cannot create fresh Host ToolCall '{runtime_tool_call_id}'"
                    )));
                }
                validate_managed_tool_acquisition(&transaction, run_id, now, true)?;
                transaction.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                        status, execution_location, requires_approval, started_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'host', ?8, ?9, ?9)",
                    params![
                        Uuid::new_v4().to_string(),
                        runtime_tool_call_id,
                        run_id,
                        conversation_id,
                        tool_name,
                        input_json,
                        status,
                        requires_approval as i64,
                        now
                    ],
                )?
            } else {
                0
            };
            let mut record = existing
                .map(Ok)
                .unwrap_or_else(|| query_tool_call(&transaction, run_id, runtime_tool_call_id))?;
            if record.conversation_id != conversation_id
                || record.tool_name != tool_name
                || record.input != *input
            {
                return Err(projection_violation(format!(
                    "Host ToolCall '{runtime_tool_call_id}' conflicts with existing immutable content"
                )));
            }
            let mut disposition = if inserted == 1 {
                super::HostToolCallDisposition::Created
            } else if matches!(record.status.as_str(), "pending" | "running") {
                super::HostToolCallDisposition::AlreadyInFlight
            } else {
                super::HostToolCallDisposition::ReplayTerminal
            };
            if record.execution_location == "runtime" {
                validate_managed_tool_acquisition(&transaction, run_id, now, false)?;
                let changed = transaction.execute(
                    "UPDATE tool_calls
                     SET status = ?3, execution_location = 'host', requires_approval = ?4,
                         updated_at = ?5
                     WHERE run_id = ?1 AND runtime_tool_call_id = ?2
                       AND execution_location = 'runtime' AND status = 'running'
                       AND requires_approval = 0
                       AND EXISTS(
                           SELECT 1 FROM runs
                           WHERE runs.id = tool_calls.run_id AND runs.status = 'running'
                       )",
                    params![
                        run_id,
                        runtime_tool_call_id,
                        status,
                        requires_approval as i64,
                        now
                    ],
                )?;
                record = query_tool_call(&transaction, run_id, runtime_tool_call_id)?;
                if changed == 1 {
                    disposition = super::HostToolCallDisposition::PromotedRuntime;
                } else if record.execution_location != "host" {
                    return Err(projection_violation(format!(
                        "Host ToolCall '{runtime_tool_call_id}' cannot claim a terminal Runtime projection or a ToolCall from terminal Run '{run_id}'"
                    )));
                }
            }
            if record.execution_location != "host" || record.requires_approval != requires_approval {
                return Err(projection_violation(format!(
                    "Host ToolCall '{runtime_tool_call_id}' conflicts with existing immutable execution authority"
                )));
            }
            transaction.commit()?;
            Ok((record, disposition))
        })
    }

    pub(crate) fn inspect_host_tool_call_replay(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
        tool_name: &str,
        input: &Value,
    ) -> Result<Option<(super::ToolCallRecord, super::HostToolCallDisposition)>, String> {
        self.with_connection(|connection| {
            let existing = query_tool_call(connection, run_id, runtime_tool_call_id).optional()?;
            let Some(record) = existing else {
                return Ok(None);
            };
            if record.tool_name != tool_name || record.input != *input {
                return Err(projection_violation(format!(
                    "ToolCall '{runtime_tool_call_id}' replay conflicts with immutable tool/input identity"
                )));
            }
            if record.execution_location == "runtime" {
                if record.status == "running" {
                    return Ok(None);
                }
                return Err(projection_violation(format!(
                    "Host execution cannot replay or claim terminal Runtime ToolCall '{runtime_tool_call_id}'"
                )));
            }
            if record.execution_location != "host" {
                return Err(projection_violation(format!(
                    "ToolCall '{runtime_tool_call_id}' has unknown execution authority '{}'",
                    record.execution_location
                )));
            }
            let disposition = if matches!(record.status.as_str(), "pending" | "running") {
                super::HostToolCallDisposition::AlreadyInFlight
            } else {
                super::HostToolCallDisposition::ReplayTerminal
            };
            Ok(Some((record, disposition)))
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
        let (compact_result, blob) = match result {
            Some(value) => {
                let (inline, blob) = split_tool_result(value);
                (Some(inline), blob)
            }
            None => (None, None),
        };
        let result_json = compact_result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|error| error.to_string())?;
        let terminal_status = if error_message.is_some() {
            "failed"
        } else {
            "completed"
        };
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            require_legacy_run_writer(&transaction, run_id)?;
            if let Some((sha, full, bytes)) = &blob {
                // Content-addressed full copy: same payload may be shared, but
                // its identity must match its hash.
                transaction.execute(
                    "INSERT INTO tool_call_result_blobs(sha256, body_json, byte_size, created_at)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT(sha256) DO NOTHING",
                    params![sha, full, bytes, now],
                )?;
            }
            let blob_sha: Option<&str> = blob.as_ref().map(|(sha, _, _)| sha.as_str());
            let changed = transaction.execute(
                "UPDATE tool_calls
                 SET status = ?3, result_json = ?4, error_message = ?5,
                     completed_at = ?6, updated_at = ?6,
                     result_blob_sha256 = ?7
                 WHERE run_id = ?1 AND runtime_tool_call_id = ?2
                  AND execution_location = 'host'
                  AND (status = 'running' OR (status = 'pending' AND ?3 = 'failed'))
                  AND EXISTS(
                      SELECT 1 FROM runs
                      WHERE runs.id = tool_calls.run_id AND runs.status = 'running'
                  )",
                params![
                    run_id,
                    runtime_tool_call_id,
                    terminal_status,
                    result_json,
                    error_message,
                    now,
                    blob_sha,
                ],
            )?;
            if changed == 0 {
                let existing = query_tool_call(&transaction, run_id, runtime_tool_call_id)?;
                if existing.execution_location != "host" {
                    return Err(projection_violation(format!(
                        "Host completion cannot claim Runtime ToolCall '{runtime_tool_call_id}'"
                    )));
                }
                if existing.status == terminal_status
                    && existing.result == compact_result
                    && existing.error_message.as_deref() == error_message
                {
                    transaction.commit()?;
                    return Ok(());
                }
                return Err(projection_violation(format!(
                    "Host ToolCall '{runtime_tool_call_id}' cannot transition from terminal/non-authoritative status '{}' to '{terminal_status}'",
                    existing.status
                )));
            }
            if error_message.is_none() {
                if let Some(result) = result {
                    let tool_name: Option<String> = transaction.query_row(
                        "SELECT tool_name FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                        params![run_id, runtime_tool_call_id],
                        |row| row.get(0),
                    ).optional()?;
                    if tool_name.as_deref() == Some("attachment_compute") {
                        computed_artifacts::persist(&transaction,run_id,runtime_tool_call_id,&result["details"],now)?;
                    }
                    if matches!(tool_name.as_deref(), Some("write_file") | Some("edit_file")) {
                        if let Some(path) = result.get("details").and_then(|details| details.get("path")).and_then(Value::as_str) {
                            let display_name = PathBuf::from(path).file_name().and_then(|value| value.to_str()).unwrap_or(path).to_owned();
                            let byte_size = result.get("details").and_then(|details| details.get("bytes")).and_then(Value::as_i64).unwrap_or(0);
                            let operation = result.get("details").and_then(|details| details.get("operation")).and_then(Value::as_str);
                            let artifact_type = if tool_name.as_deref() == Some("edit_file") || operation == Some("modified") {
                                "modified_file"
                            } else {
                                "created_file"
                            };
                            // Lifecycle class from Host facts: a file the Host
                            // resolved inside the conversation deliverable
                            // folder is a deliverable, anything else is a
                            // process file. Independent of the extension.
                            let (class, origin) = transaction
                                .query_row(
                                    "SELECT conversation_id FROM run_control_bindings WHERE run_id = ?1",
                                    params![run_id],
                                    |row| row.get::<_, Option<String>>(0),
                                )
                                .optional()?
                                .flatten()
                                .map(|conversation_id| computed_artifacts::conversation_project_root(&transaction, &conversation_id))
                                .transpose()?
                                .flatten()
                                .map(PathBuf::from)
                                .map(|root| crate::runtime_host::artifact_store::classify(
                                    Some(root.as_path()),
                                    Path::new(path),
                                    tool_name.as_deref().unwrap_or_default(),
                                    false,
                                ))
                                .unwrap_or((
                                    crate::runtime_host::artifact_store::ArtifactClass::Process,
                                    crate::runtime_host::artifact_store::ArtifactOrigin::Project,
                                ));
                            let updated = transaction.execute(
                                "UPDATE artifacts
                                 SET display_name = ?1,
                                     artifact_type = CASE
                                       WHEN artifact_type = 'created_file' THEN artifact_type
                                       ELSE ?2
                                     END,
                                     artifact_class = ?7, artifact_origin = ?8,
                                     byte_size = ?3, status = 'ready', updated_at = ?4
                                 WHERE run_id = ?5 AND storage_path = ?6",
                                params![display_name, artifact_type, byte_size, now, run_id, path, class.as_str(), origin.as_str()],
                            )?;
                            if updated == 0 {
                                transaction.execute(
                                    "INSERT INTO artifacts(id, conversation_id, run_id, display_name,
                                                           artifact_type, artifact_class, artifact_origin,
                                                           storage_path, byte_size,
                                                           status, created_at, updated_at)
                                     SELECT ?1, conversation_id, run_id, ?2, ?3, ?8, ?9, ?4, ?5,
                                            'ready', ?6, ?6 FROM tool_calls
                                     WHERE run_id = ?7 AND runtime_tool_call_id = ?10",
                                    params![Uuid::new_v4().to_string(), display_name, artifact_type, path, byte_size, now, run_id, class.as_str(), origin.as_str(), runtime_tool_call_id],
                                )?;
                            }
                        }
                    }
                    // Built-in Office writes (call_mcp_tool → fox-office) are
                    // user-facing deliverables. They were registered only as
                    // restorable managed versions, so the conversation file
                    // list showed intermediate compute files but never the
                    // xlsx/docx deliverables. Project the Host-verified write
                    // into the artifacts list as well.
                    if tool_name.as_deref() == Some("call_mcp_tool") {
                        // A rendered preview is an artifact, not a content
                        // version, and it is Host-private by default. It is
                        // projected separately so it never enters the restore
                        // history of a user document.
                        if let Some(preview) = result
                            .get("details")
                            .and_then(|details| details.get("foxPreview"))
                        {
                            if let Some(preview_path) = preview.get("storagePath").and_then(Value::as_str) {
                                // Name the *result* the preview depicts, not the
                                // cache file; the renderer adds the "预览" label,
                                // and the open action still uses the real path.
                                let preview_name = preview
                                    .get("sourceFile")
                                    .and_then(Value::as_str)
                                    .and_then(|source| {
                                        PathBuf::from(source)
                                            .file_name()
                                            .and_then(|value| value.to_str())
                                            .map(str::to_owned)
                                    })
                                    .or_else(|| {
                                        preview
                                            .get("displayName")
                                            .and_then(Value::as_str)
                                            .map(str::to_owned)
                                    })
                                    .unwrap_or_else(|| {
                                        PathBuf::from(preview_path)
                                            .file_name()
                                            .and_then(|value| value.to_str())
                                            .map(str::to_owned)
                                            .unwrap_or_else(|| preview_path.to_owned())
                                    });
                                let preview_bytes = preview
                                    .get("afterSize")
                                    .and_then(Value::as_i64)
                                    .unwrap_or(0);
                                let preview_sha = preview.get("afterHash").and_then(Value::as_str);
                                let preview_media = preview.get("mediaType").and_then(Value::as_str);
                                let project_root = transaction
                                    .query_row(
                                        "SELECT conversation_id FROM run_control_bindings WHERE run_id = ?1",
                                        params![run_id],
                                        |row| row.get::<_, Option<String>>(0),
                                    )
                                    .optional()?
                                    .flatten()
                                    .map(|conversation_id| computed_artifacts::conversation_project_root(&transaction, &conversation_id))
                                    .transpose()?
                                    .flatten()
                                    .map(PathBuf::from);
                                // Placement is recomputed structurally rather
                                // than read from the declared `artifactOrigin`:
                                // a cached preview is outside the project by
                                // construction, and one that stayed inside the
                                // project is an explicit export. A forged
                                // declaration therefore cannot relabel it.
                                let host_private = match project_root.as_deref() {
                                    Some(root) => !crate::runtime_host::artifact_store::is_inside(
                                        root,
                                        Path::new(preview_path),
                                    ),
                                    None => true,
                                };
                                let (class, origin) = project_root
                                    .map(|root| crate::runtime_host::artifact_store::classify(
                                        Some(root.as_path()),
                                        Path::new(preview_path),
                                        preview.get("tool").and_then(Value::as_str).unwrap_or("office_render"),
                                        host_private,
                                    ))
                                    .unwrap_or((
                                        crate::runtime_host::artifact_store::ArtifactClass::Preview,
                                        crate::runtime_host::artifact_store::ArtifactOrigin::HostPrivate,
                                    ));
                                let updated = transaction.execute(
                                    "UPDATE artifacts
                                     SET display_name = ?1, byte_size = ?2,
                                         sha256 = COALESCE(?3, sha256),
                                         media_type = COALESCE(?4, media_type),
                                         artifact_class = ?5, artifact_origin = ?6,
                                         status = 'ready', updated_at = ?7
                                     WHERE run_id = ?8 AND storage_path = ?9",
                                    params![preview_name, preview_bytes, preview_sha, preview_media, class.as_str(), origin.as_str(), now, run_id, preview_path],
                                )?;
                                if updated == 0 {
                                    transaction.execute(
                                        "INSERT INTO artifacts(id, conversation_id, run_id, display_name,
                                                               artifact_type, artifact_class, artifact_origin,
                                                               storage_path, byte_size, sha256, media_type,
                                                               status, created_at, updated_at)
                                         SELECT ?1, conversation_id, run_id, ?2, 'created_file', ?10, ?11,
                                                ?3, ?4, ?5, ?6, 'ready', ?7, ?7 FROM tool_calls
                                         WHERE run_id = ?8 AND runtime_tool_call_id = ?9",
                                        params![Uuid::new_v4().to_string(), preview_name, preview_path, preview_bytes, preview_sha, preview_media, now, run_id, runtime_tool_call_id, class.as_str(), origin.as_str()],
                                    )?;
                                }
                            }
                        }
                        if let Some(meta) = result
                            .get("details")
                            .and_then(|details| details.get("foxManagedFile"))
                        {
                            let path = meta.get("storagePath").and_then(Value::as_str);
                            let declared = meta.get("displayName").and_then(Value::as_str);
                            let change_kind = meta
                                .get("changeKind")
                                .and_then(Value::as_str)
                                .unwrap_or("created");
                            let byte_size = meta
                                .get("afterSize")
                                .and_then(Value::as_i64)
                                .unwrap_or(0);
                            let sha256 = meta.get("afterHash").and_then(Value::as_str);
                            if let Some(path) = path {
                                let display_name = declared
                                    .map(str::to_owned)
                                    .or_else(|| {
                                        PathBuf::from(path)
                                            .file_name()
                                            .and_then(|value| value.to_str())
                                            .map(str::to_owned)
                                    })
                                    .unwrap_or_else(|| path.to_owned());
                                let artifact_type = if change_kind == "created" {
                                    "created_file"
                                } else {
                                    "modified_file"
                                };
                                // Host verdict, not the connector's own
                                // label: derived from the verified operation
                                // and the resolved path inside this
                                // conversation's project root.
                                let inner_operation = meta.get("tool").and_then(Value::as_str).unwrap_or_default();
                                let (class, origin) = transaction
                                    .query_row(
                                        "SELECT conversation_id FROM run_control_bindings WHERE run_id = ?1",
                                        params![run_id],
                                        |row| row.get::<_, Option<String>>(0),
                                    )
                                    .optional()?
                                    .flatten()
                                    .map(|conversation_id| computed_artifacts::conversation_project_root(&transaction, &conversation_id))
                                    .transpose()?
                                    .flatten()
                                    .map(PathBuf::from)
                                    .map(|root| crate::runtime_host::artifact_store::classify(
                                        Some(root.as_path()),
                                        Path::new(path),
                                        inner_operation,
                                        false,
                                    ))
                                    .unwrap_or((
                                        crate::runtime_host::artifact_store::ArtifactClass::Process,
                                        crate::runtime_host::artifact_store::ArtifactOrigin::Project,
                                    ));
                                let updated = transaction.execute(
                                    "UPDATE artifacts
                                     SET display_name = ?1,
                                         artifact_type = CASE
                                           WHEN artifact_type = 'created_file' THEN artifact_type
                                           ELSE ?2
                                         END,
                                         artifact_class = ?8, artifact_origin = ?9,
                                         byte_size = ?3, sha256 = COALESCE(?4, sha256),
                                         status = 'ready', updated_at = ?5
                                     WHERE run_id = ?6 AND storage_path = ?7",
                                    params![display_name, artifact_type, byte_size, sha256, now, run_id, path, class.as_str(), origin.as_str()],
                                )?;
                                if updated == 0 {
                                    transaction.execute(
                                        "INSERT INTO artifacts(id, conversation_id, run_id, display_name,
                                                               artifact_type, artifact_class, artifact_origin,
                                                               storage_path, byte_size, sha256,
                                                               status, created_at, updated_at)
                                         SELECT ?1, conversation_id, run_id, ?2, ?3, ?10, ?11, ?4, ?5, ?6,
                                                'ready', ?7, ?7 FROM tool_calls
                                         WHERE run_id = ?8 AND runtime_tool_call_id = ?9",
                                        params![Uuid::new_v4().to_string(), display_name, artifact_type, path, byte_size, sha256, now, run_id, class.as_str(), origin.as_str(), runtime_tool_call_id],
                                    )?;
                                }
                            }
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
        self.create_approval_with_category(
            tool_call_id,
            requested_action,
            request,
            TOOL_EXECUTION_APPROVAL_CATEGORY,
        )
    }

    pub(crate) fn create_approval_with_category(
        &self,
        tool_call_id: &str,
        requested_action: &str,
        request: &Value,
        category: &str,
    ) -> Result<super::ApprovalRecord, String> {
        if !matches!(
            category,
            TOOL_EXECUTION_APPROVAL_CATEGORY | TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY
        ) {
            return Err(format!("unsupported approval category '{category}'"));
        }
        let now = now_ms();
        let id = Uuid::new_v4().to_string();
        let request_json = serde_json::to_string(request).map_err(|error| error.to_string())?;
        let approval = self.with_connection(|connection| {
            let run_id: String = connection.query_row("SELECT run_id FROM tool_calls WHERE id=?1", [tool_call_id], |row| row.get(0))?;
            require_legacy_run_writer(connection, &run_id)?;
            connection.execute(
                "INSERT INTO approvals(
                    id, tool_call_id, status, requested_action, request_json, requested_at,
                    category
                 )
                 SELECT ?1, ?2, 'pending', ?3, ?4, ?5, ?6
                 WHERE EXISTS(
                    SELECT 1 FROM tool_calls
                    WHERE id = ?2 AND status = 'pending' AND requires_approval = 1
                 )
                 ON CONFLICT(tool_call_id) DO NOTHING",
                params![
                    id,
                    tool_call_id,
                    requested_action,
                    request_json,
                    now,
                    category
                ],
            )?;
            query_approval_by_tool_call(connection, tool_call_id).optional()
        })?;
        let Some(approval) = approval else {
            return Err(format!(
                "ToolCall '{tool_call_id}' is not pending explicit approval"
            ));
        };
        if approval.requested_action != requested_action || approval.request != *request {
            return Err(format!(
                "approval for ToolCall '{tool_call_id}' conflicts with existing immutable content"
            ));
        }
        if approval.category != category {
            return Err(format!(
                "approval for ToolCall '{tool_call_id}' conflicts with immutable category '{}'; requested '{category}'",
                approval.category
            ));
        }
        Ok(approval)
    }

    pub(crate) fn create_task_repair_override_approval(
        &self,
        tool_call_id: &str,
        requested_action: &str,
        request: &Value,
    ) -> Result<super::ApprovalRecord, String> {
        self.create_approval_with_category(
            tool_call_id,
            requested_action,
            request,
            TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY,
        )
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        decision: super::ApprovalDecision,
    ) -> Result<Option<super::ApprovalRecord>, String> {
        let existing =
            self.with_connection(|connection| query_approval(connection, approval_id))?;
        if existing.status != "pending" {
            return Ok(None);
        }
        if existing.category == TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY
            && decision == super::ApprovalDecision::AllowConversation
        {
            return Err(
                "task repair budget override approvals only accept allow_once or deny".to_owned(),
            );
        }
        let permission_scope = existing
            .request
            .get("permissionScope")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let allows_conversation = existing
            .request
            .get("availableDecisions")
            .and_then(Value::as_array)
            .is_none_or(|items| {
                items
                    .iter()
                    .any(|item| item.as_str() == Some("allow_conversation"))
            });
        if decision == super::ApprovalDecision::AllowConversation
            && (permission_scope.is_none() || !allows_conversation)
        {
            return Err(
                "this approval is intentionally one-shot and cannot grant conversation access"
                    .to_owned(),
            );
        }
        // Denial cleanup must remain possible even if a frozen binding is corrupt.
        let window = if decision.approved() {
            Some(self.run_approval_window(&existing.run_id, approval_id)?)
        } else { None };
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_legacy_run_writer(&transaction, &existing.run_id)?;
            let now = now_ms();
            let expired = window.is_some_and(|(start, deadline)| now < start || now >= deadline);
            let approved = decision.approved() && !expired;
            let decision_json = json!({
                "approved": approved,
                "scope": if expired { "none" } else { decision.scope() },
                "category": existing.category,
                "reason": if expired { Some("approval_wait_timeout") } else { None },
            }).to_string();
            let updated = transaction.execute(
                "UPDATE approvals
                 SET status = ?2, decision_json = ?3, resolved_at = ?4
                 WHERE id = ?1 AND status = 'pending'
                   AND EXISTS (
                       SELECT 1
                       FROM tool_calls t JOIN runs r ON r.id = t.run_id
                       WHERE t.id = approvals.tool_call_id
                         AND t.status = 'pending'
                         AND r.status = 'running'
                   )",
                params![
                    approval_id,
                    if expired { "expired" } else if approved { "approved" } else { "denied" },
                    decision_json,
                    now
                ],
            )?;
            if updated == 0 {
                return Ok(None);
            }
            let approval = query_approval(&transaction, approval_id)?;
            if approved && decision == super::ApprovalDecision::AllowConversation {
                let scope_key = permission_scope.expect("validated conversation permission scope");
                transaction.execute(
                    // A fresh conversation-level approval RE-ACTIVATES the row:
                    // leaving a previous `revoked_at` in place would make the new
                    // grant dead on arrival. The revocation generation has
                    // already advanced, so a Run frozen before it still cannot
                    // use this new authorization.
                    "INSERT INTO conversation_tool_permissions(
                         conversation_id, tool_name, scope_key, granted_at,
                         revoked_at, revoke_reason
                     ) VALUES (?1, ?2, ?3, ?4, NULL, NULL)
                     ON CONFLICT(conversation_id, tool_name, scope_key)
                     DO UPDATE SET granted_at = excluded.granted_at,
                                   revoked_at = NULL, revoke_reason = NULL",
                    params![
                        &approval.conversation_id,
                        &approval.tool_name,
                        scope_key,
                        now
                    ],
                )?;
            }
            // B) The dedicated whole-file replacement authorization is settled in
            // the SAME transaction as the ordinary decision, so a waiting
            // executor can only ever observe both facts together. A denial
            // withdraws the request immediately and terminally.
            if existing.request.get("wholeFileReplacement").is_some() {
                let call_id = Self::tool_call_id_of(&transaction, &approval.tool_call_id)?;
                let settled = if approved {
                    super::kernel_execution_admission::confirm_replace_grant_from_approval_in_tx(
                        &transaction,
                        &approval.run_id,
                        &call_id,
                        now,
                    )
                    .map(|_| ())
                } else {
                    super::kernel_execution_admission::withdraw_replace_grant_in_tx(
                        &transaction,
                        &approval.run_id,
                        &call_id,
                    )
                    .map(|_| ())
                };
                if let Err(error) = settled {
                    // A settlement failure must not leave a half-applied decision:
                    // roll the whole transaction back rather than approving the
                    // ordinary decision while the dedicated request stays pending.
                    return Err(rusqlite::Error::InvalidParameterName(error));
                }
            }
            transaction.commit()?;
            Ok(Some(approval))
        })
    }

    /// The durable Host tool-call identity of one `tool_calls` row, read inside
    /// the caller's transaction (never a second connection).
    fn tool_call_id_of(
        transaction: &rusqlite::Transaction<'_>,
        tool_call_row_id: &str,
    ) -> Result<String, rusqlite::Error> {
        transaction.query_row(
            "SELECT runtime_tool_call_id FROM tool_calls WHERE id=?1",
            [tool_call_row_id],
            |row| row.get(0),
        )
    }

    pub fn claim_approved_tool_call(
        &self,
        run_id: &str,
        runtime_tool_call_id: &str,
    ) -> Result<bool, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            require_legacy_run_writer(&transaction, run_id)?;
            let approval_id = transaction
                .query_row(
                    "SELECT a.id
                     FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
                     JOIN runs r ON r.id = t.run_id
                     WHERE t.run_id = ?1 AND t.runtime_tool_call_id = ?2
                       AND t.status = 'pending' AND r.status = 'running'
                       AND a.status = 'approved' AND a.claimed_at IS NULL
                       AND a.category = 'tool_execution'
                       AND json_extract(a.decision_json, '$.approved') = 1
                       AND json_extract(a.decision_json, '$.scope') IN ('once', 'conversation')",
                    params![run_id, runtime_tool_call_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(approval_id) = approval_id else {
                return Ok(false);
            };
            if let Err(error) = validate_managed_tool_acquisition(&transaction, run_id, now, false)
            {
                let error_message = error.to_string();
                let tool_failed = transaction.execute(
                    "UPDATE tool_calls
                     SET status = 'failed', error_message = ?3,
                         completed_at = ?4, updated_at = ?4
                     WHERE run_id = ?1 AND runtime_tool_call_id = ?2
                       AND execution_location = 'host' AND status = 'pending'",
                    params![run_id, runtime_tool_call_id, error_message, now],
                )?;
                let approval_claimed = transaction.execute(
                    "UPDATE approvals
                     SET claimed_at = ?2, claimed_by_run_id = ?3
                     WHERE id = ?1 AND status = 'approved' AND claimed_at IS NULL
                       AND category = 'tool_execution'",
                    params![approval_id, now, run_id],
                )?;
                if tool_failed != 1 || approval_claimed != 1 {
                    return Err(projection_violation(
                        "managed approval budget rejection lost its terminal CAS".to_owned(),
                    ));
                }
                transaction.commit()?;
                return Err(error);
            }
            let tool_updated = transaction.execute(
                "UPDATE tool_calls
                 SET status = 'running', updated_at = ?3
                 WHERE run_id = ?1 AND runtime_tool_call_id = ?2 AND status = 'pending'
                   AND EXISTS (
                       SELECT 1 FROM runs r
                       WHERE r.id = tool_calls.run_id AND r.status = 'running'
                   )
                    ",
                params![run_id, runtime_tool_call_id, now],
            )?;
            let approval_updated = transaction.execute(
                "UPDATE approvals
                 SET claimed_at = ?2, claimed_by_run_id = ?3
                 WHERE id = ?1 AND status = 'approved' AND claimed_at IS NULL
                   AND category = 'tool_execution'",
                params![approval_id, now, run_id],
            )?;
            if tool_updated != 1 || approval_updated != 1 {
                return Ok(false);
            }
            transaction.commit()?;
            Ok(true)
        })
    }

    pub fn conversation_tool_permission_granted(
        &self,
        conversation_id: &str,
        tool_name: &str,
        scope_key: &str,
    ) -> Result<bool, String> {
        if scope_key.trim().is_empty() {
            return Ok(false);
        }
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM conversation_tool_permissions
                    WHERE conversation_id = ?1 AND tool_name = ?2 AND scope_key = ?3
                      AND revoked_at IS NULL
                 )",
                params![conversation_id, tool_name, scope_key],
                |row| row.get(0),
            )
        })
    }

    pub fn pending_approvals_for_conversation_tool(
        &self,
        conversation_id: &str,
        tool_name: &str,
    ) -> Result<Vec<super::ApprovalRecord>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                        a.status, a.requested_action, a.request_json, a.decision_json,
                        a.requested_at, a.resolved_at, a.category, a.claimed_at,
                        a.claimed_by_run_id
                 FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
                 WHERE t.conversation_id = ?1 AND t.tool_name = ?2 AND a.status = 'pending'
                 ORDER BY a.requested_at ASC, a.id ASC",
            )?;
            let approvals = statement
                .query_map(params![conversation_id, tool_name], map_approval)?
                .collect();
            approvals
        })
    }

    pub fn conversation_has_active_run(&self, conversation_id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT 1 FROM runs
                     WHERE conversation_id = ?1 AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
                     LIMIT 1",
                    [conversation_id],
                    |_| Ok(()),
                )
                .optional()
                .map(|value| value.is_some())
        })
    }

    pub fn active_run_id(&self, conversation_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT id FROM runs
                     WHERE conversation_id = ?1 AND status IN ('queued', 'running')
                     ORDER BY created_at DESC LIMIT 1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    pub fn delete_conversation(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute("DELETE FROM conversations WHERE id = ?1", [id])? > 0)
        })
    }

    pub fn purge_trashed_conversation(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            Ok(connection.execute(
                "DELETE FROM conversations WHERE id = ?1 AND trashed_at IS NOT NULL",
                [id],
            )? > 0)
        })
    }

    pub fn conversation_is_trashed(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|connection| {
            connection.query_row(
                "SELECT trashed_at IS NOT NULL FROM conversations WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
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

    pub fn create_goal_continuation_run(
        &self,
        conversation_id: &str,
        prompt: &str,
    ) -> Result<StartRunResult, String> {
        let clean_prompt = prompt.trim();
        if clean_prompt.is_empty() {
            return Err("continuation prompt cannot be empty".to_owned());
        }

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut started =
                create_run_in_transaction(&transaction, conversation_id, clean_prompt, None)?;
            transaction.execute(
                "UPDATE messages SET role = 'system', kind = 'internal' WHERE id = ?1",
                [started.user_message.id.as_str()],
            )?;
            started.user_message.role = "system".to_owned();
            started.user_message.kind = "internal".to_owned();
            transaction.commit()?;
            Ok(started)
        })
    }

    pub fn rewind_run(
        &self,
        conversation_id: &str,
        message_id: &str,
        text: &str,
        requested_model: Option<&str>,
    ) -> Result<StartRunResult, String> {
        let clean_text = text.trim();
        if clean_text.is_empty() {
            return Err("message text cannot be empty".to_owned());
        }

        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (target_ordinal, target_created_at, target_role): (i64, i64, String) = transaction
                .query_row(
                    "SELECT ordinal, created_at, role FROM messages
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![message_id, conversation_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            if target_role != "user" {
                return Err(rusqlite::Error::InvalidQuery);
            }

            let active_run = transaction
                .query_row(
                    "SELECT 1 FROM runs
                     WHERE conversation_id = ?1
                       AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
                     LIMIT 1",
                    [conversation_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if active_run {
                return Err(rusqlite::Error::InvalidQuery);
            }

            let crosses_repair_override_boundary: bool = transaction.query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM task_repair_override_events override_event
                    WHERE override_event.conversation_id = ?1
                      AND override_event.run_id IN (
                        SELECT DISTINCT run_id FROM messages
                        WHERE conversation_id = ?1 AND ordinal >= ?2 AND run_id IS NOT NULL
                      )
                 )",
                params![conversation_id, target_ordinal],
                |row| row.get(0),
            )?;
            if crosses_repair_override_boundary {
                return Err(projection_violation(
                    "rewind.repair_override_boundary: cannot rewind across an operator-authorized repair budget override; fork or start a new conversation so the append-only audit fact and Task lifetime limit remain intact"
                        .to_owned(),
                ));
            }

            let attachment_ids = {
                let mut statement = transaction.prepare(
                    "SELECT id FROM attachments WHERE conversation_id = ?1 AND message_id = ?2
                     ORDER BY created_at ASC",
                )?;
                let records = statement
                    .query_map(params![conversation_id, message_id], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                records
            };

            transaction.execute(
                "DELETE FROM attachments
                 WHERE conversation_id = ?1 AND message_id IN (
                   SELECT id FROM messages WHERE conversation_id = ?1 AND ordinal > ?2
                 )",
                params![conversation_id, target_ordinal],
            )?;
            transaction.execute(
                "UPDATE attachments SET message_id = NULL
                 WHERE conversation_id = ?1 AND message_id = ?2",
                params![conversation_id, message_id],
            )?;

            transaction.execute(
                "DELETE FROM work_tasks WHERE id IN (
                   SELECT json_extract(event_json, '$.taskId') FROM work_events
                   WHERE conversation_id = ?1 AND created_at >= ?2
                     AND event_type = 'task.created'
                     AND json_extract(event_json, '$.taskId') IS NOT NULL
                 )",
                params![conversation_id, target_created_at],
            )?;
            transaction.execute(
                "DELETE FROM goals WHERE conversation_id = ?1 AND id IN (
                   SELECT json_extract(event_json, '$.goalId') FROM work_events
                   WHERE conversation_id = ?1 AND created_at >= ?2
                     AND event_type = 'goal.proposed'
                     AND json_extract(event_json, '$.goalId') IS NOT NULL
                 )",
                params![conversation_id, target_created_at],
            )?;
            transaction.execute(
                "UPDATE goals SET
                   status = (
                     SELECT json_extract(previous.event_json, '$.data.goal.status')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.goalId') = goals.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.goal.status') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ),
                   version = COALESCE((
                     SELECT json_extract(previous.event_json, '$.data.goal.version')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.goalId') = goals.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.goal.version') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ), version),
                   updated_at = COALESCE((
                     SELECT json_extract(previous.event_json, '$.data.goal.updatedAt')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.goalId') = goals.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.goal.updatedAt') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ), updated_at),
                   completed_at = NULL,
                   blocked_reason = NULL
                 WHERE conversation_id = ?1
                   AND id IN (
                     SELECT json_extract(event_json, '$.goalId') FROM work_events
                     WHERE conversation_id = ?1 AND created_at >= ?2
                       AND json_extract(event_json, '$.goalId') IS NOT NULL
                   )
                   AND EXISTS (
                     SELECT 1 FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.goalId') = goals.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.goal.status') IS NOT NULL
                   )",
                params![conversation_id, target_created_at],
            )?;
            transaction.execute(
                "UPDATE work_tasks SET
                   status = (
                     SELECT json_extract(previous.event_json, '$.data.task.status')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.taskId') = work_tasks.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.task.status') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ),
                   version = COALESCE((
                     SELECT json_extract(previous.event_json, '$.data.task.version')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.taskId') = work_tasks.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.task.version') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ), version),
                   updated_at = COALESCE((
                     SELECT json_extract(previous.event_json, '$.data.task.updatedAt')
                     FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.taskId') = work_tasks.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.task.updatedAt') IS NOT NULL
                     ORDER BY previous.created_at DESC, previous.sequence DESC LIMIT 1
                   ), updated_at),
                   owner_run_id = NULL,
                   blocked_reason = NULL,
                   finished_at = NULL
                 WHERE id IN (
                   SELECT json_extract(event_json, '$.taskId') FROM work_events
                   WHERE conversation_id = ?1 AND created_at >= ?2
                     AND json_extract(event_json, '$.taskId') IS NOT NULL
                 )
                   AND EXISTS (
                     SELECT 1 FROM work_events previous
                     WHERE previous.conversation_id = ?1
                       AND json_extract(previous.event_json, '$.taskId') = work_tasks.id
                       AND previous.created_at < ?2
                       AND json_extract(previous.event_json, '$.data.task.status') IS NOT NULL
                   )",
                params![conversation_id, target_created_at],
            )?;
            transaction.execute(
                "DELETE FROM work_events WHERE conversation_id = ?1 AND created_at >= ?2",
                params![conversation_id, target_created_at],
            )?;
            transaction.execute(
                "DELETE FROM artifacts WHERE conversation_id = ?1 AND run_id IN (
                   SELECT DISTINCT run_id FROM messages
                   WHERE conversation_id = ?1 AND ordinal >= ?2 AND run_id IS NOT NULL
                 )",
                params![conversation_id, target_ordinal],
            )?;
            transaction.execute(
                "DELETE FROM runs WHERE conversation_id = ?1 AND id IN (
                   SELECT DISTINCT run_id FROM messages
                   WHERE conversation_id = ?1 AND ordinal >= ?2 AND run_id IS NOT NULL
                 )",
                params![conversation_id, target_ordinal],
            )?;
            transaction.execute(
                "DELETE FROM messages WHERE conversation_id = ?1 AND ordinal >= ?2",
                params![conversation_id, target_ordinal],
            )?;
            transaction.execute(
                "DELETE FROM runtime_sessions WHERE conversation_id = ?1",
                [conversation_id],
            )?;
            if target_ordinal == 1 {
                transaction.execute(
                    "UPDATE conversations SET title = '新对话' WHERE id = ?1",
                    [conversation_id],
                )?;
            }

            let mut started = create_run_in_transaction(
                &transaction,
                conversation_id,
                clean_text,
                requested_model,
            )?;
            let replacement_message_id = started.user_message.id.clone();
            transaction.execute(
                "UPDATE messages SET id = ?1, created_at = ?3
                 WHERE id = ?2 AND conversation_id = ?4",
                params![
                    message_id,
                    replacement_message_id,
                    target_created_at,
                    conversation_id
                ],
            )?;
            started.user_message.id = message_id.to_owned();
            started.user_message.created_at = target_created_at;
            for attachment_id in &attachment_ids {
                transaction.execute(
                    "UPDATE attachments SET message_id = ?2 WHERE id = ?1 AND conversation_id = ?3",
                    params![attachment_id, started.user_message.id, conversation_id],
                )?;
            }
            if !attachment_ids.is_empty() {
                let mut statement = transaction.prepare(
                    "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                            byte_size, sha256, status, created_at
                     FROM attachments WHERE conversation_id = ?1 AND message_id = ?2
                     ORDER BY created_at ASC",
                )?;
                started.attachments = statement
                    .query_map(
                        params![conversation_id, started.user_message.id],
                        map_attachment,
                    )?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
            }
            transaction.commit()?;
            Ok(started)
        })
    }

    pub fn save_pending_work_mode_dispatch(
        &self,
        goal_id: &str,
        started: &StartRunResult,
        runtime_text: &str,
    ) -> Result<StartRunResult, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let updated = transaction.execute(
                "UPDATE runs SET status = 'awaiting_confirmation'
                 WHERE id = ?1 AND conversation_id = ?2 AND status = 'queued'",
                params![started.run.id, started.run.conversation_id],
            )?;
            if updated != 1 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            transaction.execute(
                "INSERT INTO pending_work_mode_dispatches(
                    goal_id, run_id, conversation_id, runtime_text, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    goal_id,
                    started.run.id,
                    started.run.conversation_id,
                    runtime_text,
                    now_ms()
                ],
            )?;
            transaction.commit()?;
            let mut pending = started.clone();
            pending.run.status = "awaiting_confirmation".to_owned();
            Ok(pending)
        })
    }

    pub fn pending_work_mode_dispatch(
        &self,
        goal_id: &str,
    ) -> Result<Option<PendingWorkModeDispatch>, String> {
        self.with_connection(|connection| query_pending_work_mode_dispatch(connection, goal_id))
    }

    pub fn release_pending_work_mode_dispatch(
        &self,
        goal_id: &str,
    ) -> Result<PendingWorkModeDispatch, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let mut pending = query_pending_work_mode_dispatch(&transaction, goal_id)?
                .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
            if pending.goal_id != goal_id {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let updated = transaction.execute(
                "UPDATE runs SET status = 'queued'
                 WHERE id = ?1 AND status = 'awaiting_confirmation'",
                [pending.started.run.id.as_str()],
            )?;
            if updated != 1 {
                return Err(rusqlite::Error::InvalidQuery);
            }
            transaction.execute(
                "DELETE FROM pending_work_mode_dispatches WHERE goal_id = ?1",
                [goal_id],
            )?;
            transaction.commit()?;
            pending.started.run.status = "queued".to_owned();
            Ok(pending)
        })
    }

    pub fn reject_pending_work_mode_dispatch(&self, goal_id: &str) -> Result<bool, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let run_id = transaction
                .query_row(
                    "SELECT run_id FROM pending_work_mode_dispatches WHERE goal_id = ?1",
                    [goal_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            let Some(run_id) = run_id else {
                return Ok(false);
            };
            transaction.execute(
                "UPDATE runs
                 SET status = 'cancelled', finished_at = ?2,
                     error_code = 'work_mode.declined',
                     error_message = 'User declined work mode execution.'
                 WHERE id = ?1 AND status = 'awaiting_confirmation'",
                params![run_id, now],
            )?;
            observability::close_run_trace_in_transaction(
                &transaction,
                &run_id,
                now,
                "ok",
                Some("work_mode.declined"),
                Some("User declined work mode execution."),
            )?;
            transaction.execute(
                "DELETE FROM pending_work_mode_dispatches WHERE goal_id = ?1",
                [goal_id],
            )?;
            transaction.commit()?;
            Ok(true)
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
            let parent_status = transaction
                .query_row(
                    "SELECT status FROM runs
                     WHERE id = ?1 AND conversation_id = ?2",
                    params![parent_run_id, conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if parent_status.as_deref() != Some("completed") {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            let mut terminal_statement = transaction.prepare(
                "SELECT event_json FROM run_events
                 WHERE run_id = ?1 AND event_type = 'run.completed'
                 ORDER BY seq ASC",
            )?;
            let terminal_payloads = terminal_statement
                .query_map([parent_run_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(terminal_statement);
            let first_terminal = terminal_payloads
                .first()
                .and_then(|json| serde_json::from_str::<Value>(json).ok())
                .ok_or(rusqlite::Error::InvalidQuery)?;
            if first_terminal.get("type").and_then(Value::as_str) != Some("run.completed")
                || first_terminal
                    .get("completionReason")
                    .and_then(Value::as_str)
                    != Some("awaiting_user")
                || terminal_payloads.iter().any(|json| {
                    serde_json::from_str::<Value>(json)
                        .map(|payload| payload != first_terminal)
                        .unwrap_or(true)
                })
            {
                return Err(rusqlite::Error::InvalidQuery);
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

    pub fn run_user_message_content(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT content FROM messages
                     WHERE conversation_id = ?1
                       AND run_id = ?2
                       AND role = 'user'
                       AND kind = 'text'
                     ORDER BY ordinal DESC
                     LIMIT 1",
                    params![conversation_id, run_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    pub fn runtime_prompt_context_before_message(
        &self,
        conversation_id: &str,
        message_id: &str,
        limit: usize,
    ) -> Result<Vec<RuntimePromptMessage>, String> {
        self.with_connection(|connection| {
            let target_ordinal: i64 = connection.query_row(
                "SELECT ordinal FROM messages WHERE id = ?1 AND conversation_id = ?2",
                params![message_id, conversation_id],
                |row| row.get(0),
            )?;
            let mut statement = connection.prepare(
                "SELECT role, content FROM (
                    SELECT role, content, ordinal FROM messages
                    WHERE conversation_id = ?1
                      AND ordinal < ?2
                      AND role IN ('user', 'assistant')
                      AND kind = 'text'
                      AND status IN ('completed', 'streaming')
                    ORDER BY ordinal DESC LIMIT ?3
                 ) ORDER BY ordinal ASC",
            )?;
            let records = statement
                .query_map(
                    params![conversation_id, target_ordinal, limit as i64],
                    |row| {
                        Ok(RuntimePromptMessage {
                            role: row.get(0)?,
                            content: row.get(1)?,
                        })
                    },
                )?
                .collect();
            records
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
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            require_legacy_run_writer(&transaction, run_id)?;
            let conversation_id = transaction
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1 AND status IN ('queued', 'running')",
                    [run_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if conversation_id.is_some() {
                let updated = transaction.execute(
                    "UPDATE runs SET status = 'cancelling'
                     WHERE id = ?1 AND status IN ('queued', 'running')",
                    [run_id],
                )?;
                if updated == 0 {
                    return Ok(None);
                }
                cancel_pending_approvals_for_run(
                    &transaction,
                    run_id,
                    "run_cancellation_requested",
                    now,
                )?;
            }
            transaction.commit()?;
            Ok(conversation_id)
        })
    }

    pub fn mark_run_failed(&self, run_id: &str, code: &str, message: &str) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            if transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
                [run_id], |row| row.get::<_,bool>(0))? {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let (status, existing_code, existing_message): (
                String,
                Option<String>,
                Option<String>,
            ) = transaction.query_row(
                "SELECT status, error_code, error_message FROM runs WHERE id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if status == "failed"
                && existing_code.as_deref() == Some(code)
                && existing_message.as_deref() == Some(message)
            {
                transaction.commit()?;
                return Ok(());
            }
            if matches!(
                status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            ) {
                return Err(projection_violation(format!(
                    "late Host failure cannot replace terminal Run '{run_id}' outcome '{status}'"
                )));
            }
            let changed = transaction.execute(
                "UPDATE runs SET status = 'failed', finished_at = ?2, error_code = ?3, error_message = ?4
                 WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                params![run_id, now, code, message],
            )?;
            if changed != 1 {
                return Err(projection_violation(format!(
                    "Run '{run_id}' is not eligible for Host failure transition"
                )));
            }
            transaction.execute(
                "UPDATE messages SET status = 'interrupted', updated_at = ?2
                 WHERE run_id = ?1 AND role = 'assistant' AND status = 'streaming'",
                params![run_id, now],
            )?;
            transaction.execute(
                "UPDATE tool_calls
                 SET status = 'interrupted', error_message = ?2, completed_at = ?3, updated_at = ?3
                 WHERE run_id = ?1 AND status IN ('pending', 'running')",
                params![run_id, message, now],
            )?;
            expire_pending_approvals_for_run(
                &transaction,
                run_id,
                "run_failed",
                now,
            )?;
            observability::close_run_trace_in_transaction(
                &transaction,
                run_id,
                now,
                "error",
                Some(code),
                Some(message),
            )?;
            child_runs::project_child_run_event(
                &transaction,
                run_id,
                "run.failed",
                &json!({ "type": "run.failed", "code": code, "message": message }),
                now,
            )?;
            digital_colleagues::project_digital_colleague_event(
                &transaction,
                run_id,
                "run.failed",
                &json!({ "type": "run.failed", "code": code, "message": message }),
                now,
            )?;
            interrupt_tasks_owned_by_run(
                &transaction,
                run_id,
                &now.to_string(),
                "failed",
                message,
            )?;
            transaction.commit()?;
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
            let transaction = connection.transaction()?;
            if transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
                [run_id], |row| row.get::<_,bool>(0))? {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let (status, existing_code, existing_message): (
                String,
                Option<String>,
                Option<String>,
            ) = transaction.query_row(
                "SELECT status, error_code, error_message FROM runs WHERE id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            if status == "interrupted"
                && existing_code.as_deref() == Some(code)
                && existing_message.as_deref() == Some(message)
            {
                transaction.commit()?;
                return Ok(());
            }
            if matches!(
                status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            ) {
                return Err(projection_violation(format!(
                    "late Host interruption cannot replace terminal Run '{run_id}' outcome '{status}'"
                )));
            }
            let changed = transaction.execute(
                "UPDATE runs SET status = 'interrupted', finished_at = ?2,
                                 error_code = ?3, error_message = ?4
                 WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                params![run_id, now, code, message],
            )?;
            if changed != 1 {
                return Err(projection_violation(format!(
                    "Run '{run_id}' is not eligible for Host interruption transition"
                )));
            }
            transaction.execute(
                "UPDATE messages SET status = 'interrupted', updated_at = ?2
                 WHERE run_id = ?1 AND status = 'streaming'",
                params![run_id, now],
            )?;
            transaction.execute(
                "UPDATE tool_calls
                 SET status = 'interrupted', error_message = ?2, completed_at = ?3, updated_at = ?3
                 WHERE run_id = ?1 AND status IN ('pending', 'running')",
                params![run_id, message, now],
            )?;
            expire_pending_approvals_for_run(&transaction, run_id, "run_interrupted", now)?;
            observability::close_run_trace_in_transaction(
                &transaction,
                run_id,
                now,
                "error",
                Some(code),
                Some(message),
            )?;
            child_runs::project_child_run_event(
                &transaction,
                run_id,
                "run.interrupted",
                &json!({ "type": "run.interrupted", "code": code, "message": message }),
                now,
            )?;
            digital_colleagues::project_digital_colleague_event(
                &transaction,
                run_id,
                "run.interrupted",
                &json!({ "type": "run.interrupted", "code": code, "message": message }),
                now,
            )?;
            interrupt_tasks_owned_by_run(
                &transaction,
                run_id,
                &now.to_string(),
                "failed",
                message,
            )?;
            transaction.commit()?;
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
                "SELECT id, name, icon, base_url, api_type, enabled, is_default, last_status,
                        last_latency_ms, last_checked_at, created_at, updated_at
                 FROM model_providers ORDER BY is_default DESC, created_at ASC",
            )?;
            let providers = provider_statement
                .query_map([], |row| {
                    let id: String = row.get(0)?;
                    let base_url: String = row.get(3)?;
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
                        icon: row.get(2)?,
                        connection_type: connection_type(&base_url),
                        base_url,
                        api_type: row.get(4)?,
                        enabled: row.get::<_, i64>(5)? != 0,
                        is_default: row.get::<_, i64>(6)? != 0,
                        credential_configured: false,
                        last_status: row.get(7)?,
                        last_latency_ms: row.get(8)?,
                        last_checked_at: row.get(9)?,
                        models,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
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
                    id, name, icon, base_url, api_type, enabled, is_default, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name, icon = excluded.icon, base_url = excluded.base_url,
                    api_type = excluded.api_type, enabled = 1,
                    is_default = excluded.is_default,
                    last_status = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_status ELSE 'unknown' END,
                    last_latency_ms = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_latency_ms ELSE NULL END,
                    last_checked_at = CASE WHEN model_providers.base_url = excluded.base_url THEN model_providers.last_checked_at ELSE NULL END,
                    updated_at = excluded.updated_at",
                params![provider.id, provider.name, provider.icon, provider.base_url, provider.api_type, provider.is_default as i64, now],
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
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if transaction.query_row("SELECT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')",
                [run_id], |row| row.get::<_,bool>(0))? {
                return Ok(false);
            }
            let (run_status, last_seq): (String, i64) = transaction.query_row(
                "SELECT status, last_seq FROM runs WHERE id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if seq <= last_seq {
                transaction.rollback()?;
                return Ok(false);
            }
            if event_type == "usage.request" {
                model_usage::store(&transaction,run_id,&payload["record"])?;
                transaction.execute("UPDATE runs SET last_seq=?2 WHERE id=?1",params![run_id,seq])?;
                transaction.commit()?;
                return Ok(true);
            }
            let run_is_terminal = matches!(
                run_status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            );
            if run_is_terminal {
                let frozen_terminal_type = format!("run.{run_status}");
                if event_type == frozen_terminal_type {
                    authorize_runtime_event_payload_replay(
                        &transaction,
                        run_id,
                        event_type,
                        payload,
                        seq,
                    )?;
                    transaction.execute(
                        "UPDATE runs SET last_seq = ?2 WHERE id = ?1 AND status = ?3",
                        params![run_id, seq, run_status],
                    )?;
                    transaction.commit()?;
                    return Ok(true);
                }
                return Err(projection_violation(format!(
                    "Runtime event '{event_type}' is not allowed after Run '{run_id}' reached terminal status '{run_status}'"
                )));
            }
            if run_status == "cancelling"
                && !matches!(
                    event_type,
                    "usage.updated"
                        | "run.completed"
                        | "run.cancelled"
                        | "run.interrupted"
                        | "run.failed"
                )
            {
                transaction.rollback()?;
                return Ok(false);
            }
            if event_type == "usage.updated" {
                if !matches!(run_status.as_str(), "running" | "cancelling") {
                    return Err(projection_violation(format!(
                        "Runtime usage.updated requires active Run '{run_id}', found '{run_status}'"
                    )));
                }
                validate_runtime_usage_update(&transaction, run_id, payload, seq)?;
            }
            if event_type == "run.completed" {
                enforce_managed_run_completion_budget(&transaction, run_id, now)?;
            }
            let (trace_id, span_id) = observability::project_runtime_span(
                &transaction,
                run_id,
                event_type,
                payload,
                now,
            )?;
            let inserted = transaction.execute(
                "INSERT OR IGNORE INTO run_events(
                    id, run_id, seq, event_type, event_json, created_at, trace_id, span_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    Uuid::new_v4().to_string(),
                    run_id,
                    seq,
                    event_type,
                    event_json,
                    now,
                    trace_id,
                    span_id,
                ],
            )?;
            if inserted == 0 {
                transaction.rollback()?;
                return Ok(false);
            }

            let assistant_message_id = format!("assistant-{run_id}");
            match event_type {
                "run.started" => {
                    let changed = transaction.execute(
                        "UPDATE runs SET status = 'running', started_at = COALESCE(started_at, ?2),
                                         finished_at = NULL, error_code = NULL, error_message = NULL,
                                         last_seq = ?3
                         WHERE id = ?1 AND status = 'queued'",
                        params![run_id, now, seq],
                    )?;
                    if changed != 1 {
                        let status: String = transaction.query_row(
                            "SELECT status FROM runs WHERE id = ?1",
                            [run_id],
                            |row| row.get(0),
                        )?;
                        return Err(projection_violation(format!(
                            "Runtime run.started cannot reopen Run '{run_id}' from authoritative status '{status}'"
                        )));
                    }
                }
                "message.started" => {
                    let run_status: String = transaction.query_row(
                        "SELECT status FROM runs WHERE id = ?1",
                        [run_id],
                        |row| row.get(0),
                    )?;
                    if run_status != "running" {
                        return Err(projection_violation(format!(
                            "Runtime message.started requires running Run '{run_id}', found '{run_status}'"
                        )));
                    }
                    let (conversation_id, ordinal): (String, i64) = transaction.query_row(
                        "SELECT conversation_id,
                                (SELECT COALESCE(MAX(ordinal), 0) + 1 FROM messages WHERE conversation_id = runs.conversation_id)
                         FROM runs WHERE id = ?1",
                        [run_id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )?;
                    transaction.execute(
                        "INSERT INTO messages(
                            id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at
                         ) VALUES (?1, ?2, ?3, 'assistant', 'text', '', 'streaming', ?4, ?5, ?5)",
                        params![assistant_message_id, conversation_id, run_id, ordinal, now],
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "message.delta" => {
                    let (run_status, message_status): (String, Option<String>) =
                        transaction.query_row(
                            "SELECT runs.status,
                                    (SELECT status FROM messages WHERE id = ?2 AND run_id = runs.id)
                             FROM runs WHERE id = ?1",
                            params![run_id, assistant_message_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )?;
                    if run_status != "running" || message_status.as_deref() != Some("streaming") {
                        return Err(projection_violation(format!(
                            "Runtime message.delta requires running Run and streaming assistant Message for '{run_id}'"
                        )));
                    }
                    let delta = payload.get("delta").and_then(Value::as_str).unwrap_or_default();
                    let changed = transaction.execute(
                        "UPDATE messages SET content = content || ?2, updated_at = ?3
                         WHERE id = ?1 AND run_id = ?4 AND status = 'streaming'",
                        params![assistant_message_id, delta, now, run_id],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Assistant Message for Run '{run_id}' changed concurrently before delta projection"
                        )));
                    }
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "message.completed" => {
                    let (run_status, message_status): (String, Option<String>) =
                        transaction.query_row(
                            "SELECT runs.status,
                                    (SELECT status FROM messages WHERE id = ?2 AND run_id = runs.id)
                             FROM runs WHERE id = ?1",
                            params![run_id, assistant_message_id],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )?;
                    if run_status != "running" {
                        return Err(projection_violation(format!(
                            "Runtime message.completed requires running Run '{run_id}', found '{run_status}'"
                        )));
                    }
                    if message_status.as_deref() == Some("completed") {
                        authorize_runtime_event_payload_replay(
                            &transaction,
                            run_id,
                            event_type,
                            payload,
                            seq,
                        )?;
                        update_last_seq(&transaction, run_id, seq)?;
                        transaction.commit()?;
                        return Ok(true);
                    }
                    if message_status.as_deref() != Some("streaming") {
                        return Err(projection_violation(format!(
                            "Runtime message.completed requires a streaming assistant Message for Run '{run_id}'"
                        )));
                    }
                    let changed = transaction.execute(
                        "UPDATE messages SET status = 'completed', updated_at = ?2
                         WHERE id = ?1 AND run_id = ?3 AND status = 'streaming'",
                        params![assistant_message_id, now, run_id],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Assistant Message for Run '{run_id}' changed concurrently before completion"
                        )));
                    }
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.started" => {
                    project_tool_started(
                        &transaction,
                        run_id,
                        payload,
                        now,
                        Some(&trace_id),
                        Some(&span_id),
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.updated" => {
                    project_tool_updated(&transaction, run_id, payload, now)?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "tool.completed" => {
                    project_tool_completed(
                        &transaction,
                        run_id,
                        payload,
                        now,
                        Some(&trace_id),
                        Some(&span_id),
                    )?;
                    update_last_seq(&transaction, run_id, seq)?;
                }
                "run.completed" => {
                    if !authorize_runtime_terminal_transition(
                        &transaction,
                        run_id,
                        "completed",
                        None,
                        None,
                        payload,
                        seq,
                    )? {
                        transaction.commit()?;
                        return Ok(true);
                    }
                    let changed = transaction.execute(
                        "UPDATE runs SET status = 'completed', finished_at = ?2,
                                         error_code = NULL, error_message = NULL, last_seq = ?3
                         WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                        params![run_id, now, seq],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Run '{run_id}' changed concurrently before completion"
                        )));
                    }
                    transaction.execute(
                        "UPDATE messages SET status = 'completed', updated_at = ?2
                         WHERE id = ?1 AND status IN ('streaming', 'interrupted')",
                        params![assistant_message_id, now],
                    )?;
                    transaction.execute(
                        "UPDATE tool_calls
                         SET status = 'interrupted',
                             error_message = 'Run completed before the ToolCall reached a terminal state',
                             completed_at = ?2, updated_at = ?2
                         WHERE run_id = ?1 AND status IN ('pending', 'running')",
                        params![run_id, now],
                    )?;
                    expire_pending_approvals_for_run(
                        &transaction,
                        run_id,
                        "run_completed",
                        now,
                    )?;
                    interrupt_tasks_owned_by_run(
                        &transaction,
                        run_id,
                        &now.to_string(),
                        "failed",
                        "Run completed before its running Attempt reached a terminal state",
                    )?;
                }
                "run.cancelled" => {
                    if !authorize_runtime_terminal_transition(
                        &transaction,
                        run_id,
                        "cancelled",
                        None,
                        None,
                        payload,
                        seq,
                    )? {
                        transaction.commit()?;
                        return Ok(true);
                    }
                    let changed = transaction.execute(
                        "UPDATE runs SET status = 'cancelled', finished_at = ?2,
                                         error_code = NULL, error_message = NULL, last_seq = ?3
                         WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                        params![run_id, now, seq],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Run '{run_id}' changed concurrently before cancellation"
                        )));
                    }
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
                    cancel_pending_approvals_for_run(
                        &transaction,
                        run_id,
                        "run_cancelled",
                        now,
                    )?;
                    interrupt_tasks_owned_by_run(
                        &transaction,
                        run_id,
                        &now.to_string(),
                        "cancelled",
                        "Run was cancelled before its Attempt finished",
                    )?;
                }
                "run.interrupted" => {
                    let code = payload.get("code").and_then(Value::as_str);
                    let message = payload.get("message").and_then(Value::as_str);
                    if !authorize_runtime_terminal_transition(
                        &transaction,
                        run_id,
                        "interrupted",
                        code,
                        message,
                        payload,
                        seq,
                    )? {
                        transaction.commit()?;
                        return Ok(true);
                    }
                    let changed = transaction.execute(
                        "UPDATE runs SET status = 'interrupted', finished_at = ?2,
                                         error_code = ?3, error_message = ?4, last_seq = ?5
                         WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                        params![run_id, now, code, message, seq],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Run '{run_id}' changed concurrently before interruption"
                        )));
                    }
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
                    expire_pending_approvals_for_run(
                        &transaction,
                        run_id,
                        "run_interrupted",
                        now,
                    )?;
                    interrupt_tasks_owned_by_run(
                        &transaction,
                        run_id,
                        &now.to_string(),
                        "failed",
                        message.unwrap_or("Run was interrupted before its Attempt finished"),
                    )?;
                }
                "run.failed" => {
                    let code = payload.get("code").and_then(Value::as_str);
                    let message = payload.get("message").and_then(Value::as_str);
                    if !authorize_runtime_terminal_transition(
                        &transaction,
                        run_id,
                        "failed",
                        code,
                        message,
                        payload,
                        seq,
                    )? {
                        transaction.commit()?;
                        return Ok(true);
                    }
                    let changed = transaction.execute(
                        "UPDATE runs SET status = 'failed', finished_at = ?2, error_code = ?3,
                                         error_message = ?4, last_seq = ?5
                         WHERE id = ?1 AND status IN ('queued', 'running', 'cancelling')",
                        params![run_id, now, code, message, seq],
                    )?;
                    if changed != 1 {
                        return Err(projection_violation(format!(
                            "Run '{run_id}' changed concurrently before failure"
                        )));
                    }
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
                    expire_pending_approvals_for_run(
                        &transaction,
                        run_id,
                        "run_failed",
                        now,
                    )?;
                    interrupt_tasks_owned_by_run(
                        &transaction,
                        run_id,
                        &now.to_string(),
                        "failed",
                        message.unwrap_or("Run failed before its Attempt finished"),
                    )?;
                }
                _ => update_last_seq(&transaction, run_id, seq)?,
            }

            child_runs::project_child_run_event(&transaction, run_id, event_type, payload, now)?;
            digital_colleagues::project_digital_colleague_event(
                &transaction,
                run_id,
                event_type,
                payload,
                now,
            )?;

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

fn interrupt_tasks_owned_by_terminal_runs(
    connection: &Connection,
    now: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE task_attempts
         SET status = CASE
                 WHEN (SELECT status FROM runs WHERE runs.id = task_attempts.run_id) = 'cancelled'
                 THEN 'cancelled' ELSE 'failed' END,
             failure_reason = CASE
                 WHEN (SELECT status FROM runs WHERE runs.id = task_attempts.run_id) = 'cancelled'
                 THEN 'Owning Run was cancelled before the Attempt finished'
                 ELSE 'Owning Run became terminal before the Attempt finished' END,
             finished_at = ?1, version = version + 1
         WHERE status = 'running' AND EXISTS (
             SELECT 1 FROM runs WHERE runs.id = task_attempts.run_id
               AND runs.status NOT IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
         )",
        [now],
    )?;
    connection.execute(
        "UPDATE work_tasks
         SET status = 'interrupted', version = version + 1,
             finished_at = COALESCE(finished_at, ?1), updated_at = ?1
         WHERE status = 'in_progress'
           AND owner_run_id IS NOT NULL
           AND EXISTS (
               SELECT 1 FROM runs
               WHERE runs.id = work_tasks.owner_run_id
                 AND runs.status NOT IN ('queued', 'running', 'cancelling')
           )",
        [now],
    )
}

fn interrupt_tasks_owned_by_run(
    connection: &Connection,
    run_id: &str,
    now: &str,
    attempt_status: &str,
    failure_reason: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE task_attempts
         SET status = ?3, failure_reason = ?4, finished_at = ?2, version = version + 1
         WHERE run_id = ?1 AND status = 'running'",
        params![run_id, now, attempt_status, failure_reason],
    )?;
    connection.execute(
        "UPDATE work_tasks
         SET status = 'interrupted', version = version + 1,
             finished_at = COALESCE(finished_at, ?2), updated_at = ?2
         WHERE owner_run_id = ?1 AND status = 'in_progress'",
        params![run_id, now],
    )
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
    interrupt_tasks_owned_by_terminal_runs(transaction, &now.to_string())?;
    let (model, current_title): (String, String) = transaction.query_row(
        "SELECT CASE WHEN a.runtime_type = 'yuxi'
                    THEN COALESCE(?2, '')
                    ELSE COALESCE(?2,
                                  (SELECT model_id FROM model_service WHERE singleton_id = 1 AND enabled = 1),
                                  a.default_model)
                END,
                c.title
         FROM conversations c JOIN agents a ON a.id = c.agent_id
         WHERE c.id = ?1 AND c.archived = 0 AND c.trashed_at IS NULL",
        params![conversation_id, requested_model],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;

    let active_run = transaction
        .query_row(
            "SELECT id FROM runs
             WHERE conversation_id = ?1 AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
             LIMIT 1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if active_run.is_some() {
        return Err(rusqlite::Error::InvalidQuery);
    }

    expire_pending_approvals_for_conversation(
        transaction,
        conversation_id,
        "new_run_started",
        now,
    )?;

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
    let (trace_id, root_span_id) = observability::initialize_run_trace(transaction, &run_id, now)?;
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
            trace_id: Some(trace_id),
            root_span_id: Some(root_span_id),
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

fn expire_pending_approvals_for_run(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    reason: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    let decision_json = json!({ "reason": reason }).to_string();
    transaction.execute(
        "UPDATE approvals
         SET status = 'expired', decision_json = ?2, resolved_at = ?3
         WHERE status = 'pending'
           AND tool_call_id IN (SELECT id FROM tool_calls WHERE run_id = ?1)",
        params![run_id, decision_json, now],
    )
}

fn cancel_pending_approvals_for_run(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    reason: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    let decision_json = json!({ "reason": reason }).to_string();
    transaction.execute(
        "UPDATE approvals
         SET status = 'cancelled', decision_json = ?2, resolved_at = ?3
         WHERE status = 'pending'
           AND tool_call_id IN (SELECT id FROM tool_calls WHERE run_id = ?1)",
        params![run_id, decision_json, now],
    )
}

fn expire_pending_approvals_for_conversation(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    reason: &str,
    now: i64,
) -> rusqlite::Result<usize> {
    let decision_json = json!({ "reason": reason }).to_string();
    transaction.execute(
        "UPDATE approvals
         SET status = 'expired', decision_json = ?2, resolved_at = ?3
         WHERE status = 'pending'
           AND tool_call_id IN (
               SELECT t.id
               FROM tool_calls t JOIN runs r ON r.id = t.run_id
               WHERE r.conversation_id = ?1
           )",
        params![conversation_id, decision_json, now],
    )
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
        pinned: row.get(7)?,
        archived: row.get(8)?,
        archived_at: row.get(9)?,
        trashed_at: row.get(10)?,
        parent_conversation_id: row.get(11)?,
        forked_from_message_id: row.get(12)?,
        lineage_root_id: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
        last_message_at: row.get(16)?,
        permission_mode: row.get(17)?,
    })
}

impl From<rusqlite::Error> for ConversationExpertBindingError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.to_string())
    }
}

pub(crate) fn query_agent_record(
    connection: &Connection,
    agent_id: &str,
) -> rusqlite::Result<Option<AgentRecord>> {
    let sql = format!("SELECT {AGENT_RECORD_COLUMNS} FROM agents WHERE id = ?1");
    connection
        .query_row(&sql, [agent_id], agent_record_from_row)
        .optional()
}

fn agent_record_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentRecord> {
    let id: String = row.get(0)?;
    let runtime_type: String = row.get(3)?;
    let system_prompt: String = row.get(8)?;
    let metadata = if runtime_type == "yuxi" {
        parse_json(&system_prompt)
    } else {
        json!({})
    };
    Ok(AgentRecord {
        id: id.clone(),
        name: row.get(1)?,
        description: row.get(2)?,
        runtime_type: runtime_type.clone(),
        agent_kind: row.get(4)?,
        invocation_mode: row.get(5)?,
        visibility: row.get(6)?,
        default_model: row.get(7)?,
        icon: row.get::<_, Option<String>>(9)?.or_else(|| {
            metadata
                .get("icon")
                .and_then(Value::as_str)
                .map(str::to_owned)
        }),
        category: row.get(10)?,
        opening_suggestions: serde_json::from_str(&row.get::<_, String>(11)?).unwrap_or_default(),
        system_prompt: if runtime_type == "pi" {
            system_prompt
        } else {
            String::new()
        },
        is_builtin: row.get::<_, i64>(12)? != 0,
        package_version: row.get(13)?,
        package_manifest: parse_json(&row.get::<_, String>(14)?),
        package_source: row.get(15)?,
        package_id: row.get(16)?,
        package_hash: row.get(17)?,
        capabilities: metadata.get("capabilities").cloned().unwrap_or_else(|| {
            if runtime_type == "pi" {
                json!(["files", "tools", "reasoning"])
            } else {
                json!([])
            }
        }),
        resources: metadata
            .get("resources")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_else(|| {
                if runtime_type == "pi" {
                    AgentResourcesRecord {
                        tools: vec![
                            AgentResourceRecord {
                                id: "files".to_owned(),
                                name: "文件读写".to_owned(),
                                description: "读取和处理已授权项目文件".to_owned(),
                            },
                            AgentResourceRecord {
                                id: "tools".to_owned(),
                                name: "本地工具".to_owned(),
                                description: "调用 Fox Runtime 提供的本地工具".to_owned(),
                            },
                            AgentResourceRecord {
                                id: "reasoning".to_owned(),
                                name: "任务推理".to_owned(),
                                description: "规划并执行多步骤任务".to_owned(),
                            },
                        ],
                        ..AgentResourcesRecord::default()
                    }
                } else {
                    AgentResourcesRecord::default()
                }
            }),
        configurable_items: metadata
            .get("configurableItems")
            .cloned()
            .unwrap_or_else(|| json!({})),
        is_default: id == DEFAULT_AGENT_ID
            || metadata
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        available: runtime_type != "yuxi"
            || metadata
                .get("remoteAvailable")
                .and_then(Value::as_bool)
                .unwrap_or(true),
    })
}

pub(crate) fn agent_package_snapshot(agent: &AgentRecord) -> Value {
    json!({
        "id": agent.id,
        "name": agent.name,
        "description": agent.description,
        "runtimeType": agent.runtime_type,
        "agentKind": agent.agent_kind,
        "invocationMode": agent.invocation_mode,
        "visibility": agent.visibility,
        "defaultModel": agent.default_model,
        "systemPrompt": if agent.runtime_type == "pi" {
            Value::String(agent.system_prompt.clone())
        } else {
            Value::Null
        },
        "category": agent.category,
        "isBuiltin": agent.is_builtin,
        "openingSuggestions": agent.opening_suggestions,
        "packageVersion": agent.package_version,
        "packageManifest": agent.package_manifest,
        "capabilities": agent.capabilities,
        "resources": agent.resources,
    })
}

pub(crate) fn package_snapshot_hash(package_snapshot: &Value) -> String {
    hex::encode(Sha256::digest(package_snapshot.to_string().as_bytes()))
}

fn conversation_expert_binding_lock_reason(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<ConversationExpertBindingError>> {
    let archived = connection.query_row(
        "SELECT archived != 0 OR trashed_at IS NOT NULL FROM conversations WHERE id = ?1",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if archived {
        return Ok(Some(ConversationExpertBindingError::LockedArchived));
    }
    let has_messages = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM messages
            WHERE conversation_id = ?1 AND role IN ('user', 'assistant')
         )",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if has_messages {
        return Ok(Some(ConversationExpertBindingError::LockedByMessages));
    }
    let has_active_run = connection.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM runs
            WHERE conversation_id = ?1
              AND status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
         )",
        [conversation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if has_active_run {
        return Ok(Some(ConversationExpertBindingError::LockedByActiveRun));
    }
    Ok(None)
}

fn query_conversation_expert_bindings(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<ConversationExpertBinding>> {
    let mut statement = connection.prepare(
        "SELECT id, conversation_id, expert_id, state, activation_source, expert_version,
                package_hash, package_snapshot_json, display_snapshot_json, activated_at,
                deactivated_at
         FROM conversation_expert_bindings
         WHERE conversation_id = ?1
         ORDER BY activated_at ASC, rowid ASC",
    )?;
    let bindings = statement
        .query_map([conversation_id], conversation_expert_binding_from_row)?
        .collect();
    bindings
}

fn query_conversation_expert_binding_by_state(
    connection: &Connection,
    conversation_id: &str,
    state: &str,
) -> rusqlite::Result<ConversationExpertBinding> {
    connection.query_row(
        "SELECT id, conversation_id, expert_id, state, activation_source, expert_version,
                package_hash, package_snapshot_json, display_snapshot_json, activated_at,
                deactivated_at
         FROM conversation_expert_bindings
         WHERE conversation_id = ?1 AND state = ?2
         ORDER BY activated_at DESC, rowid DESC LIMIT 1",
        params![conversation_id, state],
        conversation_expert_binding_from_row,
    )
}

fn query_conversation_expert_binding(
    connection: &Connection,
    id: &str,
) -> rusqlite::Result<ConversationExpertBinding> {
    connection.query_row(
        "SELECT id, conversation_id, expert_id, state, activation_source, expert_version,
                package_hash, package_snapshot_json, display_snapshot_json, activated_at,
                deactivated_at
         FROM conversation_expert_bindings WHERE id = ?1",
        [id],
        conversation_expert_binding_from_row,
    )
}

fn conversation_expert_binding_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ConversationExpertBinding> {
    Ok(ConversationExpertBinding {
        id: row.get(0)?,
        conversation_id: row.get(1)?,
        expert_id: row.get(2)?,
        state: row.get(3)?,
        activation_source: row.get(4)?,
        expert_version: row.get(5)?,
        package_hash: row.get(6)?,
        package_snapshot: parse_json(&row.get::<_, String>(7)?),
        display_snapshot_json: parse_json(&row.get::<_, String>(8)?),
        activated_at: row.get(9)?,
        deactivated_at: row.get(10)?,
    })
}

fn query_conversation(connection: &Connection, id: &str) -> rusqlite::Result<ConversationSummary> {
    connection.query_row(
        "SELECT c.id, c.agent_id, a.name, c.title, c.project_id,
                COALESCE(p.root_path, c.project_root), c.status,
                c.pinned, c.archived, c.archived_at, c.trashed_at,
                c.parent_conversation_id, c.forked_from_message_id,
                COALESCE(c.lineage_root_id, c.id),
                c.created_at, c.updated_at, c.last_message_at,
                COALESCE(kep.mode, c.permission_mode, 'ask')
         FROM conversations c
         JOIN agents a ON a.id = c.agent_id
         LEFT JOIN projects p ON p.id = c.project_id
         LEFT JOIN kernel_execution_policies kep ON kep.conversation_id = c.id
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
             AND status <> 'superseded'
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
               'run.started', 'run.request_snapshot', 'run.phase',
               'run.retrying', 'run.retry.completed',
               'context.compaction.started', 'context.compaction.completed',
               'planner.started', 'planner.completed', 'planner.failed',
               'message.started', 'reasoning.delta',
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

/// Text of a stored tool result, plus whether the row only holds a Fox preview
/// and what the true original size was.
fn stored_result_text(result: Option<&Value>) -> (String, bool, usize) {
    match result {
        Some(Value::Object(map)) if map.get("truncated").and_then(Value::as_bool) == Some(true) => {
            let preview = map
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let original = map
                .get("originalBytes")
                .and_then(Value::as_u64)
                .unwrap_or(preview.len() as u64) as usize;
            (preview, true, original)
        }
        Some(Value::String(text)) => (text.clone(), false, text.len()),
        Some(Value::Array(blocks)) => {
            let joined = joined_tool_text_blocks(blocks);
            let length = joined.len();
            (joined, false, length)
        }
        // A settled tool result is usually `{"content":[{"type":"text",...}]}`.
        // Joining its text blocks is the same projection the model-view binder
        // works on, so a caller walking ranges reassembles exactly that text
        // instead of a serialized-and-escaped copy of the envelope.
        Some(Value::Object(map)) if map.get("content").and_then(Value::as_array).is_some() => {
            let joined = joined_tool_text_blocks(
                map.get("content")
                    .and_then(Value::as_array)
                    .map_or(&[] as &[Value], |blocks| blocks),
            );
            let length = joined.len();
            (joined, false, length)
        }
        Some(other) => {
            let text = other.to_string();
            let length = text.len();
            (text, false, length)
        }
        None => (String::new(), false, 0),
    }
}

/// The text a range read is served from: every `text` block of a result content
/// array, joined with a newline. Empty blocks contribute nothing extra.
fn joined_tool_text_blocks(blocks: &[Value]) -> String {
    blocks
        .iter()
        .filter_map(|block| block["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Smallest range a caller may request.
///
/// UTF-8 encodes one code point in at most four bytes, so a limit at least this
/// large always contains a whole code point past any valid start. That is what
/// makes a non-terminal `next` strictly larger than its own `start`: a smaller
/// limit could only be served by an empty slice, and a client walking `next`
/// would loop forever on it. Such limits are rejected instead of quietly
/// answered with nothing.
const MIN_TOOL_RESULT_RANGE_BYTES: usize = 4;

/// Clamp `[offset, offset + limit)` into `text` without splitting a UTF-8 code
/// point, returning `(start, end, next)` where `next` continues past `end`.
///
/// Range contract — deliberately explicit rather than forgiving, because silent
/// adjustment is what lets bytes go missing or get returned twice:
///
/// - `limit` below [`MIN_TOOL_RESULT_RANGE_BYTES`] is **rejected**.
/// - `offset` past the end, or landing inside a code point, is **rejected**;
///   the error names the nearest valid boundaries so callers can recover.
/// - Everything else returns exactly `[offset, end)` with `end` on a code point
///   boundary, so successive ranges neither drop nor repeat a byte, and `next`
///   is either `None` (the result ended) or strictly greater than `offset`.
fn utf8_byte_range(
    text: &str,
    offset: usize,
    limit: usize,
) -> Result<(usize, usize, Option<usize>), String> {
    let length = text.len();
    if limit < MIN_TOOL_RESULT_RANGE_BYTES {
        return Err(format!(
            "tool result range limit must be at least {MIN_TOOL_RESULT_RANGE_BYTES} bytes so every UTF-8 code point is returned whole; got {limit}"
        ));
    }
    let limit = limit.min(MAX_TOOL_RESULT_RANGE_BYTES);
    if offset > length {
        return Err(format!(
            "tool result range offset {offset} is past the end of the stored result ({length} bytes)"
        ));
    }
    if offset < length && !text.is_char_boundary(offset) {
        let previous = (1..=3)
            .map(|step| offset.saturating_sub(step))
            .find(|index| text.is_char_boundary(*index))
            .unwrap_or(0);
        return Err(match (offset..=length).find(|index| text.is_char_boundary(*index)) {
            Some(next) => format!(
                "tool result range offset {offset} splits a UTF-8 code point; use {previous} or {next}"
            ),
            None => format!(
                "tool result range offset {offset} splits a UTF-8 code point; use {previous}"
            ),
        });
    }
    // `offset` is a code point boundary and `limit >= MIN_TOOL_RESULT_RANGE_BYTES`,
    // so the code point starting at `offset` ends at a boundary no more than four
    // bytes later and inside the text: `end > offset` whenever the caller is not
    // already at the end. A non-terminal cursor therefore always advances.
    let mut end = offset.saturating_add(limit).min(length);
    while end > offset && !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((offset, end, (end < length).then_some(end)))
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
                a.requested_at, a.resolved_at, a.category, a.claimed_at,
                a.claimed_by_run_id
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
        "SELECT id, conversation_id, run_id, display_name, artifact_type,
                artifact_class, artifact_origin, storage_path,
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
                    artifact_class: row.get(5)?,
                    artifact_origin: row.get(6)?,
                    storage_path: row.get(7)?,
                    media_type: row.get(8)?,
                    byte_size: row.get(9)?,
                    sha256: row.get(10)?,
                    status: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
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
    let Some(bindings) = query_knowledge_reference_bindings_v2(connection, conversation_id)? else {
        return query_legacy_knowledge_bindings(connection, conversation_id);
    };
    Ok(bindings
        .into_iter()
        .filter_map(legacy_knowledge_binding_record)
        .collect())
}

fn query_knowledge_reference_bindings(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<KnowledgeReferenceBindingRecord>> {
    if let Some(bindings) = query_knowledge_reference_bindings_v2(connection, conversation_id)? {
        return Ok(bindings);
    }
    query_legacy_knowledge_reference_bindings(connection, conversation_id)
}

fn query_knowledge_reference_bindings_v2(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Option<Vec<KnowledgeReferenceBindingRecord>>> {
    let mut statement = match connection.prepare(
        "SELECT conversation_id, source, provider_key, connection_id,
                knowledge_base_id, knowledge_base_name, enabled, created_at, updated_at
         FROM knowledge_bindings_v2
         WHERE conversation_id = ?1 AND enabled = 1
         ORDER BY knowledge_base_name ASC, knowledge_base_id ASC",
    ) {
        Ok(statement) => statement,
        Err(error) if missing_knowledge_bindings_v2(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let records = statement
        .query_map([conversation_id], |row| {
            Ok(KnowledgeReferenceBindingRecord {
                conversation_id: row.get(0)?,
                reference: KnowledgeReference {
                    source: row.get(1)?,
                    provider_key: row.get(2)?,
                    connection_id: row.get(3)?,
                    id: row.get(4)?,
                },
                knowledge_base_name: row.get(5)?,
                enabled: row.get::<_, i64>(6)? != 0,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Some(records))
}

fn query_legacy_knowledge_reference_bindings(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<KnowledgeReferenceBindingRecord>> {
    let mut statement = connection.prepare(
        "SELECT conversation_id, service_connection_id, knowledge_base_id,
                knowledge_base_name, enabled, created_at, updated_at
         FROM knowledge_bindings WHERE conversation_id = ?1 AND enabled = 1
         ORDER BY knowledge_base_name ASC, knowledge_base_id ASC",
    )?;
    let records = statement
        .query_map([conversation_id], |row| {
            let service_connection_id: String = row.get(1)?;
            let knowledge_base_id: String = row.get(2)?;
            Ok(KnowledgeReferenceBindingRecord {
                conversation_id: row.get(0)?,
                reference: KnowledgeReference::remote(service_connection_id, knowledge_base_id),
                knowledge_base_name: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

fn query_legacy_knowledge_bindings(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<KnowledgeBindingRecord>> {
    let mut statement = connection.prepare(
        "SELECT conversation_id, service_connection_id, knowledge_base_id,
                knowledge_base_name, enabled, created_at, updated_at
         FROM knowledge_bindings WHERE conversation_id = ?1 AND enabled = 1
         ORDER BY knowledge_base_name ASC, knowledge_base_id ASC",
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
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(records)
}

fn legacy_knowledge_binding_record(
    binding: KnowledgeReferenceBindingRecord,
) -> Option<KnowledgeBindingRecord> {
    let service_connection_id = binding.reference.connection_id?;
    Some(KnowledgeBindingRecord {
        conversation_id: binding.conversation_id,
        service_connection_id,
        knowledge_base_id: binding.reference.id,
        knowledge_base_name: binding.knowledge_base_name,
        enabled: binding.enabled,
        created_at: binding.created_at,
        updated_at: binding.updated_at,
    })
}

fn knowledge_binding_v2_id(conversation_id: &str, reference: &KnowledgeReference) -> String {
    format!(
        "v2:{conversation_id}:{}:{}:{}",
        reference.source, reference.provider_key, reference.id
    )
}

fn missing_knowledge_bindings_v2(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(_, Some(message))
            if message.contains("no such table: knowledge_bindings_v2")
    )
}

fn query_approval_by_tool_call(
    connection: &Connection,
    tool_call_id: &str,
) -> rusqlite::Result<super::ApprovalRecord> {
    connection.query_row(
        "SELECT a.id, a.tool_call_id, t.run_id, t.conversation_id, t.tool_name,
                a.status, a.requested_action, a.request_json, a.decision_json,
                a.requested_at, a.resolved_at, a.category, a.claimed_at,
                a.claimed_by_run_id
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
                a.requested_at, a.resolved_at, a.category, a.claimed_at,
                a.claimed_by_run_id
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
        category: row.get(11)?,
        claimed_at: row.get(12)?,
        claimed_by_run_id: row.get(13)?,
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
    trace_id: Option<&str>,
    span_id: Option<&str>,
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
    let (conversation_id, run_status): (String, String) = transaction.query_row(
        "SELECT conversation_id, status FROM runs WHERE id = ?1",
        [run_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let existing = transaction
        .query_row(
            "SELECT conversation_id, tool_name, input_json, execution_location, status
             FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
            params![run_id, runtime_tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    if existing.is_none() {
        if run_status != "running" {
            return Err(projection_violation(format!(
                "Runtime tool.started cannot create ToolCall '{runtime_tool_call_id}' while Run '{run_id}' is '{run_status}'"
            )));
        }
        validate_managed_tool_acquisition(transaction, run_id, now, true)?;
        transaction.execute(
            "INSERT INTO tool_calls(
                id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                status, execution_location, requires_approval, started_at, updated_at,
                trace_id, span_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', 'runtime', 0, ?7, ?7, ?8, ?9)
            ",
            params![
                Uuid::new_v4().to_string(),
                runtime_tool_call_id,
                run_id,
                conversation_id,
                tool_name,
                input_json,
                now,
                trace_id,
                span_id,
            ],
        )?;
    }
    let existing = existing.map(Ok).unwrap_or_else(|| {
        transaction.query_row(
            "SELECT conversation_id, tool_name, input_json, execution_location, status
                 FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
            params![run_id, runtime_tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
    })?;
    let existing_input =
        serde_json::from_str::<Value>(&existing.2).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let incoming_input = payload.get("input").unwrap_or(&Value::Null);
    if existing.0 != conversation_id
        || existing.1 != tool_name
        || existing_input != *incoming_input
        || !matches!(existing.3.as_str(), "runtime" | "host")
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    if run_status == "running" && existing.3 == "runtime" && existing.4 == "running" {
        transaction.execute(
            "UPDATE tool_calls
             SET trace_id = COALESCE(trace_id, ?3), span_id = COALESCE(span_id, ?4)
             WHERE run_id = ?1 AND runtime_tool_call_id = ?2
               AND execution_location = 'runtime' AND status = 'running'",
            params![run_id, runtime_tool_call_id, trace_id, span_id],
        )?;
    }
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
    let (inline_result, blob) = split_tool_result(payload.get("update").unwrap_or(&Value::Null));
    let result_json = serde_json::to_string(&inline_result)
        .unwrap_or_else(|_| "null".to_owned());
    let incoming_tool_name = payload.get("tool").and_then(Value::as_str);
    if let Some((sha, full, bytes)) = &blob {
        transaction.execute(
            "INSERT INTO tool_call_result_blobs(sha256, body_json, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sha256) DO NOTHING",
            params![sha, full, bytes, now],
        )?;
    }
    let blob_sha: Option<&str> = blob.as_ref().map(|(sha, _, _)| sha.as_str());
    let changed = transaction.execute(
        "UPDATE tool_calls SET result_json = ?3, updated_at = ?4,
             result_blob_sha256 = ?6
         WHERE run_id = ?1 AND runtime_tool_call_id = ?2
           AND execution_location = 'runtime' AND status = 'running'
           AND (?5 IS NULL OR tool_name = ?5)
           AND EXISTS(
               SELECT 1 FROM runs
               WHERE runs.id = tool_calls.run_id AND runs.status = 'running'
           )",
        params![
            run_id,
            runtime_tool_call_id,
            result_json,
            now,
            incoming_tool_name,
            blob_sha,
        ],
    )?;
    if changed == 0 {
        let existing = query_tool_call(transaction, run_id, runtime_tool_call_id)?;
        let reason = if incoming_tool_name.is_some_and(|tool| tool != existing.tool_name) {
            format!("Runtime update tool name conflicts with ToolCall '{runtime_tool_call_id}'")
        } else if existing.execution_location == "host" {
            // Pi emits lifecycle events around Host callbacks too. Keep the raw RunEvent for
            // diagnostics, but never let a Runtime update overwrite the Host-owned ToolCall.
            return Ok(());
        } else {
            format!(
                "Runtime update cannot mutate ToolCall '{runtime_tool_call_id}' in terminal status '{}'",
                existing.status
            )
        };
        return Err(projection_violation(reason));
    }
    Ok(())
}

fn project_tool_completed(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    payload: &Value,
    now: i64,
    trace_id: Option<&str>,
    span_id: Option<&str>,
) -> rusqlite::Result<()> {
    let Some(runtime_tool_call_id) = payload.get("toolCallId").and_then(Value::as_str) else {
        return Ok(());
    };
    let (inline_result, blob) = split_tool_result(payload.get("result").unwrap_or(&Value::Null));
    let result_json = serde_json::to_string(&inline_result).unwrap_or_else(|_| "null".to_owned());
    let is_error = payload
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let incoming_tool_name = payload.get("tool").and_then(Value::as_str);
    let terminal_status = if is_error { "failed" } else { "completed" };
    if let Some((sha, full, bytes)) = &blob {
        transaction.execute(
            "INSERT INTO tool_call_result_blobs(sha256, body_json, byte_size, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(sha256) DO NOTHING",
            params![sha, full, bytes, now],
        )?;
    }
    let blob_sha: Option<&str> = blob.as_ref().map(|(sha, _, _)| sha.as_str());
    let changed = transaction.execute(
        "UPDATE tool_calls
         SET status = ?3, result_json = ?4, completed_at = ?5, updated_at = ?5,
             trace_id = COALESCE(trace_id, ?6), span_id = COALESCE(span_id, ?7),
             result_blob_sha256 = ?9
         WHERE run_id = ?1 AND runtime_tool_call_id = ?2
           AND execution_location = 'runtime' AND status = 'running'
           AND (?8 IS NULL OR tool_name = ?8)
           AND EXISTS(
               SELECT 1 FROM runs
               WHERE runs.id = tool_calls.run_id AND runs.status = 'running'
           )",
        params![
            run_id,
            runtime_tool_call_id,
            terminal_status,
            result_json,
            now,
            trace_id,
            span_id,
            incoming_tool_name,
            blob_sha,
        ],
    )?;
    if changed == 0 {
        let existing = query_tool_call(transaction, run_id, runtime_tool_call_id)?;
        if incoming_tool_name.is_some_and(|tool| tool != existing.tool_name) {
            return Err(projection_violation(format!(
                "Runtime completion tool name conflicts with ToolCall '{runtime_tool_call_id}'"
            )));
        }
        if existing.execution_location == "host" {
            // The Host response is already authoritative. A matching Runtime lifecycle event is
            // an acknowledged no-op, regardless of the Runtime's echoed result payload.
            return Ok(());
        }
        let incoming_result = parse_json(&result_json);
        if existing.status == terminal_status
            && existing.result.as_ref() == Some(&incoming_result)
            && existing.error_message.is_none()
        {
            return Ok(());
        }
        return Err(projection_violation(format!(
            "Runtime completion for ToolCall '{runtime_tool_call_id}' conflicts with terminal status '{}'",
            existing.status
        )));
    }
    Ok(())
}

fn authorize_runtime_terminal_transition(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    terminal_status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
    payload: &Value,
    seq: i64,
) -> rusqlite::Result<bool> {
    let (current_status, current_code, current_message): (String, Option<String>, Option<String>) =
        transaction.query_row(
            "SELECT status, error_code, error_message FROM runs WHERE id = ?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
    if current_status == terminal_status {
        let same_content = match terminal_status {
            "completed" | "cancelled" => {
                error_code.is_none()
                    && error_message.is_none()
                    && current_code.is_none()
                    && current_message.is_none()
            }
            "failed" | "interrupted" => {
                current_code.as_deref() == error_code && current_message.as_deref() == error_message
            }
            _ => false,
        };
        if !same_content {
            return Err(projection_violation(format!(
                "Runtime terminal replay for Run '{run_id}' conflicts with the frozen '{terminal_status}' outcome"
            )));
        }
        authorize_runtime_event_payload_replay(
            transaction,
            run_id,
            &format!("run.{terminal_status}"),
            payload,
            seq,
        )?;
        transaction.execute(
            "UPDATE runs SET last_seq = ?2 WHERE id = ?1 AND status = ?3",
            params![run_id, seq, terminal_status],
        )?;
        return Ok(false);
    }
    if matches!(
        current_status.as_str(),
        "completed" | "failed" | "cancelled" | "interrupted"
    ) {
        return Err(projection_violation(format!(
            "Runtime cannot replace terminal Run '{run_id}' status '{current_status}' with '{terminal_status}'"
        )));
    }
    if !matches!(current_status.as_str(), "queued" | "running" | "cancelling") {
        return Err(projection_violation(format!(
            "Runtime cannot transition Run '{run_id}' from '{current_status}' to '{terminal_status}'"
        )));
    }
    Ok(true)
}

fn authorize_runtime_event_payload_replay(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    event_type: &str,
    payload: &Value,
    seq: i64,
) -> rusqlite::Result<()> {
    let frozen_json = transaction
        .query_row(
            "SELECT event_json FROM run_events
             WHERE run_id = ?1 AND event_type = ?2 AND seq < ?3
             ORDER BY seq ASC LIMIT 1",
            params![run_id, event_type, seq],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| {
            projection_violation(format!(
                "Runtime replay for '{event_type}' on Run '{run_id}' has no frozen prior payload"
            ))
        })?;
    let frozen_payload = serde_json::from_str::<Value>(&frozen_json).map_err(|_| {
        projection_violation(format!(
            "Frozen '{event_type}' payload for Run '{run_id}' is invalid"
        ))
    })?;
    if frozen_payload != *payload {
        return Err(projection_violation(format!(
            "Runtime replay for '{event_type}' on Run '{run_id}' conflicts with its frozen canonical payload"
        )));
    }
    Ok(())
}

const MAX_RUNTIME_USAGE_COUNTER: i64 = 1_000_000_000_000;

fn validate_runtime_usage_update(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    payload: &Value,
    seq: i64,
) -> rusqlite::Result<()> {
    let incoming = runtime_usage_counters(payload)?;
    let previous_json = transaction
        .query_row(
            "SELECT event_json FROM run_events
             WHERE run_id = ?1 AND event_type = 'usage.updated' AND seq < ?2
             ORDER BY seq DESC LIMIT 1",
            params![run_id, seq],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(previous_json) = previous_json {
        let previous_payload = serde_json::from_str::<Value>(&previous_json).map_err(|_| {
            projection_violation(format!(
                "Frozen usage.updated payload for Run '{run_id}' is invalid"
            ))
        })?;
        let previous = runtime_usage_counters(&previous_payload)?;
        if incoming
            .iter()
            .zip(previous.iter())
            .any(|(next, prior)| next < prior)
        {
            return Err(projection_violation(format!(
                "Runtime cumulative usage counters cannot decrease for Run '{run_id}'"
            )));
        }
    }
    Ok(())
}

fn runtime_usage_counters(payload: &Value) -> rusqlite::Result<[i64; 5]> {
    let object = payload.as_object().ok_or_else(|| {
        projection_violation("usage.updated payload must be an object".to_owned())
    })?;
    const REQUIRED_FIELDS: [&str; 6] = [
        "type",
        "inputTokens",
        "outputTokens",
        "cacheReadTokens",
        "cacheWriteTokens",
        "totalTokens",
    ];
    if object.len() != REQUIRED_FIELDS.len()
        || REQUIRED_FIELDS
            .iter()
            .any(|field| !object.contains_key(*field))
    {
        return Err(projection_violation(
            "usage.updated must match the exact run_cumulative_v1 schema".to_owned(),
        ));
    }
    let counter = |field: &str| -> rusqlite::Result<i64> {
        object
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| (0..=MAX_RUNTIME_USAGE_COUNTER).contains(value))
            .ok_or_else(|| {
                projection_violation(format!(
                    "usage.updated field '{field}' must be a bounded non-negative integer"
                ))
            })
    };
    let input = counter("inputTokens")?;
    let output = counter("outputTokens")?;
    let cache_read = counter("cacheReadTokens")?;
    let cache_write = counter("cacheWriteTokens")?;
    let total = counter("totalTokens")?;
    let component_total = input
        .saturating_add(output)
        .saturating_add(cache_read)
        .saturating_add(cache_write);
    if total < component_total {
        return Err(projection_violation(
            "usage.updated totalTokens cannot be lower than its cumulative components".to_owned(),
        ));
    }
    Ok([input, output, cache_read, cache_write, total])
}

fn projection_violation(reason: String) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(reason)
}

fn validate_managed_tool_acquisition(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    now: i64,
    fresh_slot: bool,
) -> rusqlite::Result<()> {
    validate_managed_tool_acquisition_with_clock(transaction,run_id,now,fresh_slot,true)
}

fn validate_managed_tool_acquisition_with_clock(
    transaction: &rusqlite::Transaction<'_>, run_id: &str, now: i64, fresh_slot: bool, wall_clock: bool,
) -> rusqlite::Result<()> {
    let run_status: String =
        transaction.query_row("SELECT status FROM runs WHERE id = ?1", [run_id], |row| {
            row.get(0)
        })?;
    if run_status != "running" {
        return Err(projection_violation(format!(
            "Managed ToolCall acquisition requires running Run '{run_id}', found '{run_status}'"
        )));
    }
    let child_duration = transaction
        .query_row(
            "SELECT r.started_at, d.max_duration_ms
             FROM child_run_delegations d
             JOIN runs r ON r.id = d.child_run_id
             WHERE d.child_run_id = ?1",
            [run_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((started_at, max_duration)) = child_duration {
        if wall_clock { enforce_completion_deadline(run_id, "child_run", started_at, max_duration, now)?; }
        return Ok(());
    }

    let digital_budget = transaction
        .query_row(
            "SELECT r.started_at, r.budget_json, t.colleague_id,
                    t.total_tokens, t.output_tokens,
                    (SELECT COUNT(*) FROM tool_calls calls WHERE calls.run_id = t.run_id)
             FROM digital_colleague_triggers t
             JOIN runs r ON r.id = t.run_id
             WHERE t.run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((started_at, budget_json, colleague_id, total, output, used_tools)) = digital_budget
    else {
        return Ok(());
    };
    let budget: Value = serde_json::from_str(&budget_json).map_err(|_| {
        projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen budget"
        ))
    })?;
    if budget.get("contract").and_then(Value::as_str) != Some("digital_colleague_run_budget_v1") {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' is missing its frozen budget contract"
        )));
    }
    let positive_counter = |field: &str| -> rusqlite::Result<i64> {
        budget
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                projection_violation(format!(
                    "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen '{field}'"
                ))
            })
    };
    let max_duration = positive_counter("maxDurationMs")?;
    let max_total = positive_counter("maxTotalTokens")?;
    let max_output = positive_counter("maxOutputTokens")?;
    let max_daily = positive_counter("maxDailyTokens")?;
    let max_tools = budget
        .get("maxToolCalls")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            projection_violation(format!(
                "[digital_colleague.tool_budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen 'maxToolCalls'"
            ))
        })?;
    let day_start = budget
        .get("dailyWindowStartMs")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            projection_violation(format!(
                "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen daily window"
            ))
        })?;
    if wall_clock { enforce_completion_deadline(run_id, "digital_colleague", started_at, max_duration, now)?; }
    if total >= max_total {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen per-Run total-token budget ({total}/{max_total})"
        )));
    }
    if output >= max_output {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen per-Run output-token budget ({output}/{max_output})"
        )));
    }
    let daily_total: i64 = transaction.query_row(
        "SELECT COALESCE(SUM(total_tokens), 0)
         FROM digital_colleague_triggers
         WHERE colleague_id = ?1 AND created_at >= ?2
           AND status NOT IN ('rejected', 'skipped')",
        params![colleague_id, day_start],
        |row| row.get(0),
    )?;
    if daily_total >= max_daily {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen daily token budget ({daily_total}/{max_daily})"
        )));
    }
    if (fresh_slot && used_tools >= max_tools) || used_tools > max_tools {
        return Err(projection_violation(format!(
            "[digital_colleague.tool_budget_exceeded] Digital colleague Run '{run_id}' used {used_tools} of {max_tools} frozen tool slots"
        )));
    }
    Ok(())
}

fn enforce_managed_run_completion_budget(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
    now: i64,
) -> rusqlite::Result<()> {
    let child_duration = transaction
        .query_row(
            "SELECT r.started_at, d.max_duration_ms
             FROM child_run_delegations d
             JOIN runs r ON r.id = d.child_run_id
             WHERE d.child_run_id = ?1",
            [run_id],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    if let Some((started_at, max_duration)) = child_duration {
        enforce_completion_deadline(run_id, "child_run", started_at, max_duration, now)?;
        return Ok(());
    }

    let digital_budget = transaction
        .query_row(
            "SELECT r.started_at, r.budget_json, t.colleague_id,
                    t.total_tokens, t.output_tokens,
                    (SELECT COUNT(*) FROM tool_calls calls WHERE calls.run_id = t.run_id)
             FROM digital_colleague_triggers t
             JOIN runs r ON r.id = t.run_id
             WHERE t.run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            },
        )
        .optional()?;
    let Some((started_at, budget_json, colleague_id, total, output, used_tools)) = digital_budget
    else {
        return Ok(());
    };
    let budget: Value = serde_json::from_str(&budget_json).map_err(|_| {
        projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen budget"
        ))
    })?;
    if budget.get("contract").and_then(Value::as_str) != Some("digital_colleague_run_budget_v1") {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' is missing its frozen budget contract"
        )));
    }
    let frozen_counter = |field: &str| -> rusqlite::Result<i64> {
        budget
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
            .ok_or_else(|| {
                projection_violation(format!(
                    "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen '{field}'"
                ))
            })
    };
    let max_duration = frozen_counter("maxDurationMs")?;
    let max_total = frozen_counter("maxTotalTokens")?;
    let max_output = frozen_counter("maxOutputTokens")?;
    let max_tools = budget
        .get("maxToolCalls")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            projection_violation(format!(
                "[digital_colleague.tool_budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen 'maxToolCalls'"
            ))
        })?;
    let max_daily = frozen_counter("maxDailyTokens")?;
    let day_start = budget
        .get("dailyWindowStartMs")
        .and_then(Value::as_i64)
        .filter(|value| *value >= 0)
        .ok_or_else(|| {
            projection_violation(format!(
                "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' has an invalid frozen daily window"
            ))
        })?;
    enforce_completion_deadline(run_id, "digital_colleague", started_at, max_duration, now)?;
    if total >= max_total {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen per-Run total-token budget ({total}/{max_total})"
        )));
    }
    if output >= max_output {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen per-Run output-token budget ({output}/{max_output})"
        )));
    }
    if used_tools > max_tools {
        return Err(projection_violation(format!(
            "[digital_colleague.tool_budget_exceeded] Digital colleague Run '{run_id}' exceeded its frozen tool-call budget ({used_tools}/{max_tools})"
        )));
    }
    let daily_total: i64 = transaction.query_row(
        "SELECT COALESCE(SUM(total_tokens), 0)
         FROM digital_colleague_triggers
         WHERE colleague_id = ?1 AND created_at >= ?2
           AND status NOT IN ('rejected', 'skipped')",
        params![colleague_id, day_start],
        |row| row.get(0),
    )?;
    if daily_total >= max_daily {
        return Err(projection_violation(format!(
            "[digital_colleague.budget_exceeded] Digital colleague Run '{run_id}' reached its frozen daily token budget ({daily_total}/{max_daily})"
        )));
    }
    Ok(())
}

fn enforce_completion_deadline(
    run_id: &str,
    budget_owner: &str,
    started_at: Option<i64>,
    max_duration_ms: i64,
    now: i64,
) -> rusqlite::Result<()> {
    let code = if budget_owner == "child_run" {
        "child_run.budget_exceeded"
    } else {
        "digital_colleague.budget_exceeded"
    };
    let started_at = started_at.ok_or_else(|| {
        projection_violation(format!(
            "[{code}] Managed Run '{run_id}' cannot complete before its authoritative start"
        ))
    })?;
    let deadline = started_at.checked_add(max_duration_ms).ok_or_else(|| {
        projection_violation(format!(
            "[{code}] Managed Run '{run_id}' has an overflowing frozen duration budget"
        ))
    })?;
    if max_duration_ms <= 0 || started_at < 0 || now < started_at {
        return Err(projection_violation(format!(
            "[{code}] Managed Run '{run_id}' has an invalid authoritative duration boundary"
        )));
    }
    if now >= deadline {
        return Err(projection_violation(format!(
            "[{code}] Managed Run '{run_id}' reached its frozen duration deadline"
        )));
    }
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
        "message": "Large tool result is stored by Fox; use read_tool_result with this call's reference to page through every byte."
    })
}

/// Split a settled tool result into its inline preview and, when it exceeds the
/// inline cap, the content-addressed full copy kept in `tool_call_result_blobs`.
/// Returns (inline_json, Option<(sha256, full_json, byte_size)>). The inline row
/// gains no size change: the model-visible shape stays exactly the compact
/// preview, while every byte of the original remains recoverable through
/// `tool_result_range`. Serialization failures degrade to the old inline-only
/// behavior instead of failing the completion.
fn split_tool_result(value: &Value) -> (Value, Option<(String, String, usize)>) {
    let inline = compact_tool_result(value);
    if inline.get("truncated").and_then(Value::as_bool) != Some(true) {
        return (inline, None);
    }
    let Ok(full) = serde_json::to_string(value) else {
        return (inline, None);
    };
    let sha = format!(
        "sha256:{}",
        hex::encode(Sha256::digest(full.as_bytes()))
    );
    let bytes = full.len();
    (inline, Some((sha, full, bytes)))
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

/// Loads every run referenced by a set of loaded messages in a single query so
/// the conversation read model can surface persisted terminal run state.
fn query_runs_by_ids(
    connection: &Connection,
    run_ids: &[String],
) -> rusqlite::Result<Vec<RunRecord>> {
    if run_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat("?")
        .take(run_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, conversation_id, runtime_session_id, status, model, started_at,
                finished_at, error_code, error_message, last_seq, trace_id, root_span_id
         FROM runs WHERE id IN ({placeholders})"
    );
    let mut statement = connection.prepare(&sql)?;
    let records = statement
        .query_map(
            rusqlite::params_from_iter(run_ids.iter()),
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
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(records)
}

fn query_pending_work_mode_dispatch(
    connection: &Connection,
    goal_id: &str,
) -> rusqlite::Result<Option<PendingWorkModeDispatch>> {
    let pending = connection
        .query_row(
            "SELECT p.goal_id, p.runtime_text,
                    r.id, r.conversation_id, r.runtime_session_id, r.status, r.model,
                    r.started_at, r.finished_at, r.error_code, r.error_message, r.last_seq,
                    r.trace_id, r.root_span_id,
                    m.id, m.role, m.kind, m.content, m.status, m.ordinal,
                    m.created_at, m.updated_at
             FROM pending_work_mode_dispatches p
             JOIN runs r ON r.id = p.run_id
             JOIN messages m ON m.run_id = r.id AND m.role = 'user'
             WHERE p.goal_id = ?1
             ORDER BY m.ordinal ASC LIMIT 1",
            [goal_id],
            |row| {
                let conversation_id: String = row.get(3)?;
                let run_id: String = row.get(2)?;
                Ok(PendingWorkModeDispatch {
                    goal_id: row.get(0)?,
                    runtime_text: row.get(1)?,
                    started: StartRunResult {
                        run: RunRecord {
                            id: run_id.clone(),
                            conversation_id: conversation_id.clone(),
                            runtime_session_id: row.get(4)?,
                            status: row.get(5)?,
                            model: row.get(6)?,
                            started_at: row.get(7)?,
                            finished_at: row.get(8)?,
                            error_code: row.get(9)?,
                            error_message: row.get(10)?,
                            last_seq: row.get(11)?,
                            trace_id: row.get(12)?,
                            root_span_id: row.get(13)?,
                        },
                        user_message: MessageRecord {
                            id: row.get(14)?,
                            conversation_id,
                            run_id: Some(run_id),
                            role: row.get(15)?,
                            kind: row.get(16)?,
                            content: row.get(17)?,
                            status: row.get(18)?,
                            ordinal: row.get(19)?,
                            created_at: row.get(20)?,
                            updated_at: row.get(21)?,
                        },
                        attachments: Vec::new(),
                    },
                })
            },
        )
        .optional()?;
    let Some(mut pending) = pending else {
        return Ok(None);
    };
    let mut statement = connection.prepare(
        "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                byte_size, sha256, status, created_at
         FROM attachments WHERE message_id = ?1 AND status = 'ready' ORDER BY created_at ASC",
    )?;
    pending.started.attachments = statement
        .query_map([pending.started.user_message.id.as_str()], map_attachment)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(pending))
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

fn plugin_kind_from_db(value: String) -> rusqlite::Result<PluginKind> {
    match value.as_str() {
        "mcp" => Ok(PluginKind::Mcp),
        "skill" => Ok(PluginKind::Skill),
        "tool" => Ok(PluginKind::Tool),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn plugin_origin_from_db(value: String) -> rusqlite::Result<PluginOrigin> {
    match value.as_str() {
        "builtin" => Ok(PluginOrigin::Builtin),
        "official" => Ok(PluginOrigin::Official),
        "community" => Ok(PluginOrigin::Community),
        "local" => Ok(PluginOrigin::Local),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn plugin_install_status_from_db(value: String) -> rusqlite::Result<PluginInstallStatus> {
    match value.as_str() {
        "not_installed" => Ok(PluginInstallStatus::NotInstalled),
        "installing" => Ok(PluginInstallStatus::Installing),
        "installed" => Ok(PluginInstallStatus::Installed),
        "update_available" => Ok(PluginInstallStatus::UpdateAvailable),
        "uninstalling" => Ok(PluginInstallStatus::Uninstalling),
        "install_failed" => Ok(PluginInstallStatus::InstallFailed),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
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
    use crate::database::{GoalStatus, HostToolCallDisposition};

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

    const LEGACY_EXPERT_V1_FIXTURE: &str =
        "tests/fixtures/expert-packages/v1/legacy-remote-knowledge.json";

    fn legacy_expert_v1_fixture_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(LEGACY_EXPERT_V1_FIXTURE)
    }

    fn legacy_remote_knowledge_ids(manifest: &Value) -> Vec<String> {
        manifest
            .get("knowledge")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    fn legacy_expert_v1_generator_input() -> Value {
        json!({
            "agentId": "fox-debugger",
            "packageManifest": {
                "version": "1.0.0",
                "prompt": "Use the configured remote knowledge bases as evidence.",
                "skills": ["research"],
                "knowledge": ["remote-kb-architecture", "remote-kb-policies"],
                "mcpServers": ["remote-search"],
                "allowedTools": ["read", "search_knowledge", "read_knowledge_document"]
            }
        })
    }

    fn fixture_generation_commit() -> String {
        if let Ok(commit) = std::env::var("FOX_EXPERT_FIXTURE_COMMIT") {
            if !commit.trim().is_empty() {
                return commit;
            }
        }
        let repository_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let output = std::process::Command::new("git")
            .args([
                "-C",
                repository_root.to_str().expect("repository path"),
                "rev-parse",
                "HEAD",
            ])
            .output()
            .expect("resolve fixture generation commit");
        assert!(
            output.status.success(),
            "git rev-parse failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("commit is utf-8")
            .trim()
            .to_owned()
    }

    #[test]
    fn legacy_expert_v1_fixture_preserves_snapshot_and_remote_knowledge_contract() {
        let fixture_path = legacy_expert_v1_fixture_path();
        let fixture_bytes = std::fs::read(&fixture_path).expect("read committed v1 fixture");
        let fixture: Value =
            serde_json::from_slice(&fixture_bytes).expect("parse committed v1 fixture");
        let input = fixture.get("input").cloned().expect("fixture input");
        let agent_id = input
            .get("agentId")
            .and_then(Value::as_str)
            .expect("fixture input has agent ID");
        let package_manifest = input
            .get("packageManifest")
            .cloned()
            .expect("fixture input has package manifest");
        let expected_snapshot_bytes = fixture
            .get("packageSnapshotBytes")
            .and_then(Value::as_str)
            .expect("fixture has compact package snapshot bytes");
        let expected_hash = fixture
            .get("packageSnapshotHash")
            .and_then(Value::as_str)
            .expect("fixture has package snapshot hash");
        let expected_snapshot: Value =
            serde_json::from_str(expected_snapshot_bytes).expect("parse compact snapshot");
        assert_eq!(
            expected_snapshot.to_string(),
            expected_snapshot_bytes,
            "fixture snapshot must remain compact and byte-stable"
        );
        assert_eq!(
            fixture["generatedFromCommit"].as_str().map(str::len),
            Some(40)
        );
        assert_eq!(fixture["fixtureVersion"], 1);

        let (database, path) = test_database();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE agents SET package_manifest_json = ?2 WHERE id = ?1",
                    params![agent_id, package_manifest.to_string()],
                )?;
                Ok(())
            })
            .expect("restore v1 expert input");
        let saved = database
            .get_agent(agent_id)
            .expect("load v1 expert")
            .expect("v1 expert exists");
        let actual_snapshot = agent_package_snapshot(&saved);
        assert_eq!(
            actual_snapshot.to_string(),
            expected_snapshot_bytes,
            "current snapshot serialization must not rewrite the v1 fixture"
        );
        assert_eq!(package_snapshot_hash(&actual_snapshot), expected_hash);
        assert_eq!(package_snapshot_hash(&expected_snapshot), expected_hash);

        let manifest = actual_snapshot
            .get("packageManifest")
            .expect("snapshot has package manifest");
        let expected_legacy_ids = fixture
            .get("legacyKnowledgeIds")
            .and_then(Value::as_array)
            .expect("fixture has legacy knowledge IDs");
        assert_eq!(
            manifest.get("knowledge").and_then(Value::as_array),
            Some(expected_legacy_ids)
        );
        assert!(manifest.get("knowledgeReferences").is_none());
        assert!(manifest.get("manifestSchemaVersion").is_none());
        assert_eq!(
            legacy_remote_knowledge_ids(manifest),
            expected_legacy_ids
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            fixture["legacyKnowledgeResolution"],
            json!({
                "source": "remote",
                "ids": expected_legacy_ids
            })
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    #[ignore = "explicit fixture generation only; never run as part of compatibility tests"]
    fn generate_legacy_expert_v1_fixture_explicitly() {
        assert_eq!(
            std::env::var("FOX_GENERATE_EXPERT_V1_FIXTURE").as_deref(),
            Ok("1"),
            "set FOX_GENERATE_EXPERT_V1_FIXTURE=1 to overwrite the committed fixture"
        );
        let input = legacy_expert_v1_generator_input();
        let agent_id = input
            .get("agentId")
            .and_then(Value::as_str)
            .expect("generator agent ID");
        let package_manifest = input
            .get("packageManifest")
            .cloned()
            .expect("generator package manifest");
        let (database, path) = test_database();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE agents SET package_manifest_json = ?2 WHERE id = ?1",
                    params![agent_id, package_manifest.to_string()],
                )?;
                Ok(())
            })
            .expect("apply generator package manifest");
        let saved = database
            .get_agent(agent_id)
            .expect("load generator expert")
            .expect("generator expert exists");
        let snapshot = agent_package_snapshot(&saved);
        let legacy_ids = legacy_remote_knowledge_ids(
            snapshot
                .get("packageManifest")
                .expect("fixture package manifest"),
        );
        let fixture = json!({
            "fixtureFormat": "fox.expert-package",
            "fixtureVersion": 1,
            "generatedFromCommit": fixture_generation_commit(),
            "input": input,
            "legacyKnowledgeIds": legacy_ids,
            "legacyKnowledgeResolution": {
                "source": "remote",
                "ids": legacy_ids
            },
            "packageSnapshotBytes": snapshot.to_string(),
            "packageSnapshotHash": package_snapshot_hash(&snapshot)
        });
        let fixture_path = legacy_expert_v1_fixture_path();
        std::fs::create_dir_all(fixture_path.parent().expect("fixture directory"))
            .expect("create fixture directory");
        std::fs::write(
            fixture_path,
            serde_json::to_vec_pretty(&fixture).expect("serialize fixture"),
        )
        .expect("write generated fixture");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn seeds_builtin_experts_on_existing_database_open() {
        let (database, path) = test_database();
        let agents = database.list_agents().expect("list builtin experts");
        assert_eq!(
            agents.first().map(|agent| agent.id.as_str()),
            Some(DEFAULT_AGENT_ID)
        );
        assert_eq!(
            agents.first().map(|agent| {
                (
                    agent.agent_kind.as_str(),
                    agent.invocation_mode.as_str(),
                    agent.visibility.as_str(),
                )
            }),
            Some(("assistant", "primary", "chat_selector"))
        );
        for id in [
            "fox-debugger",
            "fox-frontend",
            "fox-reviewer",
            "fox-security",
            "fox-architect",
        ] {
            let expert = agents
                .iter()
                .find(|agent| agent.id == id)
                .unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(expert.agent_kind, "expert");
            assert_eq!(expert.invocation_mode, "inline");
            assert_eq!(expert.visibility, "expert_center");
        }

        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind debugger expert");
        assert_eq!(binding.expert_id, "fox-debugger");

        drop(database);
        let reopened = Database::open(path.clone()).expect("reopen expert database");
        assert_eq!(
            reopened
                .list_agents()
                .expect("list experts after reopen")
                .iter()
                .filter(|agent| agent.id.starts_with("fox-"))
                .count(),
            BUILTIN_AGENTS.len() + 17, // 20 adapted experts reuse 3 existing IDs.
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn default_assistant_compute_permissions_survive_existing_database_upgrade() {
        // Shared with the JS tool-selection test: exercise the default assistant
        // whitelist as well as the expert whitelist, not an unrestricted null.
        let expected: Value = serde_json::from_str(include_str!("../../../../../services/agent-runtime/test/fixtures/default-assistant-tools.json")).unwrap();
        let (database, path) = test_database();
        let agent = database.get_agent(DEFAULT_AGENT_ID).unwrap().unwrap();
        assert_eq!(agent.package_manifest["allowedTools"], expected);
        assert!(expected.as_array().unwrap().contains(&json!("attachment_compute")));
        let mut old_manifest = agent.package_manifest;
        old_manifest["allowedTools"].as_array_mut().unwrap().retain(|name| name != "attachment_compute");
        database.with_connection(|connection| {
            connection.execute("UPDATE agents SET package_manifest_json=?1 WHERE id=?2", params![old_manifest.to_string(), DEFAULT_AGENT_ID])?;
            Ok(())
        }).unwrap();
        drop(database);
        let reopened = Database::open(path.clone()).unwrap();
        assert_eq!(reopened.get_agent(DEFAULT_AGENT_ID).unwrap().unwrap().package_manifest["allowedTools"], expected);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn creates_copies_updates_and_deletes_user_expert_packages() {
        let (database, path) = test_database();
        let created = database
            .save_agent(&SaveAgentRequest {
                id: None,
                name: "项目诊断专家".to_owned(),
                description: "诊断项目问题".to_owned(),
                icon: Some("/icons/diagnostic.png".to_owned()),
                category: "engineering".to_owned(),
                system_prompt: "Inspect evidence before changing files.".to_owned(),
                default_model: "configured-model".to_owned(),
                opening_suggestions: vec!["检查项目".to_owned()],
                package_manifest: json!({
                    "skills": ["project-review"],
                    "allowedTools": ["read", "grep"]
                }),
            })
            .expect("create expert");
        assert!(!created.is_builtin);
        assert_eq!(created.agent_kind, "expert");
        assert_eq!(created.invocation_mode, "inline");
        assert_eq!(created.visibility, "expert_center");
        assert_eq!(created.package_version, "1.0.0");
        assert_eq!(
            created.package_manifest["allowedTools"],
            json!(["read", "grep"])
        );
        assert_eq!(
            database
                .enabled_agent_skills(&created.id)
                .expect("load created expert skills"),
            vec!["project-review"]
        );

        let copied = database.copy_agent(&created.id, None).expect("copy expert");
        assert_ne!(copied.id, created.id);
        assert!(copied.name.ends_with("副本"));

        let updated = database
            .save_agent(&SaveAgentRequest {
                id: Some(created.id.clone()),
                name: "项目根因专家".to_owned(),
                description: created.description.clone(),
                icon: created.icon.clone(),
                category: created.category.clone(),
                system_prompt: created.system_prompt.clone(),
                default_model: created.default_model.clone(),
                opening_suggestions: created.opening_suggestions.clone(),
                package_manifest: created.package_manifest.clone(),
            })
            .expect("update expert");
        assert_eq!(updated.name, "项目根因专家");
        assert!(database.delete_agent(&created.id).expect("delete original"));
        assert!(database.delete_agent(&copied.id).expect("delete copy"));
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn updates_default_assistant_without_reclassifying_it() {
        let (database, path) = test_database();
        let assistant = database
            .get_agent(DEFAULT_AGENT_ID)
            .expect("load default assistant")
            .expect("default assistant exists");
        let updated = database
            .save_agent(&SaveAgentRequest {
                id: Some(DEFAULT_AGENT_ID.to_owned()),
                name: "我的 Fox 助手".to_owned(),
                description: "自定义通用助手".to_owned(),
                icon: assistant.icon.clone(),
                category: assistant.category.clone(),
                system_prompt: "Answer carefully and concisely.".to_owned(),
                default_model: assistant.default_model.clone(),
                opening_suggestions: vec!["帮我整理今天的工作".to_owned()],
                package_manifest: assistant.package_manifest.clone(),
            })
            .expect("update default assistant");

        assert_eq!(updated.name, "我的 Fox 助手");
        assert_eq!(updated.description, "自定义通用助手");
        assert_eq!(updated.system_prompt, "Answer carefully and concisely.");
        assert_eq!(updated.opening_suggestions, vec!["帮我整理今天的工作"]);
        assert!(updated.is_builtin);
        assert_eq!(updated.agent_kind, "assistant");
        assert_eq!(updated.invocation_mode, "primary");
        assert_eq!(updated.visibility, "chat_selector");

        drop(database);
        let reopened = Database::open(path.clone()).expect("reopen assistant database");
        let restored = reopened
            .get_agent(DEFAULT_AGENT_ID)
            .expect("reload default assistant")
            .expect("restored default assistant exists");
        assert_eq!(restored.name, "我的 Fox 助手");
        assert_eq!(restored.system_prompt, "Answer carefully and concisely.");
        assert_eq!(restored.agent_kind, "assistant");
        assert_eq!(restored.invocation_mode, "primary");
        assert_eq!(restored.visibility, "chat_selector");

        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn installs_upgrades_and_rolls_back_versioned_expert_packages() {
        let (database, path) = test_database();
        let request = |version: &str,
                       package_hash: &str,
                       prompt: &str,
                       expected_current_hash: Option<&str>| {
            InstallExpertPackageVersionRequest {
                expert_id: "fox-package-test".to_owned(),
                package_id: "fox.test-package".to_owned(),
                version: version.to_owned(),
                package_hash: package_hash.to_owned(),
                package: json!({"id": "fox.test-package", "version": version}),
                package_manifest: json!({
                    "version": version,
                    "prompt": prompt,
                    "skills": [],
                    "allowedTools": ["read"]
                }),
                name: "Versioned Test".to_owned(),
                description: "Test package history".to_owned(),
                icon: None,
                category: "test".to_owned(),
                system_prompt: prompt.to_owned(),
                default_model: String::new(),
                opening_suggestions: vec!["Test".to_owned()],
                enabled_skills: Vec::new(),
                expected_current_hash: expected_current_hash.map(str::to_owned),
            }
        };
        let v1 = request("0.1.0", "hash-v1", "Prompt v1", None);
        let installed = database
            .install_expert_package_version(&v1)
            .expect("install v1");
        assert_eq!(installed.package_source, "imported");
        assert_eq!(installed.package_version, "0.1.0");

        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, &installed.id, "test")
            .expect("bind installed expert");
        assert_eq!(binding.package_snapshot["systemPrompt"], "Prompt v1");

        let v2 = request("0.2.0", "hash-v2", "Prompt v2", Some("hash-v1"));
        let upgraded = database
            .install_expert_package_version(&v2)
            .expect("upgrade to v2");
        assert_eq!(upgraded.package_version, "0.2.0");
        assert_eq!(upgraded.system_prompt, "Prompt v2");
        let frozen = database
            .list_conversation_expert_bindings(&conversation.id)
            .expect("load frozen binding");
        assert_eq!(frozen[0].package_snapshot["systemPrompt"], "Prompt v1");

        let tampered = request("0.2.0", "different-hash", "Tampered", Some("hash-v2"));
        assert_eq!(
            database
                .install_expert_package_version(&tampered)
                .expect_err("same version tampering must fail"),
            "expert.package_version_conflict"
        );

        let rollback = request("0.1.0", "hash-v1", "Prompt v1", Some("hash-v2"));
        let rolled_back = database
            .activate_expert_package_version(&rollback)
            .expect("reactivate v1");
        assert_eq!(rolled_back.package_version, "0.1.0");
        assert_eq!(rolled_back.system_prompt, "Prompt v1");
        let versions = database
            .list_expert_package_versions(&installed.id)
            .expect("list versions");
        assert_eq!(versions.len(), 2);
        assert_eq!(
            versions
                .iter()
                .filter(|item| item.status == "active")
                .count(),
            1
        );
        assert_eq!(
            versions
                .iter()
                .find(|item| item.status == "active")
                .map(|item| item.version.as_str()),
            Some("0.1.0")
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_invalid_assistant_and_expert_roles() {
        let (database, path) = test_database();
        assert!(database
            .create_conversation("fox-debugger", None, None, None)
            .is_err());
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");
        assert_eq!(
            database
                .bind_conversation_expert(&conversation.id, DEFAULT_AGENT_ID, "test")
                .unwrap_err(),
            ConversationExpertBindingError::InvalidRole
        );
        assert_eq!(
            database
                .bind_conversation_expert(&conversation.id, "missing-expert", "test")
                .unwrap_err(),
            ConversationExpertBindingError::ExpertNotFound
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_unavailable_remote_experts_with_stable_error() {
        let (database, path) = test_database();
        database
            .upsert_yuxi_agents(&[YuxiAgentRecord {
                id: "remote-offline".to_owned(),
                slug: "remote-offline".to_owned(),
                name: "离线专家".to_owned(),
                description: "暂不可用".to_owned(),
                icon: None,
                backend_id: "ChatAgentBackend".to_owned(),
                default_model: "remote-model".to_owned(),
                capabilities: json!(["knowledge"]),
                resources: AgentResourcesRecord::default(),
                configurable_items: json!({}),
                is_default: false,
                available: false,
            }])
            .expect("save unavailable remote expert");
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");

        let error = database
            .bind_conversation_expert(&conversation.id, "yuxi:remote-offline", "test")
            .unwrap_err();
        assert_eq!(error, ConversationExpertBindingError::RemoteUnavailable);
        assert_eq!(error.code(), "conversation.expert_remote_unavailable");
        assert!(error.retryable());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replaces_recovers_removes_and_cascades_expert_bindings() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");
        let first = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "conversation_create")
            .expect("bind first expert");
        assert_eq!(first.package_snapshot["id"], "fox-debugger");
        assert_eq!(first.package_snapshot["runtimeType"], "pi");
        assert_eq!(first.package_snapshot["agentKind"], "expert");
        assert!(first.package_snapshot["systemPrompt"].is_string());
        assert!(first.package_snapshot["packageManifest"].is_object());
        assert!(first.package_snapshot["resources"].is_object());
        assert_eq!(
            first.package_hash,
            package_snapshot_hash(&first.package_snapshot)
        );
        let second = database
            .bind_conversation_expert(&conversation.id, "fox-reviewer", "manual")
            .expect("replace expert");
        assert_ne!(first.id, second.id);
        assert_eq!(
            database
                .current_conversation_expert_binding(&conversation.id)
                .expect("load current binding")
                .expect("active binding")
                .expert_id,
            "fox-reviewer"
        );

        drop(database);
        let reopened = Database::open(path.clone()).expect("reopen binding database");
        assert_eq!(
            reopened
                .list_conversation_expert_bindings(&conversation.id)
                .expect("list restored bindings")
                .len(),
            2
        );
        let detail = reopened
            .load_conversation(&conversation.id)
            .expect("restore conversation detail");
        assert_eq!(detail.expert_bindings.len(), 2);
        assert_eq!(detail.expert_bindings[0].state, "replaced");
        assert!(detail.expert_bindings[0].deactivated_at.is_some());
        assert_eq!(detail.expert_bindings[1].state, "active");
        assert_eq!(detail.expert_bindings[1].expert_id, "fox-reviewer");

        let removed = reopened
            .remove_conversation_expert(&conversation.id)
            .expect("remove expert")
            .expect("removed binding");
        assert_eq!(removed.state, "removed");
        assert!(reopened
            .current_conversation_expert_binding(&conversation.id)
            .expect("load cleared binding")
            .is_none());
        reopened
            .delete_conversation(&conversation.id)
            .expect("delete conversation");
        reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM conversation_expert_bindings WHERE conversation_id = ?1",
                    [&conversation.id],
                    |row| row.get::<_, i64>(0),
                )
            })
            .map(|count| assert_eq!(count, 0))
            .expect("verify binding cascade");

        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn package_snapshot_remains_static_after_expert_definition_changes() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");
        let binding = database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind expert");
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE agents
                     SET system_prompt = 'changed prompt', package_manifest_json = '{\"changed\":true}'
                     WHERE id = 'fox-debugger'",
                    [],
                )?;
                Ok(())
            })
            .expect("change expert definition");

        let restored = database
            .current_conversation_expert_binding(&conversation.id)
            .expect("load binding")
            .expect("active binding");
        assert_eq!(restored.package_snapshot, binding.package_snapshot);
        assert_eq!(restored.package_hash, binding.package_hash);
        assert_eq!(
            restored.package_hash,
            package_snapshot_hash(&restored.package_snapshot)
        );
        assert_ne!(
            restored.package_snapshot["systemPrompt"],
            Value::String("changed prompt".to_owned())
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn locks_expert_changes_only_for_dialog_messages_active_runs_or_archive() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create assistant conversation");
        database
            .with_connection(|connection| {
                for (ordinal, role) in ["system", "tool"].into_iter().enumerate() {
                    connection.execute(
                        "INSERT INTO messages(
                            id, conversation_id, role, kind, content, status, ordinal,
                            created_at, updated_at
                         ) VALUES (?1, ?2, ?3, 'text', '', 'completed', ?4, 1, 1)",
                        params![
                            format!("non-dialog-{role}"),
                            conversation.id,
                            role,
                            ordinal as i64
                        ],
                    )?;
                }
                Ok(())
            })
            .expect("insert non-dialog messages");
        database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("non-dialog messages do not lock binding");
        database
            .remove_conversation_expert(&conversation.id)
            .expect("non-dialog messages do not lock removal")
            .expect("removed binding");

        database
            .archive_conversation(&conversation.id)
            .expect("archive conversation")
            .expect("archived conversation");
        assert_eq!(
            database
                .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
                .unwrap_err(),
            ConversationExpertBindingError::LockedArchived
        );
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversations SET archived = 0 WHERE id = ?1",
                    [&conversation.id],
                )?;
                Ok(())
            })
            .expect("unarchive conversation");
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO runs(id, conversation_id, status, model, last_seq, created_at)
                     VALUES ('active-run', ?1, 'queued', '', 0, 1)",
                    [&conversation.id],
                )?;
                Ok(())
            })
            .expect("insert active run without messages");
        assert_eq!(
            database
                .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
                .unwrap_err(),
            ConversationExpertBindingError::LockedByActiveRun
        );
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs SET status = 'completed', finished_at = 2 WHERE id = 'active-run'",
                    [],
                )?;
                Ok(())
            })
            .expect("finish active run");
        database
            .bind_conversation_expert(&conversation.id, "fox-debugger", "test")
            .expect("bind after run finishes");
        database
            .archive_conversation(&conversation.id)
            .expect("archive bound conversation")
            .expect("bound conversation archived");
        assert_eq!(
            database
                .remove_conversation_expert(&conversation.id)
                .unwrap_err(),
            ConversationExpertBindingError::LockedArchived
        );
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE conversations SET archived = 0 WHERE id = ?1",
                    [&conversation.id],
                )?;
                Ok(())
            })
            .expect("unarchive bound conversation");
        database
            .remove_conversation_expert(&conversation.id)
            .expect("remove expert after unarchive")
            .expect("active expert removed");
        for role in ["user", "assistant"] {
            let dialog = database
                .create_conversation(DEFAULT_AGENT_ID, None, None, None)
                .expect("create dialog conversation");
            database
                .bind_conversation_expert(&dialog.id, "fox-debugger", "test")
                .expect("bind dialog expert");
            database
                .with_connection(|connection| {
                    connection.execute(
                        "INSERT INTO messages(
                            id, conversation_id, role, kind, content, status, ordinal,
                            created_at, updated_at
                         ) VALUES (?1, ?2, ?3, 'text', 'hello', 'completed', 0, 3, 3)",
                        params![format!("dialog-{role}"), dialog.id, role],
                    )?;
                    Ok(())
                })
                .expect("insert dialog message");
            assert_eq!(
                database.remove_conversation_expert(&dialog.id).unwrap_err(),
                ConversationExpertBindingError::LockedByMessages
            );
        }

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn get_agent_uses_primary_key_lookup() {
        let (database, path) = test_database();
        let agent = database
            .get_agent("fox-debugger")
            .expect("query expert")
            .expect("expert exists");
        assert_eq!(agent.id, "fox-debugger");

        let details = database
            .with_connection(|connection| {
                let sql = format!(
                    "EXPLAIN QUERY PLAN SELECT {AGENT_RECORD_COLUMNS} FROM agents WHERE id = ?1"
                );
                let mut statement = connection.prepare(&sql)?;
                let details = statement
                    .query_map(["fox-debugger"], |row| row.get::<_, String>(3))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(details)
            })
            .expect("inspect get_agent query plan");
        assert!(details
            .iter()
            .any(|detail| detail.contains("SEARCH agents")));
        assert!(details.iter().all(|detail| !detail.contains("SCAN agents")));

        drop(database);
        let _ = std::fs::remove_file(path);
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
    fn conversation_lifecycle_restores_and_forks_without_runtime_state() {
        let (database, path) = test_database();
        let source = database
            .create_conversation(DEFAULT_AGENT_ID, Some("原会话"), None, None)
            .expect("create source conversation");
        database
            .set_knowledge_reference_bindings(
                &source.id,
                &[KnowledgeReferenceBindingInput {
                    reference: KnowledgeReference::local("knowledge-1"),
                    name: "参考资料".to_owned(),
                }],
            )
            .expect("bind knowledge");
        database
            .bind_conversation_expert(&source.id, "fox-reviewer", "manual")
            .expect("bind expert");
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO conversation_tool_permissions(
                         conversation_id, tool_name, scope_key, granted_at
                     ) VALUES (?1, 'write_file', 'test-scope', ?2)",
                    params![&source.id, now_ms()],
                )?;
                Ok(())
            })
            .expect("grant source-only tool permission");
        let started = database
            .create_run(&source.id, "先分析问题", None)
            .expect("start source run");
        for (sequence, event) in [
            json!({"type": "run.started"}),
            json!({"type": "message.started"}),
            json!({"type": "message.delta", "delta": "分析完成"}),
            json!({"type": "run.completed"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&started.run.id, sequence as i64 + 1, &event)
                .expect("persist source event");
        }
        let source_detail = database
            .load_conversation(&source.id)
            .expect("load source detail");
        let fork_point = source_detail
            .messages
            .last()
            .expect("assistant message")
            .id
            .clone();

        let fork = database
            .fork_conversation(&source.id, &fork_point, None)
            .expect("fork conversation")
            .expect("fork result");
        assert_eq!(
            fork.parent_conversation_id.as_deref(),
            Some(source.id.as_str())
        );
        assert_eq!(
            fork.forked_from_message_id.as_deref(),
            Some(fork_point.as_str())
        );
        assert_eq!(fork.lineage_root_id, source.id);
        assert!(fork.title.ends_with("· 分支"));
        let fork_detail = database
            .load_conversation(&fork.id)
            .expect("load fork detail");
        assert_eq!(fork_detail.messages.len(), source_detail.messages.len());
        assert!(fork_detail
            .messages
            .iter()
            .all(|message| message.run_id.is_none()));
        assert!(fork_detail.runtime_events.is_empty());
        assert!(fork_detail.tool_calls.is_empty());
        assert!(fork_detail.approvals.is_empty());
        assert!(fork_detail.last_run.is_none());
        assert!(database
            .conversation_tool_permission_granted(&source.id, "write_file", "test-scope")
            .expect("source permission"));
        assert!(!database
            .conversation_tool_permission_granted(&fork.id, "write_file", "test-scope")
            .expect("fork permission"));
        assert_eq!(
            database
                .conversation_knowledge_references(&fork.id)
                .expect("load fork knowledge")
                .len(),
            1
        );
        assert_eq!(
            fork_detail
                .expert_bindings
                .iter()
                .filter(|binding| binding.state == "active")
                .count(),
            1
        );

        database
            .archive_conversation(&fork.id)
            .expect("archive fork")
            .expect("archived fork");
        assert!(!database
            .list_conversations()
            .expect("active list")
            .is_empty());
        assert_eq!(
            database
                .list_archived_conversations()
                .expect("archive list")
                .len(),
            1
        );
        database
            .unarchive_conversation(&fork.id)
            .expect("unarchive fork")
            .expect("restored archive");
        database
            .trash_conversation(&fork.id)
            .expect("trash fork")
            .expect("trashed fork");
        assert_eq!(
            database
                .list_trashed_conversations()
                .expect("trash list")
                .len(),
            1
        );
        assert!(database
            .restore_trashed_conversation(&fork.id)
            .expect("restore trash")
            .expect("restored fork")
            .trashed_at
            .is_none());
        assert!(!database
            .purge_trashed_conversation(&fork.id)
            .expect("active conversations cannot be purged"));
        database
            .trash_conversation(&fork.id)
            .expect("trash fork again")
            .expect("trashed fork again");
        assert!(database
            .conversation_is_trashed(&fork.id)
            .expect("inspect trash state"));
        assert!(database
            .purge_trashed_conversation(&fork.id)
            .expect("purge trashed fork"));
        assert!(database.load_conversation(&source.id).is_ok());

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
    fn restores_runtime_diagnostics_without_assistant_text() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "diagnose runtime", None)
            .expect("create run");
        for (seq, event) in [
            serde_json::json!({"type": "run.request_snapshot", "model": "model-1", "provider": "provider-1"}),
            serde_json::json!({"type": "run.phase", "phase": "preparing"}),
            serde_json::json!({"type": "run.retrying", "attempt": 1, "maxAttempts": 2}),
            serde_json::json!({"type": "run.retry.completed", "success": true, "attempt": 1}),
            serde_json::json!({"type": "context.compaction.started", "reason": "threshold"}),
            serde_json::json!({"type": "context.compaction.completed", "reason": "threshold", "aborted": false}),
            serde_json::json!({"type": "planner.started", "model": "model-1"}),
            serde_json::json!({"type": "planner.completed", "stepCount": 3, "planHash": "plan-hash"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&started.run.id, seq as i64 + 1, &event)
                .expect("apply runtime diagnostic event");
        }

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load conversation");
        assert_eq!(
            detail
                .runtime_events
                .iter()
                .map(|event| event.event_type.as_str())
                .collect::<Vec<_>>(),
            vec![
                "run.request_snapshot",
                "run.phase",
                "run.retrying",
                "run.retry.completed",
                "context.compaction.started",
                "context.compaction.completed",
                "planner.started",
                "planner.completed",
            ]
        );
        assert_eq!(detail.messages.len(), 1);
        assert_eq!(detail.messages[0].role, "user");

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
    fn resume_requires_one_canonical_awaiting_user_completion_and_rolls_back_other_terminals() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        for (label, terminal) in [
            ("ordinary-completed", json!({"type":"run.completed"})),
            (
                "stop-completed",
                json!({"type":"run.completed","completionReason":"stop"}),
            ),
            (
                "failed",
                json!({"type":"run.failed","code":"failed","message":"failed"}),
            ),
            ("cancelled", json!({"type":"run.cancelled"})),
            (
                "interrupted",
                json!({"type":"run.interrupted","code":"interrupted","message":"interrupted"}),
            ),
        ] {
            let parent = database
                .create_run(&conversation.id, label, None)
                .unwrap()
                .run;
            for (seq, event) in [
                json!({"type":"run.started"}),
                json!({
                    "type":"user.question.requested",
                    "questions":[{"question_id":"scope","question":"scope?"}]
                }),
                terminal,
            ]
            .into_iter()
            .enumerate()
            {
                database
                    .apply_runtime_event(&parent.id, seq as i64 + 1, &event)
                    .unwrap();
            }
            let before: (i64, i64, i64) = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT
                           (SELECT COUNT(*) FROM runs WHERE conversation_id = ?1),
                           (SELECT COUNT(*) FROM messages WHERE conversation_id = ?1),
                           (SELECT COUNT(*) FROM run_events
                            WHERE run_id = ?2 AND event_type = 'user.question.responded')",
                        params![conversation.id, parent.id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                })
                .unwrap();
            assert!(database
                .create_resumed_run(&conversation.id, &parent.id, "must not persist")
                .is_err());
            let after: (i64, i64, i64, i64) = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT
                           (SELECT COUNT(*) FROM runs WHERE conversation_id = ?1),
                           (SELECT COUNT(*) FROM messages WHERE conversation_id = ?1),
                           (SELECT COUNT(*) FROM run_events
                            WHERE run_id = ?2 AND event_type = 'user.question.responded'),
                           (SELECT last_seq FROM runs WHERE id = ?2)",
                        params![conversation.id, parent.id],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                })
                .unwrap();
            assert_eq!((after.0, after.1, after.2), before);
            assert_eq!(after.3, 3);
        }

        let conflicting = database
            .create_run(&conversation.id, "legacy conflicting terminal", None)
            .unwrap()
            .run;
        for (seq, event) in [
            json!({"type":"run.started"}),
            json!({
                "type":"user.question.requested",
                "questions":[{"question_id":"scope","question":"scope?"}]
            }),
            json!({"type":"run.completed","completionReason":"awaiting_user"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&conflicting.id, seq as i64 + 1, &event)
                .unwrap();
        }
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO run_events(id, run_id, seq, event_type, event_json, created_at)
                     VALUES (?1, ?2, 4, 'run.completed', ?3, ?4)",
                    params![
                        Uuid::new_v4().to_string(),
                        conflicting.id,
                        serde_json::to_string(
                            &json!({"type":"run.completed","completionReason":"stop"})
                        )
                        .unwrap(),
                        now_ms()
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        assert!(database
            .create_resumed_run(&conversation.id, &conflicting.id, "conflict")
            .is_err());
        let residue: (i64, i64) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                       (SELECT COUNT(*) FROM run_events
                        WHERE run_id = ?1 AND event_type = 'user.question.responded'),
                       (SELECT COUNT(*) FROM runs WHERE parent_run_id = ?1)",
                    [&conflicting.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(residue, (0, 0));

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
    fn rewinds_a_message_and_restarts_from_the_same_ordinal() {
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
                .expect("complete first run");
        }
        let second = database
            .create_run(&conversation.id, "second question", None)
            .expect("create second run");
        for (seq, event) in [
            serde_json::json!({"type": "run.started"}),
            serde_json::json!({"type": "message.started"}),
            serde_json::json!({"type": "message.delta", "delta": "obsolete answer"}),
            serde_json::json!({"type": "message.completed"}),
            serde_json::json!({"type": "run.completed"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&second.run.id, seq as i64 + 1, &event)
                .expect("complete second run");
        }

        let prior = database
            .runtime_prompt_context_before_message(&conversation.id, &second.user_message.id, 20)
            .expect("load prior context");
        assert_eq!(prior.len(), 2);
        assert_eq!(prior[0].content, "first question");
        assert_eq!(prior[1].content, "first answer");

        let replacement = database
            .rewind_run(
                &conversation.id,
                &second.user_message.id,
                "edited second question",
                None,
            )
            .expect("rewind run");
        assert_eq!(
            replacement.user_message.ordinal,
            second.user_message.ordinal
        );
        assert_eq!(replacement.user_message.id, second.user_message.id);
        assert_eq!(
            replacement.user_message.created_at,
            second.user_message.created_at
        );

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load rewound conversation");
        assert_eq!(detail.messages.len(), 3);
        assert_eq!(detail.messages[2].id, second.user_message.id);
        assert_eq!(detail.messages[2].content, "edited second question");
        assert!(!detail
            .messages
            .iter()
            .any(|message| message.content == "obsolete answer"));
        assert_eq!(
            detail.last_run.as_ref().map(|run| run.id.as_str()),
            Some(replacement.run.id.as_str())
        );

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
                .apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"}))
                .expect("start run");
            database
                .apply_runtime_event(&started.run.id, 2, &json!({"type":"message.started"}))
                .expect("start answer");
            database
                .apply_runtime_event(
                    &started.run.id,
                    3,
                    &json!({"type":"message.delta", "delta": format!("answer-{index}")}),
                )
                .expect("append answer");
            database
                .apply_runtime_event(&started.run.id, 4, &json!({"type":"run.completed"}))
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
                json!({"type":"run.started"}),
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
    fn marks_cached_remote_agents_unavailable_after_sync_failure() {
        let (database, path) = test_database();
        let agent = YuxiAgentRecord {
            id: "yuxi:cached-expert".to_owned(),
            slug: "cached-expert".to_owned(),
            name: "Cached Expert".to_owned(),
            description: String::new(),
            icon: None,
            backend_id: "ChatbotAgent".to_owned(),
            default_model: "remote-model".to_owned(),
            capabilities: json!([]),
            resources: AgentResourcesRecord::default(),
            configurable_items: json!({}),
            is_default: false,
            available: true,
        };
        database
            .upsert_yuxi_agents(std::slice::from_ref(&agent))
            .expect("upsert cached remote expert");

        database
            .mark_yuxi_agents_unavailable()
            .expect("mark remote experts unavailable");
        assert!(
            !database
                .list_agents()
                .expect("list agents")
                .into_iter()
                .find(|item| item.id == "yuxi:cached-expert")
                .expect("cached remote expert")
                .available
        );

        database
            .upsert_yuxi_agents(std::slice::from_ref(&agent))
            .expect("restore remote expert availability");
        assert!(
            database
                .list_agents()
                .expect("list agents")
                .into_iter()
                .find(|item| item.id == "yuxi:cached-expert")
                .expect("cached remote expert")
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
            .apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"}))
            .expect("start run");
        database
            .apply_runtime_event(&started.run.id, 2, &json!({"type":"message.started"}))
            .expect("start assistant message");
        database
            .apply_runtime_event(
                &started.run.id,
                3,
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
    fn host_failure_interrupts_streaming_message_once_without_mutating_exact_replay() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "host failure cleanup", None)
            .unwrap()
            .run;
        for (seq, event) in [
            json!({"type":"run.started"}),
            json!({"type":"message.started"}),
            json!({"type":"message.delta","delta":"preserved partial answer"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&run.id, seq as i64 + 1, &event)
                .unwrap();
        }
        database
            .mark_run_failed(&run.id, "host.failed", "Host failure")
            .unwrap();
        let frozen: (String, String, i64) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status, content, updated_at FROM messages WHERE id = ?1",
                    [format!("assistant-{}", run.id)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(frozen.0, "interrupted");
        assert_eq!(frozen.1, "preserved partial answer");
        database
            .mark_run_failed(&run.id, "host.failed", "Host failure")
            .expect("exact Host failure replay is a no-op");
        let replayed: (String, String, i64) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status, content, updated_at FROM messages WHERE id = ?1",
                    [format!("assistant-{}", run.id)],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .unwrap();
        assert_eq!(replayed, frozen);

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
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start run");

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
            .resolve_approval(&approval.id, crate::database::ApprovalDecision::AllowOnce)
            .expect("approve")
            .expect("resolved approval");
        assert_eq!(resolved.status, "approved");
        assert!(!database
            .conversation_tool_permission_granted(&conversation.id, "write_file", "test-scope")
            .expect("check one-time approval"));
        assert!(database
            .claim_approved_tool_call(&started.run.id, "write-1")
            .expect("claim approved one-time tool"));
        assert!(database
            .resolve_approval(&approval.id, crate::database::ApprovalDecision::Deny)
            .expect("repeat approval")
            .is_none());

        let conversation_tool = database
            .create_host_tool_call(
                &started.run.id,
                "write-2",
                "write_file",
                &json!({"path":"another.md"}),
                "pending",
                true,
            )
            .expect("create second tool");
        let conversation_approval = database
            .create_approval(
                &conversation_tool.id,
                "创建 another.md",
                &json!({
                    "target":"another.md",
                    "permissionScope": "test-scope",
                    "availableDecisions": ["allow_once", "allow_conversation", "deny"]
                }),
            )
            .expect("create conversation approval");
        database
            .resolve_approval(
                &conversation_approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect("approve conversation tool")
            .expect("resolved conversation approval");
        assert!(database
            .claim_approved_tool_call(&started.run.id, "write-2")
            .expect("claim approved conversation tool"));
        assert!(database
            .conversation_tool_permission_granted(&conversation.id, "write_file", "test-scope")
            .expect("check conversation approval"));
        assert!(!database
            .conversation_tool_permission_granted(&conversation.id, "write_file", "different-scope")
            .expect("conversation grant must be isolated to its exact operation scope"));

        let one_shot_tool = database
            .create_host_tool_call(
                &started.run.id,
                "write-3",
                "write_file",
                &json!({"path":"forced-hook.md"}),
                "pending",
                true,
            )
            .expect("create one-shot-only tool");
        let one_shot_approval = database
            .create_approval(
                &one_shot_tool.id,
                "forced policy approval",
                &json!({
                    "target":"forced-hook.md",
                    "availableDecisions": ["allow_once", "deny"]
                }),
            )
            .expect("create one-shot-only approval");
        assert!(database
            .resolve_approval(
                &one_shot_approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect_err("one-shot policy approval must reject conversation grants")
            .contains("intentionally one-shot"));
        database
            .resolve_approval(
                &one_shot_approval.id,
                crate::database::ApprovalDecision::Deny,
            )
            .expect("deny one-shot-only approval")
            .expect("one-shot-only approval remains pending after rejected decision");

        database
            .complete_host_tool_call(
                &started.run.id,
                "write-1",
                Some(&json!({
                    "content": [{"type":"text","text":"done"}],
                    "details": {"path":"D:\\data\\result.md","bytes":4,"operation":"created"}
                })),
                None,
            )
            .expect("complete tool");
        database
            .complete_host_tool_call(
                &started.run.id,
                "write-2",
                Some(&json!({
                    "content": [{"type":"text","text":"updated"}],
                    "details": {"path":"D:\\data\\result.md","bytes":7,"operation":"modified"}
                })),
                None,
            )
            .expect("complete repeated file result");
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
        let artifact = database
            .artifact_for_conversation(&conversation.id, &detail.artifacts[0].id)
            .expect("scoped artifact lookup")
            .expect("artifact exists");
        assert_eq!(artifact.conversation_id, conversation.id);
        assert_eq!(artifact.artifact_type, "created_file");
        assert_eq!(artifact.byte_size, 7);
        assert!(database
            .artifact_for_conversation("another-conversation", &artifact.id)
            .expect("cross-conversation artifact lookup")
            .is_none());
        assert_eq!(detail.knowledge_bindings.len(), 1);
        assert_eq!(detail.approvals[0].status, "approved");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_tool_calls_and_approvals_are_insert_once_and_never_reopened() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "immutable approval facts", None)
            .expect("create run");
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start run");

        let completed = database
            .create_host_tool_call(
                &started.run.id,
                "read-once",
                "read",
                &json!({"path":"README.md"}),
                "running",
                false,
            )
            .expect("create running ToolCall");
        database
            .complete_host_tool_call(
                &started.run.id,
                "read-once",
                Some(&json!({"content":"done"})),
                None,
            )
            .expect("complete ToolCall");
        let replay = database
            .create_host_tool_call(
                &started.run.id,
                "read-once",
                "read",
                &json!({"path":"README.md"}),
                "running",
                false,
            )
            .expect("exact ToolCall replay");
        assert_eq!(replay.id, completed.id);
        assert_eq!(replay.status, "completed");
        database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type": "tool.started",
                    "toolCallId": "read-once",
                    "tool": "read",
                    "input": {"path":"README.md"}
                }),
            )
            .expect("exact late Runtime projection must remain idempotent");
        assert_eq!(
            database
                .create_host_tool_call(
                    &started.run.id,
                    "read-once",
                    "read",
                    &json!({"path":"README.md"}),
                    "running",
                    false,
                )
                .expect("late Runtime replay keeps terminal Host fact")
                .status,
            "completed"
        );
        assert!(database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type": "tool.started",
                    "toolCallId": "read-once",
                    "tool": "read",
                    "input": {"path":"runtime-conflict.md"}
                }),
            )
            .is_err());
        assert!(database
            .create_host_tool_call(
                &started.run.id,
                "read-once",
                "read",
                &json!({"path":"different.md"}),
                "running",
                false,
            )
            .expect_err("conflicting ToolCall replay must fail")
            .contains("immutable content"));

        for (runtime_id, decision, expected_status) in [
            (
                "approval-approved",
                crate::database::ApprovalDecision::AllowOnce,
                "approved",
            ),
            (
                "approval-denied",
                crate::database::ApprovalDecision::Deny,
                "denied",
            ),
        ] {
            let tool = database
                .create_host_tool_call(
                    &started.run.id,
                    runtime_id,
                    "write_file",
                    &json!({"path":format!("{runtime_id}.md")}),
                    "pending",
                    true,
                )
                .expect("create approval ToolCall");
            let request = json!({"target":format!("{runtime_id}.md")});
            let approval = database
                .create_approval(&tool.id, "write file", &request)
                .expect("create approval");
            database
                .resolve_approval(&approval.id, decision)
                .expect("resolve approval")
                .expect("pending approval resolved");
            let replay = database
                .create_approval(&tool.id, "write file", &request)
                .expect("exact approval replay");
            assert_eq!(replay.id, approval.id);
            assert_eq!(replay.status, expected_status);
            assert!(database
                .create_approval(&tool.id, "different action", &request)
                .expect_err("conflicting approval replay must fail")
                .contains("immutable content"));
        }

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_projection_and_late_host_completion_cannot_mutate_foreign_or_terminal_tool_calls() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let started = database
            .create_run(&conversation.id, "tool projection authority", None)
            .unwrap();
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .unwrap();

        let failed_host = database
            .create_host_tool_call(
                &started.run.id,
                "host-failed",
                "run_command",
                &json!({"command":"false"}),
                "running",
                false,
            )
            .unwrap();
        database
            .complete_host_tool_call(&started.run.id, "host-failed", None, Some("command failed"))
            .unwrap();
        for payload in [
            json!({
                "type":"tool.updated", "toolCallId":"host-failed",
                "tool":"run_command", "update":{"passed":true}
            }),
            json!({
                "type":"tool.completed", "toolCallId":"host-failed",
                "tool":"run_command", "result":{"passed":true}, "isError":false
            }),
            json!({
                "type":"tool.completed", "toolCallId":"host-failed",
                "tool":"run_command", "result":{"passed":true}, "isError":true
            }),
        ] {
            database
                .record_external_event(&started.run.id, &payload)
                .expect("matching Runtime lifecycle events are no-ops for Host-owned ToolCalls");
        }
        assert!(database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type":"tool.completed", "toolCallId":"host-failed",
                    "tool":"write_file", "result":{"passed":true}, "isError":false
                }),
            )
            .is_err());
        let preserved = query_tool_call(
            &database.connection.lock().unwrap(),
            &started.run.id,
            "host-failed",
        )
        .unwrap();
        assert_eq!(preserved.status, "failed");
        assert!(preserved.result.is_none());
        assert_eq!(preserved.error_message.as_deref(), Some("command failed"));

        let goal = database
            .goals()
            .create(crate::database::CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "Evidence authority".to_owned(),
                objective: "Reject forged completed ToolCalls".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: started.run.id.clone(),
            })
            .unwrap();
        let task = database
            .work_tasks()
            .create(crate::database::CreateTaskInput {
                id: None,
                goal_id: goal.id,
                parent_task_id: None,
                ordinal: 0,
                title: "Do not trust Runtime overwrite".to_owned(),
                detail: None,
            })
            .unwrap();
        assert!(database
            .task_evidence()
            .add(crate::database::AddEvidenceInput {
                id: None,
                task_id: task.id,
                source_run_id: Some(started.run.id.clone()),
                evidence_type: crate::database::EvidenceType::ToolCall,
                ref_kind: crate::database::EvidenceReferenceKind::ToolCall,
                ref_id: failed_host.id,
                summary: "forged success must not validate".to_owned(),
                metadata: json!({"validationCheckType":"inspection"}),
                trace_id: None,
                span_id: None,
            })
            .is_err());

        database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type":"tool.started", "toolCallId":"runtime-owned",
                    "tool":"read", "input":{"path":"README.md"}
                }),
            )
            .unwrap();
        let exact_completion = json!({
            "type":"tool.completed", "toolCallId":"runtime-owned", "tool":"read",
            "result":{"content":"ok"}, "isError":false
        });
        database
            .record_external_event(&started.run.id, &exact_completion)
            .unwrap();
        database
            .record_external_event(&started.run.id, &exact_completion)
            .expect("exact late Runtime completion is a read-only no-op");
        assert!(database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type":"tool.completed", "toolCallId":"runtime-owned", "tool":"write",
                    "result":{"content":"changed"}, "isError":false
                }),
            )
            .is_err());

        database
            .record_external_event(
                &started.run.id,
                &json!({
                    "type":"tool.started",
                    "toolCallId":"padded-override",
                    "tool":"task_repair_escalate_start",
                    "input":{
                        "taskId":" task-1 ", "attemptId":"attempt-1",
                        "expectedVersion":1, "rootCause":"root",
                        "findingIds":["finding-1"], "escalationReason":"reason"
                    }
                }),
            )
            .unwrap();
        assert!(database
            .create_host_tool_call_once(
                &started.run.id,
                "padded-override",
                "task_repair_escalate_start",
                &json!({
                    "taskId":"task-1", "attemptId":"attempt-1",
                    "expectedVersion":1, "rootCause":"root",
                    "findingIds":["finding-1"], "escalationReason":"reason"
                }),
                "pending",
                true,
            )
            .is_err());
        let padded_state: (String, String, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT execution_location FROM tool_calls
                         WHERE run_id = ?1 AND runtime_tool_call_id = 'padded-override'",
                        [started.run.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls
                         WHERE run_id = ?1 AND runtime_tool_call_id = 'padded-override'",
                        [started.run.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM approvals a JOIN tool_calls t ON t.id = a.tool_call_id
                         WHERE t.run_id = ?1 AND t.runtime_tool_call_id = 'padded-override'",
                        [started.run.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(
            padded_state,
            ("runtime".to_owned(), "running".to_owned(), 0)
        );

        let cancellable = database
            .create_host_tool_call(
                &started.run.id,
                "cancel-race",
                "read_attachment",
                &json!({"attachmentId":"a"}),
                "running",
                false,
            )
            .unwrap();
        database
            .record_external_event(&started.run.id, &json!({"type":"run.cancelled"}))
            .unwrap();
        assert!(database
            .complete_host_tool_call(
                &started.run.id,
                "cancel-race",
                Some(&json!({"content":"late"})),
                None,
            )
            .is_err());
        let cancelled_status: String = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM tool_calls WHERE id = ?1",
                    [cancellable.id.as_str()],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(cancelled_status, "cancelled");

        let irreversible = database
            .create_run(&conversation.id, "terminal runs are irreversible", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&irreversible.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .apply_runtime_event(
                &irreversible.id,
                2,
                &json!({
                    "type":"tool.started", "toolCallId":"before-terminal",
                    "tool":"read", "input":{"path":"README.md"}
                }),
            )
            .unwrap();
        database
            .apply_runtime_event(&irreversible.id, 3, &json!({"type":"run.completed"}))
            .unwrap();
        assert!(
            database
                .apply_runtime_event(
                    &irreversible.id,
                    4,
                    &json!({
                        "type":"tool.started", "toolCallId":"before-terminal",
                        "tool":"read", "input":{"path":"README.md"}
                    }),
                )
                .is_err(),
            "terminal allowlist rejects even exact late ToolCall projections"
        );
        assert!(database
            .apply_runtime_event(
                &irreversible.id,
                5,
                &json!({
                    "type":"tool.started", "toolCallId":"after-terminal",
                    "tool":"write", "input":{"path":"late.md"}
                }),
            )
            .is_err());
        assert!(database
            .apply_runtime_event(&irreversible.id, 6, &json!({"type":"run.started"}))
            .is_err());
        let (run_status, old_tool_status, fresh_tool_count): (String, String, i64) = database
            .with_connection(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT status FROM runs WHERE id = ?1",
                        [irreversible.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM tool_calls
                         WHERE run_id = ?1 AND runtime_tool_call_id = 'before-terminal'",
                        [irreversible.id.as_str()],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM tool_calls
                         WHERE run_id = ?1 AND runtime_tool_call_id = 'after-terminal'",
                        [irreversible.id.as_str()],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(run_status, "completed");
        assert_eq!(old_tool_status, "interrupted");
        assert_eq!(fresh_tool_count, 0);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn host_tool_call_execution_disposition_has_one_concurrent_winner() {
        use std::sync::{Arc, Barrier};

        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let started = database
            .create_run(&conversation.id, "exactly once host tool", None)
            .unwrap();
        database
            .apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for _ in 0..2 {
            let contender = Database::open(path.clone()).unwrap();
            let barrier = barrier.clone();
            let run_id = started.run.id.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                contender
                    .create_host_tool_call_once(
                        &run_id,
                        "same-call",
                        "memory_propose",
                        &json!({"content":"one proposal"}),
                        "running",
                        false,
                    )
                    .unwrap()
                    .1
            }));
        }
        let dispositions = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            dispositions
                .iter()
                .filter(|value| **value == HostToolCallDisposition::Created)
                .count(),
            1
        );
        assert_eq!(
            dispositions
                .iter()
                .filter(|value| **value == HostToolCallDisposition::AlreadyInFlight)
                .count(),
            1
        );
        database
            .complete_host_tool_call(
                &started.run.id,
                "same-call",
                Some(&json!({"stored":"authoritative"})),
                None,
            )
            .unwrap();
        let (_, replay) = database
            .create_host_tool_call_once(
                &started.run.id,
                "same-call",
                "memory_propose",
                &json!({"content":"one proposal"}),
                "running",
                false,
            )
            .unwrap();
        assert_eq!(replay, HostToolCallDisposition::ReplayTerminal);
        database
            .apply_runtime_event(&started.run.id, 2, &json!({"type":"run.completed"}))
            .unwrap();

        for terminal_event in [
            json!({"type":"run.completed"}),
            json!({"type":"run.failed", "code":"failed", "message":"failed"}),
            json!({"type":"run.cancelled"}),
            json!({"type":"run.interrupted", "code":"interrupted", "message":"interrupted"}),
        ] {
            let terminal_run = database
                .create_run(&conversation.id, "late request", None)
                .unwrap()
                .run;
            database
                .apply_runtime_event(&terminal_run.id, 1, &json!({"type":"run.started"}))
                .unwrap();
            database
                .apply_runtime_event(&terminal_run.id, 2, &terminal_event)
                .unwrap();
            let late_call_id = format!(
                "late-{}",
                terminal_event["type"].as_str().unwrap_or("terminal")
            );
            assert!(database
                .create_host_tool_call_once(
                    &terminal_run.id,
                    &late_call_id,
                    "run_command",
                    &json!({"command":"must-not-run"}),
                    "running",
                    false,
                )
                .is_err());
            let count: i64 = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*) FROM tool_calls
                         WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                        params![terminal_run.id, late_call_id],
                        |row| row.get(0),
                    )
                })
                .unwrap();
            assert_eq!(count, 0, "late fresh request must not leave a ToolCall");
        }

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn run_terminal_outcomes_are_immutable_across_runtime_and_late_host_events() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();

        let completed = database
            .create_run(&conversation.id, "complete once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&completed.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let completed_outcome = json!({"type":"run.completed","completionReason":"awaiting_user"});
        database
            .apply_runtime_event(&completed.id, 2, &completed_outcome)
            .unwrap();
        let root_trace_before: (
            Option<i64>,
            Option<i64>,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT ended_at, duration_ms, status, attributes_json, error_type, error_message
                     FROM trace_spans WHERE run_id = ?1 AND category = 'run'",
                    [completed.id.as_str()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
            })
            .unwrap();
        database
            .apply_runtime_event(&completed.id, 3, &completed_outcome)
            .expect("same completed terminal outcome is idempotent");
        let root_trace_after: (
            Option<i64>,
            Option<i64>,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT ended_at, duration_ms, status, attributes_json, error_type, error_message
                     FROM trace_spans WHERE run_id = ?1 AND category = 'run'",
                    [completed.id.as_str()],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                        ))
                    },
                )
            })
            .unwrap();
        assert_eq!(
            root_trace_after, root_trace_before,
            "exact terminal replay must not rewrite closed trace facts"
        );
        assert!(database
            .apply_runtime_event(&completed.id, 4, &json!({"type":"run.completed"}))
            .is_err());
        assert!(database
            .apply_runtime_event(
                &completed.id,
                5,
                &json!({"type":"run.failed","code":"late","message":"late crash"}),
            )
            .is_err());
        assert!(database
            .apply_runtime_event(&completed.id, 6, &json!({"type":"run.cancelled"}))
            .is_err());
        assert!(database
            .mark_run_failed(&completed.id, "late.host", "late Host crash")
            .is_err());

        let failed = database
            .create_run(&conversation.id, "fail once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&failed.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let failed_outcome =
            json!({"type":"run.failed","code":"runtime.failed","message":"stable failure"});
        database
            .apply_runtime_event(&failed.id, 2, &failed_outcome)
            .unwrap();
        database
            .apply_runtime_event(&failed.id, 3, &failed_outcome)
            .expect("same failed terminal outcome is idempotent");
        assert!(database
            .apply_runtime_event(
                &failed.id,
                4,
                &json!({"type":"run.failed","code":"runtime.failed","message":"changed"}),
            )
            .is_err());
        assert!(database
            .apply_runtime_event(
                &failed.id,
                5,
                &json!({
                    "type":"run.failed",
                    "code":"runtime.failed",
                    "message":"stable failure",
                    "futureSemanticField": true
                }),
            )
            .is_err());
        assert!(database
            .apply_runtime_event(&failed.id, 6, &json!({"type":"run.completed"}))
            .is_err());

        let cancelled = database
            .create_run(&conversation.id, "cancel once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&cancelled.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .apply_runtime_event(&cancelled.id, 2, &json!({"type":"run.cancelled"}))
            .unwrap();
        database
            .apply_runtime_event(&cancelled.id, 3, &json!({"type":"run.cancelled"}))
            .expect("same cancelled terminal outcome is idempotent");
        assert!(database
            .apply_runtime_event(
                &cancelled.id,
                4,
                &json!({"type":"run.cancelled","reason":"late semantic change"}),
            )
            .is_err());
        assert!(database
            .apply_runtime_event(&cancelled.id, 5, &json!({"type":"run.completed"}))
            .is_err());

        let completed_without_reason = database
            .create_run(&conversation.id, "complete without reason", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(
                &completed_without_reason.id,
                1,
                &json!({"type":"run.completed"}),
            )
            .unwrap();
        assert!(database
            .apply_runtime_event(
                &completed_without_reason.id,
                2,
                &json!({"type":"run.completed","completionReason":"awaiting_user"}),
            )
            .is_err());

        let interrupted = database
            .create_run(&conversation.id, "interrupt once", None)
            .unwrap()
            .run;
        let interrupted_outcome = json!({"type":"run.interrupted","code":"runtime.interrupted","message":"stable interruption"});
        database
            .apply_runtime_event(&interrupted.id, 1, &interrupted_outcome)
            .unwrap();
        database
            .apply_runtime_event(&interrupted.id, 2, &interrupted_outcome)
            .expect("same interrupted terminal outcome is idempotent");
        assert!(database
            .apply_runtime_event(
                &interrupted.id,
                3,
                &json!({
                    "type":"run.interrupted",
                    "code":"runtime.interrupted",
                    "message":"stable interruption",
                    "futureSemanticField":"different"
                }),
            )
            .is_err());

        for (label, first_terminal, conflicting_terminal, expected_status) in [
            (
                "queued-completed",
                json!({"type":"run.completed"}),
                json!({"type":"run.failed","code":"late","message":"late"}),
                "completed",
            ),
            (
                "queued-failed",
                json!({"type":"run.failed","code":"queued.failed","message":"queued failed"}),
                json!({"type":"run.completed"}),
                "failed",
            ),
        ] {
            let run = database
                .create_run(&conversation.id, label, None)
                .unwrap()
                .run;
            database
                .apply_runtime_event(&run.id, 1, &first_terminal)
                .unwrap();
            assert!(database
                .apply_runtime_event(&run.id, 2, &conflicting_terminal)
                .is_err());
            let status: String = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT status FROM runs WHERE id = ?1",
                        [run.id.as_str()],
                        |row| row.get(0),
                    )
                })
                .unwrap();
            assert_eq!(status, expected_status);
        }

        for (label, first_terminal, conflicting_terminal, expected_status) in [
            (
                "cancelling-completed",
                json!({"type":"run.completed"}),
                json!({"type":"run.failed","code":"late","message":"late"}),
                "completed",
            ),
            (
                "cancelling-failed",
                json!({"type":"run.failed","code":"cancel.failed","message":"cancel failed"}),
                json!({"type":"run.completed"}),
                "failed",
            ),
        ] {
            let run = database
                .create_run(&conversation.id, label, None)
                .unwrap()
                .run;
            database
                .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
                .unwrap();
            assert!(database.mark_run_cancelling(&run.id).unwrap().is_some());
            database
                .apply_runtime_event(&run.id, 2, &first_terminal)
                .unwrap();
            assert!(database
                .apply_runtime_event(&run.id, 3, &conflicting_terminal)
                .is_err());
            let status: String = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT status FROM runs WHERE id = ?1",
                        [run.id.as_str()],
                        |row| row.get(0),
                    )
                })
                .unwrap();
            assert_eq!(status, expected_status);
        }

        let outcomes: Vec<(String, Option<String>, Option<String>)> = database
            .with_connection(|connection| {
                let mut statement = connection.prepare(
                    "SELECT status, error_code, error_message FROM runs
                     WHERE id IN (?1, ?2, ?3) ORDER BY id",
                )?;
                let rows = statement
                    .query_map(params![completed.id, failed.id, cancelled.id], |row| {
                        Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                    })?;
                rows.collect::<Result<Vec<_>, _>>()
            })
            .unwrap();
        assert!(outcomes.iter().any(|outcome| outcome.0 == "completed"));
        assert!(outcomes.iter().any(|outcome| {
            outcome.0 == "failed"
                && outcome.1.as_deref() == Some("runtime.failed")
                && outcome.2.as_deref() == Some("stable failure")
        }));
        assert!(outcomes.iter().any(|outcome| {
            outcome.0 == "cancelled" && outcome.1.is_none() && outcome.2.is_none()
        }));

        for (run_id, event_type, expected_payload) in [
            (&completed.id, "run.completed", &completed_outcome),
            (&failed.id, "run.failed", &failed_outcome),
            (&interrupted.id, "run.interrupted", &interrupted_outcome),
        ] {
            let (count, latest_json): (i64, String) = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*),
                                (SELECT event_json FROM run_events
                                 WHERE run_id = ?1 AND event_type = ?2
                                 ORDER BY seq DESC LIMIT 1)
                         FROM run_events WHERE run_id = ?1 AND event_type = ?2",
                        params![run_id, event_type],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                })
                .unwrap();
            assert_eq!(
                count, 1,
                "exact terminal replay advances sequence without duplicating UI facts"
            );
            assert_eq!(parse_json(&latest_json), *expected_payload);
        }

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancelling_run_ignores_late_non_terminal_runtime_events() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let run = database
            .create_run(&conversation.id, "cancel during model startup", None)
            .expect("create run")
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .expect("start run");
        assert!(database.mark_run_cancelling(&run.id).unwrap().is_some());

        for (seq, event) in [
            json!({"type":"message.started"}),
            json!({"type":"message.delta","delta":"late answer"}),
            json!({"type":"tool.started","toolCallId":"late-tool","tool":"read","input":{}}),
            json!({"type":"run.phase","phase":"finalizing"}),
        ]
        .into_iter()
        .enumerate()
        {
            assert!(!database
                .apply_runtime_event(&run.id, seq as i64 + 2, &event)
                .expect("ignore stale event while cancelling"));
        }
        database
            .apply_runtime_event(&run.id, 6, &json!({"type":"run.cancelled"}))
            .expect("finish cancellation");

        let detail = database
            .load_conversation(&conversation.id)
            .expect("load cancelled conversation");
        assert_eq!(
            detail.last_run.as_ref().map(|item| item.status.as_str()),
            Some("cancelled")
        );
        assert!(!detail
            .messages
            .iter()
            .any(|message| message.role == "assistant"));
        assert!(detail.tool_calls.is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_runs_reject_late_message_mutation_and_message_completion_replay_is_exact() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();

        for (terminal, expected_message_status) in [
            (json!({"type":"run.completed"}), "completed"),
            (
                json!({"type":"run.failed","code":"runtime.failed","message":"failed"}),
                "interrupted",
            ),
            (json!({"type":"run.cancelled"}), "interrupted"),
            (
                json!({"type":"run.interrupted","code":"runtime.interrupted","message":"interrupted"}),
                "interrupted",
            ),
        ] {
            let run = database
                .create_run(&conversation.id, "terminal message immutability", None)
                .unwrap()
                .run;
            database
                .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
                .unwrap();
            database
                .apply_runtime_event(&run.id, 2, &json!({"type":"message.started"}))
                .unwrap();
            database
                .apply_runtime_event(
                    &run.id,
                    3,
                    &json!({"type":"message.delta","delta":"frozen answer"}),
                )
                .unwrap();
            let terminal_seq = if expected_message_status == "completed" {
                database
                    .apply_runtime_event(&run.id, 4, &json!({"type":"message.completed"}))
                    .unwrap();
                5
            } else {
                4
            };
            database
                .apply_runtime_event(&run.id, terminal_seq, &terminal)
                .unwrap();

            for late in [
                json!({"type":"message.started"}),
                json!({"type":"message.delta","delta":"tampered"}),
                json!({"type":"message.completed"}),
            ] {
                assert!(database
                    .apply_runtime_event(&run.id, terminal_seq + 10, &late)
                    .is_err());
            }
            let (status, content, late_events): (String, String, i64) = database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT status, content,
                                (SELECT COUNT(*) FROM run_events
                                 WHERE run_id = ?2 AND seq > ?3)
                         FROM messages WHERE id = ?1",
                        params![format!("assistant-{}", run.id), run.id, terminal_seq],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                })
                .unwrap();
            assert_eq!(status, expected_message_status);
            assert_eq!(content, "frozen answer");
            assert_eq!(
                late_events, 0,
                "rejected message mutations must roll back events"
            );
        }

        let replay = database
            .create_run(&conversation.id, "message completion replay", None)
            .unwrap()
            .run;
        for (seq, event) in [
            json!({"type":"run.started"}),
            json!({"type":"message.started"}),
            json!({"type":"message.delta","delta":"stable"}),
            json!({"type":"message.completed","finishReason":"stop"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&replay.id, seq as i64 + 1, &event)
                .unwrap();
        }
        database
            .apply_runtime_event(
                &replay.id,
                5,
                &json!({"finishReason":"stop","type":"message.completed"}),
            )
            .expect("object key order is canonicalized by JSON value equality");
        assert!(database
            .apply_runtime_event(
                &replay.id,
                6,
                &json!({"type":"message.completed","finishReason":"length"}),
            )
            .is_err());
        let (status, content): (String, String) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status, content FROM messages WHERE id = ?1",
                    [format!("assistant-{}", replay.id)],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(status, "completed");
        assert_eq!(content, "stable");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_run_event_allowlist_accepts_only_exact_outcome_replay() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "terminal allowlist", None)
            .unwrap()
            .run;
        for (seq, event) in [
            json!({"type":"run.started"}),
            json!({
                "type":"usage.updated",
                "inputTokens":10,
                "outputTokens":5,
                "cacheReadTokens":2,
                "cacheWriteTokens":1,
                "totalTokens":18
            }),
            json!({"type":"run.completed","completionReason":"stop"}),
        ]
        .into_iter()
        .enumerate()
        {
            database
                .apply_runtime_event(&run.id, seq as i64 + 1, &event)
                .unwrap();
        }
        assert!(
            database
                .apply_runtime_event(
                    &run.id,
                    4,
                    &json!({
                        "type":"usage.updated",
                        "inputTokens":12,
                        "outputTokens":7,
                        "cacheReadTokens":2,
                        "cacheWriteTokens":1,
                        "totalTokens":22
                    }),
                )
                .is_err(),
            "terminal Run rejects even well-formed late usage"
        );
        for rejected in [
            json!({
                "type":"usage.updated",
                "inputTokens":11,
                "outputTokens":7,
                "cacheReadTokens":2,
                "cacheWriteTokens":1,
                "totalTokens":21
            }),
            json!({
                "type":"usage.updated",
                "inputTokens":12,
                "outputTokens":-1,
                "cacheReadTokens":2,
                "cacheWriteTokens":1,
                "totalTokens":22
            }),
            json!({
                "type":"usage.updated",
                "inputTokens":12,
                "outputTokens":7,
                "cacheReadTokens":2,
                "cacheWriteTokens":1,
                "totalTokens":22,
                "futureCost":0
            }),
            json!({"type":"message.delta","delta":"late mutation"}),
            json!({"type":"reasoning.delta","delta":"late reasoning"}),
            json!({"type":"tool.started","toolCallId":"late","tool":"write_file","input":{}}),
            json!({"type":"user.question.requested","questions":[]}),
            json!({"type":"task.completed","taskId":"runtime-spoof"}),
            json!({"type":"continuation_proposed","decision":"complete"}),
            json!({"type":"runtime.unknown"}),
        ] {
            assert!(database.apply_runtime_event(&run.id, 5, &rejected).is_err());
        }
        database
            .apply_runtime_event(
                &run.id,
                6,
                &json!({"completionReason":"stop","type":"run.completed"}),
            )
            .expect("canonical exact terminal replay remains idempotent");

        let (last_seq, event_count, work_event_count, latest_usage): (
            i64,
            i64,
            i64,
            String,
        ) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT runs.last_seq,
                            (SELECT COUNT(*) FROM run_events WHERE run_id = runs.id),
                            (SELECT COUNT(*) FROM work_events WHERE conversation_id = runs.conversation_id),
                            (SELECT event_json FROM run_events
                             WHERE run_id = runs.id AND event_type = 'usage.updated'
                             ORDER BY seq DESC LIMIT 1)
                     FROM runs WHERE id = ?1",
                    [run.id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
            })
            .unwrap();
        assert_eq!(last_seq, 6);
        assert_eq!(
            event_count, 3,
            "rejected events and exact terminal replay add no rows"
        );
        assert_eq!(
            work_event_count, 0,
            "late Runtime work events cannot project facts"
        );
        assert_eq!(parse_json(&latest_usage)["totalTokens"], 18);

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn repair_override_approval_rejects_conversation_grants_and_generic_claims() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "bounded repair override", None)
            .expect("create run");
        database
            .apply_runtime_event(
                &started.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start run");
        let tool = database
            .create_host_tool_call(
                &started.run.id,
                "repair-override",
                "task_repair_escalate_start",
                &json!({
                    "taskId":"task-1",
                    "attemptId":"attempt-1",
                    "expectedVersion":3,
                    "rootCause":"root",
                    "findingIds":["finding-1"],
                    "escalationReason":"operator reviewed the exhausted budget"
                }),
                "pending",
                true,
            )
            .expect("create override ToolCall");
        let approval = database
            .create_task_repair_override_approval(
                &tool.id,
                "operator override",
                &json!({ "category":"task_repair_budget_override" }),
            )
            .expect("create override approval");
        assert!(database
            .create_approval(&tool.id, "operator override", &approval.request)
            .expect_err("generic category replay must fail closed")
            .contains("immutable category"));
        assert!(database
            .resolve_approval(
                &approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect_err("override cannot create a reusable conversation grant")
            .contains("allow_once"));
        let still_pending = database
            .pending_approvals_for_conversation_tool(&conversation.id, "task_repair_escalate_start")
            .expect("load pending override approval");
        assert_eq!(still_pending.len(), 1);
        assert_eq!(still_pending[0].status, "pending");
        let approved = database
            .resolve_approval(&approval.id, crate::database::ApprovalDecision::AllowOnce)
            .expect("resolve allow_once")
            .expect("pending override approval resolved");
        assert_eq!(approved.category, TASK_REPAIR_OVERRIDE_APPROVAL_CATEGORY);
        assert!(!database
            .claim_approved_tool_call(&started.run.id, "repair-override")
            .expect("generic claim path rejects override approval"));
        let (tool_status, claimed_at): (String, Option<i64>) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT t.status, a.claimed_at
                     FROM tool_calls t JOIN approvals a ON a.tool_call_id = t.id
                     WHERE t.id = ?1",
                    [tool.id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .expect("load immutable override state");
        assert_eq!(tool_status, "pending");
        assert!(claimed_at.is_none());
        assert!(!database
            .conversation_tool_permission_granted(
                &conversation.id,
                "task_repair_escalate_start",
                "test-scope",
            )
            .expect("override approval must never persist a grant"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn terminal_crash_cancel_new_run_and_startup_repair_close_pending_approvals() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");

        let crashed = database
            .create_run(&conversation.id, "crash", None)
            .expect("create crashed run");
        database
            .apply_runtime_event(
                &crashed.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start crashed run");
        let crashed_tool = database
            .create_host_tool_call(
                &crashed.run.id,
                "crash-write",
                "write_file",
                &json!({ "path": "crash.txt" }),
                "pending",
                true,
            )
            .expect("create crashed tool");
        let crashed_approval = database
            .create_approval(&crashed_tool.id, "write after crash", &json!({}))
            .expect("create crashed approval");
        database
            .mark_run_failed(&crashed.run.id, "runtime.process_crashed", "crashed")
            .expect("mark run failed");
        assert!(database
            .resolve_approval(
                &crashed_approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect("reject late crash approval")
            .is_none());
        assert!(!database
            .claim_approved_tool_call(&crashed.run.id, "crash-write")
            .expect("reject crashed tool claim"));
        assert!(!database
            .conversation_tool_permission_granted(&conversation.id, "write_file", "test-scope")
            .expect("crash must not grant conversation permission"));

        let cancelled = database
            .create_run(&conversation.id, "cancel", None)
            .expect("create cancelled run");
        database
            .apply_runtime_event(
                &cancelled.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start cancelled run");
        let cancelled_tool = database
            .create_host_tool_call(
                &cancelled.run.id,
                "cancel-write",
                "write_file",
                &json!({ "path": "cancel.txt" }),
                "pending",
                true,
            )
            .expect("create cancelled tool");
        let cancelled_approval = database
            .create_approval(&cancelled_tool.id, "write after cancel", &json!({}))
            .expect("create cancelled approval");
        assert_eq!(
            database
                .mark_run_cancelling(&cancelled.run.id)
                .expect("request cancellation")
                .as_deref(),
            Some(conversation.id.as_str())
        );
        assert!(database
            .resolve_approval(
                &cancelled_approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect("reject late cancelled approval")
            .is_none());
        assert!(!database
            .claim_approved_tool_call(&cancelled.run.id, "cancel-write")
            .expect("reject cancelled tool claim"));
        database
            .apply_runtime_event(&cancelled.run.id, 2, &json!({ "type": "run.cancelled" }))
            .expect("finish cancellation");

        // A pre-fix database could contain a pending Host ToolCall after its Run
        // became terminal. Build that historical corruption directly: the
        // production create path must continue to reject this state.
        let stale_tool_id = "legacy-stale-tool-row";
        let stale_approval_id = "legacy-stale-approval-row";
        database
            .with_connection(|connection| {
                let now = now_ms();
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                        status, execution_location, requires_approval, started_at, updated_at
                     ) VALUES (?1, 'legacy-stale-write', ?2, ?3, 'write_file', ?4,
                               'pending', 'host', 1, ?5, ?5)",
                    params![
                        stale_tool_id,
                        cancelled.run.id,
                        conversation.id,
                        serde_json::to_string(&json!({ "path": "stale.txt" })).unwrap(),
                        now,
                    ],
                )?;
                connection.execute(
                    "INSERT INTO approvals(
                        id, tool_call_id, status, requested_action, request_json, requested_at
                     ) VALUES (?1, ?2, 'pending', 'legacy stale', '{}', ?3)",
                    params![stale_approval_id, stale_tool_id, now],
                )?;
                Ok(())
            })
            .expect("simulate pre-fix stale approval by controlled raw fixture");
        let restarted = database
            .create_run(&conversation.id, "new run", None)
            .expect("new run expires stale approval");
        let detail = database
            .load_conversation(&conversation.id)
            .expect("load approval audit");
        assert_eq!(
            detail
                .approvals
                .iter()
                .find(|approval| approval.id == crashed_approval.id)
                .unwrap()
                .status,
            "expired"
        );
        assert_eq!(
            detail
                .approvals
                .iter()
                .find(|approval| approval.id == cancelled_approval.id)
                .unwrap()
                .status,
            "cancelled"
        );
        assert_eq!(
            detail
                .approvals
                .iter()
                .find(|approval| approval.id == stale_approval_id)
                .unwrap()
                .status,
            "expired"
        );

        database
            .apply_runtime_event(
                &restarted.run.id,
                1,
                &json!({ "type": "run.started", "model": "test" }),
            )
            .expect("start restart-repair run");
        let restart_tool = database
            .create_host_tool_call(
                &restarted.run.id,
                "restart-write",
                "write_file",
                &json!({ "path": "restart.txt" }),
                "pending",
                true,
            )
            .expect("create restart tool");
        let restart_approval = database
            .create_approval(&restart_tool.id, "restart approval", &json!({}))
            .expect("create restart approval");
        database
            .repair_interrupted_runs()
            .expect("repair startup state");
        assert!(database
            .resolve_approval(
                &restart_approval.id,
                crate::database::ApprovalDecision::AllowConversation,
            )
            .expect("reject late restart approval")
            .is_none());
        assert!(!database
            .conversation_tool_permission_granted(&conversation.id, "write_file", "test-scope")
            .expect("terminal approvals never grant permission"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn round_trips_local_and_remote_knowledge_references_through_v2() {
        let (database, path) = test_database();
        database
            .save_yuxi_service("Local", "http://127.0.0.1:5050")
            .expect("save Yuxi service");
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let local = KnowledgeReference::local("local-kb");
        let remote = KnowledgeReference::remote("yuxi-primary", "remote-kb");

        let records = database
            .set_knowledge_reference_bindings(
                &conversation.id,
                &[
                    KnowledgeReferenceBindingInput {
                        reference: local.clone(),
                        name: "本地知识库".to_owned(),
                    },
                    KnowledgeReferenceBindingInput {
                        reference: remote.clone(),
                        name: "远程知识库".to_owned(),
                    },
                ],
            )
            .expect("write v2 knowledge references");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].reference, local);
        assert_eq!(records[1].reference, remote);

        let references = database
            .conversation_knowledge_references(&conversation.id)
            .expect("read v2 knowledge references");
        assert_eq!(
            references,
            vec![
                KnowledgeReference::local("local-kb"),
                KnowledgeReference::remote("yuxi-primary", "remote-kb")
            ]
        );

        let local_json = serde_json::to_value(&references[0]).expect("serialize local reference");
        assert_eq!(local_json["source"], "local");
        assert_eq!(local_json["providerKey"], "local");
        assert!(local_json.get("connectionId").is_none());
        assert_eq!(local_json["id"], "local-kb");

        let legacy_records = database
            .conversation_knowledge_bindings(&conversation.id)
            .expect("read legacy-compatible bindings");
        assert_eq!(legacy_records.len(), 1);
        assert_eq!(legacy_records[0].service_connection_id, "yuxi-primary");
        assert_eq!(legacy_records[0].knowledge_base_id, "remote-kb");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_legacy_remote_bindings_through_the_v2_reference_api() {
        let (database, path) = test_database();
        database
            .save_yuxi_service("Local", "http://127.0.0.1:5050")
            .expect("save Yuxi service");
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        database
            .set_knowledge_bindings(
                &conversation.id,
                &[("legacy-kb".to_owned(), "历史远程知识库".to_owned())],
            )
            .expect("write legacy remote binding");

        let references = database
            .conversation_knowledge_references(&conversation.id)
            .expect("read migrated remote reference");
        assert_eq!(
            references,
            vec![KnowledgeReference::remote("yuxi-primary", "legacy-kb")]
        );

        let v2_row: (String, String, Option<String>) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT source, provider_key, connection_id
                     FROM knowledge_bindings_v2
                     WHERE conversation_id = ?1 AND knowledge_base_id = 'legacy-kb'",
                    [&conversation.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
            })
            .expect("inspect migrated v2 row");
        assert_eq!(
            v2_row,
            (
                "remote".to_owned(),
                "yuxi-primary".to_owned(),
                Some("yuxi-primary".to_owned())
            )
        );

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
    fn creates_hidden_goal_continuation_run_without_polluting_prompt_history() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_goal_continuation_run(&conversation.id, "continue the current goal")
            .expect("create continuation run");

        assert_eq!(started.user_message.role, "system");
        assert_eq!(started.user_message.kind, "internal");
        assert_eq!(
            database
                .active_run_id(&conversation.id)
                .expect("load active run")
                .as_deref(),
            Some(started.run.id.as_str())
        );
        assert!(database
            .runtime_prompt_context(&conversation.id, 20)
            .expect("load prompt history")
            .is_empty());

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
    fn pending_work_mode_dispatch_survives_restart_and_resolves_the_same_run() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "优化一下登录逻辑", None)
            .expect("create run");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "登录优化".to_owned(),
                objective: "优化登录逻辑".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Proposed,
                created_by: "test".to_owned(),
            })
            .expect("create proposed goal");
        let pending = database
            .save_pending_work_mode_dispatch(&goal.id, &started, "runtime enriched text")
            .expect("persist pending dispatch");
        assert_eq!(pending.run.id, started.run.id);
        assert_eq!(pending.run.status, "awaiting_confirmation");
        assert!(database
            .conversation_has_active_run(&conversation.id)
            .expect("pending confirmation blocks another run"));

        drop(database);
        let database = Database::open(path.clone()).expect("reopen database after restart");
        database.repair_interrupted_runs().expect("repair startup");
        let restored = database
            .pending_work_mode_dispatch(&goal.id)
            .expect("load pending dispatch")
            .expect("pending dispatch remains");
        assert_eq!(restored.started.run.id, started.run.id);
        assert_eq!(restored.started.run.status, "awaiting_confirmation");
        assert_eq!(restored.runtime_text, "runtime enriched text");

        let released = database
            .release_pending_work_mode_dispatch(&goal.id)
            .expect("release pending dispatch");
        assert_eq!(released.started.run.id, started.run.id);
        assert_eq!(released.started.run.status, "queued");
        assert!(database
            .pending_work_mode_dispatch(&goal.id)
            .expect("reload released dispatch")
            .is_none());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejecting_work_mode_cancels_the_undispatched_run() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "删除所有生产环境数据", None)
            .expect("create run");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "高风险任务".to_owned(),
                objective: "等待用户确认".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Proposed,
                created_by: "test".to_owned(),
            })
            .expect("create proposed goal");
        database
            .save_pending_work_mode_dispatch(&goal.id, &started, "do not dispatch")
            .expect("persist pending dispatch");
        assert!(database
            .reject_pending_work_mode_dispatch(&goal.id)
            .expect("reject pending dispatch"));
        let detail = database
            .load_conversation(&conversation.id)
            .expect("reload conversation");
        let run = detail.last_run.expect("last run");
        assert_eq!(run.id, started.run.id);
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.error_code.as_deref(), Some("work_mode.declined"));
        assert_eq!(
            detail.messages.last().expect("user message").status,
            "completed"
        );

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
        assert_eq!(conversation.permission_mode, "ask");
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
        // REV-03: the project default is copied into a conversation at creation
        // and never reaches an existing one afterwards. The existing
        // conversation keeps its own effective mode...
        assert_eq!(
            database
                .load_conversation(&conversation.id)
                .expect("reload project conversation")
                .conversation
                .permission_mode,
            "ask",
            "a project-default change must not rewrite an existing conversation"
        );
        assert_eq!(
            database
                .conversation_permission_mode(&conversation.id)
                .expect("effective mode"),
            "ask"
        );
        // ...while a conversation created afterwards starts from the new default.
        let later = database
            .create_conversation(DEFAULT_AGENT_ID, None, root.to_str(), Some("allow"))
            .expect("create a later project conversation");
        assert_eq!(
            database
                .conversation_permission_mode(&later.id)
                .expect("the later conversation's effective mode"),
            "allow",
            "a conversation created after the default changed starts from it"
        );
        assert_eq!(
            database.execution_policy(&later.id).expect("policy").version,
            1,
            "a brand-new conversation's policy starts at generation 1"
        );
        assert!(database
            .update_project_permission_mode(project_id, "unrestricted")
            .is_err());

        let no_project = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, Some("allow"))
            .expect("create plain conversation");
        assert_eq!(
            database
                .conversation_project_root(&no_project.id)
                .expect("load empty project root"),
            None
        );
        assert_eq!(no_project.permission_mode, "allow");
        assert_eq!(
            database
                .conversation_permission_mode(&no_project.id)
                .expect("load projectless permission"),
            "allow"
        );
        let updated = database
            .update_conversation_permission_mode(&no_project.id, "ask")
            .expect("update projectless permission")
            .expect("plain conversation exists");
        assert_eq!(updated.permission_mode, "ask");
        assert!(database
            .update_conversation_permission_mode(&no_project.id, "unrestricted")
            .is_err());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cumulative_usage_is_strict_monotonic_atomic_and_counted_once_per_run() {
        let (database, path) = test_database();
        let conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .expect("create conversation");
        let started = database
            .create_run(&conversation.id, "test", Some("model"))
            .expect("start run");
        database
            .apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        for (seq, usage) in [
            (
                2,
                json!({"type":"usage.updated","inputTokens":10,"outputTokens":15,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":35}),
            ),
            (
                3,
                json!({"type":"usage.updated","inputTokens":12,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":42}),
            ),
        ] {
            database
                .apply_runtime_event(&started.run.id, seq, &usage)
                .expect("append usage event");
        }
        assert!(!database
            .apply_runtime_event(
                &started.run.id,
                3,
                &json!({"type":"usage.updated","inputTokens":12,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":42}),
            )
            .expect("same sequence replay is ignored"));
        for invalid in [
            json!({"type":"usage.updated","inputTokens":11,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":41}),
            json!({"type":"usage.updated","inputTokens":12,"outputTokens":-1,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":42}),
            json!({"type":"usage.updated","inputTokens":12,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":41}),
            json!({"type":"usage.updated","inputTokens":12,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":42,"unexpected":true}),
            json!({"type":"usage.updated","inputTokens":1_000_000_000_001_i64,"outputTokens":20,"cacheReadTokens":8,"cacheWriteTokens":2,"totalTokens":1_000_000_000_031_i64}),
        ] {
            assert!(database
                .apply_runtime_event(&started.run.id, 4, &invalid)
                .is_err());
        }
        let statistics = database.usage_statistics().expect("usage statistics");
        assert_eq!(statistics.conversation_count, 1);
        assert_eq!(statistics.input_tokens, 12);
        assert_eq!(statistics.output_tokens, 20);
        assert_eq!(statistics.cache_read_tokens, 8);
        assert_eq!(statistics.cache_write_tokens, 2);
        assert_eq!(statistics.total_tokens, 42);
        assert_eq!(statistics.agents[0].total_tokens, 42);
        let (last_seq, usage_events): (i64, i64) = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT last_seq,
                            (SELECT COUNT(*) FROM run_events
                             WHERE run_id = runs.id AND event_type = 'usage.updated')
                     FROM runs WHERE id = ?1",
                    [started.run.id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!((last_seq, usage_events), (3, 2));

        let first_usage_at = now_ms().saturating_sub(2 * 86_400_000);
        let second_usage_at = now_ms().saturating_sub(86_400_000);
        let (first_day, second_day): (String, String) = database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE run_events SET created_at = CASE seq
                       WHEN 2 THEN ?2 WHEN 3 THEN ?3 ELSE created_at END
                     WHERE run_id = ?1 AND event_type = 'usage.updated'",
                    params![started.run.id, first_usage_at, second_usage_at],
                )?;
                connection.query_row(
                    "SELECT date(?1 / 1000, 'unixepoch', 'localtime'),
                            date(?2 / 1000, 'unixepoch', 'localtime')",
                    params![first_usage_at, second_usage_at],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_ne!(first_day, second_day);
        let daily = database.usage_statistics().expect("daily usage deltas");
        assert_eq!(daily.total_tokens, 42, "global total stays latest-per-Run");
        assert_eq!(
            daily
                .days
                .iter()
                .find(|day| day.date == first_day)
                .unwrap()
                .total_tokens,
            35,
            "the first cumulative amount belongs to its event day"
        );
        assert_eq!(
            daily
                .days
                .iter()
                .find(|day| day.date == second_day)
                .unwrap()
                .total_tokens,
            7,
            "only the cumulative delta belongs to the later event day"
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn daily_usage_falls_back_to_latest_event_for_legacy_non_cumulative_runs() {
        let (database, path) = test_database();
        let legacy_conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let legacy = database
            .create_run(&legacy_conversation.id, "legacy usage", None)
            .unwrap()
            .run;
        let cumulative_conversation = database
            .create_conversation(DEFAULT_AGENT_ID, None, None, None)
            .unwrap();
        let cumulative = database
            .create_run(&cumulative_conversation.id, "cumulative usage", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&cumulative.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .apply_runtime_event(
                &cumulative.id,
                2,
                &json!({
                    "type":"usage.updated","inputTokens":30,"outputTokens":10,
                    "cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":40
                }),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &cumulative.id,
                3,
                &json!({
                    "type":"usage.updated","inputTokens":45,"outputTokens":15,
                    "cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":60
                }),
            )
            .unwrap();

        let current = now_ms();
        let legacy_first_at = current.saturating_sub(4 * 86_400_000);
        let legacy_last_at = current.saturating_sub(3 * 86_400_000);
        let cumulative_first_at = current.saturating_sub(2 * 86_400_000);
        let cumulative_last_at = current.saturating_sub(86_400_000);
        let dates: Vec<String> = database
            .with_connection(|connection| {
                for (seq, created_at, payload) in [
                    (
                        1_i64,
                        legacy_first_at,
                        json!({
                            "type":"usage.updated","inputTokens":80,"outputTokens":20,
                            "cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":100
                        }),
                    ),
                    (
                        2_i64,
                        legacy_last_at,
                        json!({
                            "type":"usage.updated","inputTokens":40,"outputTokens":10,
                            "cacheReadTokens":0,"cacheWriteTokens":0,"totalTokens":50
                        }),
                    ),
                ] {
                    connection.execute(
                        "INSERT INTO run_events(
                           id, run_id, seq, event_type, event_json, created_at
                         ) VALUES (?1, ?2, ?3, 'usage.updated', ?4, ?5)",
                        params![
                            Uuid::new_v4().to_string(),
                            legacy.id,
                            seq,
                            payload.to_string(),
                            created_at
                        ],
                    )?;
                }
                connection.execute("UPDATE runs SET last_seq = 2 WHERE id = ?1", [&legacy.id])?;
                connection.execute(
                    "UPDATE run_events SET created_at = CASE seq
                       WHEN 2 THEN ?2 WHEN 3 THEN ?3 ELSE created_at END
                     WHERE run_id = ?1 AND event_type = 'usage.updated'",
                    params![cumulative.id, cumulative_first_at, cumulative_last_at],
                )?;
                [
                    legacy_first_at,
                    legacy_last_at,
                    cumulative_first_at,
                    cumulative_last_at,
                ]
                .into_iter()
                .map(|at| {
                    connection.query_row(
                        "SELECT date(?1 / 1000, 'unixepoch', 'localtime')",
                        [at],
                        |row| row.get(0),
                    )
                })
                .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap();
        assert_eq!(
            dates.iter().collect::<std::collections::HashSet<_>>().len(),
            4
        );

        let statistics = database.usage_statistics().unwrap();
        let tokens_for = |date: &str| {
            statistics
                .days
                .iter()
                .find(|day| day.date == date)
                .unwrap()
                .total_tokens
        };
        assert_eq!(tokens_for(&dates[0]), 0);
        assert_eq!(tokens_for(&dates[1]), 50);
        assert_eq!(tokens_for(&dates[2]), 40);
        assert_eq!(tokens_for(&dates[3]), 20);
        assert_eq!(statistics.total_tokens, 110);
        assert_eq!(statistics.agents[0].total_tokens, 110);
        assert_eq!(
            statistics
                .days
                .iter()
                .map(|day| day.total_tokens)
                .sum::<i64>(),
            statistics.total_tokens,
            "mixed legacy and cumulative Runs must keep daily and global totals equal"
        );

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

    #[test]
    fn persists_toggles_and_audits_declarative_lifecycle_hooks() {
        let (database, path) = test_database();
        let hook = database
            .save_lifecycle_hook(
                "hook-1",
                "Require shell approval",
                "before_tool",
                "run_command, *_write",
                "require_approval",
                "Project mutations require review",
                true,
                20,
            )
            .expect("save lifecycle hook");
        assert!(hook.enabled);
        assert_eq!(database.list_lifecycle_hooks().unwrap(), vec![hook.clone()]);
        let disabled = database
            .set_lifecycle_hook_enabled(&hook.id, false)
            .expect("toggle hook")
            .expect("hook exists");
        assert!(!disabled.enabled);
        database
            .record_lifecycle_hook_execution(
                &hook.id,
                None,
                Some("tool-call-1"),
                "before_tool",
                Some("run_command"),
                "require_approval",
                "applied",
                &json!({ "inputKeys": ["command"] }),
            )
            .expect("record hook execution");
        assert_eq!(
            database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT COUNT(*) FROM lifecycle_hook_executions WHERE hook_id = ?1",
                        [&hook.id],
                        |row| row.get::<_, i64>(0),
                    )
                })
                .unwrap(),
            1
        );
        assert!(database.delete_lifecycle_hook(&hook.id).unwrap());
        assert!(database.list_lifecycle_hooks().unwrap().is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    /// Insert one settled tool call directly, so the range reader can be tested
    /// against a real row without driving the whole runtime projection.
    fn seed_settled_tool_call(
        database: &Database,
        conversation_id: &str,
        run_id: &str,
        call_id: &str,
        tool_name: &str,
        result: &Value,
    ) {
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                        status, result_json, execution_location, requires_approval, started_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, '{}', 'completed', ?6, 'runtime', 0, 1, 1)",
                    params![
                        Uuid::new_v4().to_string(),
                        call_id,
                        run_id,
                        conversation_id,
                        tool_name,
                        serde_json::to_string(result).unwrap(),
                    ],
                )?;
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn tool_result_range_reads_every_stored_byte_under_conversation_authorization() {
        let path = std::env::temp_dir().join(format!("fox-tool-result-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("range"), None, None)
            .unwrap();
        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .unwrap();
        let started = database.create_run(&conversation.id, "range", None).unwrap();
        let text = "中文内容😀".repeat(20);
        seed_settled_tool_call(
            &database,
            &conversation.id,
            &started.run.id,
            "call-1",
            "read_attachment",
            &json!([{"type":"text","text": text}]),
        );
        let reference = format!("fox-result://{}/call-1", started.run.id);

        // Walk the whole stored result in small ranges and reassemble it.
        let mut assembled = String::new();
        let mut offset = 0usize;
        loop {
            let range = database
                .tool_result_range(&reference, &conversation.id, offset, 7)
                .unwrap();
            assert_eq!(range.conversation_id, conversation.id);
            assert_eq!(range.tool_name, "read_attachment");
            assert!(!range.truncated);
            assert_eq!(range.original_bytes, text.len());
            assert!(
                range.returned_bytes <= 7,
                "a range read must respect its own limit"
            );
            assert!(
                !range.content.contains('\u{FFFD}'),
                "a range read must never split a multi-byte character"
            );
            assembled.push_str(&range.content);
            match range.next_offset {
                Some(next) => {
                    assert!(next > offset, "next offset must make progress");
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(assembled, text, "every stored byte must be reachable");

        // Authorization: a reference minted in one conversation cannot be read
        // from another, and a missing authorized conversation is refused.
        assert!(database
            .tool_result_range(&reference, &other.id, 0, 64)
            .is_err());
        assert!(database.tool_result_range(&reference, "  ", 0, 64).is_err());
        // Malformed or foreign references never resolve.
        assert!(database
            .tool_result_range("fox-result://only-a-run", &conversation.id, 0, 64)
            .is_err());
        assert!(database
            .tool_result_range(
                &format!("fox-result://{}/missing-call", started.run.id),
                &conversation.id,
                0,
                64,
            )
            .is_err());

        // Reading is a pure read: the stored row is untouched and a second read
        // returns exactly the same bytes (no tool re-execution, no replay).
        let row = database
            .with_connection(|connection| {
                Ok(query_tool_call(connection, &started.run.id, "call-1").unwrap())
            })
            .unwrap();
        assert_eq!(row.status, "completed");
        assert_eq!(
            database
                .tool_result_range(&reference, &conversation.id, 0, 64)
                .unwrap()
                .content,
            database
                .tool_result_range(&reference, &conversation.id, 0, 64)
                .unwrap()
                .content
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tool_result_range_reports_a_bounded_stored_preview_instead_of_claiming_full_text() {
        let path = std::env::temp_dir().join(format!("fox-tool-result-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("preview"), None, None)
            .unwrap();
        let started = database.create_run(&conversation.id, "preview", None).unwrap();
        // Above MAX_STORED_TOOL_RESULT_BYTES the row holds a Fox preview, so the
        // reader must say so instead of implying the full text is available.
        let stored = json!({
            "truncated": true,
            "originalBytes": 200_000,
            "preview": "p".repeat(8_000),
            "message": "Large tool result was summarized by Fox."
        });
        seed_settled_tool_call(
            &database,
            &conversation.id,
            &started.run.id,
            "call-big",
            "office_read",
            &stored,
        );
        let range = database
            .tool_result_range(
                &format!("fox-result://{}/call-big", started.run.id),
                &conversation.id,
                0,
                64_000,
            )
            .unwrap();
        assert!(range.truncated, "a bounded stored copy must be reported as truncated");
        assert!(
            !range.retrievable,
            "storage only kept a preview, so nothing beyond it is reachable from here"
        );
        assert_eq!(range.original_bytes, 200_000);
        assert!(range.content.starts_with('p'));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn replacing_large_progress_clears_blob_pointer_for_small_empty_and_error_results() {
        let database = Database::open(std::env::temp_dir().join(format!("fox-blob-replacement-{}.db", Uuid::new_v4()))).unwrap();
        let conversation = database.create_conversation(database.default_agent_id(), Some("blob replacement"), None, None).unwrap();
        let started = database.create_run(&conversation.id, "test", None).unwrap();
        let big = json!({"content":[{"type":"text","text":"progress".repeat(30_000)}]});
        for (index, (final_result, is_error, host)) in [
            (json!({"content":[{"type":"text","text":"final-small"}]}), false, false),
            (Value::Null, false, false),
            (json!({"error":"final failure"}), true, false),
            (json!({"content":[{"type":"text","text":"host-final"}]}), false, true),
        ].into_iter().enumerate() {
            let call = format!("replace-{index}");
            database.with_connection(|connection| {
                connection.execute("UPDATE runs SET status='running' WHERE id=?1", [&started.run.id])?;
                connection.execute("INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,execution_location,requires_approval,started_at,updated_at)
                    VALUES (?1,?2,?3,?4,'read','{}','running','runtime',0,1,1)",
                    params![Uuid::new_v4().to_string(),call,started.run.id,conversation.id])?;
                let tx = connection.transaction()?;
                project_tool_updated(&tx, &started.run.id, &json!({"toolCallId":call,"tool":"read","update":big}), 2)?;
                tx.commit()
            }).unwrap();
            let reference = format!("fox-result://{}/{call}",started.run.id);
            assert!(database.tool_result_range(&reference,&conversation.id,0,4096).unwrap().content.starts_with("progress"));
            if host {
                database.with_connection(|connection| {
                    connection.execute("UPDATE tool_calls SET execution_location='host' WHERE run_id=?1 AND runtime_tool_call_id=?2", params![started.run.id,call])?;
                    Ok(())
                }).unwrap();
                database.complete_host_tool_call(&started.run.id,&call,Some(&final_result),is_error.then_some("failure")).unwrap();
            } else {
                database.with_connection(|connection| {
                    let tx = connection.transaction()?;
                    // The intermediate small update must clear the pointer too.
                    project_tool_updated(&tx,&started.run.id,&json!({"toolCallId":call,"update":{"content":[{"type":"text","text":"small-progress"}]}}),3)?;
                    let stale: Option<String> = tx.query_row("SELECT result_blob_sha256 FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",params![started.run.id,call],|row|row.get(0))?;
                    assert!(stale.is_none());
                    project_tool_updated(&tx,&started.run.id,&json!({"toolCallId":call,"update":big}),4)?;
                    project_tool_completed(&tx,&started.run.id,&json!({"toolCallId":call,"result":final_result,"isError":is_error}),5,None,None)?;
                    tx.commit()
                }).unwrap();
            }
            let range = database.tool_result_range(&reference,&conversation.id,0,4096).unwrap();
            let expected = final_result["content"][0]["text"].as_str().map(str::to_owned).unwrap_or_else(||final_result.to_string());
            assert_eq!(range.content,expected);
            database.with_connection(|connection| {
                let pointer: Option<String> = connection.query_row("SELECT result_blob_sha256 FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",params![started.run.id,call],|row|row.get(0))?;
                assert!(pointer.is_none());
                let copies: i64 = connection.query_row("SELECT COUNT(*) FROM tool_call_result_blobs",[],|row|row.get(0))?;
                assert_eq!(copies,1,"shared content-addressed blobs are preserved");
                Ok(())
            }).unwrap();
        }
    }

    #[test]
    fn large_result_is_spilled_to_blob_and_every_byte_stays_retrievable() {
        let path = std::env::temp_dir().join(format!("fox-tool-blob-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("blob"), None, None)
            .unwrap();
        let started = database.create_run(&conversation.id, "blob", None).unwrap();
        database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs SET status='running' WHERE id=?1",
                    [&started.run.id],
                )?;
                connection.execute(
                    "INSERT INTO tool_calls(
                        id, runtime_tool_call_id, run_id, conversation_id, tool_name, input_json,
                        status, execution_location, requires_approval, started_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, '{}', 'running', 'host', 0, 1, 1)",
                    params![
                        Uuid::new_v4().to_string(),
                        "call-big",
                        started.run.id,
                        conversation.id,
                        "office_read",
                    ],
                )?;
                Ok(())
            })
            .unwrap();
        // > MAX_STORED_TOOL_RESULT_BYTES: must not be discarded.
        let big_text = format!("{}{}", "数据".repeat(70_000), "tail-marker-终");
        let result = json!({"content":[{"type":"text","text": big_text}]});
        database
            .complete_host_tool_call(&started.run.id, "call-big", Some(&result), None)
            .unwrap();
        // The inline row holds only the bounded preview.
        let (inline, blob_sha): (String, Option<String>) = database
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT result_json, result_blob_sha256 FROM tool_calls WHERE run_id=?1 AND runtime_tool_call_id=?2",
                    params![started.run.id, "call-big"],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .unwrap();
        let inline: Value = serde_json::from_str(&inline).unwrap();
        assert_eq!(inline["truncated"], true);
        assert!(blob_sha.is_some(), "the full copy must be spilled to the blob table");
        let blob: String = database
            .with_connection(|connection| {
                Ok(connection.query_row(
                    "SELECT body_json FROM tool_call_result_blobs WHERE sha256=?1",
                    [blob_sha.as_deref().unwrap()],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert!(blob.contains("tail-marker-终"), "blob keeps the full result");
        // Range reading reassembles every byte and reports retrievable.
        let reference = format!("fox-result://{}/call-big", started.run.id);
        let first = database
            .tool_result_range(&reference, &conversation.id, 0, 64_000)
            .unwrap();
        assert!(first.retrievable, "blob-backed results are fully retrievable");
        // Served from the full blob: the range text itself is complete, so the
        // reader does not mark truncation; only an inline preview without a
        // blob reports truncated + not-retrievable (covered by the test above).
        assert!(!first.truncated, "the blob serves the complete result text");
        assert_eq!(first.original_bytes, big_text.len());
        let mut assembled = String::new();
        let mut offset = 0usize;
        loop {
            let range = database
                .tool_result_range(&reference, &conversation.id, offset, 65_537)
                .unwrap();
            assembled.push_str(&range.content);
            match range.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(assembled, big_text, "every original byte must be reachable");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn tool_result_range_cursor_always_advances_and_never_splits_or_repeats_bytes() {
        // The reported defect: fewer bytes than one code point used to come back
        // as an empty slice with `next == start`, so a client walking `next`
        // looped forever without ever reading anything.
        for limit in [1usize, 2, 3] {
            let error = utf8_byte_range("中A", 0, limit).unwrap_err();
            assert!(error.contains("at least 4 bytes"), "limit {limit}: {error}");
        }

        // The smallest legal limit returns a whole code point, never a partial one.
        let (start, end, next) = utf8_byte_range("中A", 0, MIN_TOOL_RESULT_RANGE_BYTES).unwrap();
        assert_eq!(&"中A"[start..end], "中A");
        assert_eq!(next, None, "the whole result fit in one range");

        // A CJK code point followed by more characters: with the minimum legal
        // limit the range stops after exactly one whole character and advances.
        let (_, end, next) = utf8_byte_range("中文ABC", 0, MIN_TOOL_RESULT_RANGE_BYTES).unwrap();
        assert_eq!(end, 3, "one CJK code point is what fits here");
        assert_eq!(next, Some(3));

        let text = "中文😀 mixed ASCII tail";
        // An offset inside a code point is refused with usable neighbours
        // instead of being nudged — nudging is how bytes get dropped or repeated.
        for inside in 1..text.len() {
            if text.is_char_boundary(inside) {
                continue;
            }
            let error = utf8_byte_range(text, inside, 8).unwrap_err();
            assert!(
                error.contains("splits a UTF-8 code point"),
                "offset {inside}: {error}"
            );
        }

        // Walking the whole result with several legal limits reassembles the
        // exact bytes: nothing missing, nothing repeated, nothing mangled, and
        // every non-terminal cursor strictly advances.
        for limit in [MIN_TOOL_RESULT_RANGE_BYTES, 5, 7, 12, 64] {
            let mut rebuilt = String::new();
            let mut offset = 0usize;
            let mut steps = 0usize;
            loop {
                let (start, end, next) = utf8_byte_range(text, offset, limit).unwrap();
                assert_eq!(start, offset, "a legal offset must never be moved");
                assert!(end > start, "a remaining tail must return a non-empty range");
                assert!(end - start <= limit, "the range must respect its own limit");
                assert!(
                    !text[start..end].contains('\u{FFFD}'),
                    "a range must never split a multi-byte character"
                );
                rebuilt.push_str(&text[start..end]);
                steps += 1;
                assert!(steps < 10_000, "a legal walk must terminate");
                match next {
                    Some(next) => {
                        assert!(next > offset, "a non-terminal cursor must advance");
                        offset = next;
                    }
                    None => break,
                }
            }
            assert_eq!(rebuilt, text, "limit {limit} must lose and repeat nothing");
        }

        // End of content, past the end, and a limit larger than the result.
        let last = text.len();
        assert_eq!(
            utf8_byte_range(text, last, 32).unwrap(),
            (last, last, None),
            "reading at the end is empty and terminal"
        );
        let error = utf8_byte_range(text, last + 1, 32).unwrap_err();
        assert!(error.contains("past the end"), "{error}");
        let (_, end, next) = utf8_byte_range(text, 0, MAX_TOOL_RESULT_RANGE_BYTES).unwrap();
        assert_eq!((end, next), (last, None));
    }

    #[test]
    fn tool_result_range_surfaces_unservable_ranges_instead_of_returning_nothing() {
        let path = std::env::temp_dir().join(format!("fox-tool-result-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("boundary"), None, None)
            .unwrap();
        let started = database.create_run(&conversation.id, "boundary", None).unwrap();
        seed_settled_tool_call(
            &database,
            &conversation.id,
            &started.run.id,
            "call-2",
            "read_attachment",
            &json!({"content":[{"type":"text","text":"中A"}]}),
        );
        let reference = format!("fox-result://{}/call-2", started.run.id);

        // A settled tool result envelope is walked as its joined text blocks, so
        // reassembling ranges gives back exactly what the model view was cut from.
        let mut rebuilt = String::new();
        let mut offset = 0usize;
        loop {
            let range = database
                .tool_result_range(&reference, &conversation.id, offset, 4)
                .unwrap();
            assert!(!range.truncated);
            assert!(range.retrievable);
            rebuilt.push_str(&range.content);
            match range.next_offset {
                Some(next) => {
                    assert!(next > offset);
                    offset = next;
                }
                None => break,
            }
        }
        assert_eq!(rebuilt, "中A");

        // Ranges that cannot be served whole are errors, not empty answers.
        for limit in [0usize, 1, 3] {
            let error = database
                .tool_result_range(&reference, &conversation.id, 0, limit)
                .unwrap_err();
            assert!(error.contains("at least 4 bytes"), "limit {limit}: {error}");
        }
        let error = database
            .tool_result_range(&reference, &conversation.id, 1, 64)
            .unwrap_err();
        assert!(error.contains("splits a UTF-8 code point"), "{error}");
        assert!(database
            .tool_result_range(&reference, &conversation.id, 4096, 64)
            .unwrap_err()
            .contains("past the end"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
