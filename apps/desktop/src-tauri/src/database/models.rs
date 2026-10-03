use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub description: String,
    pub runtime_type: String,
    pub agent_kind: String,
    pub invocation_mode: String,
    pub visibility: String,
    pub default_model: String,
    pub icon: Option<String>,
    pub category: String,
    pub opening_suggestions: Vec<String>,
    pub system_prompt: String,
    pub is_builtin: bool,
    pub package_version: String,
    pub package_source: String,
    pub package_id: Option<String>,
    pub package_hash: Option<String>,
    pub package_manifest: Value,
    pub capabilities: Value,
    pub resources: AgentResourcesRecord,
    pub configurable_items: Value,
    pub is_default: bool,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertPackageVersionRecord {
    pub id: String,
    pub expert_id: String,
    pub package_id: String,
    pub version: String,
    pub package_hash: String,
    pub package: Value,
    pub source: String,
    pub status: String,
    pub created_at: i64,
    pub activated_at: i64,
}

#[derive(Debug, Clone)]
pub struct InstallExpertPackageVersionRequest {
    pub expert_id: String,
    pub package_id: String,
    pub version: String,
    pub package_hash: String,
    pub package: Value,
    pub package_manifest: Value,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub category: String,
    pub system_prompt: String,
    pub default_model: String,
    pub opening_suggestions: Vec<String>,
    pub enabled_skills: Vec<String>,
    pub expected_current_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowRunRecord {
    pub id: String,
    pub conversation_id: String,
    pub expert_binding_id: String,
    pub expert_id: String,
    pub package_hash: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub workflow: Value,
    pub goal_id: String,
    pub status: String,
    pub current_stage_index: i64,
    pub input: Value,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowStageRunRecord {
    pub id: String,
    pub workflow_run_id: String,
    pub stage_id: String,
    pub task_id: String,
    pub ordinal: i64,
    pub status: String,
    pub attempt: i64,
    pub max_attempts: i64,
    pub output: Option<Value>,
    pub error_message: Option<String>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowGateRecord {
    pub id: String,
    pub workflow_run_id: String,
    pub stage_id: String,
    pub status: String,
    pub reason: String,
    pub requested_at: i64,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowSnapshot {
    pub run: ExpertWorkflowRunRecord,
    pub stages: Vec<ExpertWorkflowStageRunRecord>,
    pub gates: Vec<ExpertWorkflowGateRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertTeamRunRecord {
    pub id: String,
    pub conversation_id: String,
    pub expert_binding_id: String,
    pub expert_id: String,
    pub package_hash: String,
    pub team_id: String,
    pub team_version: String,
    pub team: Value,
    pub parent_run_id: String,
    pub status: String,
    pub objective: String,
    pub context: String,
    pub result: Option<Value>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertTeamSnapshot {
    pub run: ExpertTeamRunRecord,
    pub members: Vec<ChildRunRecord>,
}

#[derive(Debug, Clone)]
pub struct StartExpertTeamInput {
    pub conversation_id: String,
    pub expert_binding_id: String,
    pub expert_id: String,
    pub package_hash: String,
    pub team_id: String,
    pub team_version: String,
    pub team: Value,
    pub parent_run_id: String,
    pub objective: String,
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueRecord {
    pub id: String,
    pub name: String,
    pub expert_id: String,
    pub expert_binding_id: String,
    pub package_hash: String,
    pub package_snapshot: Value,
    pub conversation_id: String,
    pub objective: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub knowledge_references: Vec<KnowledgeReference>,
    pub status: String,
    pub max_runs_per_day: i64,
    pub max_tokens_per_day: i64,
    pub max_duration_ms: i64,
    pub max_output_tokens: i64,
    pub max_tool_calls: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueScheduleRecord {
    pub id: String,
    pub colleague_id: String,
    pub name: String,
    pub schedule_kind: String,
    pub interval_seconds: i64,
    pub catchup_window_seconds: i64,
    pub enabled: bool,
    pub next_due_at: i64,
    pub last_scheduled_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueChannelRecord {
    pub id: String,
    pub colleague_id: String,
    pub name: String,
    pub channel_kind: String,
    pub external_identity: String,
    pub secret_prefix: String,
    pub rate_limit_per_minute: i64,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueTriggerRecord {
    pub id: String,
    pub colleague_id: String,
    pub source_type: String,
    pub source_id: Option<String>,
    pub idempotency_key: String,
    pub status: String,
    pub payload: Value,
    pub scheduled_for: Option<i64>,
    pub run_id: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub tool_call_count: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DigitalColleagueAuditRecord {
    pub id: String,
    pub colleague_id: Option<String>,
    pub channel_id: Option<String>,
    pub trigger_id: Option<String>,
    pub event: String,
    pub outcome: String,
    pub actor: String,
    pub details: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct PreparedDigitalColleagueTrigger {
    pub colleague: DigitalColleagueRecord,
    pub trigger: DigitalColleagueTriggerRecord,
    pub started: StartRunResult,
    pub remaining_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct DigitalColleagueTriggerAcceptance {
    pub trigger: DigitalColleagueTriggerRecord,
    pub prepared: Option<PreparedDigitalColleagueTrigger>,
}

#[derive(Debug, Clone)]
pub struct CreateDigitalColleagueInput {
    pub name: String,
    pub expert_id: String,
    pub objective: String,
    pub project_id: Option<String>,
    pub project_root: Option<String>,
    pub knowledge_references: Vec<KnowledgeReference>,
    pub max_runs_per_day: i64,
    pub max_tokens_per_day: i64,
    pub max_duration_ms: i64,
    pub max_output_tokens: i64,
    pub max_tool_calls: i64,
}

#[derive(Debug, Clone)]
pub struct StartExpertWorkflowInput {
    pub conversation_id: String,
    pub expert_binding_id: String,
    pub expert_id: String,
    pub package_hash: String,
    pub workflow_id: String,
    pub workflow_version: String,
    pub workflow: Value,
    pub title: String,
    pub objective: String,
    pub acceptance_summary: String,
    pub input: Value,
    pub stages: Vec<StartExpertWorkflowStageInput>,
}

#[derive(Debug, Clone)]
pub struct StartExpertWorkflowStageInput {
    pub stage_id: String,
    pub title: String,
    pub detail: String,
    pub max_attempts: i64,
    pub user_gate: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAgentRequest {
    pub id: Option<String>,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub category: String,
    pub system_prompt: String,
    pub default_model: String,
    #[serde(default)]
    pub opening_suggestions: Vec<String>,
    #[serde(default)]
    pub package_manifest: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyAgentRequest {
    pub agent_id: String,
    pub name: Option<String>,
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
    pub permission_mode: String,
    pub status: String,
    pub pinned: bool,
    pub archived: bool,
    pub archived_at: Option<i64>,
    pub trashed_at: Option<i64>,
    pub parent_conversation_id: Option<String>,
    pub forked_from_message_id: Option<String>,
    pub lineage_root_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_message_at: Option<i64>,
    /// The conversation's own non-terminal Run, so every list row can show its own
    /// activity instead of the activity of whichever conversation happens to be open.
    pub active_run_id: Option<String>,
    pub active_run_status: Option<String>,
    /// True while that Run still has a pending approval.
    pub awaiting_approval: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSearchRequest {
    pub query: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[serde(rename_all = "camelCase")]
pub struct ChildRunBudget {
    pub max_duration_ms: i64,
    pub max_total_tokens: i64,
    pub max_output_tokens: i64,
    pub max_tool_calls: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChildAgentSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub category: String,
    pub agent_kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunRecord {
    pub id: String,
    pub parent_run_id: String,
    pub child_run_id: String,
    pub child_conversation_id: String,
    pub worker_agent_id: String,
    pub worker_agent_name: String,
    pub objective: String,
    pub context: String,
    pub team_run_id: Option<String>,
    pub team_member_id: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub status: String,
    pub depth: i64,
    pub budget: ChildRunBudget,
    pub result_text: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_tokens: i64,
    pub tool_call_count: i64,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
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
    pub version: i64,
    pub blocked_reason: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ValidationPolicySnapshot {
    pub schema_version: u32,
    pub id: String,
    pub risk_level: String,
    pub required_checks: Vec<String>,
    pub allowed_check_types: Vec<String>,
    pub reviewer_policy: String,
    pub max_repair_attempts: u32,
    pub completion_requires_acceptance: bool,
    pub hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskValidationPolicyRecord {
    pub task_id: String,
    pub snapshot: ValidationPolicySnapshot,
    pub frozen_at: String,
    pub legacy_fallback: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskAttemptKind {
    Execution,
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskAttemptStatus {
    Running,
    Succeeded,
    Failed,
    Blocked,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskAttemptRecord {
    pub id: String,
    pub task_id: String,
    pub attempt_number: i64,
    pub kind: TaskAttemptKind,
    pub status: TaskAttemptStatus,
    pub run_id: String,
    pub policy_hash: String,
    pub root_cause: Option<String>,
    pub finding_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub failure_reason: Option<String>,
    pub version: i64,
    pub evidence_rowid_watermark: i64,
    pub finding_rowid_watermark: i64,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskAttemptStartResult {
    pub task: WorkTaskRecord,
    pub attempt: Option<TaskAttemptRecord>,
    pub budget_exhausted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskAttemptFinishResult {
    pub task: WorkTaskRecord,
    pub attempt: TaskAttemptRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRepairOverrideEventRecord {
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub approval_id: String,
    pub tool_call_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub policy_id: String,
    pub policy_hash: String,
    pub normal_repair_budget: i64,
    pub normal_repair_used: i64,
    pub override_count: i64,
    pub input_hash: String,
    pub escalation_reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskRepairOverrideStartResult {
    pub task: WorkTaskRecord,
    pub attempt: TaskAttemptRecord,
    pub override_event: TaskRepairOverrideEventRecord,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlanRevisionRecord {
    pub id: String,
    pub goal_id: String,
    pub conversation_id: String,
    pub revision: i64,
    pub title: String,
    pub summary: String,
    pub tasks: Value,
    pub status: String,
    pub created_by: String,
    pub created_at: String,
    pub approved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFindingRecord {
    pub id: String,
    pub goal_id: String,
    pub task_id: Option<String>,
    pub plan_revision_id: Option<String>,
    pub conversation_id: String,
    pub severity: String,
    pub category: String,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub created_by: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceRecord {
    pub id: String,
    pub goal_id: String,
    pub plan_revision_id: Option<String>,
    pub conversation_id: String,
    pub status: String,
    pub summary: String,
    pub checks: Value,
    pub reviewer: String,
    pub created_at: String,
    pub resolved_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkEventRecord {
    #[serde(rename = "type")]
    pub event_type: String,
    pub schema_version: u32,
    pub conversation_id: String,
    pub goal_id: Option<String>,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub trace_id: Option<String>,
    pub span_id: Option<String>,
    pub sequence: i64,
    pub timestamp: String,
    pub data: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationDecisionKind {
    Continue,
    Repair,
    WaitApproval,
    Complete,
    Blocked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationReasonCode {
    WorkRemaining,
    ValidationFailed,
    ApprovalPending,
    ExternalDependencyUnavailable,
    BudgetExhausted,
    UserInputRequired,
    RetryAvailable,
    RetryExhausted,
    AcceptanceMissing,
    AcceptanceCandidate,
    AcceptancePassed,
    HostAuditRequired,
    ToolFailed,
    TaskInterrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationRetryClass {
    None,
    Recoverable,
    RetryLimited,
    NonRetryable,
    HostDecides,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationValidationOutcome {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppendContinuationDecisionInput {
    pub schema_version: u32,
    pub decision_id: String,
    pub run_id: String,
    pub event_cursor: i64,
    pub decision: ContinuationDecisionKind,
    pub reason_code: ContinuationReasonCode,
    #[serde(default)]
    pub active_task_ids: Vec<String>,
    #[serde(default)]
    pub evidence_ids: Vec<String>,
    #[serde(default)]
    pub missing_acceptance: Vec<String>,
    pub next_action: Option<String>,
    pub retry_class: Option<ContinuationRetryClass>,
    #[serde(default)]
    pub blocked_dependency_refs: Vec<String>,
    pub host_validation_outcome: ContinuationValidationOutcome,
    pub host_validation_error: Option<String>,
    #[serde(default)]
    pub expected_projection_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationDecisionRecord {
    pub schema_version: u32,
    pub decision_id: String,
    pub run_id: String,
    pub event_cursor: i64,
    pub decision: ContinuationDecisionKind,
    pub reason_code: ContinuationReasonCode,
    pub active_task_ids: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub missing_acceptance: Vec<String>,
    pub next_action: Option<String>,
    pub retry_class: Option<ContinuationRetryClass>,
    pub blocked_dependency_refs: Vec<String>,
    pub host_validation_outcome: ContinuationValidationOutcome,
    pub host_validation_error: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UnprojectedContinuationProposal {
    pub conversation_id: String,
    pub run_id: String,
    pub seq: i64,
    pub payload: Value,
    pub execution_profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RecordContinuationIngestDiagnosticInput {
    pub run_id: String,
    pub event_seq: i64,
    pub execution_profile_id: Option<String>,
    pub shadow_mode: bool,
    pub error_code: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContinuationIngestDiagnosticRecord {
    pub run_id: String,
    pub event_seq: i64,
    pub execution_profile_id: Option<String>,
    pub shadow_mode: bool,
    pub error_code: String,
    pub error_message: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerGoalProjection {
    pub id: String,
    pub title: String,
    pub status: GoalStatus,
    pub completion_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerAcceptanceCheck {
    pub acceptance_id: String,
    pub status: String,
    pub check: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerEvidenceProjection {
    pub id: String,
    pub evidence_type: EvidenceType,
    pub validity_status: EvidenceValidityStatus,
    pub summary: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerTaskProjection {
    pub task_id: String,
    pub title: String,
    pub status: WorkTaskStatus,
    pub attempt: i64,
    pub acceptance_checks: Vec<TaskLedgerAcceptanceCheck>,
    pub latest_evidence: Vec<TaskLedgerEvidenceProjection>,
    pub blockers: Vec<String>,
    pub assigned_run_id: Option<String>,
    pub child_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskLedgerPendingActionKind {
    Approval,
    ExternalWait,
    Retry,
    Review,
    Repair,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerPendingAction {
    pub kind: TaskLedgerPendingActionKind,
    pub reference_id: String,
    pub task_id: Option<String>,
    pub run_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskLedgerCompletedOutcome {
    Completed,
    Accepted,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerCompletedSummary {
    pub task_id: String,
    pub outcome: TaskLedgerCompletedOutcome,
    pub evidence_ids: Vec<String>,
    pub accepted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskLedgerPhase {
    Idle,
    Queued,
    Execute,
    Validate,
    Repair,
    WaitApproval,
    Complete,
    Blocked,
    Interrupted,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerExecutionCursor {
    pub current_phase: TaskLedgerPhase,
    pub run_id: Option<String>,
    pub event_cursor: i64,
    pub last_event_id: Option<String>,
    pub resumable_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerRunCursor {
    pub run_id: String,
    pub event_cursor: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerSourceHighWatermark {
    pub run_cursors: Vec<TaskLedgerRunCursor>,
    pub work_updated_at: Option<String>,
    pub acceptance_created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerEventRange {
    pub first: i64,
    pub last: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerProjectionMeta {
    pub schema_version: u32,
    pub projection_hash: String,
    pub source_high_watermark: TaskLedgerSourceHighWatermark,
    pub generated_at: i64,
    pub derived_goal_ids: Vec<String>,
    pub derived_task_ids: Vec<String>,
    pub derived_run_ids: Vec<String>,
    pub event_range: Option<TaskLedgerEventRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TaskLedgerProjection {
    pub schema_version: u32,
    pub conversation_id: String,
    pub goal: Option<TaskLedgerGoalProjection>,
    pub active_tasks: Vec<TaskLedgerTaskProjection>,
    pub pending_actions: Vec<TaskLedgerPendingAction>,
    pub completed_summary: Vec<TaskLedgerCompletedSummary>,
    pub execution_cursor: TaskLedgerExecutionCursor,
    pub projection_meta: TaskLedgerProjectionMeta,
    pub latest_host_accepted_decision: Option<ContinuationDecisionRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation: ConversationSummary,
    pub expert_bindings: Vec<ConversationExpertBinding>,
    pub messages: Vec<MessageRecord>,
    pub runtime_events: Vec<RunEventRecord>,
    pub tool_calls: Vec<ToolCallRecord>,
    pub approvals: Vec<ApprovalRecord>,
    pub attachments: Vec<AttachmentRecord>,
    pub artifacts: Vec<ArtifactRecord>,
    pub knowledge_bindings: Vec<KnowledgeBindingRecord>,
    pub last_run: Option<RunRecord>,
    /// Terminal state of every run referenced by the loaded messages. Exposes
    /// failure/cancellation facts that the message status column does not carry.
    pub runs: Vec<RunRecord>,
    pub has_earlier_messages: bool,
    pub goals: Vec<GoalRecord>,
    pub tasks: Vec<WorkTaskRecord>,
    pub evidence: Vec<TaskEvidenceRecord>,
    pub plan_revisions: Vec<PlanRevisionRecord>,
    pub review_findings: Vec<ReviewFindingRecord>,
    pub acceptances: Vec<AcceptanceRecord>,
    pub child_runs: Vec<ChildRunRecord>,
    pub expert_workflow: Option<ExpertWorkflowSnapshot>,
    pub expert_team: Option<ExpertTeamSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationExpertBinding {
    pub id: String,
    pub conversation_id: String,
    pub expert_id: String,
    pub state: String,
    pub activation_source: String,
    pub expert_version: String,
    pub package_hash: String,
    pub package_snapshot: Value,
    pub display_snapshot_json: Value,
    pub activated_at: i64,
    pub deactivated_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversationExpertBindingError {
    ConversationNotFound,
    ExpertNotFound,
    InvalidRole,
    RemoteUnavailable,
    LockedArchived,
    LockedByMessages,
    LockedByActiveRun,
    InvalidActivationSource,
    Storage(String),
}

impl ConversationExpertBindingError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::ConversationNotFound => "conversation.not_found",
            Self::ExpertNotFound => "expert.not_found",
            Self::InvalidRole => "conversation.expert_invalid_role",
            Self::RemoteUnavailable => "conversation.expert_remote_unavailable",
            Self::LockedArchived | Self::LockedByMessages | Self::LockedByActiveRun => {
                "conversation.expert_locked"
            }
            Self::InvalidActivationSource => "conversation.expert_invalid_source",
            Self::Storage(_) => "storage.operation_failed",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(self, Self::RemoteUnavailable | Self::Storage(_))
    }
}

impl std::fmt::Display for ConversationExpertBindingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConversationNotFound => formatter.write_str("conversation was not found"),
            Self::ExpertNotFound => formatter.write_str("expert was not found"),
            Self::InvalidRole => formatter.write_str("selected agent is not an inline expert"),
            Self::RemoteUnavailable => formatter.write_str("remote expert is unavailable"),
            Self::LockedArchived => {
                formatter.write_str("conversation expert cannot change after conversation archive")
            }
            Self::LockedByMessages => formatter.write_str(
                "conversation expert cannot change after user or assistant messages exist",
            ),
            Self::LockedByActiveRun => {
                formatter.write_str("conversation expert cannot change while a run is active")
            }
            Self::InvalidActivationSource => {
                formatter.write_str("expert activation source is required")
            }
            Self::Storage(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ConversationExpertBindingError {}

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
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
    pub agents: Vec<UsageAgentStat>,
    pub days: Vec<UsageDayStat>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TraceRunSummary {
    pub run_id: String,
    pub conversation_id: String,
    pub trace_id: String,
    pub root_span_id: String,
    pub status: String,
    pub model: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub total_duration_ms: Option<i64>,
    pub planning_duration_ms: i64,
    pub model_duration_ms: i64,
    pub tool_duration_ms: i64,
    pub ui_duration_ms: i64,
    pub span_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyMetric {
    pub operation: String,
    pub sample_count: i64,
    pub average_ms: i64,
    pub p50_ms: i64,
    pub p95_ms: i64,
    pub max_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationSuiteSummary {
    pub name: String,
    pub source: Option<String>,
    pub source_url: Option<String>,
    pub total: i64,
    pub passed: i64,
    pub failed: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluationRunSummary {
    pub id: String,
    pub generated_at: String,
    pub recorded_at: i64,
    pub duration_ms: i64,
    pub report_hash: String,
    pub suites: i64,
    pub total: i64,
    pub passed: i64,
    pub failed: i64,
    pub suite_results: Vec<EvaluationSuiteSummary>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservabilityStatistics {
    pub trace_schema_version: i64,
    pub traced_run_count: i64,
    pub total_run_count: i64,
    pub trace_coverage_percent: i64,
    pub recent_runs: Vec<TraceRunSummary>,
    pub latency_metrics: Vec<LatencyMetric>,
    pub evaluation_history: Vec<EvaluationRunSummary>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordUiMetricRequest {
    pub run_id: String,
    pub metric: String,
    pub duration_ms: i64,
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

/// One authorized byte range of a settled tool call's stored result, resolved
/// from a `fox-result://<runId>/<toolCallId>` reference. Produced only after the
/// caller proves it is authorized for the owning conversation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultRange {
    pub run_id: String,
    pub tool_call_id: String,
    pub conversation_id: String,
    pub tool_name: String,
    pub status: String,
    /// True when the stored copy is a Fox preview rather than the full result,
    /// because the original exceeded `MAX_STORED_TOOL_RESULT_BYTES`.
    pub truncated: bool,
    /// True only while Host still holds every byte of the original result, so
    /// every byte omitted from a bounded model view can be recovered here.
    /// `false` means this reader can serve the preview and nothing beyond it,
    /// and callers must say so instead of implying the rest is reachable.
    pub retrievable: bool,
    /// Size of the original result, not of the stored copy.
    pub original_bytes: usize,
    pub offset: usize,
    pub returned_bytes: usize,
    /// Offset to pass back to continue reading, or `None` when this range
    /// reaches the end of what Host stored.
    pub next_offset: Option<usize>,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostToolCallDisposition {
    Created,
    PromotedRuntime,
    ReplayTerminal,
    AlreadyInFlight,
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
    pub category: String,
    pub claimed_at: Option<i64>,
    pub claimed_by_run_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    Deny,
    AllowOnce,
    AllowConversation,
}

impl ApprovalDecision {
    pub fn approved(self) -> bool {
        !matches!(self, Self::Deny)
    }

    pub fn scope(self) -> &'static str {
        match self {
            Self::Deny => "none",
            Self::AllowOnce => "once",
            Self::AllowConversation => "conversation",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Host-verified lifecycle bucket: `deliverable`, `preview` or `process`.
    /// The renderer groups by this value instead of guessing from the file
    /// extension, so a deliberately delivered CSV is not demoted and an
    /// intermediate JSON is not promoted.
    pub artifact_class: String,
    /// `project` when the bytes live in the conversation's project folder,
    /// `host_private` when they live in an application-private Host area.
    pub artifact_origin: String,
    pub storage_path: String,
    pub media_type: Option<String>,
    pub byte_size: i64,
    pub sha256: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// Read projection over the Host's version ledger and delivery checklist.
    /// Historical rows without a matching version remain unverified.
    pub delivery: Option<ArtifactDeliveryView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactDeliveryView {
    pub version_id: Option<String>,
    pub version_no: Option<i64>,
    pub source_tool_call_id: Option<String>,
    pub purpose_source: String,
    pub verification_status: String,
    pub summary: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeReference {
    pub source: String,
    pub provider_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_id: Option<String>,
    pub id: String,
}

impl KnowledgeReference {
    pub fn local(id: impl Into<String>) -> Self {
        Self {
            source: "local".to_owned(),
            provider_key: "local".to_owned(),
            connection_id: None,
            id: id.into(),
        }
    }

    pub fn remote(connection_id: impl Into<String>, id: impl Into<String>) -> Self {
        let connection_id = connection_id.into();
        Self {
            source: "remote".to_owned(),
            provider_key: connection_id.clone(),
            connection_id: Some(connection_id),
            id: id.into(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("knowledge reference id must not be empty".to_owned());
        }
        if self.provider_key.trim().is_empty() {
            return Err("knowledge reference providerKey must not be empty".to_owned());
        }
        match self.source.as_str() {
            "local" if self.provider_key == "local" && self.connection_id.is_none() => Ok(()),
            "remote"
                if self.connection_id.as_deref().is_some_and(|connection_id| {
                    !connection_id.trim().is_empty() && connection_id == self.provider_key
                }) =>
            {
                Ok(())
            }
            "local" => Err(
                "local knowledge references require providerKey=local and no connectionId"
                    .to_owned(),
            ),
            "remote" => {
                Err("remote knowledge references require connectionId=providerKey".to_owned())
            }
            _ => Err("knowledge reference source must be local or remote".to_owned()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeReferenceBindingRecord {
    pub conversation_id: String,
    pub reference: KnowledgeReference,
    pub knowledge_base_name: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeReferenceBindingInput {
    pub reference: KnowledgeReference,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum PluginKind {
    Mcp,
    Skill,
    Tool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginOrigin {
    Builtin,
    Official,
    Community,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PluginInstallStatus {
    NotInstalled,
    Installing,
    Installed,
    UpdateAvailable,
    Uninstalling,
    InstallFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpRuntimeStatus {
    Unknown,
    Ready,
    Connecting,
    Healthy,
    Degraded,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PluginActivation {
    Global { enabled: bool },
    PerAgent { enabled_agent_count: usize },
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum PluginActivationUpdate {
    Global { enabled: bool },
    PerAgent { agent_id: String, enabled: bool },
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCardView {
    pub id: String,
    pub kind: PluginKind,
    pub origin: PluginOrigin,
    pub category: String,
    pub name: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub install_status: PluginInstallStatus,
    pub activation: PluginActivation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_status: Option<McpRuntimeStatus>,
    pub permissions: Vec<String>,
    pub compatible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incompatibility_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogQuery {
    pub kind: PluginKind,
    pub search: Option<String>,
    pub category: Option<String>,
    pub page_size: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginCatalogPage {
    pub items: Vec<PluginCardView>,
    pub total: usize,
    pub categories: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

pub type PluginCatalogDTO = PluginCatalogPage;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationsPage {
    pub items: Vec<PluginCardView>,
    pub total: usize,
}

pub type PluginInstallationsDTO = PluginInstallationsPage;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSetActivationRequest {
    pub plugin_id: String,
    pub update: PluginActivationUpdate,
}

#[derive(Debug, Clone)]
pub struct PluginCatalogEntryRecord {
    pub plugin_id: String,
    pub version: String,
    pub kind: PluginKind,
    pub origin: PluginOrigin,
    pub manifest: Value,
    pub package_hash: Option<String>,
    pub signature: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone)]
pub struct PluginInstallationRecord {
    pub plugin_id: String,
    pub installed_version: String,
    pub origin: PluginOrigin,
    pub install_status: PluginInstallStatus,
    pub install_path: Option<String>,
    pub package_hash: Option<String>,
    pub installed_at: i64,
    pub updated_at: i64,
    pub last_error_code: Option<String>,
    pub last_error_message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct McpPluginSourceRecord {
    pub server_id: String,
    pub catalog_plugin_id: Option<String>,
    pub origin: PluginOrigin,
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
    pub transport: String,
    pub endpoint_url: Option<String>,
    pub definition: Option<String>,
    pub enabled: bool,
    pub status: String,
    pub credential_configured: bool,
    pub last_error: Option<String>,
    pub last_checked_at: Option<i64>,
    pub last_latency_ms: Option<i64>,
    pub tool_count: Option<i64>,
    pub consecutive_failures: i64,
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
    #[serde(default = "default_mcp_transport")]
    pub transport: String,
    pub endpoint_url: Option<String>,
    pub definition: Option<String>,
    #[serde(default)]
    pub environment: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub clear_environment: bool,
}

fn default_mcp_transport() -> String {
    "stdio".to_owned()
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleHookRecord {
    pub id: String,
    pub name: String,
    pub event: String,
    pub matcher: String,
    pub action: String,
    pub reason: String,
    pub enabled: bool,
    pub priority: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLifecycleHookRequest {
    pub id: Option<String>,
    pub name: String,
    pub event: String,
    pub matcher: String,
    pub action: String,
    pub reason: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_hook_priority")]
    pub priority: i64,
}

fn default_true() -> bool {
    true
}

fn default_hook_priority() -> i64 {
    100
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleHookIdRequest {
    pub hook_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLifecycleHookEnabledRequest {
    pub hook_id: String,
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
    #[serde(default)]
    pub expert_id: Option<String>,
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
pub struct ConversationExpertBindRequest {
    pub conversation_id: String,
    pub expert_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationPinnedRequest {
    pub conversation_id: String,
    pub pinned: bool,
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
pub struct ConversationForkRequest {
    pub conversation_id: String,
    pub message_id: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalIdRequest {
    pub conversation_id: String,
    pub goal_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateProjectPermissionRequest {
    pub project_id: String,
    pub permission_mode: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateConversationPermissionRequest {
    pub conversation_id: String,
    pub permission_mode: String,
    pub request_id: String,
    pub expected_version: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectIdRequest {
    pub project_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveApprovalRequest {
    pub approval_id: String,
    pub decision: ApprovalDecision,
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
    /// #6: an explicit user-chosen execution budget. Absent means the historical
    /// default, so an old caller behaves exactly as before.
    #[serde(default)]
    pub budget: Option<RunBudgetSelection>,
}

/// A user's explicit run-budget choice. The tier is frozen into the Run's control
/// binding and shown back to the user; it never means "unbounded".
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBudgetSelection {
    #[serde(default)]
    pub tier: crate::database::BudgetTier,
    /// Only meaningful with `tier = "custom"`.
    #[serde(default)]
    pub custom_execution_ms: Option<i64>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRunResult {
    pub run: RunRecord,
    pub user_message: MessageRecord,
    pub attachments: Vec<AttachmentRecord>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RewindRunRequest {
    pub conversation_id: String,
    pub message_id: String,
    pub text: String,
    pub runtime_text: Option<String>,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGoalRunningRequest {
    pub conversation_id: String,
    pub goal_id: String,
    pub expected_version: i64,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGoalRunningResult {
    pub goal: GoalRecord,
    pub started_run: Option<StartRunResult>,
}

#[derive(Debug, Clone)]
pub struct PendingWorkModeDispatch {
    pub goal_id: String,
    pub runtime_text: String,
    pub started: StartRunResult,
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
    pub icon: Option<String>,
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
    pub icon: Option<String>,
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
pub struct MemoryEntityRecord {
    pub id: String,
    pub scope: String,
    pub scope_key: String,
    pub kind: String,
    pub canonical_key: String,
    pub content: String,
    pub state: String,
    pub enabled: bool,
    pub source_conversation_id: Option<String>,
    pub source_message_id: Option<String>,
    pub source_run_id: Option<String>,
    pub evidence_excerpt: String,
    pub created_by: String,
    pub confidence: f64,
    pub version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub confirmed_at: Option<i64>,
    pub disabled_at: Option<i64>,
    pub deleted_at: Option<i64>,
    pub open_conflict_id: Option<String>,
    pub recall_count: i64,
    pub last_recalled_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRevisionRecord {
    pub id: String,
    pub memory_id: String,
    pub actor: String,
    pub action: String,
    pub before: Option<Value>,
    pub after: Option<Value>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryConflictRecord {
    pub id: String,
    pub existing_memory_id: String,
    pub competing_memory_id: String,
    pub status: String,
    pub resolution: Option<String>,
    pub created_at: i64,
    pub resolved_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallRecord {
    pub id: String,
    pub memory_id: String,
    pub conversation_id: String,
    pub run_id: Option<String>,
    pub query: String,
    pub reason: String,
    pub score: f64,
    pub rank: i64,
    pub evidence_excerpt: String,
    pub recalled_at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallItem {
    pub id: String,
    pub scope: String,
    pub kind: String,
    pub canonical_key: String,
    pub content: String,
    pub evidence_excerpt: String,
    pub reason: String,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallBundle {
    pub status: String,
    pub query: String,
    pub items: Vec<MemoryRecallItem>,
    pub total_chars: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryListRequest {
    pub agent_id: Option<String>,
    pub project_id: Option<String>,
    #[serde(default)]
    pub include_deleted: bool,
    pub query: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMemoryRequest {
    pub scope: String,
    pub scope_key: Option<String>,
    pub kind: String,
    pub canonical_key: String,
    pub content: String,
    pub evidence_excerpt: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryIdRequest {
    pub memory_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryMutationRequest {
    pub memory_id: String,
    pub expected_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemoryRequest {
    pub memory_id: String,
    pub kind: String,
    pub canonical_key: String,
    pub content: String,
    pub evidence_excerpt: Option<String>,
    pub expected_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemoryEnabledRequest {
    pub memory_id: String,
    pub enabled: bool,
    pub expected_version: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveMemoryConflictRequest {
    pub conflict_id: String,
    pub decision: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryRecallListRequest {
    pub memory_id: String,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct MemoryProposalInput {
    pub scope: String,
    pub kind: String,
    pub canonical_key: String,
    pub content: String,
    pub evidence_excerpt: String,
    pub source_message_id: Option<String>,
    pub confidence: f64,
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
