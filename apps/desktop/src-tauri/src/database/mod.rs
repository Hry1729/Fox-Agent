mod migrations;
mod kernel_changes;
mod models;
mod repositories;

pub(crate) const DATABASE_SCHEMA_VERSION: i64 = 84;
pub(crate) use repositories::kernel_execution_admission;
pub(crate) use repositories::kernel_reconciliation::*;
pub(crate) use repositories::{BudgetTier, ContinuableRun, ContinuationRequest, GrantRegistration, GrantScopeKind, GrantSkipReason};
pub(crate) use repositories::{JobSnapshot, JobStartOutcome, JobStartRequest, JobState};
pub(crate) use repositories::{
    run_budget_for_tier, PreparedContinuation, VerifiedContinuationPermission,
};
pub use models::*;
/// Cap on `tool_calls.result_json`. Published here because the model-view binder
/// uses it to decide whether omitted bytes are recoverable at all — one source
/// of truth for "what Host keeps" instead of a duplicated literal.
pub(crate) use repositories::MAX_STORED_TOOL_RESULT_BYTES;
#[allow(unused_imports)]
pub use repositories::{
    now_ms, ActivateReadOnlyGraphInput, ActiveGraphNodeReviewRequest, AddEvidenceInput,
    AppNotificationRecord, AppNotificationUpsert, CreateGoalInput,
    CreateGraphNodeCancelIntentInput, CreateGraphNodeReviewRequestInput,
    CreateReadOnlyGraphAcceptanceInput, CreateTaskInput, Database, FinishReadOnlyGraphNodeInput,
    FinishTaskAttemptInput, GlobalSearchRecord, GraphAcceptanceIntentResult, GraphAcceptanceResult,
    GraphCriterionEvidenceInput, GraphNodeReviewActivationResult, GraphNodeReviewDecisionResult,
    GraphNodeReviewOutcome, GraphNodeReviewRequestResult, GraphReviewerDispatchResult,
    MessageFeedbackRecord, NotificationPreferencesRecord, PendingGraphAcceptance,
    PreflightTaskRepairOverrideInput, ProjectManagementRecord, RepositoryError,
    StartReadyReadOnlyGraphNodeInput, StartTaskAttemptInput, StartTaskRepairOverrideInput,
    WORK_EVENT_TYPES,
};
pub(crate) use repositories::{
    package_snapshot_hash, CreateChildRunInput, DeliveryArtifactRow, DeliveryChecklistItem,
    DeliveryChecklistSeed, DeliveryRequirement, RequirementKind,
    KernelHostScope, ManagedFileSource, ManagedFileVersion, ManagedFileVersionInput,
    RestoreClaim, RestoreRequestRecord,
    MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS,
    SkillActivationRecord, SteeringDecision, SteeringMessage, MAX_STEERING_FOLLOWUPS,
};
#[allow(unused_imports)]
pub use repositories::{
    GraphLeadActivationResult, GraphLeadEdgeSnapshot, GraphLeadNodeFinishResult,
    GraphLeadNodeReadiness, GraphLeadNodeSnapshot, GraphLeadNodeStartResult, GraphLeadSnapshot,
};
