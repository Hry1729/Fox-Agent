use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub runtime_type: String,
    pub default_model: String,
    pub icon: Option<String>,
    pub capabilities: Value,
    pub resources: AgentResourcesRecord,
    pub configurable_items: Value,
    pub is_default: bool,
    pub available: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResourceRecord {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentResourcesRecord {
    pub tools: Vec<AgentResourceRecord>,
    pub knowledges: Vec<AgentResourceRecord>,
    pub mcps: Vec<AgentResourceRecord>,
    pub skills: Vec<AgentResourceRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub id: String,
    pub agent_id: String,
    pub agent_name: String,
    pub title: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_message_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRecord {
    pub id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub role: String,
    pub kind: String,
    pub content: String,
    pub status: String,
    pub ordinal: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub conversation_id: String,
    pub runtime_session_id: Option<String>,
    pub status: String,
    pub model: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub last_seq: i64,
    pub trace_id: Option<String>,
    pub root_span_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Proposed,
    Active,
    Blocked,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GoalRecord {
    pub id: String,
    pub conversation_id: String,
    pub title: String,
    pub objective: String,
    pub acceptance_summary: Option<String>,
    pub status: GoalStatus,
    pub version: i64,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkTaskStatus {
    Queued,
    InProgress,
    Completed,
    Blocked,
    Interrupted,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkTaskRecord {
    pub id: String,
    pub goal_id: String,
    pub parent_task_id: Option<String>,
    pub ordinal: i64,
    pub title: String,
    pub detail: Option<String>,
    pub status: WorkTaskStatus,
    pub owner_run_id: Option<String>,
    pub attempt: i64,
    pub blocked_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    ToolCall,
    TraceSpan,
    TestResult,
    FileDiff,
    Artifact,
    UserConfirmation,
    ExternalReference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceReferenceKind {
    ToolCall,
    Artifact,
    RunEvent,
    Message,
    Source,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceValidityStatus {
    Unverified,
    Valid,
    Stale,
    Missing,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvidenceRecord {
    pub id: String,
    pub task_id: String,
    pub source_run_id: Option<String>,
    pub evidence_type: EvidenceType,
    pub ref_kind: EvidenceReferenceKind,
    pub ref_id: String,
    pub summary: String,
    pub metadata: Value,
    pub validity_status: EvidenceValidityStatus,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub checked_at: Option<String>,
    pub invalid_reason: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation: ConversationSummary,
    pub messages: Vec<MessageRecord>,
    pub runtime_events: Vec<RunEventRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub approvals: Vec<ApprovalRecord>,
    pub attachments: Vec<AttachmentRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub knowledge_bindings: Vec<KnowledgeBindingRecord>,
    pub last_run: Option<RunRecord>,
    pub has_earlier_messages: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistoryPage {
    pub messages: Vec<MessageRecord>,
    pub runtime_events: Vec<RunEventRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub approvals: Vec<ApprovalRecord>,
    pub attachments: Vec<AttachmentRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub has_earlier_messages: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationHistoryRequest {
    pub conversation_id: String,
    pub before_ordinal: i64,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub permission_mode: String,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_opened_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageAgentStat {
    pub agent_id: String,
    pub agent_name: String,
    pub conversation_count: i64,
    pub run_count: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDayStat {
    pub date: String,
    pub conversation_count: i64,
    pub run_count: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageStatistics {
    pub conversation_count: i64,
    pub completed_run_count: i64,
    pub user_message_count: i64,
    pub active_day_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub agents: Vec<UsageAgentStat>,
    pub days: Vec<UsageDayStat>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilesRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileReadRequest {
    pub conversation_id: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFileEntry {
    pub path: String,
    pub name: String,
    pub parent: String,
    pub is_directory: bool,
    pub byte_size: i64,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectFilePreview {
    pub path: String,
    pub name: String,
    pub content: String,
    pub byte_size: i64,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRecord {
    pub id: String,
    pub runtime_tool_call_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub tool_name: String,
    pub input: Value,
    pub status: String,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub execution_location: String,
    pub requires_approval: bool,
    pub started_at: i64,
    pub completed_at: Option<i64>,
    pub updated_at: i64,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRecord {
    pub id: String,
    pub tool_call_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub tool_name: String,
    pub status: String,
    pub requested_action: String,
    pub request: Value,
    pub decision: Option<Value>,
    pub requested_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRecord {
    pub id: String,
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub display_name: String,
    pub storage_path: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub display_name: String,
    pub artifact_type: String,
    pub storage_path: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBindingRecord {
    pub conversation_id: String,
    pub service_connection_id: String,
    pub knowledge_base_id: String,
    pub knowledge_base_name: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeConfigRecord {
    pub agent_id: String,
    pub scope_type: String,
    pub scope_id: String,
    pub config_key: String,
    pub value: Value,
    pub source: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub required_tools: Vec<String>,
    pub source_path: String,
    pub enabled: bool,
    pub valid: bool,
    pub validation_error: Option<String>,
    pub instructions: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetSkillEnabledRequest {
    pub agent_id: String,
    pub skill_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentIdRequest {
    pub agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRecord {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub enabled: bool,
    pub status: String,
    pub credential_configured: bool,
    pub last_error: Option<String>,
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveMcpServerRequest {
    pub id: Option<String>,
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub clear_environment: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerIdRequest {
    pub server_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMcpServerEnabledRequest {
    pub server_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreBackupRequest {
    pub backup_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanupDataRequest {
    pub runtime_session_days: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserProfileRecord {
    pub name: String,
    pub avatar: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveUserProfileRequest {
    pub name: String,
    pub avatar: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheStatistics {
    pub total_files: u64,
    pub active_files: u64,
    pub total_bytes: u64,
    pub active_bytes: u64,
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheLimitRequest {
    pub limit_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunEventRecord {
    pub run_id: String,
    pub seq: i64,
    pub event_type: String,
    pub event: Value,
    pub created_at: i64,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeInitialization {
    pub database_ready: bool,
    pub runtime_available: bool,
    pub default_agent_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateConversationRequest {
    pub agent_id: String,
    pub title: Option<String>,
    pub project_root: Option<String>,
    pub permission_mode: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameConversationRequest {
    pub conversation_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAttachmentInput {
    pub filename: String,
    pub media_type: Option<String>,
    pub data_url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAttachmentsRequest {
    pub conversation_id: String,
    pub message_id: Option<String>,
    pub files: Vec<SaveAttachmentInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetKnowledgeBindingsRequest {
    pub conversation_id: String,
    pub knowledge_bases: Vec<KnowledgeBindingInput>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBindingInput {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationIdRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectPermissionRequest {
    pub project_id: String,
    pub permission_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalRequest {
    pub approval_id: String,
    pub approved: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunRequest {
    pub conversation_id: String,
    pub text: String,
    pub runtime_text: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeYuxiRunRequest {
    pub conversation_id: String,
    pub parent_run_id: String,
    pub text: String,
    pub answers: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelRunRequest {
    pub run_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunResult {
    pub run: RunRecord,
    pub user_message: MessageRecord,
    pub attachments: Vec<AttachmentRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePromptMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeSessionRecord {
    pub runtime_session_id: String,
    pub runtime_version: Option<String>,
    pub session_path: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct ConversationRuntimeRecord {
    pub runtime_type: String,
    pub remote_agent_id: Option<String>,
    pub remote_thread_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExternalRunRecord {
    pub conversation_id: String,
    pub external_run_id: Option<String>,
    pub external_cursor: Option<String>,
    pub runtime_type: String,
}

#[derive(Debug, Clone)]
pub struct RecoverableExternalRunRecord {
    pub local_run_id: String,
    pub conversation_id: String,
    pub external_run_id: String,
    pub external_cursor: Option<String>,
    pub remote_thread_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiLoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiUserRecord {
    pub uid: String,
    pub username: String,
    pub avatar: Option<String>,
    pub role: String,
    pub department_id: Option<i64>,
    pub department_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiAgentRecord {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub backend_id: String,
    pub default_model: String,
    pub capabilities: Value,
    pub resources: AgentResourcesRecord,
    pub configurable_items: Value,
    pub is_default: bool,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiModelRecord {
    pub spec: String,
    pub model_id: String,
    pub display_name: String,
    pub provider_id: String,
    pub provider_display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kb_type: Option<String>,
    pub status: Option<String>,
    pub file_count: i64,
    pub processed_count: i64,
    pub row_count: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentRecord {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub is_folder: bool,
    pub status: Option<String>,
    pub size: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDetailRecord {
    pub database: KnowledgeBaseRecord,
    pub documents: Vec<KnowledgeDocumentRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeIdRequest {
    pub knowledge_base_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentRangeRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub start: u64,
    pub end: u64,
    pub source_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentSourceMetadata {
    pub filename: String,
    pub media_type: String,
    pub size: u64,
    pub source_revision: String,
    pub version_id: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub accepts_ranges: bool,
    pub preview_api_version: Option<u32>,
    pub supported_preview_variants: Vec<String>,
    pub available_variants: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentReadingState {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub page: i64,
    pub scroll_offset: f64,
    pub zoom: Option<f64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentBookmark {
    pub id: String,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub page: i64,
    pub anchor: String,
    pub excerpt: String,
    pub label: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentAnnotation {
    pub id: String,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub annotation_type: String,
    pub page: i64,
    pub anchor: String,
    pub excerpt: String,
    pub note: String,
    pub color: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentActivity {
    pub reading_state: Option<KnowledgeDocumentReadingState>,
    pub bookmarks: Vec<KnowledgeDocumentBookmark>,
    pub annotations: Vec<KnowledgeDocumentAnnotation>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveKnowledgeDocumentReadingStateRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub page: i64,
    pub scroll_offset: f64,
    pub zoom: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveKnowledgeDocumentBookmarkRequest {
    pub id: Option<String>,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub page: i64,
    pub anchor: Option<String>,
    pub excerpt: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveKnowledgeDocumentAnnotationRequest {
    pub id: Option<String>,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: Option<String>,
    pub annotation_type: Option<String>,
    pub page: i64,
    pub anchor: Option<String>,
    pub excerpt: Option<String>,
    pub note: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteKnowledgeDocumentActivityItemRequest {
    pub knowledge_base_id: String,
    pub document_id: String,
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheAcquireRequest {
    pub operation_id: String,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: String,
    pub media_type: String,
    pub size: u64,
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheCancelRequest {
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheLease {
    pub cache_key: String,
    pub media_type: String,
    pub size: u64,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheReadRequest {
    pub cache_key: String,
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheReleaseRequest {
    pub cache_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgePreviewCacheOpenRequest {
    pub cache_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentDownloadRequest {
    pub download_id: String,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub filename: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDownloadCancelRequest {
    pub download_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDownloadProgress {
    pub download_id: String,
    pub document_id: String,
    pub bytes_written: u64,
    pub total_bytes: Option<u64>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeDocumentDownloadResult {
    pub path: String,
    pub filename: String,
    pub media_type: String,
    pub bytes_written: u64,
    pub can_open_directly: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDownloadedFileRequest {
    pub path: String,
    pub reveal: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct KnowledgePreviewCacheEntry {
    pub cache_key: String,
    pub knowledge_base_id: String,
    pub document_id: String,
    pub source_revision: String,
    pub variant: String,
    pub storage_path: String,
    pub media_type: Option<String>,
    pub byte_size: u64,
    pub lease_count: u32,
    pub created_at: i64,
    pub last_accessed_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeQueryRequest {
    pub knowledge_base_id: String,
    pub query: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphQueryRequest {
    pub knowledge_base_id: String,
    pub keyword: Option<String>,
    pub max_depth: Option<i64>,
    pub max_nodes: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiServiceRecord {
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub connection_type: String,
    pub credential_configured: bool,
    pub last_status: String,
    pub last_version: Option<String>,
    pub last_latency_ms: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveYuxiServiceRequest {
    pub name: String,
    pub base_url: String,
    pub access_token: Option<String>,
    #[serde(default)]
    pub clear_access_token: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestYuxiServiceRequest {
    pub base_url: Option<String>,
    pub access_token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YuxiConnectionTest {
    pub ok: bool,
    pub base_url: String,
    pub connection_type: String,
    pub status: String,
    pub authenticated: bool,
    pub auth_status: String,
    pub version: Option<String>,
    pub message: String,
    pub latency_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelServiceRecord {
    pub name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_type: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    pub supports_image_input: bool,
    pub enabled: bool,
    pub credential_configured: bool,
    pub connection_type: String,
    pub last_status: String,
    pub last_latency_ms: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveModelServiceRequest {
    pub name: String,
    pub base_url: String,
    pub model_id: String,
    pub api_type: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    #[serde(default)]
    pub supports_image_input: bool,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestModelServiceRequest {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub api_type: Option<String>,
    pub model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConnectionTest {
    pub ok: bool,
    pub base_url: String,
    pub connection_type: String,
    pub status: String,
    pub latency_ms: i64,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelRecord {
    pub id: String,
    pub model_id: String,
    pub display_name: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    pub supports_image_input: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderRecord {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_type: String,
    pub enabled: bool,
    pub is_default: bool,
    pub credential_configured: bool,
    pub connection_type: String,
    pub last_status: String,
    pub last_latency_ms: Option<i64>,
    pub last_checked_at: Option<i64>,
    pub models: Vec<ProviderModelRecord>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProviderModelRequest {
    pub id: Option<String>,
    pub model_id: String,
    pub display_name: String,
    pub context_window: i64,
    pub max_output_tokens: i64,
    #[serde(default)]
    pub supports_image_input: bool,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveModelProviderRequest {
    pub id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub api_type: String,
    #[serde(default)]
    pub is_default: bool,
    pub models: Vec<SaveProviderModelRequest>,
    pub api_key: Option<String>,
    #[serde(default)]
    pub clear_api_key: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelProviderIdRequest {
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ApiError>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn failure(code: &str, message: impl Into<String>, retryable: bool) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(ApiError {
                code: code.to_owned(),
                message: message.into(),
                retryable,
            }),
        }
    }
}
