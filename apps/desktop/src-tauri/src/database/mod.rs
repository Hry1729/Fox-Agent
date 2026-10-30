mod migrations;
mod migration_backup;
mod kernel_changes;
mod models;
mod repositories;
#[cfg(test)]
mod migration_copy_tests;

/// Highest schema version this binary writes. v100 is deliberately skipped: the
/// contacts plan reserves that number, so the single-path convergence work took
/// v101 for tools and v102 for model admission. The runner applies each version by presence in
/// `schema_migrations`, not by ordering, so a later v100 still lands correctly.
pub(crate) const DATABASE_SCHEMA_VERSION: i64 = 104;
pub(crate) use repositories::{AcceptanceBinding, AcceptanceRow, AcceptanceWrite};
pub(crate) use repositories::{
    delivery_review_downgrade_blocker, DowngradeBlocker, CURRENT_U1_AWARE_SCHEMA_VERSION,
};
pub(crate) use repositories::kernel_execution_admission;
pub(crate) use repositories::permission_rules::{ConversationAccess, PermissionAuditRecord, PermissionRuleRecord};
pub(crate) use repositories::RunBackendIdentity;
pub(crate) use repositories::{EXECUTOR_ROUTE_RETIRED, EXECUTOR_ROUTE_RETIRED_MESSAGE, EXECUTOR_ROUTE_UNKNOWN};
pub(crate) use repositories::kernel_reconciliation::*;
pub(crate) use repositories::ModelNoticeInput;
pub(crate) use repositories::{BudgetTier, ContinuableRun, ContinuationRequest, GrantRegistration, GrantScopeKind, GrantSkipReason};
pub(crate) use repositories::{JobSnapshot, JobStartOutcome, JobStartRequest, JobState};
pub(crate) use repositories::{
    run_budget_for_tier, PreparedContinuation, VerifiedContinuationPermission,
};
#[cfg(test)]
pub(crate) use repositories::finalize_fault;
#[cfg(test)]
pub(crate) use repositories::native_session_fault;
#[cfg(test)]
pub(crate) use repositories::stage_fault;
pub use models::*;
/// Cap on `tool_calls.result_json`. Published here because the model-view binder
/// uses it to decide whether omitted bytes are recoverable at all — one source
/// of truth for "what Host keeps" instead of a duplicated literal.
pub(crate) use repositories::MAX_STORED_TOOL_RESULT_BYTES;
pub(crate) use repositories::bounded_tool_result_preview;
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
    EVIDENCE_REFERENCE_PAIRS, WORK_EVENT_TYPES,
};
pub(crate) use repositories::{
    package_snapshot_hash, CreateChildRunInput, DeliveryArtifactRow, DeliveryChecklistItem,
    DeliveryChecklistSeed, DeliveryRequirement, RequirementKind,
    KernelHostScope, ManagedFileSource, ManagedFileVersion, ManagedFileVersionInput,
    RestoreClaim, RestoreRequestRecord, RunWriteReceipt, StagedDeliveryItem,
    MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS,
    SkillActivationRecord, SteeringDecision, SteeringMessage, MAX_STEERING_FOLLOWUPS,
    STEERING_LANE_CURRENT, STEERING_LANE_NEXT_TURN,
};
#[cfg(test)]
pub(crate) use repositories::cancel_open_steering_in_tx;
#[allow(unused_imports)]
pub use repositories::{
    GraphLeadActivationResult, GraphLeadEdgeSnapshot, GraphLeadNodeFinishResult,
    GraphLeadNodeReadiness, GraphLeadNodeSnapshot, GraphLeadNodeStartResult, GraphLeadSnapshot,
};
