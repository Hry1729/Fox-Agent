use super::{
    child_runs::{
        create_child_run_in_transaction, query_child_run_by_tool_call, query_child_start_result,
        validate_child_run_budget, CreateChildRunInput,
    },
    now_ms,
    work_graph::{
        finish_graph_task_attempt_in_transaction, finish_task_attempt_in_transaction,
        freeze_run_execution_profile_in_transaction, load_evidence, load_task, load_task_attempt,
        start_task_attempt_in_transaction, validate_evidence_check_semantics, validate_reference,
        with_write_transaction, StartTaskAttemptInput,
    },
    Database, RepositoryError,
};
use crate::database::{
    ChildRunBudget, ChildRunRecord, EvidenceReferenceKind, EvidenceType, EvidenceValidityStatus,
    FinishTaskAttemptInput, StartRunResult, TaskAttemptFinishResult, TaskAttemptKind,
    TaskAttemptRecord, TaskAttemptStatus, WorkTaskRecord, WorkTaskStatus,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

const GRAPH_SCHEMA_VERSION: i64 = 1;
const GRAPH_MODE_READ_ONLY: &str = "read_only";
const GRAPH_ACCESS_READ_ONLY: &str = "read_only";
const GRAPH_MAX_NODES: usize = 3;
const GRAPH_MAX_DEPTH: i64 = 1;
const MAX_NODE_KEY_CHARS: usize = 100;
const MAX_TITLE_CHARS: usize = 300;
const MAX_DETAIL_CHARS: usize = 4_000;
const MAX_ACCEPTANCE_CRITERIA: usize = 8;
const MAX_ACCEPTANCE_CRITERION_CHARS: usize = 1_000;
const MAX_SNAPSHOT_EVIDENCE_IDS: usize = 100;
const GRAPH_NODE_START_TOOL: &str = "graph_readonly_node_start";
const GRAPH_NODE_FINISH_TOOL: &str = "graph_readonly_node_finish";
const GRAPH_NODE_CANCEL_TOOL: &str = "graph_readonly_node_cancel";
const GRAPH_NODE_REVIEW_TOOL: &str = "graph_readonly_node_review";
const GRAPH_ACCEPT_TOOL: &str = "graph_readonly_accept";
const GRAPH_CHILD_PROFILE: &str = "durable_v2";
const GRAPH_REVIEWER_CONTRACT_HASH: &str =
    "24a66ac5c9d85eeacabaf662192815995c600ff77703231624d2971e191b1c18";
const GRAPH_CHILD_ALLOWED_TOOLS: [&str; 4] = ["read", "ls", "find", "grep"];
const MAX_NODE_OBJECTIVE_CHARS: usize = 8_000;
const MAX_NODE_CONTEXT_CHARS: usize = 12_000;
const GRAPH_CHILD_MAX_DURATION_MS: i64 = 45_000;
const GRAPH_CHILD_MAX_TOTAL_TOKENS: i64 = 4_096;
const GRAPH_CHILD_MAX_OUTPUT_TOKENS: i64 = 1_024;
const GRAPH_CHILD_MAX_TOOL_CALLS: i64 = 6;
const MAX_CRITERION_EVIDENCE_PER_CRITERION: usize = 16;
const MAX_GRAPH_FINISH_EVIDENCE_BINDINGS: usize = 100;
const MAX_GRAPH_FINISH_SUMMARY_CHARS: usize = 4_000;
const MAX_GRAPH_CANCEL_REASON_CHARS: usize = 2_000;
const GRAPH_REVIEWER_OBJECTIVE: &str = "Independently review high-risk Graph node";

#[derive(Debug, Clone)]
pub struct ActivateReadOnlyGraphInput {
    pub plan_revision_id: String,
    pub parent_run_id: String,
    pub activation_tool_call_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphLeadNodeReadiness {
    Runnable,
    Waiting,
    Active,
    Accepted,
    Blocked,
    Interrupted,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadNodeSnapshot {
    pub task_id: String,
    pub node_key: String,
    pub title: String,
    pub detail: Option<String>,
    pub ordinal: i64,
    pub depth: i64,
    pub acceptance_criteria: Vec<String>,
    pub task_status: String,
    pub task_version: i64,
    pub readiness: GraphLeadNodeReadiness,
    pub dependency_task_ids: Vec<String>,
    pub authoritative_attempt_id: Option<String>,
    pub evidence_ids: Vec<String>,
    pub blocker: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadEdgeSnapshot {
    pub dependency_task_id: String,
    pub dependent_task_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadSnapshot {
    pub schema_version: i64,
    pub graph_id: String,
    pub plan_revision_id: String,
    pub graph_mode: String,
    pub max_nodes: i64,
    pub max_depth: i64,
    pub spec_hash: String,
    pub activated_by_run_id: String,
    pub activated_at: i64,
    pub nodes: Vec<GraphLeadNodeSnapshot>,
    pub edges: Vec<GraphLeadEdgeSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadActivationResult {
    pub replayed: bool,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone)]
pub struct StartReadyReadOnlyGraphNodeInput {
    pub goal_id: String,
    pub task_id: String,
    pub parent_run_id: String,
    pub node_start_tool_call_id: String,
    pub expected_task_version: i64,
    pub attempt_id: String,
    pub worker_agent_id: String,
    pub objective: String,
    pub context: String,
    pub budget: ChildRunBudget,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadNodeStartResult {
    pub created: bool,
    pub replayed: bool,
    pub started: StartRunResult,
    pub child_run: ChildRunRecord,
    pub task: WorkTaskRecord,
    pub attempt: TaskAttemptRecord,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphCriterionEvidenceInput {
    pub criterion: String,
    pub evidence_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FinishReadOnlyGraphNodeInput {
    pub goal_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub parent_run_id: String,
    pub node_finish_tool_call_id: String,
    pub expected_task_version: i64,
    pub expected_attempt_version: i64,
    pub criterion_evidence: Vec<GraphCriterionEvidenceInput>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphLeadNodeFinishResult {
    pub replayed: bool,
    pub child_run_id: String,
    pub child_result_hash: String,
    pub criterion_evidence: Vec<GraphCriterionEvidenceInput>,
    pub finish: TaskAttemptFinishResult,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone)]
pub struct CreateGraphNodeCancelIntentInput {
    pub goal_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub parent_run_id: String,
    pub node_cancel_tool_call_id: String,
    pub expected_task_version: i64,
    pub expected_attempt_version: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeCancelIntentResult {
    pub created: bool,
    pub replayed: bool,
    pub pending_activation: bool,
    pub intent_id: String,
    pub child_run_id: String,
    pub task: WorkTaskRecord,
    pub attempt: TaskAttemptRecord,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeCancelActivationResult {
    pub activated: bool,
    pub stale: bool,
    pub intent_id: String,
    pub child_run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGraphNodeCancelIntent {
    pub intent_id: String,
    pub child_run_id: String,
}

#[derive(Debug, Clone)]
pub struct CreateGraphNodeReviewRequestInput {
    pub goal_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub parent_run_id: String,
    pub review_tool_call_id: String,
    pub expected_task_version: i64,
    pub expected_attempt_version: i64,
    pub criterion_evidence: Vec<GraphCriterionEvidenceInput>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeReviewRequestResult {
    pub created: bool,
    pub replayed: bool,
    pub pending_activation: bool,
    pub request_id: String,
    pub implementation_child_run_id: String,
    pub candidate_hash: String,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeReviewActivationResult {
    pub request_id: String,
    pub activated: bool,
    pub stale: bool,
    pub replayed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveGraphNodeReviewRequest {
    pub request_id: String,
    pub dispatched: bool,
    pub reviewer_child_run_id: Option<String>,
    pub reviewer_agent_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphReviewerDispatchResult {
    pub request_id: String,
    pub created: bool,
    pub replayed: bool,
    pub dispatch_required: bool,
    pub started: StartRunResult,
    pub child_run: ChildRunRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GraphNodeReviewOutcome {
    Pass,
    Revise,
    Inconclusive,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphNodeReviewDecisionResult {
    pub request_id: String,
    pub decision_id: String,
    pub reviewer_child_run_id: String,
    pub outcome: GraphNodeReviewOutcome,
    pub summary: String,
    pub replayed: bool,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone)]
pub struct CreateReadOnlyGraphAcceptanceInput {
    pub goal_id: String,
    pub submitter_run_id: String,
    pub accept_tool_call_id: String,
    pub expected_goal_version: i64,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphAcceptanceIntentResult {
    pub acceptance_id: String,
    pub created: bool,
    pub replayed: bool,
    pub pending_activation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingGraphAcceptance {
    pub acceptance_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphAcceptanceResult {
    pub acceptance_id: String,
    pub accepted: bool,
    pub stale: bool,
    pub replayed: bool,
    pub snapshot: GraphLeadSnapshot,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct NormalizedPlanNode {
    node_key: String,
    title: String,
    detail: Option<String>,
    ordinal: i64,
    depends_on: Vec<String>,
    acceptance_criteria: Vec<String>,
    depth: i64,
}

#[derive(Debug, Clone)]
struct BoundPlanNode {
    normalized: NormalizedPlanNode,
    task_id: String,
}

#[derive(Debug, Clone)]
struct StoredSpec {
    goal_id: String,
    plan_revision_id: String,
    schema_version: i64,
    graph_mode: String,
    max_nodes: i64,
    max_depth: i64,
    spec_hash: String,
    activated_by_run_id: String,
    activation_tool_call_id: String,
    activated_at: i64,
}

#[derive(Debug, Clone)]
struct StoredNode {
    task_id: String,
    node_key: String,
    depth: i64,
    acceptance_criteria: Vec<String>,
    title: String,
    detail: Option<String>,
    ordinal: i64,
    task_status: String,
    task_version: i64,
    task_attempt: i64,
}

#[derive(Debug, Clone)]
struct StoredGraphDelegation {
    id: String,
    parent_run_id: String,
    child_run_id: String,
    child_conversation_id: String,
    status: String,
    child_status: String,
    child_finished_at: Option<i64>,
}

#[derive(Debug)]
struct GraphTerminalCandidate {
    goal_id: String,
    task_id: String,
    attempt_id: String,
    parent_run_id: String,
    delegation_id: String,
    delegation_status: String,
    child_status: String,
    child_finished_at: Option<i64>,
    child_error_code: Option<String>,
    child_error_message: Option<String>,
    task_version: i64,
    attempt_version: i64,
    task_status: String,
    task_owner_run_id: Option<String>,
    attempt_status: String,
    attempt_kind: String,
    goal_status: String,
    parent_run_status: String,
    active_cancel_intent_id: Option<String>,
}

#[derive(Debug, Clone)]
struct StoredGraphAcceptance {
    goal_id: String,
    plan_revision_id: String,
    submitter_run_id: String,
    tool_call_id: String,
    expected_goal_version: i64,
    summary: String,
    node_projection_json: String,
    node_projection_hash: String,
    checks_json: String,
    checks_hash: String,
    input_json: String,
    accepted_at: Option<i64>,
}

#[derive(Debug, Clone)]
struct GraphReviewerProofBinding {
    criterion_ordinal: i64,
    criterion: String,
    tool_call_id: String,
    runtime_tool_call_id: String,
}

#[derive(Debug, Clone, Default)]
struct AcceptedAttemptProjection {
    accepted: bool,
    attempt_id: Option<String>,
    evidence_ids: Vec<String>,
    blocker: Option<String>,
}

impl Database {
    pub fn activate_read_only_graph(
        &self,
        input: ActivateReadOnlyGraphInput,
    ) -> Result<GraphLeadActivationResult, RepositoryError> {
        validate_identifier("plan revision id", &input.plan_revision_id)?;
        validate_identifier("parent run id", &input.parent_run_id)?;
        validate_identifier("activation tool call id", &input.activation_tool_call_id)?;

        with_write_transaction(self, |transaction| {
            let (goal_id, conversation_id, plan_revision, tasks_json, status) = transaction
                .query_row(
                    "SELECT goal_id, conversation_id, revision, tasks_json, status
                     FROM plan_revisions WHERE id = ?1",
                    [input.plan_revision_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "plan revision",
                    id: input.plan_revision_id.clone(),
                })?;
            if status != "approved" {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "read-only Graph activation requires an approved PlanRevision; '{}' is '{status}'",
                    input.plan_revision_id
                )));
            }
            require_frozen_durable_parent_run(transaction, &input.parent_run_id, &conversation_id)?;
            require_activation_tool_call(
                transaction,
                &input.activation_tool_call_id,
                &input.parent_run_id,
                &conversation_id,
                &input.plan_revision_id,
            )?;

            let tasks_value = serde_json::from_str::<Value>(&tasks_json).map_err(|error| {
                RepositoryError::InvalidInput(format!(
                    "PlanRevision tasks are not valid JSON: {error}"
                ))
            })?;
            let normalized_nodes = normalize_plan_nodes(&tasks_value)?;
            let bound_nodes = bind_plan_nodes(transaction, &goal_id, normalized_nodes)?;
            let spec_hash = graph_spec_hash(&goal_id, &input.plan_revision_id, &bound_nodes)?;

            if let Some(existing) = load_spec(transaction, &goal_id)? {
                let exact = existing.plan_revision_id == input.plan_revision_id
                    && existing.activated_by_run_id == input.parent_run_id
                    && existing.activation_tool_call_id == input.activation_tool_call_id
                    && existing.schema_version == GRAPH_SCHEMA_VERSION
                    && existing.graph_mode == GRAPH_MODE_READ_ONLY
                    && existing.max_nodes == GRAPH_MAX_NODES as i64
                    && existing.max_depth == GRAPH_MAX_DEPTH
                    && existing.spec_hash == spec_hash;
                if !exact {
                    return Err(RepositoryError::ConstraintViolation(format!(
                        "Goal '{goal_id}' already has an immutable Graph activation with conflicting content"
                    )));
                }
                return Ok(GraphLeadActivationResult {
                    replayed: true,
                    snapshot: project_snapshot(transaction, existing)?,
                });
            }

            let conflicting_tool_goal = transaction
                .query_row(
                    "SELECT goal_id FROM work_graph_specs
                     WHERE activation_tool_call_id = ?1 AND goal_id <> ?2",
                    params![input.activation_tool_call_id, goal_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?;
            if let Some(other_goal_id) = conflicting_tool_goal {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "activation ToolCall '{}' already belongs to Goal '{other_goal_id}'",
                    input.activation_tool_call_id
                )));
            }

            let goal_status = transaction
                .query_row(
                    "SELECT status FROM goals WHERE id = ?1 AND conversation_id = ?2",
                    params![goal_id, conversation_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: goal_id.clone(),
                })?;
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph activation",
                    from: goal_status,
                    to: "active_graph".to_owned(),
                });
            }
            let newer_authoritative_plan = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1 FROM plan_revisions
                        WHERE goal_id = ?1
                          AND status IN ('proposed', 'approved')
                          AND revision > ?2
                     )",
                    params![goal_id, plan_revision],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?;
            if newer_authoritative_plan {
                return Err(RepositoryError::ConstraintViolation(
                    "read-only Graph activation is paused because a newer PlanRevision is proposed or approved"
                        .to_owned(),
                ));
            }
            require_pristine_durable_tasks(transaction, &goal_id, &bound_nodes)?;

            let activated_at = now_ms();
            transaction
                .execute(
                    "INSERT INTO work_graph_specs(
                        goal_id, plan_revision_id, schema_version, graph_mode, max_nodes,
                        max_depth, spec_hash, activated_by_run_id, activation_tool_call_id,
                        activated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        goal_id,
                        input.plan_revision_id,
                        GRAPH_SCHEMA_VERSION,
                        GRAPH_MODE_READ_ONLY,
                        GRAPH_MAX_NODES as i64,
                        GRAPH_MAX_DEPTH,
                        spec_hash,
                        input.parent_run_id,
                        input.activation_tool_call_id,
                        activated_at,
                    ],
                )
                .map_err(database_error)?;

            let by_key = bound_nodes
                .iter()
                .map(|node| (node.normalized.node_key.as_str(), node.task_id.as_str()))
                .collect::<HashMap<_, _>>();
            for node in &bound_nodes {
                let criteria_json = serde_json::to_string(&node.normalized.acceptance_criteria)
                    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
                transaction
                    .execute(
                        "INSERT INTO work_graph_nodes(
                            task_id, goal_id, node_key, access_mode, depth,
                            acceptance_criteria_json, created_at
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![
                            node.task_id,
                            goal_id,
                            node.normalized.node_key,
                            GRAPH_ACCESS_READ_ONLY,
                            node.normalized.depth,
                            criteria_json,
                            activated_at,
                        ],
                    )
                    .map_err(database_error)?;
            }
            for dependent in &bound_nodes {
                for dependency_key in &dependent.normalized.depends_on {
                    let dependency_task_id =
                        by_key.get(dependency_key.as_str()).ok_or_else(|| {
                            RepositoryError::InvalidReference(format!(
                                "Graph dependency '{dependency_key}' disappeared during activation"
                            ))
                        })?;
                    transaction
                        .execute(
                            "INSERT INTO work_task_edges(
                                goal_id, dependency_task_id, dependent_task_id, created_at
                             ) VALUES (?1, ?2, ?3, ?4)",
                            params![goal_id, dependency_task_id, dependent.task_id, activated_at],
                        )
                        .map_err(database_error)?;
                }
            }

            let stored = load_spec(transaction, &goal_id)?.ok_or_else(|| {
                RepositoryError::ConstraintViolation(
                    "Graph activation was not visible inside its transaction".to_owned(),
                )
            })?;
            Ok(GraphLeadActivationResult {
                replayed: false,
                snapshot: project_snapshot(transaction, stored)?,
            })
        })
    }

    pub fn read_only_graph_snapshot(
        &self,
        goal_id: &str,
    ) -> Result<Option<GraphLeadSnapshot>, RepositoryError> {
        validate_identifier("goal id", goal_id)?;
        with_graph_read_transaction(self, |transaction| {
            load_spec(transaction, goal_id)?
                .map(|spec| project_snapshot(transaction, spec))
                .transpose()
        })
    }

    pub fn start_ready_read_only_graph_node(
        &self,
        input: StartReadyReadOnlyGraphNodeInput,
    ) -> Result<GraphLeadNodeStartResult, RepositoryError> {
        validate_graph_node_start_input(&input)?;
        let allowed_tools = GRAPH_CHILD_ALLOWED_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect::<Vec<_>>();
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let spec = load_spec(transaction, &input.goal_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: input.goal_id.clone(),
                }
            })?;
            let (conversation_id, goal_status) = transaction
                .query_row(
                    "SELECT conversation_id, status FROM goals WHERE id = ?1",
                    [input.goal_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: input.goal_id.clone(),
                })?;
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node",
                    from: goal_status,
                    to: "in_progress".to_owned(),
                });
            }
            require_frozen_durable_parent_run(transaction, &input.parent_run_id, &conversation_id)?;
            let newer_authoritative_plan = transaction
                .query_row(
                    "SELECT EXISTS(
                        SELECT 1
                        FROM plan_revisions newer
                        JOIN plan_revisions activated ON activated.id = ?2
                        WHERE newer.goal_id = ?1
                          AND newer.status IN ('proposed', 'approved')
                          AND newer.revision > activated.revision
                     )",
                    params![input.goal_id, spec.plan_revision_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(database_error)?;
            if newer_authoritative_plan {
                return Err(RepositoryError::ConstraintViolation(
                    "read-only Graph node start is paused because a newer PlanRevision is proposed or approved"
                        .to_owned(),
                ));
            }
            require_node_start_tool_call(transaction, &input, &conversation_id)?;

            if let Some(existing_child) = query_child_run_by_tool_call(
                transaction,
                &input.parent_run_id,
                &input.node_start_tool_call_id,
            )
            .map_err(database_error)?
            {
                return replay_graph_node_start(
                    transaction,
                    &input,
                    spec,
                    existing_child,
                    &allowed_tools,
                );
            }
            if load_task_attempt(transaction, &input.attempt_id)?.is_some() {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node Attempt '{}' already exists without this exact delegation",
                    input.attempt_id
                )));
            }
            let attempt_delegation = transaction
                .query_row(
                    "SELECT tool_call_id FROM child_run_delegations
                     WHERE graph_task_attempt_id = ?1",
                    [input.attempt_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?;
            if attempt_delegation.is_some() {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node Attempt '{}' already belongs to another delegation",
                    input.attempt_id
                )));
            }

            let before = project_snapshot(transaction, spec.clone())?;
            let node = before
                .nodes
                .iter()
                .find(|node| node.task_id == input.task_id)
                .ok_or_else(|| {
                    RepositoryError::InvalidReference(format!(
                        "Task '{}' is not a node in Graph '{}'",
                        input.task_id, input.goal_id
                    ))
                })?;
            if node.readiness != GraphLeadNodeReadiness::Runnable || node.task_status != "queued" {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node '{}' is not runnable; readiness={:?}, status={}",
                    node.node_key, node.readiness, node.task_status
                )));
            }
            let current_task = load_task(transaction, &input.task_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task",
                    id: input.task_id.clone(),
                }
            })?;
            if current_task.goal_id != input.goal_id {
                return Err(RepositoryError::CrossConversationReference);
            }
            if current_task.version != input.expected_task_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.task_id.clone(),
                    expected_version: input.expected_task_version,
                });
            }

            let attempt_start = start_task_attempt_in_transaction(
                transaction,
                &StartTaskAttemptInput {
                    id: input.attempt_id.clone(),
                    task_id: input.task_id.clone(),
                    run_id: input.parent_run_id.clone(),
                    expected_task_version: input.expected_task_version,
                    kind: TaskAttemptKind::Execution,
                    root_cause: None,
                    finding_ids: Vec::new(),
                },
                &now.to_string(),
            )?;
            let attempt = attempt_start.attempt.ok_or_else(|| {
                RepositoryError::ConstraintViolation(
                    "Graph node Execution Attempt was not created".to_owned(),
                )
            })?;
            let (started, child_run, created) = create_child_run_in_transaction(
                transaction,
                CreateChildRunInput {
                    parent_run_id: &input.parent_run_id,
                    tool_call_id: &input.node_start_tool_call_id,
                    worker_agent_id: &input.worker_agent_id,
                    objective: &input.objective,
                    context: &input.context,
                    budget: &input.budget,
                    team_run_id: None,
                    team_member_id: None,
                    allowed_tools: Some(&allowed_tools),
                },
                now,
                Some(&input.attempt_id),
            )
            .map_err(database_error)?;
            if !created {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph node delegation appeared after the exact replay check".to_owned(),
                ));
            }
            freeze_run_execution_profile_in_transaction(
                transaction,
                &child_run.child_run_id,
                GRAPH_CHILD_PROFILE,
                &now.to_string(),
            )?;
            let snapshot = project_snapshot(transaction, spec)?;
            Ok(GraphLeadNodeStartResult {
                created: true,
                replayed: false,
                started,
                child_run,
                task: attempt_start.task,
                attempt,
                snapshot,
            })
        })
    }

    pub fn finish_read_only_graph_node(
        &self,
        input: FinishReadOnlyGraphNodeInput,
    ) -> Result<GraphLeadNodeFinishResult, RepositoryError> {
        validate_graph_node_finish_input(&input)?;
        let canonical_input = node_finish_tool_input(&input);
        let input_json = serde_json::to_string(&canonical_input)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let input_hash = sha256_hex(input_json.as_bytes());
        let now = now_ms();

        with_write_transaction(self, |transaction| {
            let spec = load_spec(transaction, &input.goal_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: input.goal_id.clone(),
                }
            })?;
            let (conversation_id, goal_status) = transaction
                .query_row(
                    "SELECT conversation_id, status FROM goals WHERE id = ?1",
                    [input.goal_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: input.goal_id.clone(),
                })?;

            if let Some(replayed) =
                replay_graph_node_finish(transaction, &input, &spec, &input_json, &input_hash)?
            {
                return Ok(replayed);
            }
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node",
                    from: goal_status,
                    to: "completed".to_owned(),
                });
            }
            require_frozen_durable_parent_run(transaction, &input.parent_run_id, &conversation_id)?;
            require_no_newer_authoritative_plan(
                transaction,
                &input.goal_id,
                &spec.plan_revision_id,
                "finish",
            )?;
            require_node_finish_tool_call(
                transaction,
                &input,
                &conversation_id,
                &input_json,
                false,
            )?;

            let node = load_stored_node(transaction, &input.goal_id, &input.task_id)?;
            if node.acceptance_criteria.len() != input.criterion_evidence.len()
                || node
                    .acceptance_criteria
                    .iter()
                    .zip(&input.criterion_evidence)
                    .any(|(frozen, supplied)| frozen != &supplied.criterion)
            {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph node finish must bind every frozen criterion exactly once in snapshot order"
                        .to_owned(),
                ));
            }

            let task = load_task(transaction, &input.task_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task",
                    id: input.task_id.clone(),
                }
            })?;
            let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task attempt",
                    id: input.attempt_id.clone(),
                }
            })?;
            if task.goal_id != input.goal_id
                || attempt.task_id != input.task_id
                || attempt.run_id != input.parent_run_id
            {
                return Err(RepositoryError::CrossConversationReference);
            }
            if task.version != input.expected_task_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.task_id.clone(),
                    expected_version: input.expected_task_version,
                });
            }
            if attempt.version != input.expected_attempt_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.attempt_id.clone(),
                    expected_version: input.expected_attempt_version,
                });
            }
            if task.status != WorkTaskStatus::InProgress
                || task.owner_run_id.as_deref() != Some(input.parent_run_id.as_str())
                || attempt.kind != TaskAttemptKind::Execution
                || attempt.status != TaskAttemptStatus::Running
            {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node Attempt",
                    from: format!("{:?}", attempt.status).to_ascii_lowercase(),
                    to: "succeeded".to_owned(),
                });
            }

            let (policy_id, policy_hash) = transaction
                .query_row(
                    "SELECT policy_id, policy_hash FROM task_validation_policies WHERE task_id = ?1",
                    [input.task_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::InvalidValidationPolicy(
                    "Graph Task has no frozen ValidationPolicy".to_owned(),
                ))?;
            if policy_hash != attempt.policy_hash {
                return Err(RepositoryError::InvalidValidationPolicy(
                    "Graph Attempt policy hash no longer matches the Task frozen policy".to_owned(),
                ));
            }
            if !matches!(policy_id.as_str(), "standard_v1" | "high_risk_v1") {
                return Err(RepositoryError::InvalidValidationPolicy(format!(
                    "read-only Graph node finish supports standard_v1 or reviewed high_risk_v1; found '{policy_id}'"
                )));
            }

            let (child_run_id, child_result_hash) = authoritative_completed_graph_child(
                transaction,
                &input.attempt_id,
                &input.parent_run_id,
            )?;
            let attempt_started_at = transaction
                .query_row(
                    "SELECT started_at FROM task_attempts WHERE id = ?1",
                    [input.attempt_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?;
            for binding in &input.criterion_evidence {
                for evidence_id in &binding.evidence_ids {
                    validate_graph_criterion_evidence(
                        transaction,
                        &input.task_id,
                        &input.parent_run_id,
                        &conversation_id,
                        attempt.evidence_rowid_watermark,
                        attempt_started_at,
                        evidence_id,
                    )?;
                }
            }
            if policy_id == "high_risk_v1" {
                require_graph_review_pass(
                    transaction,
                    &input.attempt_id,
                    &input_json,
                    &child_result_hash,
                    &policy_hash,
                )?;
            }

            let finish_id = graph_finish_id(&input.node_finish_tool_call_id);
            transaction
                .execute(
                    "INSERT INTO work_graph_node_finishes(
                        id, goal_id, task_id, attempt_id, parent_run_id, child_run_id,
                        tool_call_id, validation_policy_id, validation_policy_hash,
                        expected_task_version, expected_attempt_version, child_result_hash,
                        summary, input_json, input_hash, created_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                        ?13, ?14, ?15, ?16
                     )",
                    params![
                        finish_id,
                        input.goal_id,
                        input.task_id,
                        input.attempt_id,
                        input.parent_run_id,
                        child_run_id,
                        input.node_finish_tool_call_id,
                        policy_id,
                        policy_hash,
                        input.expected_task_version,
                        input.expected_attempt_version,
                        child_result_hash,
                        input.summary,
                        input_json,
                        input_hash,
                        now,
                    ],
                )
                .map_err(database_error)?;
            for (criterion_ordinal, criterion) in input.criterion_evidence.iter().enumerate() {
                for evidence_id in &criterion.evidence_ids {
                    transaction
                        .execute(
                            "INSERT INTO work_graph_node_criterion_evidence(
                                finish_id, criterion_ordinal, criterion, evidence_id
                             ) VALUES (?1, ?2, ?3, ?4)",
                            params![
                                finish_id,
                                criterion_ordinal as i64,
                                criterion.criterion,
                                evidence_id,
                            ],
                        )
                        .map_err(database_error)?;
                }
            }

            let finish = finish_graph_task_attempt_in_transaction(
                transaction,
                &FinishTaskAttemptInput {
                    id: input.attempt_id.clone(),
                    task_id: input.task_id.clone(),
                    run_id: input.parent_run_id.clone(),
                    expected_task_version: input.expected_task_version,
                    expected_attempt_version: input.expected_attempt_version,
                    status: TaskAttemptStatus::Succeeded,
                    failure_reason: None,
                },
                &now.to_string(),
            )?;
            let snapshot = project_snapshot(transaction, spec)?;
            Ok(GraphLeadNodeFinishResult {
                replayed: false,
                child_run_id,
                child_result_hash,
                criterion_evidence: input.criterion_evidence.clone(),
                finish,
                snapshot,
            })
        })
    }

    pub fn create_graph_node_review_request(
        &self,
        input: CreateGraphNodeReviewRequestInput,
    ) -> Result<GraphNodeReviewRequestResult, RepositoryError> {
        validate_graph_node_review_input(&input)?;
        let canonical_input = node_review_tool_input(&input);
        let input_json = serde_json::to_string(&canonical_input)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let input_hash = sha256_hex(input_json.as_bytes());
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let spec = load_spec(transaction, &input.goal_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: input.goal_id.clone(),
                }
            })?;
            if let Some(existing) = replay_graph_node_review_request(
                transaction,
                &input,
                &spec,
                &input_json,
                &input_hash,
            )? {
                return Ok(existing);
            }
            let (conversation_id, goal_status) = transaction
                .query_row(
                    "SELECT conversation_id, status FROM goals WHERE id = ?1",
                    [input.goal_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: input.goal_id.clone(),
                })?;
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node",
                    from: goal_status,
                    to: "review_pending".to_owned(),
                });
            }
            require_frozen_durable_parent_run(transaction, &input.parent_run_id, &conversation_id)?;
            require_no_newer_authoritative_plan(
                transaction,
                &input.goal_id,
                &spec.plan_revision_id,
                "review",
            )?;
            require_node_review_tool_call(
                transaction,
                &input,
                &conversation_id,
                &input_json,
                false,
            )?;
            let node = load_stored_node(transaction, &input.goal_id, &input.task_id)?;
            require_exact_criterion_bindings(&node, &input.criterion_evidence)?;
            let task = load_task(transaction, &input.task_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task",
                    id: input.task_id.clone(),
                }
            })?;
            let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task attempt",
                    id: input.attempt_id.clone(),
                }
            })?;
            require_live_graph_attempt(
                &input.goal_id,
                &input.task_id,
                &input.attempt_id,
                &input.parent_run_id,
                input.expected_task_version,
                input.expected_attempt_version,
                &task,
                &attempt,
                "review_pending",
            )?;
            let (policy_id, policy_hash) = graph_attempt_policy(transaction, &task.id, &attempt)?;
            if policy_id != "high_risk_v1" {
                return Err(RepositoryError::InvalidValidationPolicy(format!(
                    "Graph node review requires high_risk_v1; found '{policy_id}'"
                )));
            }
            let delegations = load_graph_attempt_delegations(transaction, &input.attempt_id)?;
            if delegations.len() != 1 {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph Attempt '{}' requires exactly one implementation Child",
                    input.attempt_id
                )));
            }
            let implementation = &delegations[0];
            if implementation.parent_run_id != input.parent_run_id
                || implementation.status != "completed"
                || implementation.child_status != "completed"
                || implementation.child_finished_at.is_none()
            {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph review requires the authoritative implementation Child result"
                        .to_owned(),
                ));
            }
            let (child_run_id, child_result_hash) = authoritative_completed_graph_child(
                transaction,
                &input.attempt_id,
                &input.parent_run_id,
            )?;
            let attempt_started_at = transaction
                .query_row(
                    "SELECT started_at FROM task_attempts WHERE id = ?1",
                    [input.attempt_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(database_error)?;
            for binding in &input.criterion_evidence {
                for evidence_id in &binding.evidence_ids {
                    validate_graph_criterion_evidence(
                        transaction,
                        &input.task_id,
                        &input.parent_run_id,
                        &conversation_id,
                        attempt.evidence_rowid_watermark,
                        attempt_started_at,
                        evidence_id,
                    )?;
                }
            }
            let candidate = json!({
                "attemptId": input.attempt_id,
                "childResultHash": child_result_hash,
                "criterionEvidence": input.criterion_evidence,
                "expectedAttemptVersion": input.expected_attempt_version,
                "expectedTaskVersion": input.expected_task_version,
                "goalId": input.goal_id,
                "policyHash": policy_hash,
                "summary": input.summary,
                "taskId": input.task_id,
            });
            let candidate_json = serde_json::to_string(&candidate)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            let candidate_hash = sha256_hex(candidate_json.as_bytes());
            let request_id = graph_review_request_id(&input.review_tool_call_id);
            transaction
                .execute(
                    "INSERT INTO work_graph_node_review_requests(
                        id, goal_id, task_id, attempt_id, parent_run_id,
                        implementation_delegation_id, implementation_child_run_id,
                        tool_call_id, review_contract_id, review_contract_hash,
                        validation_policy_id, validation_policy_hash,
                        expected_task_version, expected_attempt_version, child_result_hash,
                        summary, candidate_json, candidate_hash, input_json, input_hash,
                        created_at, activated_at, settled_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                        'graph_reviewer_v1', ?9, 'high_risk_v1', ?10,
                        ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, NULL, NULL
                     )",
                    params![
                        request_id,
                        input.goal_id,
                        input.task_id,
                        input.attempt_id,
                        input.parent_run_id,
                        implementation.id,
                        child_run_id,
                        input.review_tool_call_id,
                        GRAPH_REVIEWER_CONTRACT_HASH,
                        policy_hash,
                        input.expected_task_version,
                        input.expected_attempt_version,
                        child_result_hash,
                        input.summary,
                        candidate_json,
                        candidate_hash,
                        input_json,
                        input_hash,
                        now,
                    ],
                )
                .map_err(database_error)?;
            for (criterion_ordinal, criterion) in input.criterion_evidence.iter().enumerate() {
                for evidence_id in &criterion.evidence_ids {
                    transaction
                        .execute(
                            "INSERT INTO work_graph_node_review_criterion_evidence(
                                request_id, criterion_ordinal, criterion, evidence_id
                             ) VALUES (?1, ?2, ?3, ?4)",
                            params![
                                request_id,
                                criterion_ordinal as i64,
                                criterion.criterion,
                                evidence_id,
                            ],
                        )
                        .map_err(database_error)?;
                }
            }
            Ok(GraphNodeReviewRequestResult {
                created: true,
                replayed: false,
                pending_activation: true,
                request_id,
                implementation_child_run_id: child_run_id,
                candidate_hash,
                snapshot: project_snapshot(transaction, spec)?,
            })
        })
    }

    pub fn activate_graph_node_review_request(
        &self,
        request_id: &str,
    ) -> Result<GraphNodeReviewActivationResult, RepositoryError> {
        validate_identifier("Graph node review request id", request_id)?;
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let (activated_at, settled_at, live) = transaction
                .query_row(
                    "SELECT request.activated_at, request.settled_at,
                            EXISTS(
                                SELECT 1
                                FROM goals AS goal
                                JOIN work_tasks AS task ON task.goal_id = goal.id
                                  AND task.id = request.task_id
                                  AND task.status = 'in_progress'
                                  AND task.owner_run_id = request.parent_run_id
                                  AND task.version = request.expected_task_version
                                JOIN task_attempts AS attempt ON attempt.id = request.attempt_id
                                  AND attempt.task_id = task.id
                                  AND attempt.status = 'running'
                                  AND attempt.version = request.expected_attempt_version
                                JOIN child_run_delegations AS implementation
                                  ON implementation.id = request.implementation_delegation_id
                                  AND implementation.status = 'completed'
                                JOIN runs AS child_run
                                  ON child_run.id = request.implementation_child_run_id
                                  AND child_run.status = 'completed'
                                JOIN tool_calls AS review_call ON review_call.id = request.tool_call_id
                                  AND review_call.status = 'completed'
                                  AND review_call.error_message IS NULL
                                  AND review_call.result_json IS NOT NULL
                                WHERE goal.id = request.goal_id
                                  AND goal.status = 'active'
                            )
                     FROM work_graph_node_review_requests AS request
                     WHERE request.id = ?1",
                    [request_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, bool>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "Graph node review request",
                    id: request_id.to_owned(),
                })?;
            if activated_at.is_some() {
                return Ok(GraphNodeReviewActivationResult {
                    request_id: request_id.to_owned(),
                    activated: false,
                    stale: false,
                    replayed: true,
                });
            }
            if settled_at.is_some() || !live {
                return Ok(GraphNodeReviewActivationResult {
                    request_id: request_id.to_owned(),
                    activated: false,
                    stale: true,
                    replayed: false,
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE work_graph_node_review_requests SET activated_at = ?2
                     WHERE id = ?1 AND activated_at IS NULL AND settled_at IS NULL",
                    params![request_id, now],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph node review request changed concurrently before activation".to_owned(),
                ));
            }
            Ok(GraphNodeReviewActivationResult {
                request_id: request_id.to_owned(),
                activated: true,
                stale: false,
                replayed: false,
            })
        })
    }

    pub fn active_graph_node_review_requests_for_recovery(
        &self,
    ) -> Result<Vec<ActiveGraphNodeReviewRequest>, RepositoryError> {
        with_graph_read_transaction(self, |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT request.id, reviewer.child_run_id, parent_conversation.agent_id
                     FROM work_graph_node_review_requests AS request
                     JOIN goals AS goal ON goal.id = request.goal_id AND goal.status = 'active'
                     JOIN runs AS parent_run ON parent_run.id = request.parent_run_id
                     JOIN conversations AS parent_conversation
                       ON parent_conversation.id = parent_run.conversation_id
                     LEFT JOIN child_run_delegations AS reviewer
                       ON reviewer.parent_run_id = request.parent_run_id
                      AND reviewer.tool_call_id = request.tool_call_id
                      AND reviewer.graph_task_attempt_id IS NULL
                     WHERE request.activated_at IS NOT NULL
                       AND request.settled_at IS NULL
                       AND NOT EXISTS(SELECT 1 FROM run_control_bindings binding
                           WHERE binding.run_id=request.parent_run_id AND binding.authority='authoritative')
                     ORDER BY request.activated_at, request.id",
                )
                .map_err(database_error)?;
            let rows = statement
                .query_map([], |row| {
                    let child_run_id = row.get::<_, Option<String>>(1)?;
                    Ok(ActiveGraphNodeReviewRequest {
                        request_id: row.get(0)?,
                        dispatched: child_run_id.is_some(),
                        reviewer_child_run_id: child_run_id,
                        reviewer_agent_id: row.get(2)?,
                    })
                })
                .map_err(database_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
        })
    }

    pub fn dispatch_graph_node_reviewer(
        &self,
        request_id: &str,
        reviewer_agent_id: &str,
    ) -> Result<GraphReviewerDispatchResult, RepositoryError> {
        validate_identifier("Graph node review request id", request_id)?;
        validate_identifier("Graph Reviewer agent id", reviewer_agent_id)?;
        let now = now_ms();
        let budget = ChildRunBudget {
            max_duration_ms: GRAPH_CHILD_MAX_DURATION_MS,
            max_total_tokens: GRAPH_CHILD_MAX_TOTAL_TOKENS,
            max_output_tokens: GRAPH_CHILD_MAX_OUTPUT_TOKENS,
            max_tool_calls: GRAPH_CHILD_MAX_TOOL_CALLS,
        };
        let allowed_tools = GRAPH_CHILD_ALLOWED_TOOLS
            .iter()
            .map(|tool| (*tool).to_owned())
            .collect::<Vec<_>>();
        with_write_transaction(self, |transaction| {
            let (parent_run_id, tool_call_id, candidate_json, activated_at, settled_at) = transaction
                .query_row(
                    "SELECT parent_run_id, tool_call_id, candidate_json, activated_at, settled_at
                     FROM work_graph_node_review_requests WHERE id = ?1",
                    [request_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "Graph node review request",
                    id: request_id.to_owned(),
                })?;
            if activated_at.is_none() || settled_at.is_some() {
                return Err(RepositoryError::InvalidTransition {
                    entity: "Graph node review request",
                    from: if activated_at.is_none() {
                        "pending"
                    } else {
                        "settled"
                    }
                    .to_owned(),
                    to: "reviewer_dispatched".to_owned(),
                });
            }
            let (started, child_run, created) = create_child_run_in_transaction(
                transaction,
                CreateChildRunInput {
                    parent_run_id: &parent_run_id,
                    tool_call_id: &tool_call_id,
                    worker_agent_id: reviewer_agent_id,
                    objective: GRAPH_REVIEWER_OBJECTIVE,
                    context: &candidate_json,
                    budget: &budget,
                    team_run_id: None,
                    team_member_id: None,
                    allowed_tools: Some(&allowed_tools),
                },
                now,
                None,
            )
            .map_err(database_error)?;
            let dispatch_required = matches!(child_run.status.as_str(), "queued" | "running");
            Ok(GraphReviewerDispatchResult {
                request_id: request_id.to_owned(),
                created,
                replayed: !created,
                dispatch_required,
                started,
                child_run,
            })
        })
    }

    pub fn settle_graph_node_review(
        &self,
        request_id: &str,
    ) -> Result<GraphNodeReviewDecisionResult, RepositoryError> {
        validate_identifier("Graph node review request id", request_id)?;
        settle_graph_node_review_by(self, "request.id = ?1", request_id)
    }

    pub fn settle_graph_node_review_for_child(
        &self,
        child_run_id: &str,
    ) -> Result<GraphNodeReviewDecisionResult, RepositoryError> {
        validate_identifier("Graph Reviewer Child Run id", child_run_id)?;
        settle_graph_node_review_by(self, "reviewer.child_run_id = ?1", child_run_id)
    }

    pub fn create_read_only_graph_acceptance(
        &self,
        input: CreateReadOnlyGraphAcceptanceInput,
    ) -> Result<GraphAcceptanceIntentResult, RepositoryError> {
        validate_graph_acceptance_input(&input)?;
        let canonical_input = graph_accept_tool_input(&input);
        let input_json = serde_json::to_string(&canonical_input)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let input_hash = sha256_hex(input_json.as_bytes());
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            if let Some(existing) =
                replay_graph_acceptance_intent(transaction, &input, &input_json, &input_hash)?
            {
                return Ok(existing);
            }
            let spec = load_spec(transaction, &input.goal_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: input.goal_id.clone(),
                }
            })?;
            let (conversation_id, goal_status, goal_version) = transaction
                .query_row(
                    "SELECT conversation_id, status, version FROM goals WHERE id = ?1",
                    [input.goal_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: input.goal_id.clone(),
                })?;
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph Goal",
                    from: goal_status,
                    to: "accepted".to_owned(),
                });
            }
            if goal_version != input.expected_goal_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.goal_id.clone(),
                    expected_version: input.expected_goal_version,
                });
            }
            require_frozen_durable_parent_run(
                transaction,
                &input.submitter_run_id,
                &conversation_id,
            )?;
            require_no_newer_authoritative_plan(
                transaction,
                &input.goal_id,
                &spec.plan_revision_id,
                "accept",
            )?;
            require_graph_accept_tool_call(
                transaction,
                &input,
                &conversation_id,
                &input_json,
                false,
            )?;
            let (node_projection, checks) = graph_acceptance_projection(transaction, &spec)?;
            let node_projection_json = serde_json::to_string(&node_projection)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            let checks_json = serde_json::to_string(&checks)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            let acceptance_id = graph_acceptance_id(&input.accept_tool_call_id);
            transaction
                .execute(
                    "INSERT INTO work_graph_acceptances(
                        id, goal_id, plan_revision_id, spec_hash, submitter_run_id,
                        tool_call_id, expected_goal_version, summary,
                        node_projection_json, node_projection_hash, checks_json, checks_hash,
                        input_json, input_hash, created_at, accepted_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14, ?15, NULL
                     )",
                    params![
                        acceptance_id,
                        input.goal_id,
                        spec.plan_revision_id,
                        spec.spec_hash,
                        input.submitter_run_id,
                        input.accept_tool_call_id,
                        input.expected_goal_version,
                        input.summary,
                        node_projection_json,
                        sha256_hex(node_projection_json.as_bytes()),
                        checks_json,
                        sha256_hex(checks_json.as_bytes()),
                        input_json,
                        input_hash,
                        now,
                    ],
                )
                .map_err(database_error)?;
            Ok(GraphAcceptanceIntentResult {
                acceptance_id,
                created: true,
                replayed: false,
                pending_activation: true,
            })
        })
    }

    pub fn activate_read_only_graph_acceptance(
        &self,
        acceptance_id: &str,
    ) -> Result<GraphAcceptanceResult, RepositoryError> {
        validate_identifier("Graph acceptance id", acceptance_id)?;
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let row = load_graph_acceptance_row(transaction, acceptance_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "Graph acceptance",
                    id: acceptance_id.to_owned(),
                }
            })?;
            let spec =
                load_spec(transaction, &row.goal_id)?.ok_or_else(|| RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: row.goal_id.clone(),
                })?;
            if row.accepted_at.is_some() {
                return Ok(GraphAcceptanceResult {
                    acceptance_id: acceptance_id.to_owned(),
                    accepted: false,
                    stale: false,
                    replayed: true,
                    snapshot: project_snapshot(transaction, spec)?,
                });
            }
            let (goal_status, goal_version, conversation_id) = transaction
                .query_row(
                    "SELECT status, version, conversation_id FROM goals WHERE id = ?1",
                    [row.goal_id.as_str()],
                    |record| {
                        Ok((
                            record.get::<_, String>(0)?,
                            record.get::<_, i64>(1)?,
                            record.get::<_, String>(2)?,
                        ))
                    },
                )
                .map_err(database_error)?;
            if goal_status != "active" || goal_version != row.expected_goal_version {
                return Ok(GraphAcceptanceResult {
                    acceptance_id: acceptance_id.to_owned(),
                    accepted: false,
                    stale: true,
                    replayed: false,
                    snapshot: project_snapshot(transaction, spec)?,
                });
            }
            require_no_newer_authoritative_plan(
                transaction,
                &row.goal_id,
                &row.plan_revision_id,
                "accept",
            )?;
            require_graph_accept_tool_call_stored(
                transaction,
                &row.tool_call_id,
                &row.submitter_run_id,
                &conversation_id,
                &row.input_json,
            )?;
            let (projection, checks) = graph_acceptance_projection(transaction, &spec)?;
            let projection_json = serde_json::to_string(&projection)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            let checks_json = serde_json::to_string(&checks)
                .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
            if projection_json != row.node_projection_json
                || sha256_hex(projection_json.as_bytes()) != row.node_projection_hash
                || checks_json != row.checks_json
                || sha256_hex(checks_json.as_bytes()) != row.checks_hash
            {
                return Ok(GraphAcceptanceResult {
                    acceptance_id: acceptance_id.to_owned(),
                    accepted: false,
                    stale: true,
                    replayed: false,
                    snapshot: project_snapshot(transaction, spec)?,
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE work_graph_acceptances SET accepted_at = ?2
                     WHERE id = ?1 AND accepted_at IS NULL",
                    params![acceptance_id, now],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph acceptance changed concurrently before activation".to_owned(),
                ));
            }
            transaction
                .execute(
                    "INSERT INTO acceptances(
                        id, goal_id, plan_revision_id, conversation_id, status,
                        summary, checks_json, reviewer, created_at, resolved_at
                     ) VALUES (?1, ?2, ?3, ?4, 'accepted', ?5, ?6, ?7, ?8, ?8)",
                    params![
                        acceptance_id,
                        row.goal_id,
                        row.plan_revision_id,
                        conversation_id,
                        row.summary,
                        row.checks_json,
                        row.submitter_run_id,
                        now.to_string(),
                    ],
                )
                .map_err(database_error)?;
            let changed = transaction
                .execute(
                    "UPDATE goals
                     SET status = 'completed', version = version + 1,
                         acceptance_summary = ?3, updated_at = ?4, completed_at = ?4,
                         blocked_reason = NULL
                     WHERE id = ?1 AND status = 'active' AND version = ?2",
                    params![
                        row.goal_id,
                        row.expected_goal_version,
                        row.summary,
                        now.to_string(),
                    ],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: row.goal_id,
                    expected_version: row.expected_goal_version,
                });
            }
            Ok(GraphAcceptanceResult {
                acceptance_id: acceptance_id.to_owned(),
                accepted: true,
                stale: false,
                replayed: false,
                snapshot: project_snapshot(transaction, spec)?,
            })
        })
    }

    pub fn pending_graph_acceptances_for_recovery(
        &self,
    ) -> Result<Vec<PendingGraphAcceptance>, RepositoryError> {
        with_graph_read_transaction(self, |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT graph_acceptance.id
                     FROM work_graph_acceptances AS graph_acceptance
                     JOIN goals AS goal
                       ON goal.id = graph_acceptance.goal_id AND goal.status = 'active'
                     JOIN tool_calls AS accept_call
                       ON accept_call.id = graph_acceptance.tool_call_id
                      AND accept_call.status = 'completed'
                      AND accept_call.error_message IS NULL
                     WHERE graph_acceptance.accepted_at IS NULL
                       AND NOT EXISTS(SELECT 1 FROM run_control_bindings binding
                           WHERE binding.run_id=accept_call.run_id AND binding.authority='authoritative')
                     ORDER BY graph_acceptance.created_at, graph_acceptance.id",
                )
                .map_err(database_error)?;
            let rows = statement
                .query_map([], |row| {
                    Ok(PendingGraphAcceptance {
                        acceptance_id: row.get(0)?,
                    })
                })
                .map_err(database_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(database_error)
        })
    }

    pub fn create_graph_node_cancel_intent(
        &self,
        input: CreateGraphNodeCancelIntentInput,
    ) -> Result<GraphNodeCancelIntentResult, RepositoryError> {
        validate_graph_node_cancel_input(&input)?;
        let canonical_input = node_cancel_tool_input(&input);
        let input_json = serde_json::to_string(&canonical_input)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let input_hash = sha256_hex(input_json.as_bytes());
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let spec = load_spec(transaction, &input.goal_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "read-only graph",
                    id: input.goal_id.clone(),
                }
            })?;
            if let Some(existing) = replay_graph_node_cancel_intent(
                transaction,
                &input,
                &spec,
                &input_json,
                &input_hash,
            )? {
                return Ok(existing);
            }
            let (conversation_id, goal_status) = transaction
                .query_row(
                    "SELECT conversation_id, status FROM goals WHERE id = ?1",
                    [input.goal_id.as_str()],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "goal",
                    id: input.goal_id.clone(),
                })?;
            if goal_status != "active" {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node",
                    from: goal_status,
                    to: "cancel_pending".to_owned(),
                });
            }
            require_frozen_durable_parent_run(transaction, &input.parent_run_id, &conversation_id)?;
            require_no_newer_authoritative_plan(
                transaction,
                &input.goal_id,
                &spec.plan_revision_id,
                "cancel",
            )?;
            require_node_cancel_tool_call(
                transaction,
                &input,
                &conversation_id,
                &input_json,
                false,
            )?;
            let task = load_task(transaction, &input.task_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task",
                    id: input.task_id.clone(),
                }
            })?;
            let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
                RepositoryError::NotFound {
                    entity: "graph task attempt",
                    id: input.attempt_id.clone(),
                }
            })?;
            if task.goal_id != input.goal_id
                || attempt.task_id != input.task_id
                || attempt.run_id != input.parent_run_id
            {
                return Err(RepositoryError::CrossConversationReference);
            }
            if task.version != input.expected_task_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.task_id.clone(),
                    expected_version: input.expected_task_version,
                });
            }
            if attempt.version != input.expected_attempt_version {
                return Err(RepositoryError::OptimisticLockFailed {
                    id: input.attempt_id.clone(),
                    expected_version: input.expected_attempt_version,
                });
            }
            if task.status != WorkTaskStatus::InProgress
                || task.owner_run_id.as_deref() != Some(input.parent_run_id.as_str())
                || attempt.kind != TaskAttemptKind::Execution
                || attempt.status != TaskAttemptStatus::Running
            {
                return Err(RepositoryError::InvalidTransition {
                    entity: "read-only Graph node Attempt",
                    from: format!("{:?}", attempt.status).to_ascii_lowercase(),
                    to: "cancel_pending".to_owned(),
                });
            }
            require_standard_graph_policy(transaction, &task.id, &attempt)?;
            let delegations = load_graph_attempt_delegations(transaction, &input.attempt_id)?;
            if delegations.len() != 1 {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph Attempt '{}' requires exactly one authoritative Child delegation",
                    input.attempt_id
                )));
            }
            let delegation = &delegations[0];
            if delegation.parent_run_id != input.parent_run_id
                || !matches!(delegation.status.as_str(), "queued" | "running")
                || !matches!(
                    delegation.child_status.as_str(),
                    "queued" | "running" | "cancelling"
                )
                || delegation.child_finished_at.is_some()
            {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph node cancel intent requires a live parent-owned Child delegation"
                        .to_owned(),
                ));
            }
            require_frozen_durable_child_run(
                transaction,
                &delegation.child_run_id,
                &input.parent_run_id,
                &delegation.child_conversation_id,
            )?;
            let intent_id = graph_cancel_intent_id(&input.node_cancel_tool_call_id);
            transaction
                .execute(
                    "INSERT INTO work_graph_node_cancel_intents(
                        id, goal_id, task_id, attempt_id, parent_run_id, delegation_id,
                        child_run_id, tool_call_id, expected_task_version,
                        expected_attempt_version, reason, input_json, input_hash, created_at,
                        activated_at
                     ) VALUES (
                        ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, NULL
                     )",
                    params![
                        intent_id,
                        input.goal_id,
                        input.task_id,
                        input.attempt_id,
                        input.parent_run_id,
                        delegation.id,
                        delegation.child_run_id,
                        input.node_cancel_tool_call_id,
                        input.expected_task_version,
                        input.expected_attempt_version,
                        input.reason,
                        input_json,
                        input_hash,
                        now,
                    ],
                )
                .map_err(database_error)?;
            Ok(GraphNodeCancelIntentResult {
                created: true,
                replayed: false,
                pending_activation: true,
                intent_id,
                child_run_id: delegation.child_run_id.clone(),
                task,
                attempt,
                snapshot: project_snapshot(transaction, spec)?,
            })
        })
    }

    pub fn activate_graph_node_cancel_intent(
        &self,
        intent_id: &str,
    ) -> Result<GraphNodeCancelActivationResult, RepositoryError> {
        validate_identifier("Graph node cancel intent id", intent_id)?;
        let now = now_ms();
        with_write_transaction(self, |transaction| {
            let row = transaction
                .query_row(
                    "SELECT intent.child_run_id, intent.activated_at,
                            EXISTS(
                                SELECT 1
                                FROM work_graph_nodes AS node
                                JOIN work_tasks AS task ON task.id = node.task_id
                                  AND task.goal_id = node.goal_id
                                  AND task.status = 'in_progress'
                                  AND task.owner_run_id = intent.parent_run_id
                                  AND task.version = intent.expected_task_version
                                JOIN goals AS goal ON goal.id = node.goal_id
                                  AND goal.status = 'active'
                                JOIN task_attempts AS attempt ON attempt.id = intent.attempt_id
                                  AND attempt.task_id = node.task_id
                                  AND attempt.run_id = intent.parent_run_id
                                  AND attempt.kind = 'execution'
                                  AND attempt.status = 'running'
                                  AND attempt.version = intent.expected_attempt_version
                                JOIN child_run_delegations AS delegation
                                  ON delegation.id = intent.delegation_id
                                  AND delegation.graph_task_attempt_id = attempt.id
                                  AND delegation.parent_run_id = intent.parent_run_id
                                  AND delegation.child_run_id = intent.child_run_id
                                  AND delegation.status IN ('queued', 'running')
                                JOIN runs AS parent_run ON parent_run.id = intent.parent_run_id
                                  AND parent_run.conversation_id = goal.conversation_id
                                  AND parent_run.status = 'running'
                                JOIN runs AS child_run ON child_run.id = intent.child_run_id
                                  AND child_run.status IN ('queued', 'running', 'cancelling')
                                  AND child_run.finished_at IS NULL
                                JOIN tool_calls AS cancel_call ON cancel_call.id = intent.tool_call_id
                                  AND cancel_call.status = 'completed'
                                  AND cancel_call.error_message IS NULL
                                  AND cancel_call.result_json IS NOT NULL
                                  AND cancel_call.input_json = intent.input_json
                                WHERE node.task_id = intent.task_id
                                  AND node.goal_id = intent.goal_id
                            )
                     FROM work_graph_node_cancel_intents AS intent
                     WHERE intent.id = ?1",
                    [intent_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, bool>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "Graph node cancel intent",
                    id: intent_id.to_owned(),
                })?;
            if row.1.is_some() {
                return Ok(GraphNodeCancelActivationResult {
                    activated: false,
                    stale: false,
                    intent_id: intent_id.to_owned(),
                    child_run_id: row.0,
                });
            }
            if !row.2 {
                return Ok(GraphNodeCancelActivationResult {
                    activated: false,
                    stale: true,
                    intent_id: intent_id.to_owned(),
                    child_run_id: row.0,
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE work_graph_node_cancel_intents SET activated_at = ?2
                     WHERE id = ?1 AND activated_at IS NULL",
                    params![intent_id, now],
                )
                .map_err(database_error)?;
            if changed != 1 {
                return Err(RepositoryError::ConstraintViolation(
                    "Graph node cancel intent changed concurrently before activation".to_owned(),
                ));
            }
            Ok(GraphNodeCancelActivationResult {
                activated: true,
                stale: false,
                intent_id: intent_id.to_owned(),
                child_run_id: row.0,
            })
        })
    }

    pub fn active_graph_node_cancel_intents_for_recovery(
        &self,
    ) -> Result<Vec<ActiveGraphNodeCancelIntent>, RepositoryError> {
        with_graph_read_transaction(self, |connection| {
            connection
                .prepare(
                    "SELECT intent.id, intent.child_run_id
                     FROM work_graph_node_cancel_intents AS intent
                     JOIN goals AS goal ON goal.id = intent.goal_id AND goal.status = 'active'
                     JOIN work_tasks AS task ON task.id = intent.task_id
                       AND task.status = 'in_progress'
                       AND task.owner_run_id = intent.parent_run_id
                       AND task.version = intent.expected_task_version
                     JOIN task_attempts AS attempt ON attempt.id = intent.attempt_id
                       AND attempt.status = 'running'
                       AND attempt.version = intent.expected_attempt_version
                     JOIN child_run_delegations AS delegation ON delegation.id = intent.delegation_id
                       AND delegation.status IN ('queued', 'running')
                     JOIN runs AS parent_run ON parent_run.id = intent.parent_run_id
                       AND parent_run.status = 'running'
                     JOIN runs AS child_run ON child_run.id = intent.child_run_id
                       AND child_run.status IN ('queued', 'running', 'cancelling')
                       AND child_run.finished_at IS NULL
                     WHERE intent.activated_at IS NOT NULL
                       AND NOT EXISTS(SELECT 1 FROM run_control_bindings binding
                           WHERE binding.run_id=intent.parent_run_id AND binding.authority='authoritative')
                     ORDER BY intent.activated_at, intent.id",
                )
                .map_err(database_error)?
                .query_map([], |row| {
                    Ok(ActiveGraphNodeCancelIntent {
                        intent_id: row.get(0)?,
                        child_run_id: row.get(1)?,
                    })
                })
                .map_err(database_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(database_error)
        })
    }
}

fn settle_graph_node_review_by(
    database: &Database,
    predicate: &str,
    value: &str,
) -> Result<GraphNodeReviewDecisionResult, RepositoryError> {
    let now = now_ms();
    with_write_transaction(database, |transaction| {
        let sql = format!(
            "SELECT request.id, request.goal_id, request.task_id, request.attempt_id,
                    request.parent_run_id, request.candidate_hash,
                    request.expected_task_version, request.expected_attempt_version,
                    request.activated_at, request.settled_at,
                    reviewer.id, reviewer.child_run_id, reviewer.child_conversation_id,
                    reviewer.worker_agent_id, reviewer.status, reviewer.result_text,
                    reviewer.error_code, reviewer.error_message, reviewer.finished_at,
                    reviewer_run.status, reviewer_run.finished_at,
                    reviewer_run.error_code, reviewer_run.error_message
             FROM work_graph_node_review_requests AS request
             LEFT JOIN child_run_delegations AS reviewer
               ON reviewer.parent_run_id = request.parent_run_id
              AND reviewer.tool_call_id = request.tool_call_id
              AND reviewer.graph_task_attempt_id IS NULL
             LEFT JOIN runs AS reviewer_run ON reviewer_run.id = reviewer.child_run_id
             WHERE {predicate}"
        );
        let row = transaction
            .query_row(&sql, [value], |record| {
                Ok((
                    record.get::<_, String>(0)?,
                    record.get::<_, String>(1)?,
                    record.get::<_, String>(2)?,
                    record.get::<_, String>(3)?,
                    record.get::<_, String>(4)?,
                    record.get::<_, String>(5)?,
                    record.get::<_, i64>(6)?,
                    record.get::<_, i64>(7)?,
                    record.get::<_, Option<i64>>(8)?,
                    record.get::<_, Option<i64>>(9)?,
                    record.get::<_, Option<String>>(10)?,
                    record.get::<_, Option<String>>(11)?,
                    record.get::<_, Option<String>>(12)?,
                    record.get::<_, Option<String>>(13)?,
                    record.get::<_, Option<String>>(14)?,
                    record.get::<_, Option<String>>(15)?,
                    record.get::<_, Option<String>>(16)?,
                    record.get::<_, Option<String>>(17)?,
                    record.get::<_, Option<i64>>(18)?,
                    record.get::<_, Option<String>>(19)?,
                    record.get::<_, Option<i64>>(20)?,
                    record.get::<_, Option<String>>(21)?,
                    record.get::<_, Option<String>>(22)?,
                ))
            })
            .optional()
            .map_err(database_error)?
            .ok_or_else(|| RepositoryError::NotFound {
                entity: "active Graph node review request",
                id: value.to_owned(),
            })?;
        let request_id = row.0;
        let spec = load_spec(transaction, &row.1)?.ok_or_else(|| RepositoryError::NotFound {
            entity: "read-only graph",
            id: row.1.clone(),
        })?;
        if let Some(existing) = load_graph_review_decision(transaction, &request_id)? {
            return Ok(GraphNodeReviewDecisionResult {
                request_id,
                decision_id: existing.0,
                reviewer_child_run_id: existing.1,
                outcome: parse_graph_review_outcome(&existing.2)?,
                summary: existing.3,
                replayed: true,
                snapshot: project_snapshot(transaction, spec)?,
            });
        }
        if row.8.is_none() || row.9.is_some() {
            return Err(RepositoryError::InvalidTransition {
                entity: "Graph node review request",
                from: if row.8.is_none() {
                    "pending"
                } else {
                    "settled"
                }
                .to_owned(),
                to: "settled".to_owned(),
            });
        }
        let reviewer_delegation_id = row.10.ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "active Graph review request has no Reviewer delegation".to_owned(),
            )
        })?;
        let reviewer_run_id = row.11.ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "Graph Reviewer delegation has no Child Run".to_owned(),
            )
        })?;
        let reviewer_conversation_id = row.12.ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "Graph Reviewer delegation has no Child conversation".to_owned(),
            )
        })?;
        let reviewer_agent_id = row.13.ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "Graph Reviewer delegation has no worker Agent".to_owned(),
            )
        })?;
        let delegation_status = row.14.unwrap_or_default();
        let run_status = row.19.unwrap_or_default();
        if delegation_status != run_status
            || !matches!(
                run_status.as_str(),
                "completed" | "failed" | "cancelled" | "interrupted"
            )
            || row.20.is_none()
        {
            return Err(RepositoryError::InvalidTransition {
                entity: "Graph Reviewer Child",
                from: run_status,
                to: "terminal decision".to_owned(),
            });
        }
        let raw_result = row
            .15
            .clone()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| {
                transaction
                    .query_row(
                        "SELECT content FROM messages
                     WHERE run_id = ?1 AND role = 'assistant' AND length(trim(content)) > 0
                     ORDER BY ordinal DESC, id DESC LIMIT 1",
                        [reviewer_run_id.as_str()],
                        |record| record.get::<_, String>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
            });
        let (mut outcome, mut summary, mut decision_json) = authoritative_graph_review_decision(
            &run_status,
            raw_result.as_deref(),
            row.22.as_deref(),
        );
        let mut proof_bindings = Vec::new();
        if outcome != GraphNodeReviewOutcome::Inconclusive {
            match validate_graph_reviewer_decision_proof(
                transaction,
                &row.2,
                &reviewer_run_id,
                &reviewer_conversation_id,
                row.8.expect("active request has activation time"),
                row.20.expect("terminal Reviewer has finish time"),
                &outcome,
                &decision_json,
            ) {
                Ok(bindings) => proof_bindings = bindings,
                Err(_) => {
                    outcome = GraphNodeReviewOutcome::Inconclusive;
                    summary =
                        "independent Graph Reviewer proof was invalid or incomplete".to_owned();
                    decision_json = serde_json::to_string(&json!({
                        "criteria": [],
                        "findings": [],
                        "recommendation": "inconclusive",
                        "summary": summary,
                    }))
                    .expect("static Reviewer decision is JSON");
                }
            }
        }
        let terminal = json!({
            "childRunId": reviewer_run_id,
            "errorCode": row.21,
            "errorMessage": row.22,
            "finishedAt": row.20,
            "result": raw_result,
            "status": run_status,
        });
        let terminal_json = serde_json::to_string(&terminal)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let terminal_hash = sha256_hex(terminal_json.as_bytes());
        let proof_tool_call_mappings = proof_bindings
            .iter()
            .map(|binding| {
                json!({
                    "runtimeToolCallId": binding.runtime_tool_call_id,
                    "toolCallId": binding.tool_call_id,
                })
            })
            .collect::<Vec<_>>();
        let proof_tool_call_mappings_json = serde_json::to_string(&proof_tool_call_mappings)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let proof = json!({
            "candidateHash": row.5,
            "requestId": request_id,
            "reviewContractHash": GRAPH_REVIEWER_CONTRACT_HASH,
            "reviewerConversationId": reviewer_conversation_id,
            "reviewerRunId": reviewer_run_id,
            "terminalHash": terminal_hash,
            "toolCallMappings": proof_tool_call_mappings,
            "toolCallMappingsHash": sha256_hex(proof_tool_call_mappings_json.as_bytes()),
        });
        let proof_json = serde_json::to_string(&proof)
            .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
        let decision_id = graph_review_decision_id(&request_id);
        transaction
            .execute(
                "INSERT INTO work_graph_node_review_decisions(
                    id, request_id, goal_id, task_id, attempt_id,
                    reviewer_delegation_id, reviewer_run_id, reviewer_conversation_id,
                    reviewer_agent_id, outcome, summary, decision_json, decision_hash,
                    reviewer_terminal_json, reviewer_terminal_hash,
                    proof_json, proof_hash, created_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                    ?13, ?14, ?15, ?16, ?17, ?18
                 )",
                params![
                    decision_id,
                    request_id,
                    row.1,
                    row.2,
                    row.3,
                    reviewer_delegation_id,
                    reviewer_run_id,
                    reviewer_conversation_id,
                    reviewer_agent_id,
                    graph_review_outcome_text(&outcome),
                    summary,
                    decision_json,
                    sha256_hex(decision_json.as_bytes()),
                    terminal_json,
                    terminal_hash,
                    proof_json,
                    sha256_hex(proof_json.as_bytes()),
                    now,
                ],
            )
            .map_err(database_error)?;
        for binding in &proof_bindings {
            transaction
                .execute(
                    "INSERT INTO work_graph_node_review_proofs(
                        decision_id, criterion_ordinal, criterion,
                        runtime_tool_call_id, tool_call_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        decision_id,
                        binding.criterion_ordinal,
                        binding.criterion,
                        binding.runtime_tool_call_id,
                        binding.tool_call_id,
                    ],
                )
                .map_err(database_error)?;
        }
        if outcome == GraphNodeReviewOutcome::Revise {
            finish_task_attempt_in_transaction(
                transaction,
                &FinishTaskAttemptInput {
                    id: row.3.clone(),
                    task_id: row.2.clone(),
                    run_id: row.4.clone(),
                    expected_task_version: row.6,
                    expected_attempt_version: row.7,
                    status: TaskAttemptStatus::Blocked,
                    failure_reason: Some(summary.clone()),
                },
                &now.to_string(),
            )?;
        }
        transaction
            .execute(
                "UPDATE work_graph_node_review_requests SET settled_at = ?2
                 WHERE id = ?1 AND settled_at IS NULL",
                params![request_id, now],
            )
            .map_err(database_error)?;
        Ok(GraphNodeReviewDecisionResult {
            request_id,
            decision_id,
            reviewer_child_run_id: reviewer_run_id,
            outcome,
            summary,
            replayed: false,
            snapshot: project_snapshot(transaction, spec)?,
        })
    })
}

pub(super) fn reconcile_graph_child_terminal_in_transaction(
    transaction: &Transaction<'_>,
    child_run_id: &str,
    now: i64,
) -> Result<bool, RepositoryError> {
    let candidate = transaction
        .query_row(
            "SELECT node.goal_id, task.id, attempt.id, attempt.run_id, delegation.id,
                    delegation.status, child.status, child.finished_at,
                    child.error_code, child.error_message, task.version, attempt.version,
                    task.status, task.owner_run_id, attempt.status, attempt.kind,
                    goal.status, parent_run.status,
                    (
                        SELECT intent.id FROM work_graph_node_cancel_intents AS intent
                        WHERE intent.attempt_id = attempt.id
                          AND intent.child_run_id = child.id
                          AND intent.activated_at IS NOT NULL
                        LIMIT 1
                    )
             FROM child_run_delegations AS delegation
             JOIN task_attempts AS attempt ON attempt.id = delegation.graph_task_attempt_id
             JOIN work_tasks AS task ON task.id = attempt.task_id
             JOIN work_graph_nodes AS node ON node.task_id = task.id
             JOIN goals AS goal ON goal.id = node.goal_id
             JOIN runs AS parent_run ON parent_run.id = attempt.run_id
             JOIN runs AS child ON child.id = delegation.child_run_id
             WHERE delegation.child_run_id = ?1",
            [child_run_id],
            |row| {
                Ok(GraphTerminalCandidate {
                    goal_id: row.get(0)?,
                    task_id: row.get(1)?,
                    attempt_id: row.get(2)?,
                    parent_run_id: row.get(3)?,
                    delegation_id: row.get(4)?,
                    delegation_status: row.get(5)?,
                    child_status: row.get(6)?,
                    child_finished_at: row.get(7)?,
                    child_error_code: row.get(8)?,
                    child_error_message: row.get(9)?,
                    task_version: row.get(10)?,
                    attempt_version: row.get(11)?,
                    task_status: row.get(12)?,
                    task_owner_run_id: row.get(13)?,
                    attempt_status: row.get(14)?,
                    attempt_kind: row.get(15)?,
                    goal_status: row.get(16)?,
                    parent_run_status: row.get(17)?,
                    active_cancel_intent_id: row.get(18)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(candidate) = candidate else {
        return Ok(false);
    };
    if !matches!(
        candidate.child_status.as_str(),
        "failed" | "cancelled" | "interrupted"
    ) {
        return Ok(false);
    }
    if candidate.child_finished_at.is_none()
        || candidate.delegation_status != candidate.child_status
        || candidate.task_status != "in_progress"
        || candidate.task_owner_run_id.as_deref() != Some(candidate.parent_run_id.as_str())
        || candidate.attempt_status != "running"
        || candidate.attempt_kind != "execution"
        || candidate.goal_status != "active"
        || candidate.parent_run_status != "running"
    {
        // Parent terminal transitions retain their existing authority chain. Completed and stale
        // Graph attempts are never rewritten by Child reconciliation.
        return Ok(false);
    }
    let terminal_json = serde_json::to_string(&json!({
        "childRunId": child_run_id,
        "errorCode": &candidate.child_error_code,
        "errorMessage": &candidate.child_error_message,
        "finishedAt": candidate.child_finished_at,
        "status": &candidate.child_status,
    }))
    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let terminal_hash = sha256_hex(terminal_json.as_bytes());
    let attempt_terminal_status = if candidate.child_status == "cancelled" {
        TaskAttemptStatus::Cancelled
    } else {
        TaskAttemptStatus::Failed
    };
    let failure_reason = graph_child_terminal_failure_reason(
        &candidate.child_status,
        candidate.child_error_code.as_deref(),
        candidate.child_error_message.as_deref(),
    );
    let reconciliation_id = graph_terminal_reconciliation_id(child_run_id);
    let existing = transaction
        .query_row(
            "SELECT child_terminal_status, attempt_terminal_status, child_terminal_json,
                    child_terminal_hash, failure_reason, cancel_intent_id
             FROM work_graph_node_terminal_reconciliations
             WHERE attempt_id = ?1 OR child_run_id = ?2",
            params![candidate.attempt_id, child_run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    if let Some(existing) = existing {
        let expected_attempt_status = match attempt_terminal_status {
            TaskAttemptStatus::Cancelled => "cancelled",
            TaskAttemptStatus::Failed => "failed",
            _ => unreachable!("Graph negative reconciliation is bounded"),
        };
        if existing.0 != candidate.child_status
            || existing.1 != expected_attempt_status
            || existing.2 != terminal_json
            || existing.3 != terminal_hash
            || existing.4 != failure_reason
            || existing.5 != candidate.active_cancel_intent_id
        {
            return Err(RepositoryError::ConstraintViolation(
                "Graph terminal reconciliation conflicts with its immutable terminal fact"
                    .to_owned(),
            ));
        }
        return Ok(false);
    }
    let attempt_terminal_status_text = match attempt_terminal_status {
        TaskAttemptStatus::Cancelled => "cancelled",
        TaskAttemptStatus::Failed => "failed",
        _ => unreachable!("Graph negative reconciliation is bounded"),
    };
    transaction
        .execute(
            "INSERT INTO work_graph_node_terminal_reconciliations(
                id, goal_id, task_id, attempt_id, parent_run_id, delegation_id,
                child_run_id, cancel_intent_id, child_terminal_status,
                attempt_terminal_status, expected_task_version, expected_attempt_version,
                child_terminal_json, child_terminal_hash, failure_reason, created_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16
             )",
            params![
                reconciliation_id,
                candidate.goal_id,
                candidate.task_id,
                candidate.attempt_id,
                candidate.parent_run_id,
                candidate.delegation_id,
                child_run_id,
                candidate.active_cancel_intent_id,
                candidate.child_status,
                attempt_terminal_status_text,
                candidate.task_version,
                candidate.attempt_version,
                terminal_json,
                terminal_hash,
                failure_reason,
                now,
            ],
        )
        .map_err(database_error)?;
    finish_task_attempt_in_transaction(
        transaction,
        &FinishTaskAttemptInput {
            id: candidate.attempt_id,
            task_id: candidate.task_id,
            run_id: candidate.parent_run_id,
            expected_task_version: candidate.task_version,
            expected_attempt_version: candidate.attempt_version,
            status: attempt_terminal_status,
            failure_reason: Some(failure_reason),
        },
        &now.to_string(),
    )?;
    Ok(true)
}

fn graph_child_terminal_failure_reason(
    status: &str,
    error_code: Option<&str>,
    error_message: Option<&str>,
) -> String {
    let prefix = match status {
        "cancelled" => "Graph Child Run was cancelled before node acceptance",
        "interrupted" => "Graph Child Run was interrupted before node acceptance",
        _ => "Graph Child Run failed before node acceptance",
    };
    let detail = match (error_code, error_message) {
        (Some(code), Some(message)) if !message.trim().is_empty() => {
            format!("{prefix}: [{code}] {}", message.trim())
        }
        (Some(code), _) => format!("{prefix}: [{code}]"),
        (_, Some(message)) if !message.trim().is_empty() => {
            format!("{prefix}: {}", message.trim())
        }
        _ => prefix.to_owned(),
    };
    detail.chars().take(4_000).collect()
}

fn validate_graph_node_start_input(
    input: &StartReadyReadOnlyGraphNodeInput,
) -> Result<(), RepositoryError> {
    validate_identifier("goal id", &input.goal_id)?;
    validate_identifier("task id", &input.task_id)?;
    validate_identifier("parent run id", &input.parent_run_id)?;
    validate_identifier("node-start tool call id", &input.node_start_tool_call_id)?;
    validate_identifier("attempt id", &input.attempt_id)?;
    validate_identifier("worker agent id", &input.worker_agent_id)?;
    bounded_non_empty(
        &input.objective,
        "Graph node objective",
        MAX_NODE_OBJECTIVE_CHARS,
    )?;
    bounded_non_empty(&input.context, "Graph node context", MAX_NODE_CONTEXT_CHARS)?;
    if input.expected_task_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Graph Task version must be at least 1".to_owned(),
        ));
    }
    validate_child_run_budget(&input.budget).map_err(RepositoryError::InvalidInput)?;
    if input.budget.max_duration_ms > GRAPH_CHILD_MAX_DURATION_MS
        || input.budget.max_total_tokens > GRAPH_CHILD_MAX_TOTAL_TOKENS
        || input.budget.max_output_tokens > GRAPH_CHILD_MAX_OUTPUT_TOKENS
        || input.budget.max_tool_calls > GRAPH_CHILD_MAX_TOOL_CALLS
    {
        return Err(RepositoryError::InvalidInput(format!(
            "read-only Graph Child budget exceeds the bounded MVP limits: duration<={GRAPH_CHILD_MAX_DURATION_MS}ms, totalTokens<={GRAPH_CHILD_MAX_TOTAL_TOKENS}, outputTokens<={GRAPH_CHILD_MAX_OUTPUT_TOKENS}, toolCalls<={GRAPH_CHILD_MAX_TOOL_CALLS}"
        )));
    }
    Ok(())
}

fn validate_graph_node_finish_input(
    input: &FinishReadOnlyGraphNodeInput,
) -> Result<(), RepositoryError> {
    validate_identifier("goal id", &input.goal_id)?;
    validate_identifier("task id", &input.task_id)?;
    validate_identifier("attempt id", &input.attempt_id)?;
    validate_identifier("parent run id", &input.parent_run_id)?;
    validate_identifier("node-finish tool call id", &input.node_finish_tool_call_id)?;
    if input.expected_task_version < 1 || input.expected_attempt_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Graph Task and Attempt versions must be at least 1".to_owned(),
        ));
    }
    bounded_non_empty(
        &input.summary,
        "Graph node completion summary",
        MAX_GRAPH_FINISH_SUMMARY_CHARS,
    )?;
    require_canonical_trimmed("Graph node completion summary", &input.summary)?;
    if input.criterion_evidence.is_empty()
        || input.criterion_evidence.len() > MAX_ACCEPTANCE_CRITERIA
    {
        return Err(RepositoryError::InvalidInput(format!(
            "criterionEvidence must contain 1-{MAX_ACCEPTANCE_CRITERIA} frozen criteria"
        )));
    }
    let mut criteria = HashSet::new();
    let mut total_bindings = 0usize;
    for binding in &input.criterion_evidence {
        bounded_non_empty(
            &binding.criterion,
            "Graph acceptance criterion",
            MAX_ACCEPTANCE_CRITERION_CHARS,
        )?;
        require_canonical_trimmed("Graph acceptance criterion", &binding.criterion)?;
        if !criteria.insert(binding.criterion.as_str()) {
            return Err(RepositoryError::InvalidInput(
                "criterionEvidence cannot repeat a criterion".to_owned(),
            ));
        }
        if binding.evidence_ids.is_empty()
            || binding.evidence_ids.len() > MAX_CRITERION_EVIDENCE_PER_CRITERION
        {
            return Err(RepositoryError::InvalidInput(format!(
                "every criterion requires 1-{MAX_CRITERION_EVIDENCE_PER_CRITERION} Evidence IDs"
            )));
        }
        total_bindings = total_bindings.saturating_add(binding.evidence_ids.len());
        let mut evidence_ids = HashSet::new();
        for evidence_id in &binding.evidence_ids {
            validate_identifier("criterion Evidence id", evidence_id)?;
            require_canonical_trimmed("criterion Evidence id", evidence_id)?;
            if !evidence_ids.insert(evidence_id.as_str()) {
                return Err(RepositoryError::InvalidInput(
                    "a criterion cannot repeat an Evidence id".to_owned(),
                ));
            }
        }
    }
    if total_bindings > MAX_GRAPH_FINISH_EVIDENCE_BINDINGS {
        return Err(RepositoryError::InvalidInput(format!(
            "criterionEvidence cannot contain more than {MAX_GRAPH_FINISH_EVIDENCE_BINDINGS} total bindings"
        )));
    }
    Ok(())
}

fn validate_graph_node_review_input(
    input: &CreateGraphNodeReviewRequestInput,
) -> Result<(), RepositoryError> {
    validate_graph_node_finish_input(&FinishReadOnlyGraphNodeInput {
        goal_id: input.goal_id.clone(),
        task_id: input.task_id.clone(),
        attempt_id: input.attempt_id.clone(),
        parent_run_id: input.parent_run_id.clone(),
        node_finish_tool_call_id: input.review_tool_call_id.clone(),
        expected_task_version: input.expected_task_version,
        expected_attempt_version: input.expected_attempt_version,
        criterion_evidence: input.criterion_evidence.clone(),
        summary: input.summary.clone(),
    })
}

fn validate_graph_acceptance_input(
    input: &CreateReadOnlyGraphAcceptanceInput,
) -> Result<(), RepositoryError> {
    validate_identifier("goal id", &input.goal_id)?;
    validate_identifier("submitter run id", &input.submitter_run_id)?;
    validate_identifier("Graph accept tool call id", &input.accept_tool_call_id)?;
    if input.expected_goal_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Goal version must be at least 1".to_owned(),
        ));
    }
    bounded_non_empty(
        &input.summary,
        "Graph acceptance summary",
        MAX_GRAPH_FINISH_SUMMARY_CHARS,
    )?;
    require_canonical_trimmed("Graph acceptance summary", &input.summary)
}

fn validate_graph_node_cancel_input(
    input: &CreateGraphNodeCancelIntentInput,
) -> Result<(), RepositoryError> {
    validate_identifier("goal id", &input.goal_id)?;
    validate_identifier("task id", &input.task_id)?;
    validate_identifier("attempt id", &input.attempt_id)?;
    validate_identifier("parent run id", &input.parent_run_id)?;
    validate_identifier("node-cancel tool call id", &input.node_cancel_tool_call_id)?;
    if input.expected_task_version < 1 || input.expected_attempt_version < 1 {
        return Err(RepositoryError::InvalidInput(
            "expected Graph Task and Attempt versions must be at least 1".to_owned(),
        ));
    }
    bounded_non_empty(
        &input.reason,
        "Graph node cancellation reason",
        MAX_GRAPH_CANCEL_REASON_CHARS,
    )?;
    require_canonical_trimmed("Graph node cancellation reason", &input.reason)
}

fn require_canonical_trimmed(label: &str, value: &str) -> Result<(), RepositoryError> {
    if value.trim() != value {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} must not contain leading or trailing whitespace"
        )));
    }
    Ok(())
}

fn node_finish_tool_input(input: &FinishReadOnlyGraphNodeInput) -> Value {
    json!({
        "goalId": input.goal_id,
        "taskId": input.task_id,
        "attemptId": input.attempt_id,
        "expectedTaskVersion": input.expected_task_version,
        "expectedAttemptVersion": input.expected_attempt_version,
        "criterionEvidence": input.criterion_evidence,
        "summary": input.summary,
    })
}

fn node_review_tool_input(input: &CreateGraphNodeReviewRequestInput) -> Value {
    json!({
        "goalId": input.goal_id,
        "taskId": input.task_id,
        "attemptId": input.attempt_id,
        "expectedTaskVersion": input.expected_task_version,
        "expectedAttemptVersion": input.expected_attempt_version,
        "criterionEvidence": input.criterion_evidence,
        "summary": input.summary,
    })
}

fn graph_accept_tool_input(input: &CreateReadOnlyGraphAcceptanceInput) -> Value {
    json!({
        "goalId": input.goal_id,
        "expectedGoalVersion": input.expected_goal_version,
        "summary": input.summary,
    })
}

fn node_cancel_tool_input(input: &CreateGraphNodeCancelIntentInput) -> Value {
    json!({
        "goalId": input.goal_id,
        "taskId": input.task_id,
        "attemptId": input.attempt_id,
        "expectedTaskVersion": input.expected_task_version,
        "expectedAttemptVersion": input.expected_attempt_version,
        "reason": input.reason,
    })
}

fn require_no_newer_authoritative_plan(
    transaction: &Transaction<'_>,
    goal_id: &str,
    plan_revision_id: &str,
    operation: &str,
) -> Result<(), RepositoryError> {
    let newer_authoritative_plan = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM plan_revisions newer
                JOIN plan_revisions activated ON activated.id = ?2
                WHERE newer.goal_id = ?1
                  AND newer.status IN ('proposed', 'approved')
                  AND newer.revision > activated.revision
             )",
            params![goal_id, plan_revision_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?;
    if newer_authoritative_plan {
        return Err(RepositoryError::ConstraintViolation(format!(
            "read-only Graph node {operation} is paused because a newer PlanRevision is proposed or approved"
        )));
    }
    Ok(())
}

fn require_standard_graph_policy(
    transaction: &Connection,
    task_id: &str,
    attempt: &TaskAttemptRecord,
) -> Result<(), RepositoryError> {
    let (policy_id, policy_hash) = transaction
        .query_row(
            "SELECT policy_id, policy_hash FROM task_validation_policies WHERE task_id = ?1",
            [task_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            RepositoryError::InvalidValidationPolicy(
                "Graph Task has no frozen ValidationPolicy".to_owned(),
            )
        })?;
    if policy_hash != attempt.policy_hash {
        return Err(RepositoryError::InvalidValidationPolicy(
            "Graph Attempt policy hash no longer matches the Task frozen policy".to_owned(),
        ));
    }
    if policy_id != "standard_v1" {
        return Err(RepositoryError::InvalidValidationPolicy(format!(
            "read-only Graph node cancel supports only standard_v1; found '{policy_id}'"
        )));
    }
    Ok(())
}

fn graph_attempt_policy(
    transaction: &Connection,
    task_id: &str,
    attempt: &TaskAttemptRecord,
) -> Result<(String, String), RepositoryError> {
    let policy = transaction
        .query_row(
            "SELECT policy_id, policy_hash FROM task_validation_policies WHERE task_id = ?1",
            [task_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            RepositoryError::InvalidValidationPolicy(
                "Graph Task has no frozen ValidationPolicy".to_owned(),
            )
        })?;
    if policy.1 != attempt.policy_hash {
        return Err(RepositoryError::InvalidValidationPolicy(
            "Graph Attempt policy hash no longer matches the Task frozen policy".to_owned(),
        ));
    }
    Ok(policy)
}

#[allow(clippy::too_many_arguments)]
fn require_live_graph_attempt(
    goal_id: &str,
    task_id: &str,
    attempt_id: &str,
    parent_run_id: &str,
    expected_task_version: i64,
    expected_attempt_version: i64,
    task: &WorkTaskRecord,
    attempt: &TaskAttemptRecord,
    target: &str,
) -> Result<(), RepositoryError> {
    if task.goal_id != goal_id
        || task.id != task_id
        || attempt.id != attempt_id
        || attempt.task_id != task_id
        || attempt.run_id != parent_run_id
    {
        return Err(RepositoryError::CrossConversationReference);
    }
    if task.version != expected_task_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: task_id.to_owned(),
            expected_version: expected_task_version,
        });
    }
    if attempt.version != expected_attempt_version {
        return Err(RepositoryError::OptimisticLockFailed {
            id: attempt_id.to_owned(),
            expected_version: expected_attempt_version,
        });
    }
    if task.status != WorkTaskStatus::InProgress
        || task.owner_run_id.as_deref() != Some(parent_run_id)
        || attempt.kind != TaskAttemptKind::Execution
        || attempt.status != TaskAttemptStatus::Running
    {
        return Err(RepositoryError::InvalidTransition {
            entity: "read-only Graph node Attempt",
            from: format!("{:?}", attempt.status).to_ascii_lowercase(),
            to: target.to_owned(),
        });
    }
    Ok(())
}

fn require_exact_criterion_bindings(
    node: &StoredNode,
    supplied: &[GraphCriterionEvidenceInput],
) -> Result<(), RepositoryError> {
    if node.acceptance_criteria.len() != supplied.len()
        || node
            .acceptance_criteria
            .iter()
            .zip(supplied)
            .any(|(frozen, binding)| frozen != &binding.criterion)
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph review must bind every frozen criterion exactly once in snapshot order"
                .to_owned(),
        ));
    }
    Ok(())
}

fn load_graph_attempt_delegations(
    transaction: &Connection,
    attempt_id: &str,
) -> Result<Vec<StoredGraphDelegation>, RepositoryError> {
    transaction
        .prepare(
            "SELECT delegation.id, delegation.parent_run_id, delegation.child_run_id,
                    delegation.child_conversation_id, delegation.status,
                    child.status, child.finished_at
             FROM child_run_delegations AS delegation
             JOIN runs AS child ON child.id = delegation.child_run_id
             WHERE delegation.graph_task_attempt_id = ?1
             ORDER BY delegation.created_at, delegation.id",
        )
        .map_err(database_error)?
        .query_map([attempt_id], |row| {
            Ok(StoredGraphDelegation {
                id: row.get(0)?,
                parent_run_id: row.get(1)?,
                child_run_id: row.get(2)?,
                child_conversation_id: row.get(3)?,
                status: row.get(4)?,
                child_status: row.get(5)?,
                child_finished_at: row.get(6)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)
}

fn require_node_cancel_tool_call(
    transaction: &Connection,
    input: &CreateGraphNodeCancelIntentInput,
    conversation_id: &str,
    input_json: &str,
    allow_terminal_replay: bool,
) -> Result<(), RepositoryError> {
    let source = transaction
        .query_row(
            "SELECT run_id, conversation_id, execution_location, tool_name, status,
                    input_json, requires_approval, error_message
             FROM tool_calls WHERE id = ?1",
            [input.node_cancel_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "Graph node-cancel tool call",
            id: input.node_cancel_tool_call_id.clone(),
        })?;
    if source.0 != input.parent_run_id || source.1 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    let allowed_status = source.4 == "running"
        || (allow_terminal_replay && matches!(source.4.as_str(), "completed" | "failed"));
    if source.2 != "host"
        || source.3 != GRAPH_NODE_CANCEL_TOOL
        || !allowed_status
        || source.6
        || source.7.is_some()
        || source.5 != input_json
    {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node-cancel ToolCall '{}' must be an exact unapproved Host '{}' call",
            input.node_cancel_tool_call_id, GRAPH_NODE_CANCEL_TOOL
        )));
    }
    Ok(())
}

fn replay_graph_node_cancel_intent(
    transaction: &Transaction<'_>,
    input: &CreateGraphNodeCancelIntentInput,
    spec: &StoredSpec,
    input_json: &str,
    input_hash: &str,
) -> Result<Option<GraphNodeCancelIntentResult>, RepositoryError> {
    let existing = transaction
        .query_row(
            "SELECT id, goal_id, task_id, attempt_id, parent_run_id, child_run_id,
                    tool_call_id, expected_task_version, expected_attempt_version,
                    reason, input_json, input_hash, activated_at
             FROM work_graph_node_cancel_intents WHERE tool_call_id = ?1",
            [input.node_cancel_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.1 != input.goal_id
        || existing.2 != input.task_id
        || existing.3 != input.attempt_id
        || existing.4 != input.parent_run_id
        || existing.6 != input.node_cancel_tool_call_id
        || existing.7 != input.expected_task_version
        || existing.8 != input.expected_attempt_version
        || existing.9 != input.reason
        || existing.10 != input_json
        || existing.11 != input_hash
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph node-cancel replay conflicts with the immutable cancel intent".to_owned(),
        ));
    }
    let conversation_id = transaction
        .query_row(
            "SELECT conversation_id FROM goals WHERE id = ?1",
            [input.goal_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    require_node_cancel_tool_call(transaction, input, &conversation_id, input_json, true)?;
    let task =
        load_task(transaction, &input.task_id)?.ok_or_else(|| RepositoryError::NotFound {
            entity: "graph task",
            id: input.task_id.clone(),
        })?;
    let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
        RepositoryError::NotFound {
            entity: "graph task attempt",
            id: input.attempt_id.clone(),
        }
    })?;
    let tool_status = transaction
        .query_row(
            "SELECT status FROM tool_calls WHERE id = ?1",
            [input.node_cancel_tool_call_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    Ok(Some(GraphNodeCancelIntentResult {
        created: false,
        replayed: true,
        pending_activation: existing.12.is_none() && tool_status == "running",
        intent_id: existing.0,
        child_run_id: existing.5,
        task,
        attempt,
        snapshot: project_snapshot(transaction, spec.clone())?,
    }))
}

fn replay_graph_node_review_request(
    transaction: &Transaction<'_>,
    input: &CreateGraphNodeReviewRequestInput,
    spec: &StoredSpec,
    input_json: &str,
    input_hash: &str,
) -> Result<Option<GraphNodeReviewRequestResult>, RepositoryError> {
    let existing = transaction
        .query_row(
            "SELECT id, goal_id, task_id, attempt_id, parent_run_id,
                    implementation_child_run_id, expected_task_version,
                    expected_attempt_version, summary, input_json, input_hash,
                    candidate_hash, activated_at
             FROM work_graph_node_review_requests WHERE tool_call_id = ?1",
            [input.review_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, Option<i64>>(12)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.1 != input.goal_id
        || existing.2 != input.task_id
        || existing.3 != input.attempt_id
        || existing.4 != input.parent_run_id
        || existing.6 != input.expected_task_version
        || existing.7 != input.expected_attempt_version
        || existing.8 != input.summary
        || existing.9 != input_json
        || existing.10 != input_hash
    {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node review request ToolCall '{}' conflicts with its immutable replay",
            input.review_tool_call_id
        )));
    }
    Ok(Some(GraphNodeReviewRequestResult {
        created: false,
        replayed: true,
        pending_activation: existing.12.is_none(),
        request_id: existing.0,
        implementation_child_run_id: existing.5,
        candidate_hash: existing.11,
        snapshot: project_snapshot(transaction, spec.clone())?,
    }))
}

fn graph_review_request_id(tool_call_id: &str) -> String {
    format!(
        "graph-review-{}",
        &sha256_hex(tool_call_id.as_bytes())[..32]
    )
}

fn graph_review_decision_id(request_id: &str) -> String {
    format!(
        "graph-review-decision-{}",
        &sha256_hex(request_id.as_bytes())[..32]
    )
}

fn graph_acceptance_id(tool_call_id: &str) -> String {
    format!(
        "graph-acceptance-{}",
        &sha256_hex(tool_call_id.as_bytes())[..32]
    )
}

fn graph_review_outcome_text(outcome: &GraphNodeReviewOutcome) -> &'static str {
    match outcome {
        GraphNodeReviewOutcome::Pass => "pass",
        GraphNodeReviewOutcome::Revise => "revise",
        GraphNodeReviewOutcome::Inconclusive => "inconclusive",
    }
}

fn parse_graph_review_outcome(value: &str) -> Result<GraphNodeReviewOutcome, RepositoryError> {
    match value {
        "pass" => Ok(GraphNodeReviewOutcome::Pass),
        "revise" => Ok(GraphNodeReviewOutcome::Revise),
        "inconclusive" => Ok(GraphNodeReviewOutcome::Inconclusive),
        other => Err(RepositoryError::ConstraintViolation(format!(
            "unknown Graph Reviewer outcome '{other}'"
        ))),
    }
}

fn load_graph_review_decision(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<(String, String, String, String)>, RepositoryError> {
    connection
        .query_row(
            "SELECT id, reviewer_run_id, outcome, summary
             FROM work_graph_node_review_decisions WHERE request_id = ?1",
            [request_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(database_error)
}

fn authoritative_graph_review_decision(
    run_status: &str,
    raw_result: Option<&str>,
    terminal_error: Option<&str>,
) -> (GraphNodeReviewOutcome, String, String) {
    if run_status != "completed" {
        let summary = terminal_error
            .filter(|value| !value.trim().is_empty())
            .map(str::trim)
            .unwrap_or("independent Graph Reviewer did not complete")
            .chars()
            .take(MAX_GRAPH_FINISH_SUMMARY_CHARS)
            .collect::<String>();
        let decision = json!({
            "criteria": [],
            "findings": [],
            "recommendation": "inconclusive",
            "summary": summary,
        });
        return (
            GraphNodeReviewOutcome::Inconclusive,
            summary,
            serde_json::to_string(&decision).expect("static Reviewer decision is JSON"),
        );
    }
    let parsed = raw_result
        .and_then(|value| serde_json::from_str::<Value>(value).ok())
        .filter(|value| value.is_object());
    let Some(parsed) = parsed else {
        let summary = "independent Graph Reviewer returned invalid structured output".to_owned();
        let decision = json!({
            "criteria": [],
            "findings": [],
            "recommendation": "inconclusive",
            "summary": summary,
        });
        return (
            GraphNodeReviewOutcome::Inconclusive,
            summary,
            serde_json::to_string(&decision).expect("static Reviewer decision is JSON"),
        );
    };
    let object = parsed.as_object().expect("filtered object");
    let exact_keys = object.len() == 4
        && object.contains_key("criteria")
        && object.contains_key("findings")
        && object.contains_key("recommendation")
        && object.contains_key("summary");
    let summary = object
        .get("summary")
        .and_then(Value::as_str)
        .filter(|value| value.trim() == *value && !value.is_empty())
        .filter(|value| value.chars().count() <= MAX_GRAPH_FINISH_SUMMARY_CHARS);
    let recommendation = object.get("recommendation").and_then(Value::as_str);
    let criteria = object.get("criteria").and_then(Value::as_array);
    let findings = object.get("findings").and_then(Value::as_array);
    if !exact_keys || summary.is_none() || criteria.is_none() || findings.is_none() {
        let summary = "independent Graph Reviewer returned invalid structured output".to_owned();
        let decision = json!({
            "criteria": [],
            "findings": [],
            "recommendation": "inconclusive",
            "summary": summary,
        });
        return (
            GraphNodeReviewOutcome::Inconclusive,
            summary,
            serde_json::to_string(&decision).expect("static Reviewer decision is JSON"),
        );
    }
    let outcome = match recommendation {
        Some("pass") => GraphNodeReviewOutcome::Pass,
        Some("revise") => GraphNodeReviewOutcome::Revise,
        _ => GraphNodeReviewOutcome::Inconclusive,
    };
    let summary = summary.expect("validated summary").to_owned();
    if outcome == GraphNodeReviewOutcome::Inconclusive {
        let decision = json!({
            "criteria": [],
            "findings": [],
            "recommendation": "inconclusive",
            "summary": summary,
        });
        return (
            GraphNodeReviewOutcome::Inconclusive,
            summary,
            serde_json::to_string(&decision).expect("canonical inconclusive is JSON"),
        );
    }
    let canonical = serde_json::to_string(&parsed).expect("parsed Reviewer output is JSON");
    (outcome, summary, canonical)
}

#[allow(clippy::too_many_arguments)]
fn validate_graph_reviewer_decision_proof(
    connection: &Connection,
    task_id: &str,
    reviewer_run_id: &str,
    reviewer_conversation_id: &str,
    activated_at: i64,
    reviewer_finished_at: i64,
    outcome: &GraphNodeReviewOutcome,
    decision_json: &str,
) -> Result<Vec<GraphReviewerProofBinding>, RepositoryError> {
    let frozen_criteria_json = connection
        .query_row(
            "SELECT acceptance_criteria_json FROM work_graph_nodes WHERE task_id = ?1",
            [task_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    let frozen_criteria = serde_json::from_str::<Vec<String>>(&frozen_criteria_json)
        .map_err(|error| RepositoryError::ConstraintViolation(error.to_string()))?;
    let decision = serde_json::from_str::<Value>(decision_json)
        .map_err(|error| RepositoryError::ConstraintViolation(error.to_string()))?;
    let object = decision.as_object().ok_or_else(|| {
        RepositoryError::ConstraintViolation("Reviewer decision must be an object".to_owned())
    })?;
    if object.len() != 4
        || !object.contains_key("criteria")
        || !object.contains_key("findings")
        || !object.contains_key("recommendation")
        || !object.contains_key("summary")
    {
        return Err(RepositoryError::ConstraintViolation(
            "Reviewer decision must contain only criteria, findings, recommendation, and summary"
                .to_owned(),
        ));
    }
    let recommendation = object
        .get("recommendation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if recommendation != graph_review_outcome_text(outcome) {
        return Err(RepositoryError::ConstraintViolation(
            "Reviewer recommendation does not match the authoritative outcome".to_owned(),
        ));
    }
    let criteria = object
        .get("criteria")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "Reviewer decision criteria must be an array".to_owned(),
            )
        })?;
    if criteria.len() != frozen_criteria.len() || criteria.is_empty() || criteria.len() > 8 {
        return Err(RepositoryError::ConstraintViolation(
            "Reviewer decision must cover every frozen criterion exactly once".to_owned(),
        ));
    }
    let mut proof_bindings = Vec::new();
    let mut tool_call_ids = HashSet::new();
    let mut failed_seen = false;
    for (ordinal, (criterion_value, frozen)) in
        criteria.iter().zip(frozen_criteria.iter()).enumerate()
    {
        let criterion = criterion_value.as_object().ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "Reviewer criterion decision must be an object".to_owned(),
            )
        })?;
        if criterion.len() != 3
            || !criterion.contains_key("criterion")
            || !criterion.contains_key("status")
            || !criterion.contains_key("toolCallIds")
            || criterion.get("criterion").and_then(Value::as_str) != Some(frozen.as_str())
        {
            return Err(RepositoryError::ConstraintViolation(
                "Reviewer criterion decision does not exactly match the frozen criterion"
                    .to_owned(),
            ));
        }
        let status = criterion
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !matches!(status, "passed" | "failed" | "inconclusive") {
            return Err(RepositoryError::ConstraintViolation(
                "Reviewer criterion status is invalid".to_owned(),
            ));
        }
        if *outcome == GraphNodeReviewOutcome::Pass && status != "passed" {
            return Err(RepositoryError::ConstraintViolation(
                "pass requires every frozen criterion to be passed".to_owned(),
            ));
        }
        failed_seen |= status == "failed";
        let ids = criterion
            .get("toolCallIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RepositoryError::ConstraintViolation(
                    "Reviewer criterion toolCallIds must be an array".to_owned(),
                )
            })?;
        if ids.is_empty() || ids.len() > 16 {
            return Err(RepositoryError::ConstraintViolation(
                "each Reviewer criterion requires between 1 and 16 proof ToolCalls".to_owned(),
            ));
        }
        for tool_call_id in ids {
            let tool_call_id = tool_call_id.as_str().ok_or_else(|| {
                RepositoryError::ConstraintViolation(
                    "Reviewer proof ToolCall ids must be strings".to_owned(),
                )
            })?;
            validate_identifier("Reviewer proof ToolCall id", tool_call_id)?;
            if !tool_call_ids.insert(tool_call_id.to_owned()) {
                return Err(RepositoryError::ConstraintViolation(
                    "Reviewer proof ToolCall ids must be globally unique".to_owned(),
                ));
            }
            if tool_call_ids.len() > MAX_GRAPH_FINISH_EVIDENCE_BINDINGS {
                return Err(RepositoryError::ConstraintViolation(
                    "Reviewer proof cannot exceed 100 ToolCalls".to_owned(),
                ));
            }
            let internal_tool_call_id = connection
                .query_row(
                    "SELECT id FROM tool_calls
                     WHERE runtime_tool_call_id = ?1 AND run_id = ?2 AND conversation_id = ?3
                       AND execution_location = 'runtime'
                       AND tool_name IN ('read', 'ls', 'find', 'grep')
                       AND requires_approval = 0
                       AND status = 'completed'
                       AND COALESCE(error_message, '') = ''
                       AND started_at > ?4
                       AND completed_at IS NOT NULL
                       AND completed_at >= started_at
                       AND completed_at <= ?5
                       AND updated_at >= completed_at",
                    params![
                        tool_call_id,
                        reviewer_run_id,
                        reviewer_conversation_id,
                        activated_at,
                        reviewer_finished_at,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?;
            let internal_tool_call_id = internal_tool_call_id.ok_or_else(|| {
                RepositoryError::InvalidReference(format!(
                    "Reviewer proof ToolCall '{tool_call_id}' is stale, foreign, failed, or outside the exact readonly contract"
                ))
            })?;
            proof_bindings.push(GraphReviewerProofBinding {
                criterion_ordinal: ordinal as i64,
                criterion: frozen.clone(),
                tool_call_id: internal_tool_call_id,
                runtime_tool_call_id: tool_call_id.to_owned(),
            });
        }
    }
    if *outcome == GraphNodeReviewOutcome::Revise && !failed_seen {
        return Err(RepositoryError::ConstraintViolation(
            "revise requires at least one failed frozen criterion".to_owned(),
        ));
    }
    let findings = object
        .get("findings")
        .and_then(Value::as_array)
        .expect("exact Reviewer object has findings array");
    if (*outcome == GraphNodeReviewOutcome::Pass && !findings.is_empty())
        || (*outcome == GraphNodeReviewOutcome::Revise
            && (findings.is_empty() || findings.len() > 16))
    {
        return Err(RepositoryError::ConstraintViolation(
            "pass requires no findings and revise requires between 1 and 16 findings".to_owned(),
        ));
    }
    let criterion_proofs = criteria
        .iter()
        .filter_map(Value::as_object)
        .map(|criterion| {
            let name = criterion
                .get("criterion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let status = criterion
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let ids = criterion
                .get("toolCallIds")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<HashSet<_>>();
            (name, (status, ids))
        })
        .collect::<HashMap<_, _>>();
    for finding in findings {
        let finding = finding.as_object().ok_or_else(|| {
            RepositoryError::ConstraintViolation("Reviewer finding must be an object".to_owned())
        })?;
        if finding.len() != 5
            || !finding.contains_key("criterion")
            || !finding.contains_key("detail")
            || !finding.contains_key("severity")
            || !finding.contains_key("title")
            || !finding.contains_key("toolCallIds")
        {
            return Err(RepositoryError::ConstraintViolation(
                "Reviewer finding must contain only criterion, severity, title, detail, and toolCallIds"
                    .to_owned(),
            ));
        }
        let finding_criterion = finding
            .get("criterion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let title = finding
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let detail = finding
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let severity = finding
            .get("severity")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let finding_tool_ids = finding
            .get("toolCallIds")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                RepositoryError::ConstraintViolation(
                    "Reviewer finding toolCallIds must be an array".to_owned(),
                )
            })?;
        let corresponding = criterion_proofs.get(finding_criterion);
        let mut unique_finding_ids = HashSet::new();
        let bounded_subset = !finding_tool_ids.is_empty()
            && finding_tool_ids.len() <= 16
            && finding_tool_ids.iter().all(|id| {
                id.as_str().is_some_and(|id| {
                    unique_finding_ids.insert(id)
                        && corresponding.is_some_and(|(_, proof_ids)| proof_ids.contains(id))
                })
            });
        if corresponding.is_none_or(|(status, _)| status != "failed")
            || title.trim() != title
            || title.is_empty()
            || title.chars().count() > 200
            || detail.trim() != detail
            || detail.is_empty()
            || detail.chars().count() > 2_000
            || !matches!(severity, "critical" | "high" | "medium" | "low" | "info")
            || !bounded_subset
        {
            return Err(RepositoryError::ConstraintViolation(
                "Reviewer finding is not bounded or does not reference a frozen criterion"
                    .to_owned(),
            ));
        }
    }
    Ok(proof_bindings)
}

fn replay_graph_acceptance_intent(
    transaction: &Transaction<'_>,
    input: &CreateReadOnlyGraphAcceptanceInput,
    input_json: &str,
    input_hash: &str,
) -> Result<Option<GraphAcceptanceIntentResult>, RepositoryError> {
    let existing = transaction
        .query_row(
            "SELECT id, goal_id, submitter_run_id, expected_goal_version, summary,
                    input_json, input_hash, accepted_at
             FROM work_graph_acceptances WHERE tool_call_id = ?1",
            [input.accept_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.1 != input.goal_id
        || existing.2 != input.submitter_run_id
        || existing.3 != input.expected_goal_version
        || existing.4 != input.summary
        || existing.5 != input_json
        || existing.6 != input_hash
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph acceptance ToolCall conflicts with its immutable replay".to_owned(),
        ));
    }
    Ok(Some(GraphAcceptanceIntentResult {
        acceptance_id: existing.0,
        created: false,
        replayed: true,
        pending_activation: existing.7.is_none(),
    }))
}

fn load_graph_acceptance_row(
    connection: &Connection,
    acceptance_id: &str,
) -> Result<Option<StoredGraphAcceptance>, RepositoryError> {
    connection
        .query_row(
            "SELECT goal_id, plan_revision_id, submitter_run_id, tool_call_id,
                    expected_goal_version, summary, node_projection_json,
                    node_projection_hash, checks_json, checks_hash, input_json, accepted_at
             FROM work_graph_acceptances WHERE id = ?1",
            [acceptance_id],
            |row| {
                Ok(StoredGraphAcceptance {
                    goal_id: row.get(0)?,
                    plan_revision_id: row.get(1)?,
                    submitter_run_id: row.get(2)?,
                    tool_call_id: row.get(3)?,
                    expected_goal_version: row.get(4)?,
                    summary: row.get(5)?,
                    node_projection_json: row.get(6)?,
                    node_projection_hash: row.get(7)?,
                    checks_json: row.get(8)?,
                    checks_hash: row.get(9)?,
                    input_json: row.get(10)?,
                    accepted_at: row.get(11)?,
                })
            },
        )
        .optional()
        .map_err(database_error)
}

fn graph_acceptance_projection(
    transaction: &Connection,
    spec: &StoredSpec,
) -> Result<(Value, Value), RepositoryError> {
    let open_findings = transaction
        .query_row(
            "SELECT COUNT(*) FROM review_findings
             WHERE goal_id = ?1 AND status = 'open'",
            [spec.goal_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if open_findings != 0 {
        return Err(RepositoryError::ReviewBlocked {
            task_id: format!("goal:{}", spec.goal_id),
        });
    }
    let active_reviews = transaction
        .query_row(
            "SELECT COUNT(*) FROM work_graph_node_review_requests
             WHERE goal_id = ?1 AND activated_at IS NOT NULL AND settled_at IS NULL",
            [spec.goal_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if active_reviews != 0 {
        return Err(RepositoryError::ConstraintViolation(
            "Graph acceptance is blocked while an independent node review is active".to_owned(),
        ));
    }

    let rows = transaction
        .prepare(
            "SELECT node.task_id, task.status, task.version, node.acceptance_criteria_json,
                    attempt.id, attempt.run_id, attempt.status,
                    attempt.evidence_rowid_watermark, attempt.started_at,
                    finish.id, finish.input_json, finish.child_result_hash,
                    finish.validation_policy_id, finish.validation_policy_hash,
                    finish.expected_task_version, finish.expected_attempt_version,
                    finish.child_run_id
             FROM work_graph_nodes AS node
             JOIN work_tasks AS task ON task.id = node.task_id
             JOIN work_graph_node_finishes AS finish
               ON finish.goal_id = node.goal_id AND finish.task_id = node.task_id
             JOIN task_attempts AS attempt ON attempt.id = finish.attempt_id
             WHERE node.goal_id = ?1
             ORDER BY task.ordinal, task.id",
        )
        .map_err(database_error)?
        .query_map([spec.goal_id.as_str()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, i64>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, String>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, i64>(15)?,
                row.get::<_, String>(16)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let node_count = transaction
        .query_row(
            "SELECT COUNT(*) FROM work_graph_nodes WHERE goal_id = ?1",
            [spec.goal_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if node_count == 0 || rows.len() as i64 != node_count {
        return Err(RepositoryError::ConstraintViolation(
            "Graph acceptance requires an immutable finish for every node".to_owned(),
        ));
    }
    let conversation_id = transaction
        .query_row(
            "SELECT conversation_id FROM goals WHERE id = ?1",
            [spec.goal_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    let mut projection = Vec::with_capacity(rows.len());
    for row in rows {
        if row.1 != "completed" || row.6 != "succeeded" {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph node '{}' is not accepted",
                row.0
            )));
        }
        let criteria = serde_json::from_str::<Vec<String>>(&row.3).map_err(|error| {
            RepositoryError::ConstraintViolation(format!(
                "Graph node '{}' has invalid frozen criteria: {error}",
                row.0
            ))
        })?;
        let bindings = transaction
            .prepare(
                "SELECT criterion_ordinal, criterion, evidence_id
                 FROM work_graph_node_criterion_evidence
                 WHERE finish_id = ?1
                 ORDER BY criterion_ordinal, evidence_id",
            )
            .map_err(database_error)?
            .query_map([row.9.as_str()], |binding| {
                Ok((
                    binding.get::<_, i64>(0)?,
                    binding.get::<_, String>(1)?,
                    binding.get::<_, String>(2)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        let mut seen = BTreeSet::new();
        for (ordinal, criterion, evidence_id) in &bindings {
            let expected = criteria.get(*ordinal as usize).ok_or_else(|| {
                RepositoryError::ConstraintViolation(format!(
                    "Graph node '{}' finish has an out-of-range criterion binding",
                    row.0
                ))
            })?;
            if expected != criterion {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node '{}' finish criterion no longer matches its frozen spec",
                    row.0
                )));
            }
            seen.insert(*ordinal);
            validate_graph_criterion_evidence(
                transaction,
                &row.0,
                &row.5,
                &conversation_id,
                row.7,
                row.8,
                evidence_id,
            )?;
        }
        if bindings.is_empty() || seen.len() != criteria.len() {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph node '{}' finish does not cover every frozen criterion",
                row.0
            )));
        }
        let (_, live_child_hash) =
            authoritative_completed_graph_child(transaction, &row.4, &row.5)?;
        if live_child_hash != row.11 {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph node '{}' Child terminal result changed after finish",
                row.0
            )));
        }
        let decision_id = if row.12 == "high_risk_v1" {
            require_graph_review_pass(transaction, &row.4, &row.10, &row.11, &row.13)?;
            transaction
                .query_row(
                    "SELECT decision.id
                     FROM work_graph_node_review_requests AS request
                     JOIN work_graph_node_review_decisions AS decision
                       ON decision.request_id = request.id AND decision.outcome = 'pass'
                     WHERE request.attempt_id = ?1 AND request.input_json = ?2
                       AND request.child_result_hash = ?3
                       AND request.validation_policy_hash = ?4
                       AND request.settled_at IS NOT NULL
                     ORDER BY decision.created_at DESC LIMIT 1",
                    params![row.4, row.10, row.11, row.13],
                    |record| record.get::<_, String>(0),
                )
                .optional()
                .map_err(database_error)?
        } else if row.12 == "standard_v1" {
            None
        } else {
            return Err(RepositoryError::InvalidValidationPolicy(format!(
                "Graph node '{}' uses unsupported policy '{}'",
                row.0, row.12
            )));
        };
        projection.push(json!({
            "attemptId": row.4,
            "childResultHash": row.11,
            "childRunId": row.16,
            "decisionId": decision_id,
            "finishId": row.9,
            "policyHash": row.13,
            "policyId": row.12,
            "taskId": row.0,
            "taskVersion": row.2,
        }));
    }
    let active_children = transaction
        .query_row(
            "SELECT COUNT(*)
             FROM child_run_delegations AS delegation
             LEFT JOIN task_attempts AS attempt
               ON attempt.id = delegation.graph_task_attempt_id
             LEFT JOIN work_tasks AS task ON task.id = attempt.task_id
             LEFT JOIN work_graph_node_review_requests AS review
               ON review.parent_run_id = delegation.parent_run_id
              AND review.tool_call_id = delegation.tool_call_id
             WHERE (task.goal_id = ?1 OR review.goal_id = ?1)
               AND delegation.status IN ('queued', 'running')",
            [spec.goal_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if active_children != 0 {
        return Err(RepositoryError::ConstraintViolation(
            "Graph acceptance is blocked while a Graph Child is nonterminal".to_owned(),
        ));
    }
    let checks = json!({
        "activeReviewCount": 0,
        "currentPlanRevisionId": spec.plan_revision_id,
        "nodeCount": node_count,
        "openFindingCount": 0,
        "specHash": spec.spec_hash,
    });
    Ok((Value::Array(projection), checks))
}

fn load_stored_node(
    transaction: &Transaction<'_>,
    goal_id: &str,
    task_id: &str,
) -> Result<StoredNode, RepositoryError> {
    transaction
        .query_row(
            "SELECT nodes.task_id, nodes.node_key, nodes.depth,
                    nodes.acceptance_criteria_json, tasks.title, tasks.detail,
                    tasks.ordinal, tasks.status, tasks.version, tasks.attempt
             FROM work_graph_nodes nodes
             JOIN work_tasks tasks ON tasks.id = nodes.task_id
             WHERE nodes.goal_id = ?1 AND nodes.task_id = ?2
               AND nodes.access_mode = 'read_only'",
            params![goal_id, task_id],
            |row| {
                let criteria_json = row.get::<_, String>(3)?;
                let acceptance_criteria = serde_json::from_str::<Vec<String>>(&criteria_json)
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?;
                Ok(StoredNode {
                    task_id: row.get(0)?,
                    node_key: row.get(1)?,
                    depth: row.get(2)?,
                    acceptance_criteria,
                    title: row.get(4)?,
                    detail: row.get(5)?,
                    ordinal: row.get(6)?,
                    task_status: row.get(7)?,
                    task_version: row.get(8)?,
                    task_attempt: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            RepositoryError::InvalidReference(format!(
                "Task '{task_id}' is not a node in read-only Graph '{goal_id}'"
            ))
        })
}

fn require_node_finish_tool_call(
    transaction: &Transaction<'_>,
    input: &FinishReadOnlyGraphNodeInput,
    conversation_id: &str,
    input_json: &str,
    allow_terminal_replay: bool,
) -> Result<(), RepositoryError> {
    let source = transaction
        .query_row(
            "SELECT run_id, conversation_id, execution_location, tool_name, status,
                    input_json, requires_approval
             FROM tool_calls WHERE id = ?1",
            [input.node_finish_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "Graph node-finish tool call",
            id: input.node_finish_tool_call_id.clone(),
        })?;
    if source.0 != input.parent_run_id || source.1 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    let allowed_status =
        source.4 == "running" || (allow_terminal_replay && source.4 == "completed");
    if source.2 != "host" || source.3 != GRAPH_NODE_FINISH_TOOL || !allowed_status {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node-finish ToolCall '{}' must be an exact running Host '{}' call",
            input.node_finish_tool_call_id, GRAPH_NODE_FINISH_TOOL
        )));
    }
    if source.6 {
        return Err(RepositoryError::ConstraintViolation(
            "read-only Graph node-finish ToolCall cannot require an approval grant".to_owned(),
        ));
    }
    if source.5 != input_json {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node-finish ToolCall '{}' does not exactly bind the Repository input",
            input.node_finish_tool_call_id
        )));
    }
    Ok(())
}

fn require_node_review_tool_call(
    transaction: &Connection,
    input: &CreateGraphNodeReviewRequestInput,
    conversation_id: &str,
    input_json: &str,
    allow_terminal_replay: bool,
) -> Result<(), RepositoryError> {
    require_exact_graph_host_tool_call(
        transaction,
        &input.review_tool_call_id,
        &input.parent_run_id,
        conversation_id,
        GRAPH_NODE_REVIEW_TOOL,
        input_json,
        allow_terminal_replay,
    )
}

fn require_graph_accept_tool_call(
    transaction: &Connection,
    input: &CreateReadOnlyGraphAcceptanceInput,
    conversation_id: &str,
    input_json: &str,
    allow_terminal_replay: bool,
) -> Result<(), RepositoryError> {
    require_exact_graph_host_tool_call(
        transaction,
        &input.accept_tool_call_id,
        &input.submitter_run_id,
        conversation_id,
        GRAPH_ACCEPT_TOOL,
        input_json,
        allow_terminal_replay,
    )
}

fn require_graph_accept_tool_call_stored(
    transaction: &Connection,
    tool_call_id: &str,
    submitter_run_id: &str,
    conversation_id: &str,
    input_json: &str,
) -> Result<(), RepositoryError> {
    require_exact_graph_host_tool_call(
        transaction,
        tool_call_id,
        submitter_run_id,
        conversation_id,
        GRAPH_ACCEPT_TOOL,
        input_json,
        true,
    )
}

fn require_exact_graph_host_tool_call(
    transaction: &Connection,
    tool_call_id: &str,
    run_id: &str,
    conversation_id: &str,
    tool_name: &str,
    input_json: &str,
    allow_terminal_replay: bool,
) -> Result<(), RepositoryError> {
    let source = transaction
        .query_row(
            "SELECT run_id, conversation_id, execution_location, tool_name, status,
                    input_json, requires_approval, error_message, result_json,
                    completed_at
             FROM tool_calls WHERE id = ?1",
            [tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, Option<i64>>(9)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "Graph Host tool call",
            id: tool_call_id.to_owned(),
        })?;
    if source.0 != run_id || source.1 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    let allowed_status = source.4 == "running"
        || (allow_terminal_replay
            && source.4 == "completed"
            && source.7.is_none()
            && source.8.is_some()
            && source.9.is_some());
    if source.2 != "host"
        || source.3 != tool_name
        || !allowed_status
        || source.6
        || source.5 != input_json
    {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph ToolCall '{tool_call_id}' does not match exact Host '{tool_name}' authority"
        )));
    }
    Ok(())
}

fn require_graph_review_pass(
    transaction: &Connection,
    attempt_id: &str,
    input_json: &str,
    child_result_hash: &str,
    policy_hash: &str,
) -> Result<(), RepositoryError> {
    let approved = transaction
        .query_row(
            "SELECT decision.id, decision.reviewer_run_id,
                    decision.reviewer_conversation_id, request.activated_at,
                    reviewer_run.finished_at, decision.decision_json
                FROM work_graph_node_review_requests AS request
                JOIN work_graph_node_review_decisions AS decision
                  ON decision.request_id = request.id
                 AND decision.outcome = 'pass'
                JOIN runs AS reviewer_run ON reviewer_run.id = decision.reviewer_run_id
                WHERE request.attempt_id = ?1
                  AND request.input_json = ?2
                  AND request.child_result_hash = ?3
                  AND request.validation_policy_hash = ?4
                  AND request.activated_at IS NOT NULL
                  AND request.settled_at IS NOT NULL
                ORDER BY decision.created_at DESC LIMIT 1",
            params![attempt_id, input_json, child_result_hash, policy_hash],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(approved) = approved else {
        let stale_pass = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1
                    FROM work_graph_node_review_requests AS request
                    JOIN work_graph_node_review_decisions AS decision
                      ON decision.request_id = request.id AND decision.outcome = 'pass'
                    WHERE request.attempt_id = ?1
                 )",
                [attempt_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        let code = if stale_pass {
            "[graph.readonly_review_candidate_mismatch]"
        } else {
            "[graph.readonly_review_required]"
        };
        return Err(RepositoryError::ConstraintViolation(format!(
            "{code} high-risk Graph node finish requires an independent pass for the exact current candidate"
        )));
    };
    let task_id = transaction
        .query_row(
            "SELECT task_id FROM task_attempts WHERE id = ?1",
            [attempt_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    let live_bindings = validate_graph_reviewer_decision_proof(
        transaction,
        &task_id,
        &approved.1,
        &approved.2,
        approved.3,
        approved.4,
        &GraphNodeReviewOutcome::Pass,
        &approved.5,
    )?;
    let stored = transaction
        .prepare(
            "SELECT criterion_ordinal, criterion, runtime_tool_call_id, tool_call_id
             FROM work_graph_node_review_proofs WHERE decision_id = ?1
             ORDER BY criterion_ordinal, runtime_tool_call_id",
        )
        .map_err(database_error)?
        .query_map([approved.0.as_str()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut expected = live_bindings
        .into_iter()
        .map(|binding| {
            (
                binding.criterion_ordinal,
                binding.criterion,
                binding.runtime_tool_call_id,
                binding.tool_call_id,
            )
        })
        .collect::<Vec<_>>();
    expected.sort();
    if stored != expected {
        return Err(RepositoryError::ConstraintViolation(
            "[graph.readonly_review_required] Reviewer pass proof bindings are incomplete or stale"
                .to_owned(),
        ));
    }
    Ok(())
}

fn authoritative_completed_graph_child(
    transaction: &Connection,
    attempt_id: &str,
    parent_run_id: &str,
) -> Result<(String, String), RepositoryError> {
    let child = transaction
        .query_row(
            "SELECT delegation.child_run_id, delegation.child_conversation_id,
                    delegation.status, runs.status, runs.finished_at,
                    COALESCE((
                        SELECT substr(messages.content, 1, 32000)
                        FROM messages
                        WHERE messages.run_id = runs.id AND messages.role = 'assistant'
                          AND trim(messages.content) <> ''
                        ORDER BY messages.ordinal DESC LIMIT 1
                    ), NULLIF(delegation.result_text, ''), '')
             FROM child_run_delegations delegation
             JOIN runs ON runs.id = delegation.child_run_id
             WHERE delegation.graph_task_attempt_id = ?1
               AND delegation.parent_run_id = ?2",
            params![attempt_id, parent_run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<i64>>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| {
            RepositoryError::InvalidReference(format!(
                "Graph Attempt '{attempt_id}' has no exact parent-owned Child delegation"
            ))
        })?;
    if child.2 != "completed" || child.3 != "completed" || child.4.is_none() {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph Attempt '{attempt_id}' cannot finish before its authoritative Child Run completes"
        )));
    }
    if child.5.trim().is_empty() {
        return Err(RepositoryError::ConstraintViolation(format!(
            "[graph.readonly_child_result_required] completed Child Run '{}' has no persisted assistant result",
            child.0
        )));
    }
    require_frozen_durable_child_run(transaction, &child.0, parent_run_id, &child.1)?;
    let result_json = serde_json::to_string(&json!({
        "childRunId": &child.0,
        "status": &child.3,
        "finishedAt": child.4,
        "result": &child.5,
    }))
    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    Ok((child.0, sha256_hex(result_json.as_bytes())))
}

const GRAPH_CRITERION_RUNTIME_TOOL_ALLOWLIST: &[&str] = &["read", "ls", "find", "grep"];
const GRAPH_CRITERION_HOST_INSPECTION_TOOL_ALLOWLIST: &[&str] =
    &["git_read", "structured_data", "tabular_data"];
const GRAPH_CRITERION_HOST_TEST_TOOL_ALLOWLIST: &[&str] = &["test_run", "code_check"];

fn validate_graph_criterion_evidence(
    transaction: &Connection,
    task_id: &str,
    parent_run_id: &str,
    conversation_id: &str,
    evidence_rowid_watermark: i64,
    attempt_started_at: i64,
    evidence_id: &str,
) -> Result<(), RepositoryError> {
    let evidence =
        load_evidence(transaction, evidence_id)?.ok_or_else(|| RepositoryError::NotFound {
            entity: "criterion Evidence",
            id: evidence_id.to_owned(),
        })?;
    let evidence_rowid = transaction
        .query_row(
            "SELECT rowid FROM task_evidence WHERE id = ?1",
            [evidence_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    let check_type = evidence
        .metadata
        .get("validationCheckType")
        .and_then(Value::as_str);
    let valid_evidence_shape = matches!(
        (&evidence.evidence_type, check_type),
        (EvidenceType::ToolCall, Some("inspection")) | (EvidenceType::TestResult, Some("test"))
    );
    if evidence.task_id != task_id
        || evidence.source_run_id.as_deref() != Some(parent_run_id)
        || evidence.validity_status != EvidenceValidityStatus::Valid
        || evidence_rowid <= evidence_rowid_watermark
        || evidence.ref_kind != EvidenceReferenceKind::ToolCall
        || evidence.checked_at.is_none()
        || !valid_evidence_shape
    {
        return Err(RepositoryError::InvalidReference(format!(
            "criterion Evidence '{evidence_id}' must be a fresh, valid parent-Lead inspection or passing test ToolCall for the current Attempt"
        )));
    }
    validate_reference(
        transaction,
        task_id,
        &evidence.evidence_type,
        &evidence.ref_kind,
        &evidence.ref_id,
        evidence.trace_id.as_deref(),
        evidence.span_id.as_deref(),
    )?;
    validate_evidence_check_semantics(
        transaction,
        check_type.expect("valid shape requires a check type"),
        match evidence.evidence_type {
            EvidenceType::ToolCall => "tool_call",
            EvidenceType::TestResult => "test_result",
            _ => unreachable!("valid Graph criterion Evidence shape is bounded"),
        },
        "tool_call",
        &evidence.ref_id,
        evidence.source_run_id.as_deref(),
        &evidence.metadata,
    )?;
    let tool = transaction
        .query_row(
            "SELECT run_id, conversation_id, tool_name, execution_location, status,
                    error_message, started_at, completed_at, updated_at, requires_approval
             FROM tool_calls WHERE id = ?1",
            [evidence.ref_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<i64>>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, bool>(9)?,
                ))
            },
        )
        .map_err(database_error)?;
    let safe_tool = match (check_type, tool.3.as_str()) {
        (Some("inspection"), "runtime") => {
            !tool.9 && GRAPH_CRITERION_RUNTIME_TOOL_ALLOWLIST.contains(&tool.2.as_str())
        }
        (Some("inspection"), "host") => {
            (!tool.9 && GRAPH_CRITERION_HOST_INSPECTION_TOOL_ALLOWLIST.contains(&tool.2.as_str()))
                || (tool.9 && tool.2 == "sqlite_read")
        }
        (Some("test"), "host") => {
            tool.9 && GRAPH_CRITERION_HOST_TEST_TOOL_ALLOWLIST.contains(&tool.2.as_str())
        }
        _ => false,
    };
    if tool.0 != parent_run_id
        || tool.1 != conversation_id
        || !safe_tool
        || tool.4 != "completed"
        || tool.5.as_deref().is_some_and(|message| !message.is_empty())
        || tool.6 <= attempt_started_at
        || tool.7.is_none()
        || tool.7.is_some_and(|completed_at| completed_at < tool.6)
        || tool.8 < tool.6
    {
        return Err(RepositoryError::InvalidReference(format!(
            "criterion Evidence '{evidence_id}' references a stale, foreign, failed, or non-allowlisted ToolCall"
        )));
    }
    Ok(())
}

fn graph_finish_id(tool_call_id: &str) -> String {
    format!(
        "graph-finish-{}",
        &sha256_hex(tool_call_id.as_bytes())[..32]
    )
}

fn graph_cancel_intent_id(tool_call_id: &str) -> String {
    format!(
        "graph-cancel-{}",
        &sha256_hex(tool_call_id.as_bytes())[..32]
    )
}

fn graph_terminal_reconciliation_id(child_run_id: &str) -> String {
    format!(
        "graph-terminal-{}",
        &sha256_hex(child_run_id.as_bytes())[..32]
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn replay_graph_node_finish(
    transaction: &Transaction<'_>,
    input: &FinishReadOnlyGraphNodeInput,
    spec: &StoredSpec,
    input_json: &str,
    input_hash: &str,
) -> Result<Option<GraphLeadNodeFinishResult>, RepositoryError> {
    let existing = transaction
        .query_row(
            "SELECT id, goal_id, task_id, attempt_id, parent_run_id, child_run_id,
                    tool_call_id, expected_task_version, expected_attempt_version,
                    child_result_hash, summary, input_json, input_hash
             FROM work_graph_node_finishes
             WHERE tool_call_id = ?1 OR attempt_id = ?2
             ORDER BY CASE WHEN tool_call_id = ?1 THEN 0 ELSE 1 END
             LIMIT 1",
            params![input.node_finish_tool_call_id, input.attempt_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(existing) = existing else {
        return Ok(None);
    };
    if existing.1 != input.goal_id
        || existing.2 != input.task_id
        || existing.3 != input.attempt_id
        || existing.4 != input.parent_run_id
        || existing.6 != input.node_finish_tool_call_id
        || existing.7 != input.expected_task_version
        || existing.8 != input.expected_attempt_version
        || existing.10 != input.summary
        || existing.11 != input_json
        || existing.12 != input_hash
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph node-finish replay conflicts with the immutable finish header".to_owned(),
        ));
    }
    let conversation_id = transaction
        .query_row(
            "SELECT conversation_id FROM goals WHERE id = ?1",
            [input.goal_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    require_node_finish_tool_call(transaction, input, &conversation_id, input_json, true)?;
    let (child_run_id, child_result_hash) =
        authoritative_completed_graph_child(transaction, &input.attempt_id, &input.parent_run_id)?;
    if existing.5 != child_run_id || existing.9 != child_result_hash {
        return Err(RepositoryError::ConstraintViolation(
            "Graph node-finish replay no longer matches the authoritative Child result".to_owned(),
        ));
    }
    let rows = transaction
        .prepare(
            "SELECT criterion_ordinal, criterion, evidence_id
             FROM work_graph_node_criterion_evidence
             WHERE finish_id = ?1
             ORDER BY criterion_ordinal, rowid",
        )
        .map_err(database_error)?
        .query_map([existing.0.as_str()], |row| {
            Ok((
                row.get::<_, usize>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut stored_bindings = vec![Vec::<String>::new(); input.criterion_evidence.len()];
    for (ordinal, criterion, evidence_id) in rows {
        let expected = input.criterion_evidence.get(ordinal).ok_or_else(|| {
            RepositoryError::ConstraintViolation(
                "immutable Graph finish contains an extra criterion ordinal".to_owned(),
            )
        })?;
        if criterion != expected.criterion {
            return Err(RepositoryError::ConstraintViolation(
                "immutable Graph finish criterion no longer matches its exact replay".to_owned(),
            ));
        }
        stored_bindings[ordinal].push(evidence_id);
    }
    if stored_bindings
        .iter()
        .zip(&input.criterion_evidence)
        .any(|(stored, expected)| stored != &expected.evidence_ids)
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph node-finish replay conflicts with immutable criterion Evidence bindings"
                .to_owned(),
        ));
    }
    let task =
        load_task(transaction, &input.task_id)?.ok_or_else(|| RepositoryError::NotFound {
            entity: "graph task",
            id: input.task_id.clone(),
        })?;
    let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
        RepositoryError::NotFound {
            entity: "graph task attempt",
            id: input.attempt_id.clone(),
        }
    })?;
    if task.status != WorkTaskStatus::Completed || attempt.status != TaskAttemptStatus::Succeeded {
        return Err(RepositoryError::ConstraintViolation(
            "immutable Graph finish exists without its terminal Task and Attempt facts".to_owned(),
        ));
    }
    Ok(Some(GraphLeadNodeFinishResult {
        replayed: true,
        child_run_id,
        child_result_hash,
        criterion_evidence: input.criterion_evidence.clone(),
        finish: TaskAttemptFinishResult { task, attempt },
        snapshot: project_snapshot(transaction, spec.clone())?,
    }))
}

fn node_start_tool_input(input: &StartReadyReadOnlyGraphNodeInput) -> Value {
    json!({
        "goalId": input.goal_id,
        "taskId": input.task_id,
        "expectedTaskVersion": input.expected_task_version,
        "attemptId": input.attempt_id,
        "workerAgentId": input.worker_agent_id,
        "objective": input.objective,
        "context": input.context,
        "budget": input.budget,
    })
}

fn require_node_start_tool_call(
    transaction: &Transaction<'_>,
    input: &StartReadyReadOnlyGraphNodeInput,
    conversation_id: &str,
) -> Result<(), RepositoryError> {
    let source = transaction
        .query_row(
            "SELECT run_id, conversation_id, execution_location, tool_name, status,
                    input_json, requires_approval
             FROM tool_calls WHERE id = ?1",
            [input.node_start_tool_call_id.as_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "Graph node-start tool call",
            id: input.node_start_tool_call_id.clone(),
        })?;
    if source.0 != input.parent_run_id || source.1 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if source.2 != "host" || source.3 != GRAPH_NODE_START_TOOL || source.4 != "running" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node-start ToolCall '{}' must be a running Host '{}' call",
            input.node_start_tool_call_id, GRAPH_NODE_START_TOOL
        )));
    }
    if source.6 {
        return Err(RepositoryError::ConstraintViolation(
            "read-only Graph node-start ToolCall cannot require an approval grant".to_owned(),
        ));
    }
    let stored_input = serde_json::from_str::<Value>(&source.5).map_err(|error| {
        RepositoryError::InvalidInput(format!(
            "Graph node-start ToolCall input is not valid JSON: {error}"
        ))
    })?;
    if stored_input != node_start_tool_input(input) {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph node-start ToolCall '{}' does not exactly bind the Repository input",
            input.node_start_tool_call_id
        )));
    }
    Ok(())
}

fn replay_graph_node_start(
    transaction: &Transaction<'_>,
    input: &StartReadyReadOnlyGraphNodeInput,
    spec: StoredSpec,
    child_run: ChildRunRecord,
    allowed_tools: &[String],
) -> Result<GraphLeadNodeStartResult, RepositoryError> {
    let graph_task_attempt_id = transaction
        .query_row(
            "SELECT graph_task_attempt_id FROM child_run_delegations
             WHERE child_run_id = ?1",
            [child_run.child_run_id.as_str()],
            |row| row.get::<_, Option<String>>(0),
        )
        .map_err(database_error)?;
    if graph_task_attempt_id.as_deref() != Some(input.attempt_id.as_str())
        || child_run.parent_run_id != input.parent_run_id
        || child_run.worker_agent_id != input.worker_agent_id
        || child_run.objective != input.objective
        || child_run.context != input.context
        || child_run.team_run_id.is_some()
        || child_run.team_member_id.is_some()
        || child_run.allowed_tools.as_deref() != Some(allowed_tools)
        || child_run.budget != input.budget
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph node-start ToolCall conflicts with an existing Child Run".to_owned(),
        ));
    }
    let attempt = load_task_attempt(transaction, &input.attempt_id)?.ok_or_else(|| {
        RepositoryError::NotFound {
            entity: "graph task attempt",
            id: input.attempt_id.clone(),
        }
    })?;
    if attempt.task_id != input.task_id
        || attempt.run_id != input.parent_run_id
        || attempt.kind != TaskAttemptKind::Execution
    {
        return Err(RepositoryError::ConstraintViolation(
            "existing Graph delegation does not match its parent-owned Execution Attempt"
                .to_owned(),
        ));
    }
    let task =
        load_task(transaction, &input.task_id)?.ok_or_else(|| RepositoryError::NotFound {
            entity: "graph task",
            id: input.task_id.clone(),
        })?;
    if task.goal_id != input.goal_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    require_frozen_durable_child_run(
        transaction,
        &child_run.child_run_id,
        &input.parent_run_id,
        &child_run.child_conversation_id,
    )?;
    let started =
        query_child_start_result(transaction, &child_run.child_run_id).map_err(database_error)?;
    let snapshot = project_snapshot(transaction, spec)?;
    Ok(GraphLeadNodeStartResult {
        created: false,
        replayed: true,
        started,
        child_run,
        task,
        attempt,
        snapshot,
    })
}

fn require_frozen_durable_child_run(
    transaction: &Connection,
    child_run_id: &str,
    parent_run_id: &str,
    child_conversation_id: &str,
) -> Result<(), RepositoryError> {
    let row = transaction
        .query_row(
            "SELECT runs.parent_run_id, runs.conversation_id, runs.run_kind,
                    profiles.schema_version, profiles.profile_id,
                    profiles.snapshot_json, profiles.profile_hash
             FROM runs
             LEFT JOIN run_execution_profiles profiles ON profiles.run_id = runs.id
             WHERE runs.id = ?1",
            [child_run_id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "Graph child run",
            id: child_run_id.to_owned(),
        })?;
    if row.0.as_deref() != Some(parent_run_id) || row.1 != child_conversation_id || row.2 != "child"
    {
        return Err(RepositoryError::ConstraintViolation(
            "Graph Child Run lineage does not match its parent delegation".to_owned(),
        ));
    }
    let expected_snapshot = serde_json::to_string(&json!({
        "schemaVersion": 1,
        "id": GRAPH_CHILD_PROFILE
    }))
    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let expected_hash = format!("{:x}", Sha256::digest(expected_snapshot.as_bytes()));
    if row.3 != Some(1)
        || row.4.as_deref() != Some(GRAPH_CHILD_PROFILE)
        || row.5.as_deref() != Some(expected_snapshot.as_str())
        || row.6.as_deref() != Some(expected_hash.as_str())
    {
        return Err(RepositoryError::InvalidValidationPolicy(
            "Graph Child Run requires an untampered Host-frozen durable_v2 profile".to_owned(),
        ));
    }
    Ok(())
}

fn normalize_plan_nodes(tasks: &Value) -> Result<Vec<NormalizedPlanNode>, RepositoryError> {
    let tasks = tasks
        .as_array()
        .filter(|nodes| !nodes.is_empty())
        .ok_or_else(|| {
            RepositoryError::InvalidInput(
                "an approved PlanRevision must contain a non-empty task array".to_owned(),
            )
        })?;
    if tasks.len() > GRAPH_MAX_NODES {
        return Err(RepositoryError::InvalidInput(format!(
            "read-only Graph supports 1-{GRAPH_MAX_NODES} nodes"
        )));
    }

    let mut nodes = Vec::with_capacity(tasks.len());
    let mut keys = HashSet::new();
    let mut ordinals = HashSet::new();
    for value in tasks {
        let object = value.as_object().ok_or_else(|| {
            RepositoryError::InvalidInput("every Graph plan node must be an object".to_owned())
        })?;
        let allowed_fields = [
            "nodeKey",
            "title",
            "detail",
            "ordinal",
            "dependsOn",
            "acceptanceCriteria",
        ]
        .into_iter()
        .collect::<HashSet<_>>();
        if let Some(field) = object
            .keys()
            .find(|field| !allowed_fields.contains(field.as_str()))
        {
            return Err(RepositoryError::InvalidInput(format!(
                "Graph plan node contains unsupported field '{field}'"
            )));
        }
        let ordinal = object
            .get("ordinal")
            .and_then(Value::as_i64)
            .filter(|ordinal| *ordinal >= 0)
            .ok_or_else(|| {
                RepositoryError::InvalidInput(
                    "every Graph plan node requires a non-negative ordinal".to_owned(),
                )
            })?;
        if !ordinals.insert(ordinal) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "duplicate Graph node ordinal '{ordinal}'"
            )));
        }
        let node_key = object
            .get("nodeKey")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                RepositoryError::InvalidInput(
                    "every Graph plan node requires an explicit nodeKey".to_owned(),
                )
            })?
            .to_owned();
        validate_node_key(&node_key)?;
        if !keys.insert(node_key.clone()) {
            return Err(RepositoryError::ConstraintViolation(format!(
                "duplicate Graph node key '{node_key}'"
            )));
        }
        let title =
            bounded_required_string(object.get("title"), "Graph node title", MAX_TITLE_CHARS)?;
        let detail =
            bounded_optional_string(object.get("detail"), "Graph node detail", MAX_DETAIL_CHARS)?;
        let depends_on = string_array(object.get("dependsOn"), "dependsOn", GRAPH_MAX_NODES)?;
        let acceptance_criteria = string_array(
            object.get("acceptanceCriteria"),
            "acceptanceCriteria",
            MAX_ACCEPTANCE_CRITERIA,
        )?
        .into_iter()
        .map(|criterion| {
            bounded_non_empty(
                &criterion,
                "acceptance criterion",
                MAX_ACCEPTANCE_CRITERION_CHARS,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
        if acceptance_criteria.is_empty() {
            return Err(RepositoryError::InvalidInput(
                "every Graph plan node requires 1-8 explicit acceptanceCriteria".to_owned(),
            ));
        }
        if acceptance_criteria.iter().collect::<HashSet<_>>().len() != acceptance_criteria.len() {
            return Err(RepositoryError::ConstraintViolation(
                "Graph acceptanceCriteria cannot contain duplicates".to_owned(),
            ));
        }
        nodes.push(NormalizedPlanNode {
            node_key,
            title,
            detail,
            ordinal,
            depends_on,
            acceptance_criteria,
            depth: 0,
        });
    }
    nodes.sort_by(|left, right| {
        left.ordinal
            .cmp(&right.ordinal)
            .then_with(|| left.node_key.cmp(&right.node_key))
    });
    for (expected, node) in nodes.iter().enumerate() {
        if node.ordinal != expected as i64 {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph node ordinals must be contiguous from zero; expected {expected}, got {}",
                node.ordinal
            )));
        }
    }

    let key_set = nodes
        .iter()
        .map(|node| node.node_key.clone())
        .collect::<HashSet<_>>();
    for node in &mut nodes {
        let mut seen_dependencies = HashSet::new();
        for dependency in &node.depends_on {
            validate_node_key(dependency)?;
            if dependency == &node.node_key {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node '{}' cannot depend on itself",
                    node.node_key
                )));
            }
            if !key_set.contains(dependency) {
                return Err(RepositoryError::InvalidReference(format!(
                    "Graph node '{}' depends on unknown node '{dependency}'",
                    node.node_key
                )));
            }
            if !seen_dependencies.insert(dependency.clone()) {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Graph node '{}' repeats dependency '{dependency}'",
                    node.node_key
                )));
            }
        }
        node.depends_on.sort();
    }
    apply_depths(&mut nodes)?;
    Ok(nodes)
}

fn apply_depths(nodes: &mut [NormalizedPlanNode]) -> Result<(), RepositoryError> {
    let indexes = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.node_key.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut indegree = nodes
        .iter()
        .map(|node| (node.node_key.clone(), node.depends_on.len()))
        .collect::<HashMap<_, _>>();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
    for node in nodes.iter() {
        for dependency in &node.depends_on {
            dependents
                .entry(dependency.clone())
                .or_default()
                .push(node.node_key.clone());
        }
    }
    for values in dependents.values_mut() {
        values.sort();
    }
    let mut ready = nodes
        .iter()
        .filter(|node| indegree.get(&node.node_key) == Some(&0))
        .map(|node| node.node_key.clone())
        .collect::<BTreeSet<_>>();
    let mut depths = HashMap::<String, i64>::new();
    let mut visited = 0usize;
    while let Some(node_key) = ready.pop_first() {
        visited += 1;
        let depth = depths.get(&node_key).copied().unwrap_or(0);
        if depth > GRAPH_MAX_DEPTH {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph depth {depth} exceeds read-only limit {GRAPH_MAX_DEPTH}"
            )));
        }
        let node_index = indexes[&node_key];
        nodes[node_index].depth = depth;
        for dependent in dependents.get(&node_key).into_iter().flatten() {
            depths
                .entry(dependent.clone())
                .and_modify(|current| *current = (*current).max(depth + 1))
                .or_insert(depth + 1);
            let remaining = indegree.get_mut(dependent).expect("validated Graph node");
            *remaining -= 1;
            if *remaining == 0 {
                ready.insert(dependent.clone());
            }
        }
    }
    if visited != nodes.len() {
        return Err(RepositoryError::ConstraintViolation(
            "read-only Graph must be acyclic".to_owned(),
        ));
    }
    Ok(())
}

fn bind_plan_nodes(
    transaction: &Transaction<'_>,
    goal_id: &str,
    normalized_nodes: Vec<NormalizedPlanNode>,
) -> Result<Vec<BoundPlanNode>, RepositoryError> {
    let mut statement = transaction
        .prepare(
            "SELECT id, ordinal, title, detail
             FROM work_tasks WHERE goal_id = ?1 ORDER BY ordinal, id",
        )
        .map_err(database_error)?;
    let tasks = statement
        .query_map([goal_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);
    if tasks.len() != normalized_nodes.len() {
        return Err(RepositoryError::ConstraintViolation(format!(
            "approved PlanRevision contains {} nodes but Goal '{goal_id}' owns {} Tasks",
            normalized_nodes.len(),
            tasks.len()
        )));
    }
    let by_ordinal = tasks
        .into_iter()
        .map(|task| (task.1, task))
        .collect::<BTreeMap<_, _>>();
    normalized_nodes
        .into_iter()
        .map(|normalized| {
            let task = by_ordinal.get(&normalized.ordinal).ok_or_else(|| {
                RepositoryError::ConstraintViolation(format!(
                    "Goal '{goal_id}' has no Task at Plan ordinal {}",
                    normalized.ordinal
                ))
            })?;
            if task.2 != normalized.title || task.3 != normalized.detail {
                return Err(RepositoryError::ConstraintViolation(format!(
                    "Goal Task '{}' diverges from approved Plan node '{}'",
                    task.0, normalized.node_key
                )));
            }
            Ok(BoundPlanNode {
                normalized,
                task_id: task.0.clone(),
            })
        })
        .collect()
}

fn require_pristine_durable_tasks(
    transaction: &Transaction<'_>,
    goal_id: &str,
    nodes: &[BoundPlanNode],
) -> Result<(), RepositoryError> {
    for node in nodes {
        let (status, owner_run_id, attempt, policy_id): (String, Option<String>, i64, String) =
            transaction
                .query_row(
                    "SELECT tasks.status, tasks.owner_run_id, tasks.attempt, policies.policy_id
                     FROM work_tasks tasks
                     JOIN task_validation_policies policies ON policies.task_id = tasks.id
                     WHERE tasks.id = ?1 AND tasks.goal_id = ?2",
                    params![node.task_id, goal_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(database_error)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity: "durable graph task",
                    id: node.task_id.clone(),
                })?;
        // WorkTask's first durable attempt number is reserved as `1` at Task creation;
        // pristine means no owner and no TaskAttempt/Evidence facts, not attempt zero.
        if status != "queued" || owner_run_id.is_some() || attempt != 1 {
            return Err(RepositoryError::ConstraintViolation(format!(
                "Graph Task '{}' must be pristine and queued before activation",
                node.task_id
            )));
        }
        if policy_id == "legacy_v1" {
            return Err(RepositoryError::InvalidValidationPolicy(format!(
                "Graph Task '{}' must freeze a durable ValidationPolicy",
                node.task_id
            )));
        }
    }
    let history_count = transaction
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM task_attempts attempts
                 JOIN work_tasks tasks ON tasks.id = attempts.task_id WHERE tasks.goal_id = ?1) +
                (SELECT COUNT(*) FROM task_evidence evidence
                 JOIN work_tasks tasks ON tasks.id = evidence.task_id WHERE tasks.goal_id = ?1)",
            [goal_id],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if history_count != 0 {
        return Err(RepositoryError::ConstraintViolation(
            "read-only Graph activation cannot adopt Tasks with Attempt or Evidence history"
                .to_owned(),
        ));
    }
    Ok(())
}

fn require_frozen_durable_parent_run(
    transaction: &Transaction<'_>,
    run_id: &str,
    conversation_id: &str,
) -> Result<(), RepositoryError> {
    let row = transaction
        .query_row(
            "SELECT runs.conversation_id, runs.status, runs.run_kind, runs.parent_run_id,
                    runs.depth, profiles.schema_version, profiles.profile_id,
                    profiles.snapshot_json, profiles.profile_hash
             FROM runs
             LEFT JOIN run_execution_profiles profiles ON profiles.run_id = runs.id
             WHERE runs.id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "parent run",
            id: run_id.to_owned(),
        })?;
    if row.0 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if row.1 != "running" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph parent Lead Run '{run_id}' must be running"
        )));
    }
    if row.2 != "primary" || row.3.is_some() || row.4 != 0 {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph parent Run '{run_id}' must be a depth-zero primary Lead Run"
        )));
    }
    let expected_snapshot = serde_json::to_string(&json!({
        "schemaVersion": 1,
        "id": "durable_v2"
    }))
    .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    let expected_hash = format!("{:x}", Sha256::digest(expected_snapshot.as_bytes()));
    if row.5 != Some(1)
        || row.6.as_deref() != Some("durable_v2")
        || row.7.as_deref() != Some(expected_snapshot.as_str())
        || row.8.as_deref() != Some(expected_hash.as_str())
    {
        return Err(RepositoryError::InvalidValidationPolicy(
            "Graph parent Lead Run requires an untampered Host-frozen durable_v2 profile"
                .to_owned(),
        ));
    }
    Ok(())
}

fn require_activation_tool_call(
    transaction: &Transaction<'_>,
    tool_call_id: &str,
    run_id: &str,
    conversation_id: &str,
    plan_revision_id: &str,
) -> Result<(), RepositoryError> {
    let activation_source = transaction
        .query_row(
            "SELECT run_id, conversation_id, execution_location, tool_name, status,
                    input_json, requires_approval
             FROM tool_calls WHERE id = ?1",
            [tool_call_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity: "activation tool call",
            id: tool_call_id.to_owned(),
        })?;
    if activation_source.0 != run_id || activation_source.1 != conversation_id {
        return Err(RepositoryError::CrossConversationReference);
    }
    if activation_source.2 != "host" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph activation ToolCall '{tool_call_id}' must execute on the Host"
        )));
    }
    if activation_source.3 != "graph_readonly_activate" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph activation ToolCall '{tool_call_id}' must use 'graph_readonly_activate'"
        )));
    }
    if activation_source.4 != "running" {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph activation ToolCall '{tool_call_id}' must still be running"
        )));
    }
    if activation_source.6 {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph activation ToolCall '{tool_call_id}' cannot require approval"
        )));
    }
    let stored_input = serde_json::from_str::<Value>(&activation_source.5).map_err(|error| {
        RepositoryError::InvalidInput(format!(
            "Graph activation ToolCall input is not valid JSON: {error}"
        ))
    })?;
    if stored_input != json!({ "planRevisionId": plan_revision_id }) {
        return Err(RepositoryError::ConstraintViolation(format!(
            "Graph activation ToolCall '{tool_call_id}' does not exactly bind PlanRevision '{plan_revision_id}'"
        )));
    }
    Ok(())
}

fn graph_spec_hash(
    goal_id: &str,
    plan_revision_id: &str,
    nodes: &[BoundPlanNode],
) -> Result<String, RepositoryError> {
    let canonical_nodes = nodes
        .iter()
        .map(|node| {
            json!({
                "taskId": node.task_id,
                "node": node.normalized,
            })
        })
        .collect::<Vec<_>>();
    let canonical = json!({
        "schemaVersion": GRAPH_SCHEMA_VERSION,
        "graphId": goal_id,
        "planRevisionId": plan_revision_id,
        "graphMode": GRAPH_MODE_READ_ONLY,
        "maxNodes": GRAPH_MAX_NODES,
        "maxDepth": GRAPH_MAX_DEPTH,
        "nodes": canonical_nodes,
    });
    let encoded = serde_json::to_vec(&canonical)
        .map_err(|error| RepositoryError::InvalidInput(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn load_spec(
    connection: &Connection,
    goal_id: &str,
) -> Result<Option<StoredSpec>, RepositoryError> {
    connection
        .query_row(
            "SELECT goal_id, plan_revision_id, schema_version, graph_mode, max_nodes,
                    max_depth, spec_hash, activated_by_run_id, activation_tool_call_id,
                    activated_at
             FROM work_graph_specs WHERE goal_id = ?1",
            [goal_id],
            |row| {
                Ok(StoredSpec {
                    goal_id: row.get(0)?,
                    plan_revision_id: row.get(1)?,
                    schema_version: row.get(2)?,
                    graph_mode: row.get(3)?,
                    max_nodes: row.get(4)?,
                    max_depth: row.get(5)?,
                    spec_hash: row.get(6)?,
                    activated_by_run_id: row.get(7)?,
                    activation_tool_call_id: row.get(8)?,
                    activated_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(database_error)
}

fn project_snapshot(
    connection: &Connection,
    spec: StoredSpec,
) -> Result<GraphLeadSnapshot, RepositoryError> {
    let mut node_statement = connection
        .prepare(
            "SELECT nodes.task_id, nodes.node_key, nodes.depth,
                    nodes.acceptance_criteria_json, tasks.title, tasks.detail,
                    tasks.ordinal, tasks.status, tasks.version, tasks.attempt
             FROM work_graph_nodes nodes
             JOIN work_tasks tasks ON tasks.id = nodes.task_id
             WHERE nodes.goal_id = ?1 AND nodes.access_mode = 'read_only'
             ORDER BY tasks.ordinal, nodes.node_key, nodes.task_id
             LIMIT 4",
        )
        .map_err(database_error)?;
    let stored_nodes = node_statement
        .query_map([spec.goal_id.as_str()], |row| {
            let criteria_json = row.get::<_, String>(3)?;
            let acceptance_criteria =
                serde_json::from_str::<Vec<String>>(&criteria_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(StoredNode {
                task_id: row.get(0)?,
                node_key: row.get(1)?,
                depth: row.get(2)?,
                acceptance_criteria,
                title: row.get(4)?,
                detail: row.get(5)?,
                ordinal: row.get(6)?,
                task_status: row.get(7)?,
                task_version: row.get(8)?,
                task_attempt: row.get(9)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(node_statement);
    if stored_nodes.is_empty() || stored_nodes.len() > GRAPH_MAX_NODES {
        return Err(RepositoryError::ConstraintViolation(format!(
            "stored read-only Graph '{}' contains {} nodes outside its bounded contract",
            spec.goal_id,
            stored_nodes.len()
        )));
    }

    let mut edge_statement = connection
        .prepare(
            "SELECT dependency_task_id, dependent_task_id
             FROM work_task_edges WHERE goal_id = ?1
             ORDER BY dependency_task_id, dependent_task_id LIMIT 7",
        )
        .map_err(database_error)?;
    let edges = edge_statement
        .query_map([spec.goal_id.as_str()], |row| {
            Ok(GraphLeadEdgeSnapshot {
                dependency_task_id: row.get(0)?,
                dependent_task_id: row.get(1)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(edge_statement);

    let node_ids = stored_nodes
        .iter()
        .map(|node| node.task_id.as_str())
        .collect::<HashSet<_>>();
    if edges.iter().any(|edge| {
        !node_ids.contains(edge.dependency_task_id.as_str())
            || !node_ids.contains(edge.dependent_task_id.as_str())
    }) {
        return Err(RepositoryError::InvalidReference(
            "stored Graph contains an Edge outside its Node set".to_owned(),
        ));
    }
    let dependencies = edges
        .iter()
        .fold(HashMap::<String, Vec<String>>::new(), |mut map, edge| {
            map.entry(edge.dependent_task_id.clone())
                .or_default()
                .push(edge.dependency_task_id.clone());
            map
        });

    let mut accepted = HashMap::new();
    for node in &stored_nodes {
        accepted.insert(
            node.task_id.clone(),
            accepted_attempt_projection(connection, node)?,
        );
    }
    let permanently_skipped = stored_nodes
        .iter()
        .filter(|node| node.task_status == "skipped")
        .map(|node| node.task_id.clone())
        .collect::<HashSet<_>>();

    let nodes = stored_nodes
        .into_iter()
        .map(|node| {
            let dependency_task_ids = dependencies.get(&node.task_id).cloned().unwrap_or_default();
            let attempt = accepted.get(&node.task_id).cloned().unwrap_or_default();
            let dependency_skipped = dependency_task_ids
                .iter()
                .any(|task_id| permanently_skipped.contains(task_id));
            let dependencies_accepted = dependency_task_ids.iter().all(|task_id| {
                accepted
                    .get(task_id)
                    .is_some_and(|projection| projection.accepted)
            });
            let (readiness, blocker) = if attempt.accepted {
                (GraphLeadNodeReadiness::Accepted, None)
            } else {
                match node.task_status.as_str() {
                    "queued" if dependency_skipped => (
                        GraphLeadNodeReadiness::Waiting,
                        Some(
                            "a required dependency was explicitly skipped; Host or human decision is required"
                                .to_owned(),
                        ),
                    ),
                    "queued" if dependencies_accepted => (GraphLeadNodeReadiness::Runnable, None),
                    "queued" => (
                        GraphLeadNodeReadiness::Waiting,
                        Some("waiting for mechanically accepted dependencies".to_owned()),
                    ),
                    "in_progress" => (GraphLeadNodeReadiness::Active, None),
                    "blocked" => (
                        GraphLeadNodeReadiness::Blocked,
                        Some("Task is blocked pending Host repair or human action".to_owned()),
                    ),
                    "interrupted" => (
                        GraphLeadNodeReadiness::Interrupted,
                        Some("Task execution was interrupted and remains retryable".to_owned()),
                    ),
                    "skipped" => (GraphLeadNodeReadiness::Skipped, None),
                    "completed" => (
                        GraphLeadNodeReadiness::Blocked,
                        attempt.blocker.clone().or_else(|| {
                            Some(
                                "completed Task lacks a current mechanically accepted Attempt"
                                    .to_owned(),
                            )
                        }),
                    ),
                    status => (
                        GraphLeadNodeReadiness::Blocked,
                        Some(format!("unsupported durable Task status '{status}'")),
                    ),
                }
            };
            GraphLeadNodeSnapshot {
                task_id: node.task_id,
                node_key: node.node_key,
                title: node.title,
                detail: node.detail,
                ordinal: node.ordinal,
                depth: node.depth,
                acceptance_criteria: node.acceptance_criteria,
                task_status: node.task_status,
                task_version: node.task_version,
                readiness,
                dependency_task_ids,
                authoritative_attempt_id: attempt.attempt_id,
                evidence_ids: attempt.evidence_ids,
                blocker,
            }
        })
        .collect();

    Ok(GraphLeadSnapshot {
        schema_version: spec.schema_version,
        graph_id: spec.goal_id,
        plan_revision_id: spec.plan_revision_id,
        graph_mode: spec.graph_mode,
        max_nodes: spec.max_nodes,
        max_depth: spec.max_depth,
        spec_hash: spec.spec_hash,
        activated_by_run_id: spec.activated_by_run_id,
        activated_at: spec.activated_at,
        nodes,
        edges,
    })
}

fn accepted_attempt_projection(
    connection: &Connection,
    node: &StoredNode,
) -> Result<AcceptedAttemptProjection, RepositoryError> {
    if node.task_status != "completed" || node.task_attempt < 1 {
        return Ok(AcceptedAttemptProjection::default());
    }
    let attempt = connection
        .query_row(
            "SELECT attempts.id, attempts.status, attempts.policy_hash,
                    attempts.evidence_ids_json, policies.policy_hash,
                    attempts.run_id, attempts.evidence_rowid_watermark, attempts.started_at
             FROM task_attempts attempts
             JOIN task_validation_policies policies ON policies.task_id = attempts.task_id
             WHERE attempts.task_id = ?1 AND attempts.attempt_number = ?2",
            params![node.task_id, node.task_attempt],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some((
        attempt_id,
        status,
        attempt_policy_hash,
        evidence_json,
        policy_hash,
        parent_run_id,
        evidence_rowid_watermark,
        attempt_started_at,
    )) = attempt
    else {
        return Ok(AcceptedAttemptProjection {
            blocker: Some("current Task attempt record is missing".to_owned()),
            ..Default::default()
        });
    };
    if status != "succeeded" || attempt_policy_hash != policy_hash {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("current Attempt is not a policy-matched success".to_owned()),
            ..Default::default()
        });
    }
    let evidence_ids = serde_json::from_str::<Vec<String>>(&evidence_json).map_err(|error| {
        RepositoryError::InvalidInput(format!(
            "Attempt '{}' has invalid evidence_ids_json: {error}",
            attempt_id
        ))
    })?;
    if evidence_ids.is_empty() || evidence_ids.len() > MAX_SNAPSHOT_EVIDENCE_IDS {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("current successful Attempt has no bounded Evidence set".to_owned()),
            ..Default::default()
        });
    }
    let unique = evidence_ids.iter().collect::<HashSet<_>>();
    if unique.len() != evidence_ids.len() {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("current successful Attempt repeats Evidence identifiers".to_owned()),
            ..Default::default()
        });
    }
    let finish = connection
        .query_row(
            "SELECT id, parent_run_id, child_run_id, child_result_hash
             FROM work_graph_node_finishes
             WHERE attempt_id = ?1 AND task_id = ?2",
            params![attempt_id, node.task_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some((finish_id, finish_parent_run_id, child_run_id, stored_child_hash)) = finish else {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some(
                "successful Graph Attempt lacks its immutable criterion-bound finish header"
                    .to_owned(),
            ),
            ..Default::default()
        });
    };
    if finish_parent_run_id != parent_run_id {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("Graph finish header belongs to a different parent Lead".to_owned()),
            ..Default::default()
        });
    }
    let (authoritative_child_run_id, child_result_hash) =
        match authoritative_completed_graph_child(connection, &attempt_id, &parent_run_id) {
            Ok(result) => result,
            Err(error @ RepositoryError::Database { .. }) => return Err(error),
            Err(error) => {
                return Ok(AcceptedAttemptProjection {
                    attempt_id: Some(attempt_id),
                    blocker: Some(error.to_string()),
                    ..Default::default()
                })
            }
        };
    if child_run_id != authoritative_child_run_id || stored_child_hash != child_result_hash {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some(
                "Graph finish header no longer matches the authoritative Child result".to_owned(),
            ),
            ..Default::default()
        });
    }
    let conversation_id = connection
        .query_row(
            "SELECT goals.conversation_id
             FROM work_tasks JOIN goals ON goals.id = work_tasks.goal_id
             WHERE work_tasks.id = ?1",
            [node.task_id.as_str()],
            |row| row.get::<_, String>(0),
        )
        .map_err(database_error)?;
    let binding_rows = connection
        .prepare(
            "SELECT criterion_ordinal, criterion, evidence_id
             FROM work_graph_node_criterion_evidence
             WHERE finish_id = ?1
             ORDER BY criterion_ordinal, rowid",
        )
        .map_err(database_error)?
        .query_map([finish_id.as_str()], |row| {
            Ok((
                row.get::<_, usize>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut criterion_bindings = vec![Vec::<String>::new(); node.acceptance_criteria.len()];
    for (ordinal, criterion, evidence_id) in binding_rows {
        let Some(expected_criterion) = node.acceptance_criteria.get(ordinal) else {
            return Ok(AcceptedAttemptProjection {
                attempt_id: Some(attempt_id),
                blocker: Some("Graph finish contains an extra criterion binding".to_owned()),
                ..Default::default()
            });
        };
        if &criterion != expected_criterion {
            return Ok(AcceptedAttemptProjection {
                attempt_id: Some(attempt_id),
                blocker: Some("Graph finish criterion diverges from the frozen node".to_owned()),
                ..Default::default()
            });
        }
        criterion_bindings[ordinal].push(evidence_id);
    }
    if criterion_bindings.iter().any(Vec::is_empty) {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("Graph finish does not cover every frozen criterion".to_owned()),
            ..Default::default()
        });
    }
    let attempt_evidence = evidence_ids.iter().collect::<HashSet<_>>();
    let mut bound_evidence_ids = Vec::new();
    let mut returned = HashSet::new();
    for evidence_id in criterion_bindings.iter().flatten() {
        if !attempt_evidence.contains(evidence_id) {
            return Ok(AcceptedAttemptProjection {
                attempt_id: Some(attempt_id),
                blocker: Some(format!(
                    "criterion Evidence '{evidence_id}' is not frozen on the successful Attempt"
                )),
                ..Default::default()
            });
        }
        match validate_graph_criterion_evidence(
            connection,
            &node.task_id,
            &parent_run_id,
            &conversation_id,
            evidence_rowid_watermark,
            attempt_started_at,
            evidence_id,
        ) {
            Ok(()) => {}
            Err(error @ RepositoryError::Database { .. }) => return Err(error),
            Err(error) => {
                return Ok(AcceptedAttemptProjection {
                    attempt_id: Some(attempt_id),
                    blocker: Some(error.to_string()),
                    ..Default::default()
                })
            }
        }
        if returned.insert(evidence_id.clone()) {
            bound_evidence_ids.push(evidence_id.clone());
        }
    }
    let blockers = connection
        .query_row(
            "SELECT COUNT(*) FROM review_findings findings
             JOIN work_tasks tasks ON tasks.goal_id = findings.goal_id
             WHERE tasks.id = ?1
               AND (findings.task_id = tasks.id OR findings.task_id IS NULL)
               AND findings.status = 'open'
               AND findings.severity IN ('critical', 'high', 'medium')",
            [node.task_id.as_str()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(database_error)?;
    if blockers > 0 {
        return Ok(AcceptedAttemptProjection {
            attempt_id: Some(attempt_id),
            blocker: Some("open high-risk Review Findings invalidate node acceptance".to_owned()),
            ..Default::default()
        });
    }
    Ok(AcceptedAttemptProjection {
        accepted: true,
        attempt_id: Some(attempt_id),
        evidence_ids: bound_evidence_ids,
        blocker: None,
    })
}

fn validate_identifier(label: &str, value: &str) -> Result<(), RepositoryError> {
    bounded_non_empty(value, label, 200).map(|_| ())
}

fn validate_node_key(value: &str) -> Result<(), RepositoryError> {
    let value = bounded_non_empty(value, "Graph node key", MAX_NODE_KEY_CHARS)?;
    if !value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || "._-".contains(character))
    {
        return Err(RepositoryError::InvalidInput(format!(
            "Graph node key '{value}' contains unsupported characters"
        )));
    }
    Ok(())
}

fn bounded_required_string(
    value: Option<&Value>,
    label: &str,
    maximum_chars: usize,
) -> Result<String, RepositoryError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or_else(|| RepositoryError::InvalidInput(format!("{label} must be a string")))?;
    bounded_non_empty(value, label, maximum_chars)
}

fn bounded_optional_string(
    value: Option<&Value>,
    label: &str,
    maximum_chars: usize,
) -> Result<Option<String>, RepositoryError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => bounded_non_empty(value, label, maximum_chars).map(Some),
        Some(_) => Err(RepositoryError::InvalidInput(format!(
            "{label} must be a string or null"
        ))),
    }
}

fn bounded_non_empty(
    value: &str,
    label: &str,
    maximum_chars: usize,
) -> Result<String, RepositoryError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} cannot be empty"
        )));
    }
    if trimmed.chars().count() > maximum_chars {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} exceeds {maximum_chars} characters"
        )));
    }
    Ok(trimmed.to_owned())
}

fn string_array(
    value: Option<&Value>,
    label: &str,
    maximum_items: usize,
) -> Result<Vec<String>, RepositoryError> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .ok_or_else(|| RepositoryError::InvalidInput(format!("{label} must be an array")))?;
    if values.len() > maximum_items {
        return Err(RepositoryError::InvalidInput(format!(
            "{label} cannot contain more than {maximum_items} items"
        )));
    }
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                RepositoryError::InvalidInput(format!("{label} must contain only strings"))
            })
        })
        .collect()
}

fn with_graph_read_transaction<T>(
    database: &Database,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, RepositoryError> {
    let mut connection = database
        .connection
        .lock()
        .map_err(|_| RepositoryError::Database {
            message: "database lock is poisoned".to_owned(),
            busy: false,
        })?;
    let transaction = connection.transaction().map_err(database_error)?;
    let value = operation(&transaction)?;
    transaction.commit().map_err(database_error)?;
    Ok(value)
}

fn database_error(error: rusqlite::Error) -> RepositoryError {
    let busy = matches!(
        &error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    );
    RepositoryError::Database {
        message: error.to_string(),
        busy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{
        AddEvidenceInput, CreateGoalInput, CreateTaskInput, EvidenceReferenceKind, EvidenceType,
        EvidenceValidityStatus, FinishTaskAttemptInput, GoalStatus, StartRunResult,
        StartTaskAttemptInput, TaskAttemptKind, TaskAttemptStatus, TaskEvidenceRecord,
        WorkTaskStatus,
    };
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use uuid::Uuid;

    struct Fixture {
        database: Database,
        path: PathBuf,
        conversation_id: String,
        run: StartRunResult,
        goal_id: String,
        plan_revision_id: String,
        activation_tool_call_id: String,
    }

    fn fixture(plan_tasks: Value, profile_id: &str) -> Fixture {
        fixture_with_policy(plan_tasks, profile_id, "standard_v1")
    }

    fn fixture_with_policy(
        plan_tasks: Value,
        profile_id: &str,
        validation_policy_id: &str,
    ) -> Fixture {
        let path = std::env::temp_dir().join(format!("fox-graph-lead-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open graph test database");
        let conversation = database
            .create_conversation(
                "fox-general",
                Some("Graph Lead"),
                Some("D:/workspace"),
                None,
            )
            .expect("create conversation");
        let run = database
            .create_run(
                &conversation.id,
                "Activate the approved read-only graph",
                None,
            )
            .expect("create parent run");
        database
            .apply_runtime_event(&run.run.id, 1, &json!({ "type": "run.started" }))
            .expect("start parent run");
        database
            .freeze_run_execution_profile(&run.run.id, profile_id)
            .expect("freeze execution profile");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "Readonly Graph".to_owned(),
                objective: "Execute a bounded read-only task DAG".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run.run.id.clone(),
            })
            .expect("create graph goal");
        let plan_nodes = plan_tasks.as_array().expect("plan tasks array");
        let tasks = plan_nodes
            .iter()
            .map(|node| CreateTaskInput {
                id: None,
                goal_id: goal.id.clone(),
                parent_task_id: None,
                ordinal: node["ordinal"].as_i64().expect("ordinal"),
                title: node["title"].as_str().expect("title").to_owned(),
                detail: node
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            })
            .collect::<Vec<_>>();
        database
            .work_tasks()
            .create_many_with_policy_ids(
                tasks,
                vec![validation_policy_id.to_owned(); plan_nodes.len()],
            )
            .expect("create durable graph tasks");
        let plan = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "Readonly Graph plan",
                "A bounded immutable DAG",
                plan_tasks,
                &run.run.id,
            )
            .expect("create graph plan revision");
        database
            .resolve_plan_revision(&conversation.id, &plan.id, "approved")
            .expect("approve graph plan revision");
        let activation_tool_call_id = database
            .create_host_tool_call(
                &run.run.id,
                &format!("graph-activation-{}", Uuid::new_v4()),
                "graph_readonly_activate",
                &json!({ "planRevisionId": plan.id }),
                "running",
                false,
            )
            .expect("create activation Host ToolCall")
            .id;
        Fixture {
            database,
            path,
            conversation_id: conversation.id,
            run,
            goal_id: goal.id,
            plan_revision_id: plan.id,
            activation_tool_call_id,
        }
    }

    fn activate(fixture: &Fixture) -> Result<GraphLeadActivationResult, RepositoryError> {
        activate_with_tool_call(fixture, &fixture.activation_tool_call_id)
    }

    fn activate_with_tool_call(
        fixture: &Fixture,
        tool_call_id: &str,
    ) -> Result<GraphLeadActivationResult, RepositoryError> {
        fixture
            .database
            .activate_read_only_graph(ActivateReadOnlyGraphInput {
                plan_revision_id: fixture.plan_revision_id.clone(),
                parent_run_id: fixture.run.run.id.clone(),
                activation_tool_call_id: tool_call_id.to_owned(),
            })
    }

    fn create_activation_tool_call(fixture: &Fixture, runtime_tool_call_id: &str) -> String {
        fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                runtime_tool_call_id,
                "graph_readonly_activate",
                &json!({ "planRevisionId": fixture.plan_revision_id }),
                "running",
                false,
            )
            .expect("create activation Host ToolCall")
            .id
    }

    fn graph_child_budget() -> ChildRunBudget {
        ChildRunBudget {
            max_duration_ms: 45_000,
            max_total_tokens: 4_096,
            max_output_tokens: 1_024,
            max_tool_calls: 6,
        }
    }

    fn create_running_lead_run(
        database: &Database,
        conversation_id: &str,
        profile_id: &str,
    ) -> StartRunResult {
        let run = database
            .create_run(
                conversation_id,
                "Continue the durable read-only Graph",
                None,
            )
            .expect("create continuing Lead Run");
        database
            .apply_runtime_event(&run.run.id, 1, &json!({ "type": "run.started" }))
            .expect("start continuing Lead Run");
        database
            .freeze_run_execution_profile(&run.run.id, profile_id)
            .expect("freeze continuing Lead profile");
        run
    }

    fn create_node_start_input(
        fixture: &Fixture,
        node: &GraphLeadNodeSnapshot,
        parent_run_id: &str,
        runtime_tool_call_id: &str,
        attempt_id: &str,
    ) -> StartReadyReadOnlyGraphNodeInput {
        create_node_start_input_with_budget(
            fixture,
            node,
            parent_run_id,
            runtime_tool_call_id,
            attempt_id,
            graph_child_budget(),
        )
    }

    fn create_node_start_input_with_budget(
        fixture: &Fixture,
        node: &GraphLeadNodeSnapshot,
        parent_run_id: &str,
        runtime_tool_call_id: &str,
        attempt_id: &str,
        budget: ChildRunBudget,
    ) -> StartReadyReadOnlyGraphNodeInput {
        let mut input = StartReadyReadOnlyGraphNodeInput {
            goal_id: fixture.goal_id.clone(),
            task_id: node.task_id.clone(),
            parent_run_id: parent_run_id.to_owned(),
            node_start_tool_call_id: "pending-tool-call-id".to_owned(),
            expected_task_version: 1,
            attempt_id: attempt_id.to_owned(),
            worker_agent_id: "fox-general".to_owned(),
            objective: format!("Execute Graph node {} read-only", node.node_key),
            context: format!("Goal {} / Task {}", fixture.goal_id, node.task_id),
            budget,
        };
        input.node_start_tool_call_id = fixture
            .database
            .create_host_tool_call(
                parent_run_id,
                runtime_tool_call_id,
                GRAPH_NODE_START_TOOL,
                &node_start_tool_input(&input),
                "running",
                false,
            )
            .expect("create Graph node-start Host ToolCall")
            .id;
        input
    }

    fn cleanup(fixture: Fixture) {
        let path = fixture.path.clone();
        drop(fixture);
        let _ = std::fs::remove_file(path);
    }

    fn complete_graph_child(fixture: &Fixture, child_run_id: &str, result: Option<&str>) {
        fixture
            .database
            .apply_runtime_event(child_run_id, 1, &json!({ "type": "run.started" }))
            .expect("start Graph Child Run");
        if let Some(result) = result {
            fixture
                .database
                .apply_runtime_event(child_run_id, 2, &json!({ "type": "message.started" }))
                .expect("start Child assistant message");
            fixture
                .database
                .apply_runtime_event(
                    child_run_id,
                    3,
                    &json!({ "type": "message.delta", "delta": result }),
                )
                .expect("persist Child result");
            fixture
                .database
                .apply_runtime_event(child_run_id, 4, &json!({ "type": "message.completed" }))
                .expect("complete Child assistant message");
            fixture
                .database
                .apply_runtime_event(child_run_id, 5, &json!({ "type": "run.completed" }))
                .expect("complete Graph Child Run");
        } else {
            fixture
                .database
                .apply_runtime_event(child_run_id, 2, &json!({ "type": "run.completed" }))
                .expect("complete empty Graph Child Run");
        }
    }

    fn add_graph_host_evidence(
        fixture: &Fixture,
        task_id: &str,
        suffix: &str,
        tool_name: &str,
        requires_approval: bool,
        evidence_type: EvidenceType,
        check_type: &str,
    ) -> TaskEvidenceRecord {
        thread::sleep(std::time::Duration::from_millis(2));
        let runtime_tool_call_id = format!("graph-proof-{suffix}");
        let tool_call = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                &runtime_tool_call_id,
                tool_name,
                &json!({ "proof": suffix }),
                if requires_approval {
                    "pending"
                } else {
                    "running"
                },
                requires_approval,
            )
            .expect("create Graph proof Host ToolCall");
        if requires_approval {
            fixture
                .database
                .with_connection(|connection| {
                    connection.execute(
                        "UPDATE tool_calls SET status = 'running' WHERE id = ?1 AND status = 'pending'",
                        [tool_call.id.as_str()],
                    )?;
                    Ok(())
                })
                .expect("simulate completed Host approval for proof ToolCall");
        }
        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                &runtime_tool_call_id,
                Some(&json!({ "passed": true, "status": "passed" })),
                None,
            )
            .expect("complete Graph proof Host ToolCall");
        let evidence = fixture
            .database
            .task_evidence()
            .add(AddEvidenceInput {
                id: None,
                task_id: task_id.to_owned(),
                source_run_id: Some(fixture.run.run.id.clone()),
                evidence_type,
                ref_kind: EvidenceReferenceKind::ToolCall,
                ref_id: tool_call.id,
                summary: format!("Graph {check_type} proof passed"),
                metadata: json!({ "validationCheckType": check_type, "passed": true }),
                trace_id: None,
                span_id: None,
            })
            .expect("add Graph criterion Evidence");
        assert_eq!(
            fixture
                .database
                .task_evidence()
                .validate(&evidence.id)
                .expect("validate Graph criterion Evidence"),
            EvidenceValidityStatus::Valid
        );
        evidence
    }

    fn add_graph_runtime_read_evidence(
        fixture: &Fixture,
        task_id: &str,
        suffix: &str,
    ) -> TaskEvidenceRecord {
        thread::sleep(std::time::Duration::from_millis(2));
        let next_seq = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COALESCE(MAX(seq), 0) + 1 FROM run_events WHERE run_id = ?1",
                    [fixture.run.run.id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("next parent Runtime sequence");
        let runtime_tool_call_id = format!("graph-runtime-read-{suffix}");
        fixture
            .database
            .apply_runtime_event(
                &fixture.run.run.id,
                next_seq,
                &json!({
                    "type": "tool.started",
                    "toolCallId": runtime_tool_call_id,
                    "tool": "read",
                    "input": { "path": format!("{suffix}.txt") },
                }),
            )
            .expect("start runtime read proof");
        fixture
            .database
            .apply_runtime_event(
                &fixture.run.run.id,
                next_seq + 1,
                &json!({
                    "type": "tool.completed",
                    "toolCallId": runtime_tool_call_id,
                    "tool": "read",
                    "result": { "content": "inspected" },
                    "isError": false,
                }),
            )
            .expect("complete runtime read proof");
        let tool_call_id = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT id FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                    params![fixture.run.run.id, runtime_tool_call_id],
                    |row| row.get::<_, String>(0),
                )
            })
            .expect("load runtime read ToolCall");
        let evidence = fixture
            .database
            .task_evidence()
            .add(AddEvidenceInput {
                id: None,
                task_id: task_id.to_owned(),
                source_run_id: Some(fixture.run.run.id.clone()),
                evidence_type: EvidenceType::ToolCall,
                ref_kind: EvidenceReferenceKind::ToolCall,
                ref_id: tool_call_id,
                summary: "Parent Lead runtime read inspected the Child result".to_owned(),
                metadata: json!({ "validationCheckType": "inspection" }),
                trace_id: None,
                span_id: None,
            })
            .expect("add runtime read Evidence");
        assert_eq!(
            fixture
                .database
                .task_evidence()
                .validate(&evidence.id)
                .unwrap(),
            EvidenceValidityStatus::Valid
        );
        evidence
    }

    fn create_node_finish_input(
        fixture: &Fixture,
        started: &GraphLeadNodeStartResult,
        runtime_tool_call_id: &str,
        criterion_evidence: Vec<GraphCriterionEvidenceInput>,
    ) -> FinishReadOnlyGraphNodeInput {
        let mut input = FinishReadOnlyGraphNodeInput {
            goal_id: fixture.goal_id.clone(),
            task_id: started.task.id.clone(),
            attempt_id: started.attempt.id.clone(),
            parent_run_id: fixture.run.run.id.clone(),
            node_finish_tool_call_id: "pending-node-finish-tool-call".to_owned(),
            expected_task_version: started.task.version,
            expected_attempt_version: started.attempt.version,
            criterion_evidence,
            summary: "Parent Lead verified every frozen criterion against live Evidence".to_owned(),
        };
        input.node_finish_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                runtime_tool_call_id,
                GRAPH_NODE_FINISH_TOOL,
                &node_finish_tool_input(&input),
                "running",
                false,
            )
            .expect("create Graph node-finish Host ToolCall")
            .id;
        input
    }

    fn create_node_cancel_input(
        fixture: &Fixture,
        started: &GraphLeadNodeStartResult,
        runtime_tool_call_id: &str,
        reason: &str,
    ) -> CreateGraphNodeCancelIntentInput {
        let mut input = CreateGraphNodeCancelIntentInput {
            goal_id: fixture.goal_id.clone(),
            task_id: started.task.id.clone(),
            attempt_id: started.attempt.id.clone(),
            parent_run_id: fixture.run.run.id.clone(),
            node_cancel_tool_call_id: "pending-node-cancel-tool-call".to_owned(),
            expected_task_version: started.task.version,
            expected_attempt_version: started.attempt.version,
            reason: reason.to_owned(),
        };
        input.node_cancel_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                runtime_tool_call_id,
                GRAPH_NODE_CANCEL_TOOL,
                &node_cancel_tool_input(&input),
                "running",
                false,
            )
            .expect("create Graph node-cancel Host ToolCall")
            .id;
        input
    }

    fn start_single_graph_node(fixture: &Fixture, suffix: &str) -> GraphLeadNodeStartResult {
        let activated = activate(fixture).expect("activate Graph");
        let start_input = create_node_start_input(
            fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            &format!("cancel-{suffix}-start"),
            &format!("cancel-{suffix}-attempt"),
        );
        fixture
            .database
            .start_ready_read_only_graph_node(start_input)
            .expect("start Graph node")
    }

    #[test]
    fn graph_cancel_intent_is_pending_until_completed_tool_call_and_exactly_once_active() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected"] }
            ]),
            "durable_v2",
        );
        let started = start_single_graph_node(&fixture, "intent");
        let input = create_node_cancel_input(
            &fixture,
            &started,
            "cancel-intent",
            "User no longer needs this read-only node",
        );
        let created = fixture
            .database
            .create_graph_node_cancel_intent(input.clone())
            .expect("create pending cancel intent");
        assert!(created.created);
        assert!(created.pending_activation);

        let before_finalize = fixture
            .database
            .activate_graph_node_cancel_intent(&created.intent_id)
            .expect("pre-finalizer activation is a stable no-op");
        assert!(!before_finalize.activated);
        assert!(before_finalize.stale);

        let replayed = fixture
            .database
            .create_graph_node_cancel_intent(input.clone())
            .expect("exact pending replay");
        assert!(replayed.replayed);
        assert!(replayed.pending_activation);
        assert_eq!(replayed.intent_id, created.intent_id);

        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                "cancel-intent",
                Some(&json!({ "pendingActivation": true })),
                None,
            )
            .expect("complete cancel ToolCall through common finalizer");
        let activated = fixture
            .database
            .activate_graph_node_cancel_intent(&created.intent_id)
            .expect("activate durable cancel intent");
        assert!(activated.activated);
        assert!(!activated.stale);
        assert_eq!(
            fixture
                .database
                .active_graph_node_cancel_intents_for_recovery()
                .unwrap()
                .len(),
            1
        );
        let exact_activation_replay = fixture
            .database
            .activate_graph_node_cancel_intent(&created.intent_id)
            .expect("exact activation replay");
        assert!(!exact_activation_replay.activated);
        assert!(!exact_activation_replay.stale);

        let mut changed = input;
        changed.reason = "Different cancel reason".to_owned();
        assert!(matches!(
            fixture.database.create_graph_node_cancel_intent(changed),
            Err(RepositoryError::ConstraintViolation(_))
        ));
        // Once the parent is Kernel-owned, legacy recovery must not redispatch
        // even this already-activated intent. The owning Host action journal
        // is the sole recovery path, including uncertain claimed actions.
        fixture.database.freeze_kernel_run_control(
            &fixture.run.run.id, "durable_v2", fox_engine_protocol::TimeBudgets::default(),
        ).unwrap();
        assert!(fixture.database.active_graph_node_cancel_intents_for_recovery().unwrap().is_empty());
        cleanup(fixture);
    }

    #[test]
    fn failed_cancel_finalizer_is_inert_and_completed_child_wins_race() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected"] }
            ]),
            "durable_v2",
        );
        let started = start_single_graph_node(&fixture, "failed-finalizer");
        let input = create_node_cancel_input(
            &fixture,
            &started,
            "cancel-failed-finalizer",
            "Cancel after bounded validation",
        );
        let pending = fixture
            .database
            .create_graph_node_cancel_intent(input)
            .expect("create pending cancel intent");
        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                "cancel-failed-finalizer",
                None,
                Some("common finalizer rejected the ToolCall"),
            )
            .expect("persist failed common finalizer");
        let failed_activation = fixture
            .database
            .activate_graph_node_cancel_intent(&pending.intent_id)
            .expect("failed finalizer cannot activate");
        assert!(!failed_activation.activated);
        assert!(failed_activation.stale);
        assert!(fixture
            .database
            .active_graph_node_cancel_intents_for_recovery()
            .unwrap()
            .is_empty());

        for status in [TaskAttemptStatus::Failed, TaskAttemptStatus::Cancelled] {
            let bypass = fixture
                .database
                .finish_task_attempt(FinishTaskAttemptInput {
                    id: started.attempt.id.clone(),
                    task_id: started.task.id.clone(),
                    run_id: fixture.run.run.id.clone(),
                    expected_task_version: started.task.version,
                    expected_attempt_version: started.attempt.version,
                    status,
                    failure_reason: Some("model-controlled terminal".to_owned()),
                })
                .expect_err("generic Graph negative finish must fail closed");
            assert!(bypass
                .to_string()
                .contains("graph.readonly_dedicated_terminal_required"));
        }

        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("Child completed before cancellation became authoritative"),
        );
        let attempt = fixture
            .database
            .list_task_attempts(&started.task.id, 10)
            .unwrap()
            .into_iter()
            .find(|attempt| attempt.id == started.attempt.id)
            .expect("Graph Attempt remains available for finish");
        assert_eq!(attempt.status, TaskAttemptStatus::Running);
        let reconciliation_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM work_graph_node_terminal_reconciliations WHERE attempt_id = ?1",
                    [started.attempt.id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(reconciliation_count, 0);
        cleanup(fixture);
    }

    #[test]
    fn graph_child_negative_terminals_reconcile_once_using_actual_status() {
        for (suffix, terminal_event, expected_attempt_status) in [
            ("failed", "run.failed", TaskAttemptStatus::Failed),
            ("cancelled", "run.cancelled", TaskAttemptStatus::Cancelled),
            ("interrupted", "run.interrupted", TaskAttemptStatus::Failed),
        ] {
            let fixture = fixture(
                json!([
                    { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected"] }
                ]),
                "durable_v2",
            );
            let started = start_single_graph_node(&fixture, suffix);
            fixture
                .database
                .apply_runtime_event(
                    &started.child_run.child_run_id,
                    1,
                    &json!({ "type": "run.started" }),
                )
                .expect("start Graph Child");
            fixture
                .database
                .apply_runtime_event(
                    &started.child_run.child_run_id,
                    2,
                    &json!({
                        "type": terminal_event,
                        "code": format!("child.{suffix}"),
                        "message": format!("Child ended as {suffix}"),
                    }),
                )
                .expect("project authoritative Child terminal");
            let attempt = fixture
                .database
                .list_task_attempts(&started.task.id, 10)
                .unwrap()
                .into_iter()
                .find(|attempt| attempt.id == started.attempt.id)
                .expect("reconciled Graph Attempt");
            let task = fixture
                .database
                .work_tasks()
                .get(&started.task.id)
                .unwrap()
                .expect("reconciled Graph Task");
            assert_eq!(attempt.status, expected_attempt_status, "{suffix}");
            assert_eq!(task.status, WorkTaskStatus::Interrupted, "{suffix}");
            let (child_status, attempt_status, terminal_json, terminal_hash): (
                String,
                String,
                String,
                String,
            ) = fixture
                .database
                .with_connection(|connection| {
                    connection.query_row(
                        "SELECT child_terminal_status, attempt_terminal_status,
                                child_terminal_json, child_terminal_hash
                         FROM work_graph_node_terminal_reconciliations WHERE attempt_id = ?1",
                        [started.attempt.id.as_str()],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                })
                .expect("load immutable reconciliation");
            assert_eq!(child_status, suffix);
            let expected_attempt_status_text = match expected_attempt_status {
                TaskAttemptStatus::Cancelled => "cancelled",
                TaskAttemptStatus::Failed => "failed",
                _ => unreachable!("negative reconciliation has a bounded terminal status"),
            };
            assert_eq!(attempt_status, expected_attempt_status_text);
            assert_eq!(terminal_hash, sha256_hex(terminal_json.as_bytes()));
            assert!(!fixture
                .database
                .apply_runtime_event(
                    &started.child_run.child_run_id,
                    2,
                    &json!({
                        "type": terminal_event,
                        "code": format!("child.{suffix}"),
                        "message": format!("Child ended as {suffix}"),
                    }),
                )
                .expect("exact terminal event replay"));
            cleanup(fixture);
        }
    }

    #[test]
    fn startup_repair_closes_crash_active_graph_child_before_owner_audit_returns() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected"] }
            ]),
            "durable_v2",
        );
        let started = start_single_graph_node(&fixture, "startup-crash");
        fixture
            .database
            .apply_runtime_event(
                &started.child_run.child_run_id,
                1,
                &json!({ "type": "run.started" }),
            )
            .expect("start Graph Child before simulated crash");

        fixture
            .database
            .repair_interrupted_runs()
            .expect("startup repair is self-contained");

        assert_eq!(
            fixture
                .database
                .child_run_status(&started.child_run.child_run_id)
                .unwrap()
                .as_deref(),
            Some("interrupted")
        );
        let attempt = fixture
            .database
            .list_task_attempts(&started.task.id, 10)
            .unwrap()
            .into_iter()
            .find(|attempt| attempt.id == started.attempt.id)
            .expect("startup-repaired Graph Attempt");
        let task = fixture
            .database
            .work_tasks()
            .get(&started.task.id)
            .unwrap()
            .expect("startup-repaired Graph Task");
        assert_eq!(attempt.status, TaskAttemptStatus::Failed);
        assert_eq!(task.status, WorkTaskStatus::Interrupted);
        cleanup(fixture);
    }

    #[test]
    fn graph_cancel_rejects_high_risk_and_stale_cas_without_intent() {
        let fixture = fixture_with_policy(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected"] }
            ]),
            "durable_v2",
            "high_risk_v1",
        );
        let started = start_single_graph_node(&fixture, "high-risk");
        let high_risk = create_node_cancel_input(
            &fixture,
            &started,
            "cancel-high-risk",
            "Cancel high risk node",
        );
        assert!(matches!(
            fixture.database.create_graph_node_cancel_intent(high_risk),
            Err(RepositoryError::InvalidValidationPolicy(_))
        ));

        let stale = create_node_cancel_input(
            &fixture,
            &started,
            "cancel-stale-cas",
            "Cancel with stale version",
        );
        let mut stale = stale;
        stale.expected_attempt_version += 1;
        assert!(matches!(
            fixture.database.create_graph_node_cancel_intent(stale),
            Err(RepositoryError::ConstraintViolation(_))
                | Err(RepositoryError::OptimisticLockFailed { .. })
        ));
        let intent_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM work_graph_node_cancel_intents",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(intent_count, 0);
        cleanup(fixture);
    }

    #[test]
    fn activates_branching_read_only_graph_and_projects_stable_readiness() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["non-empty report"] },
                { "nodeKey": "left", "title": "Left", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["left result inspected"] },
                { "nodeKey": "right", "title": "Right", "ordinal": 2, "dependsOn": ["root"], "acceptanceCriteria": ["right result inspected"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        assert!(!activated.replayed);
        assert_eq!(activated.snapshot.graph_id, fixture.goal_id);
        assert_eq!(activated.snapshot.nodes.len(), 3);
        assert_eq!(activated.snapshot.edges.len(), 2);
        assert_eq!(
            activated.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Runnable
        );
        assert_eq!(
            activated.snapshot.nodes[1].readiness,
            GraphLeadNodeReadiness::Waiting
        );
        assert_eq!(
            activated.snapshot.nodes[2].readiness,
            GraphLeadNodeReadiness::Waiting
        );
        assert_eq!(activated.snapshot.nodes[1].depth, 1);
        cleanup(fixture);
    }

    #[test]
    fn standard_graph_finish_binds_live_runtime_evidence_replays_and_unlocks_dependency() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["report inspected", "scope inspected"] },
                { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate Graph");
        let start_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "finish-root-start",
            "finish-root-attempt",
        );
        let started = fixture
            .database
            .start_ready_read_only_graph_node(start_input)
            .expect("start root node");
        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("bounded read-only report"),
        );
        let evidence = add_graph_runtime_read_evidence(&fixture, &started.task.id, "finish-root");
        let finish_input = create_node_finish_input(
            &fixture,
            &started,
            "finish-root-call",
            vec![
                GraphCriterionEvidenceInput {
                    criterion: "report inspected".to_owned(),
                    evidence_ids: vec![evidence.id.clone()],
                },
                GraphCriterionEvidenceInput {
                    criterion: "scope inspected".to_owned(),
                    evidence_ids: vec![evidence.id.clone()],
                },
            ],
        );
        let finished = fixture
            .database
            .finish_read_only_graph_node(finish_input.clone())
            .expect("finish standard Graph node");
        assert!(!finished.replayed);
        assert_eq!(finished.finish.task.status, WorkTaskStatus::Completed);
        assert_eq!(finished.finish.attempt.status, TaskAttemptStatus::Succeeded);
        assert_eq!(
            finished.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );
        assert_eq!(
            finished.snapshot.nodes[1].readiness,
            GraphLeadNodeReadiness::Runnable
        );
        assert_eq!(
            finished.snapshot.nodes[0].evidence_ids,
            vec![evidence.id.clone()]
        );
        let replayed = fixture
            .database
            .finish_read_only_graph_node(finish_input)
            .expect("exact Repository replay");
        assert!(replayed.replayed);
        assert_eq!(replayed.child_result_hash, finished.child_result_hash);
        let counts = fixture
            .database
            .with_connection(|connection| {
                Ok::<_, rusqlite::Error>((
                    connection.query_row(
                        "SELECT COUNT(*) FROM work_graph_node_finishes WHERE attempt_id = ?1",
                        [started.attempt.id.as_str()],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM work_graph_node_criterion_evidence",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row("SELECT COUNT(*) FROM acceptances", [], |row| {
                        row.get::<_, i64>(0)
                    })?,
                ))
            })
            .expect("count immutable finish facts");
        assert_eq!(counts, (1, 2, 0));
        let path = fixture.path.clone();
        let goal_id = fixture.goal_id.clone();
        drop(fixture);
        let reopened = Database::open(path.clone()).expect("reopen completed Graph database");
        let reopened_snapshot = reopened
            .read_only_graph_snapshot(&goal_id)
            .expect("read completed Graph after restart")
            .expect("persisted Graph after restart");
        assert_eq!(
            reopened_snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );
        assert_eq!(
            reopened_snapshot.nodes[1].readiness,
            GraphLeadNodeReadiness::Runnable
        );
        reopened
            .task_evidence()
            .mark_stale(&evidence.id, "source changed after restart".to_owned())
            .expect("mark bound Graph Evidence stale");
        let stale_snapshot = reopened
            .read_only_graph_snapshot(&goal_id)
            .expect("recompute Graph readiness after Evidence becomes stale")
            .expect("persisted Graph after Evidence becomes stale");
        assert_eq!(
            stale_snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Blocked
        );
        assert_eq!(
            stale_snapshot.nodes[1].readiness,
            GraphLeadNodeReadiness::Waiting
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn graph_finish_accepts_passing_test_criterion_but_still_requires_fresh_inspection() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["tests pass"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).unwrap();
        let started = fixture
            .database
            .start_ready_read_only_graph_node(create_node_start_input(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                "test-proof-start",
                "test-proof-attempt",
            ))
            .unwrap();
        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("testable result"),
        );
        let _inspection =
            add_graph_runtime_read_evidence(&fixture, &started.task.id, "test-proof-inspection");
        let test_evidence = add_graph_host_evidence(
            &fixture,
            &started.task.id,
            "test-proof",
            "test_run",
            true,
            EvidenceType::TestResult,
            "test",
        );
        let finished = fixture
            .database
            .finish_read_only_graph_node(create_node_finish_input(
                &fixture,
                &started,
                "test-proof-finish",
                vec![GraphCriterionEvidenceInput {
                    criterion: "tests pass".to_owned(),
                    evidence_ids: vec![test_evidence.id],
                }],
            ))
            .expect("passing test Evidence may cover a criterion");
        assert_eq!(
            finished.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );
        cleanup(fixture);
    }

    #[test]
    fn graph_finish_rejects_high_risk_empty_child_and_blocked_goal_without_writes() {
        for case in ["high-risk", "empty-child", "blocked-goal"] {
            let policy = if case == "high-risk" {
                "high_risk_v1"
            } else {
                "standard_v1"
            };
            let fixture = fixture_with_policy(
                json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
                "durable_v2",
                policy,
            );
            let activated = activate(&fixture).unwrap();
            let started = fixture
                .database
                .start_ready_read_only_graph_node(create_node_start_input(
                    &fixture,
                    &activated.snapshot.nodes[0],
                    &fixture.run.run.id,
                    &format!("{case}-start"),
                    &format!("{case}-attempt"),
                ))
                .unwrap();
            complete_graph_child(
                &fixture,
                &started.child_run.child_run_id,
                (case != "empty-child").then_some("non-empty child result"),
            );
            let evidence = add_graph_runtime_read_evidence(
                &fixture,
                &started.task.id,
                &format!("{case}-evidence"),
            );
            let finish_input = create_node_finish_input(
                &fixture,
                &started,
                &format!("{case}-finish"),
                vec![GraphCriterionEvidenceInput {
                    criterion: "result inspected".to_owned(),
                    evidence_ids: vec![evidence.id],
                }],
            );
            if case == "blocked-goal" {
                fixture
                    .database
                    .with_connection(|connection| {
                        connection.execute(
                            "UPDATE goals SET status = 'blocked', blocked_reason = 'pause' WHERE id = ?1",
                            [fixture.goal_id.as_str()],
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
            let error = fixture
                .database
                .finish_read_only_graph_node(finish_input)
                .expect_err("closed finish case must fail");
            match case {
                "high-risk" => {
                    assert!(error.to_string().contains("graph.readonly_review_required"))
                }
                "empty-child" => assert!(error
                    .to_string()
                    .contains("graph.readonly_child_result_required")),
                "blocked-goal" => {
                    assert!(matches!(error, RepositoryError::InvalidTransition { .. }))
                }
                _ => unreachable!(),
            }
            let facts = fixture
                .database
                .with_connection(|connection| {
                    Ok::<_, rusqlite::Error>((
                        connection.query_row(
                            "SELECT COUNT(*) FROM work_graph_node_finishes",
                            [],
                            |row| row.get::<_, i64>(0),
                        )?,
                        connection.query_row(
                            "SELECT COUNT(*) FROM work_graph_node_criterion_evidence",
                            [],
                            |row| row.get::<_, i64>(0),
                        )?,
                        connection.query_row(
                            "SELECT version FROM task_attempts WHERE id = ?1",
                            [started.attempt.id.as_str()],
                            |row| row.get::<_, i64>(0),
                        )?,
                    ))
                })
                .unwrap();
            assert_eq!(facts, (0, 0, 1));
            cleanup(fixture);
        }
    }

    #[test]
    fn high_risk_graph_review_finish_and_final_acceptance_complete_end_to_end() {
        let fixture = fixture_with_policy(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
            "high_risk_v1",
        );
        let activated = activate(&fixture).unwrap();
        let started = fixture
            .database
            .start_ready_read_only_graph_node(create_node_start_input(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                "reviewed-start",
                "reviewed-attempt",
            ))
            .unwrap();
        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("implementation result"),
        );
        let evidence =
            add_graph_runtime_read_evidence(&fixture, &started.task.id, "reviewed-parent-proof");
        let criterion_evidence = vec![GraphCriterionEvidenceInput {
            criterion: "result inspected".to_owned(),
            evidence_ids: vec![evidence.id],
        }];
        let finish_input = create_node_finish_input(
            &fixture,
            &started,
            "reviewed-finish",
            criterion_evidence.clone(),
        );
        let mut review_input = CreateGraphNodeReviewRequestInput {
            goal_id: fixture.goal_id.clone(),
            task_id: started.task.id.clone(),
            attempt_id: started.attempt.id.clone(),
            parent_run_id: fixture.run.run.id.clone(),
            review_tool_call_id: "pending-review-call".to_owned(),
            expected_task_version: started.task.version,
            expected_attempt_version: started.attempt.version,
            criterion_evidence,
            summary: finish_input.summary.clone(),
        };
        review_input.review_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                "reviewed-review",
                GRAPH_NODE_REVIEW_TOOL,
                &node_review_tool_input(&review_input),
                "running",
                false,
            )
            .unwrap()
            .id;
        let request = fixture
            .database
            .create_graph_node_review_request(review_input)
            .expect("persist pending Graph review");
        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                "reviewed-review",
                Some(&json!({ "pendingActivation": true })),
                None,
            )
            .unwrap();
        assert!(
            fixture
                .database
                .activate_graph_node_review_request(&request.request_id)
                .unwrap()
                .activated
        );
        let reviewer = fixture
            .database
            .dispatch_graph_node_reviewer(&request.request_id, "fox-general")
            .expect("dispatch independent Reviewer");
        thread::sleep(std::time::Duration::from_millis(2));
        let reviewer_run = reviewer.child_run.child_run_id;
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 1, &json!({ "type": "run.started" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                2,
                &json!({
                    "type": "tool.started",
                    "toolCallId": "reviewer-runtime-read",
                    "tool": "read",
                    "input": { "path": "result.txt" },
                }),
            )
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                3,
                &json!({
                    "type": "tool.completed",
                    "toolCallId": "reviewer-runtime-read",
                    "tool": "read",
                    "result": { "content": "independently inspected" },
                    "isError": false,
                }),
            )
            .unwrap();
        thread::sleep(std::time::Duration::from_millis(2));
        let reviewer_result = serde_json::to_string(&json!({
            "criteria": [{
                "criterion": "result inspected",
                "status": "passed",
                "toolCallIds": ["reviewer-runtime-read"],
            }],
            "findings": [],
            "recommendation": "pass",
            "summary": "independent inspection passed",
        }))
        .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 4, &json!({ "type": "message.started" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                5,
                &json!({ "type": "message.delta", "delta": reviewer_result }),
            )
            .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 6, &json!({ "type": "message.completed" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 7, &json!({ "type": "run.completed" }))
            .unwrap();
        let decision = fixture
            .database
            .settle_graph_node_review_for_child(&reviewer_run)
            .expect("settle independent pass");
        assert_eq!(decision.outcome, GraphNodeReviewOutcome::Pass);
        let finished = fixture
            .database
            .finish_read_only_graph_node(finish_input)
            .expect("matching high-risk pass authorizes finish");
        assert_eq!(
            finished.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );

        let goal_version = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT version FROM goals WHERE id = ?1",
                    [fixture.goal_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        let low_finding = fixture
            .database
            .add_review_finding(
                &fixture.conversation_id,
                &fixture.goal_id,
                Some(&started.task.id),
                Some(&fixture.plan_revision_id),
                "low",
                "validation",
                "Low severity still blocks final Graph acceptance",
                "Final acceptance requires every open Graph finding to be resolved or waived",
                "open",
                &fixture.run.run.id,
            )
            .unwrap();
        let mut blocked_accept_input = CreateReadOnlyGraphAcceptanceInput {
            goal_id: fixture.goal_id.clone(),
            submitter_run_id: fixture.run.run.id.clone(),
            accept_tool_call_id: "pending-blocked-accept-call".to_owned(),
            expected_goal_version: goal_version,
            summary: "must not accept with an open low finding".to_owned(),
        };
        blocked_accept_input.accept_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                "reviewed-blocked-accept",
                GRAPH_ACCEPT_TOOL,
                &graph_accept_tool_input(&blocked_accept_input),
                "running",
                false,
            )
            .unwrap()
            .id;
        assert!(matches!(
            fixture
                .database
                .create_read_only_graph_acceptance(blocked_accept_input),
            Err(RepositoryError::ReviewBlocked { .. })
        ));
        fixture
            .database
            .resolve_review_finding(&low_finding.id, "resolved", &fixture.run.run.id)
            .unwrap();
        let mut accept_input = CreateReadOnlyGraphAcceptanceInput {
            goal_id: fixture.goal_id.clone(),
            submitter_run_id: fixture.run.run.id.clone(),
            accept_tool_call_id: "pending-accept-call".to_owned(),
            expected_goal_version: goal_version,
            summary: "all Graph nodes independently verified".to_owned(),
        };
        accept_input.accept_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                "reviewed-accept",
                GRAPH_ACCEPT_TOOL,
                &graph_accept_tool_input(&accept_input),
                "running",
                false,
            )
            .unwrap()
            .id;
        let acceptance = fixture
            .database
            .create_read_only_graph_acceptance(accept_input)
            .expect("persist pending Graph acceptance");
        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                "reviewed-accept",
                Some(&json!({ "pendingActivation": true })),
                None,
            )
            .unwrap();
        let accepted = fixture
            .database
            .activate_read_only_graph_acceptance(&acceptance.acceptance_id)
            .expect("activate final Graph acceptance");
        assert!(accepted.accepted);
        let goal_status = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT status FROM goals WHERE id = ?1",
                    [fixture.goal_id.as_str()],
                    |row| row.get::<_, String>(0),
                )
            })
            .unwrap();
        assert_eq!(goal_status, "completed");
        cleanup(fixture);
    }

    #[test]
    fn high_risk_graph_review_revise_blocks_attempt_with_immutable_findings_and_proof() {
        let fixture = fixture_with_policy(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
            "high_risk_v1",
        );
        let activated = activate(&fixture).unwrap();
        let started = fixture
            .database
            .start_ready_read_only_graph_node(create_node_start_input(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                "revise-start",
                "revise-attempt",
            ))
            .unwrap();
        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("implementation result requiring revision"),
        );
        let evidence =
            add_graph_runtime_read_evidence(&fixture, &started.task.id, "revise-parent-proof");
        let mut review_input = CreateGraphNodeReviewRequestInput {
            goal_id: fixture.goal_id.clone(),
            task_id: started.task.id.clone(),
            attempt_id: started.attempt.id.clone(),
            parent_run_id: fixture.run.run.id.clone(),
            review_tool_call_id: "pending-revise-review-call".to_owned(),
            expected_task_version: started.task.version,
            expected_attempt_version: started.attempt.version,
            criterion_evidence: vec![GraphCriterionEvidenceInput {
                criterion: "result inspected".to_owned(),
                evidence_ids: vec![evidence.id],
            }],
            summary: "independently inspect the high-risk result".to_owned(),
        };
        review_input.review_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                "revise-review",
                GRAPH_NODE_REVIEW_TOOL,
                &node_review_tool_input(&review_input),
                "running",
                false,
            )
            .unwrap()
            .id;
        let request = fixture
            .database
            .create_graph_node_review_request(review_input)
            .expect("persist pending revise review");
        fixture
            .database
            .complete_host_tool_call(
                &fixture.run.run.id,
                "revise-review",
                Some(&json!({ "pendingActivation": true })),
                None,
            )
            .unwrap();
        assert!(
            fixture
                .database
                .activate_graph_node_review_request(&request.request_id)
                .unwrap()
                .activated
        );
        let reviewer = fixture
            .database
            .dispatch_graph_node_reviewer(&request.request_id, "fox-general")
            .expect("dispatch independent Reviewer");
        thread::sleep(std::time::Duration::from_millis(2));
        let reviewer_run = reviewer.child_run.child_run_id;
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 1, &json!({ "type": "run.started" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                2,
                &json!({
                    "type": "tool.started",
                    "toolCallId": "reviewer-revise-read",
                    "tool": "read",
                    "input": { "path": "result.txt" },
                }),
            )
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                3,
                &json!({
                    "type": "tool.completed",
                    "toolCallId": "reviewer-revise-read",
                    "tool": "read",
                    "result": { "content": "criterion is not met" },
                    "isError": false,
                }),
            )
            .unwrap();
        thread::sleep(std::time::Duration::from_millis(2));
        let reviewer_result = serde_json::to_string(&json!({
            "criteria": [{
                "criterion": "result inspected",
                "status": "failed",
                "toolCallIds": ["reviewer-revise-read"],
            }],
            "findings": [{
                "criterion": "result inspected",
                "severity": "high",
                "title": "Observed result does not meet the criterion",
                "detail": "The independent read found that the implementation result still requires revision.",
                "toolCallIds": ["reviewer-revise-read"],
            }],
            "recommendation": "revise",
            "summary": "revision required",
        }))
        .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 4, &json!({ "type": "message.started" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(
                &reviewer_run,
                5,
                &json!({ "type": "message.delta", "delta": reviewer_result }),
            )
            .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 6, &json!({ "type": "message.completed" }))
            .unwrap();
        fixture
            .database
            .apply_runtime_event(&reviewer_run, 7, &json!({ "type": "run.completed" }))
            .unwrap();

        let decision = fixture
            .database
            .settle_graph_node_review_for_child(&reviewer_run)
            .expect("settle authoritative revise decision");
        assert_eq!(decision.outcome, GraphNodeReviewOutcome::Revise);
        let facts = fixture
            .database
            .with_connection(|connection| {
                Ok::<_, rusqlite::Error>((
                    connection.query_row(
                        "SELECT status FROM task_attempts WHERE id = ?1",
                        [started.attempt.id.as_str()],
                        |row| row.get::<_, String>(0),
                    )?,
                    connection.query_row(
                        "SELECT status FROM work_tasks WHERE id = ?1",
                        [started.task.id.as_str()],
                        |row| row.get::<_, String>(0),
                    )?,
                    connection.query_row(
                        "SELECT COUNT(*) FROM work_graph_node_review_proofs WHERE decision_id = ?1",
                        [decision.decision_id.as_str()],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT decision_json FROM work_graph_node_review_decisions WHERE id = ?1",
                        [decision.decision_id.as_str()],
                        |row| row.get::<_, String>(0),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(facts.0, "blocked");
        assert_eq!(facts.1, "blocked");
        assert_eq!(facts.2, 1);
        let stored_decision: Value = serde_json::from_str(&facts.3).unwrap();
        assert_eq!(stored_decision["recommendation"], "revise");
        assert_eq!(stored_decision["findings"].as_array().unwrap().len(), 1);
        cleanup(fixture);
    }

    #[test]
    fn newer_approved_plan_blocks_old_graph_finish_without_side_effects() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).unwrap();
        let started = fixture
            .database
            .start_ready_read_only_graph_node(create_node_start_input(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                "superseded-finish-start",
                "superseded-finish-attempt",
            ))
            .unwrap();
        complete_graph_child(
            &fixture,
            &started.child_run.child_run_id,
            Some("result from the superseded plan"),
        );
        let evidence =
            add_graph_runtime_read_evidence(&fixture, &started.task.id, "superseded-finish-proof");
        let newer = fixture
            .database
            .create_plan_revision(
                &fixture.conversation_id,
                &fixture.goal_id,
                "Approved replacement",
                "Supersede the active immutable Graph",
                json!([{ "title": "Replacement", "detail": null, "ordinal": 0 }]),
                &fixture.run.run.id,
            )
            .expect("create newer PlanRevision while the old Graph Attempt is running");
        fixture
            .database
            .resolve_plan_revision(&fixture.conversation_id, &newer.id, "approved")
            .expect("approve replacement PlanRevision");
        let error = fixture
            .database
            .finish_read_only_graph_node(create_node_finish_input(
                &fixture,
                &started,
                "superseded-finish-call",
                vec![GraphCriterionEvidenceInput {
                    criterion: "result inspected".to_owned(),
                    evidence_ids: vec![evidence.id],
                }],
            ))
            .expect_err("newer approved PlanRevision invalidates old Graph finish authority");
        assert!(
            matches!(error, RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision") && message.contains("approved"))
        );
        let facts = fixture
            .database
            .with_connection(|connection| {
                Ok::<_, rusqlite::Error>((
                    connection.query_row(
                        "SELECT COUNT(*) FROM work_graph_node_finishes",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?,
                    connection.query_row(
                        "SELECT status, version FROM task_attempts WHERE id = ?1",
                        [started.attempt.id.as_str()],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                    )?,
                ))
            })
            .unwrap();
        assert_eq!(facts, (0, ("running".to_owned(), 1)));
        cleanup(fixture);
    }

    #[test]
    fn generic_graph_attempt_start_is_rejected_without_creating_facts() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).unwrap();
        let error = fixture
            .database
            .start_task_attempt(StartTaskAttemptInput {
                id: "generic-graph-attempt".to_owned(),
                task_id: activated.snapshot.nodes[0].task_id.clone(),
                run_id: fixture.run.run.id.clone(),
                expected_task_version: 1,
                kind: TaskAttemptKind::Execution,
                root_cause: None,
                finding_ids: Vec::new(),
            })
            .expect_err("generic Attempt start cannot acquire a Graph node");
        assert!(error
            .to_string()
            .contains("graph.readonly_dedicated_start_required"));
        let count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE id = 'generic-graph-attempt'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap();
        assert_eq!(count, 0);
        cleanup(fixture);
    }

    #[test]
    fn forged_succeeded_graph_attempt_without_finish_header_never_unlocks_dependency() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["root inspected"] },
                { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).unwrap();
        let started = fixture
            .database
            .start_ready_read_only_graph_node(create_node_start_input(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                "forged-success-start",
                "forged-success-attempt",
            ))
            .unwrap();
        fixture
            .database
            .with_connection(|connection| {
                // Simulate a legacy/corrupt database in which the v37 write gates were absent.
                // The read projection must still fail closed without immutable finish facts.
                connection.execute_batch(
                    "DROP TRIGGER task_attempts_require_graph_node_finish;
                     DROP TRIGGER work_tasks_require_graph_node_finish;",
                )?;
                connection.execute(
                    "UPDATE task_attempts
                     SET status = 'succeeded', version = version + 1, finished_at = started_at
                     WHERE id = ?1",
                    [started.attempt.id.as_str()],
                )?;
                connection.execute(
                    "UPDATE work_tasks
                     SET status = 'completed', version = version + 1, finished_at = updated_at
                     WHERE id = ?1",
                    [started.task.id.as_str()],
                )?;
                Ok::<_, rusqlite::Error>(())
            })
            .expect("forge terminal rows after dropping only the test database gates");
        let snapshot = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read fail-closed forged Graph")
            .expect("forged Graph remains projected");
        assert_ne!(
            snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );
        assert_eq!(snapshot.nodes[1].readiness, GraphLeadNodeReadiness::Waiting);
        assert!(snapshot.nodes[0].evidence_ids.is_empty());
        cleanup(fixture);
    }

    #[test]
    fn graph_finish_rejects_old_foreign_and_forged_approval_tool_evidence_atomically() {
        for case in ["old", "foreign", "approval"] {
            let fixture = fixture(
                json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
                "durable_v2",
            );
            let foreign_run_id = (case == "foreign").then(|| {
                let run_id = "foreign-proof-run".to_owned();
                fixture
                    .database
                    .with_connection(|connection| {
                        connection.execute(
                            "INSERT INTO runs(
                                id, conversation_id, status, model, last_seq, created_at,
                                finished_at, root_run_id, run_kind, depth
                             ) VALUES (?1, ?2, 'completed', 'model', 0, ?3, ?3, ?1, 'primary', 0)",
                            rusqlite::params![
                                run_id.as_str(),
                                fixture.conversation_id.as_str(),
                                "2000-01-01T00:00:00.000Z",
                            ],
                        )?;
                        Ok::<_, rusqlite::Error>(())
                    })
                    .expect("insert a terminal foreign Lead fixture without a second active Run");
                run_id
            });
            let activated = activate(&fixture).unwrap();
            let old_tool = if case == "old" {
                let tool = fixture
                    .database
                    .create_host_tool_call(
                        &fixture.run.run.id,
                        "old-proof-call",
                        "git_read",
                        &json!({ "operation": "status" }),
                        "running",
                        false,
                    )
                    .unwrap();
                fixture
                    .database
                    .complete_host_tool_call(
                        &fixture.run.run.id,
                        "old-proof-call",
                        Some(&json!({ "ok": true })),
                        None,
                    )
                    .unwrap();
                thread::sleep(std::time::Duration::from_millis(2));
                Some(tool)
            } else {
                None
            };
            let started = fixture
                .database
                .start_ready_read_only_graph_node(create_node_start_input(
                    &fixture,
                    &activated.snapshot.nodes[0],
                    &fixture.run.run.id,
                    &format!("{case}-proof-start"),
                    &format!("{case}-proof-attempt"),
                ))
                .unwrap();
            complete_graph_child(
                &fixture,
                &started.child_run.child_run_id,
                Some("non-empty result"),
            );
            let evidence = match case {
                "approval" => add_graph_host_evidence(
                    &fixture,
                    &started.task.id,
                    "approval-forged",
                    "git_read",
                    true,
                    EvidenceType::ToolCall,
                    "inspection",
                ),
                "old" => {
                    let evidence = fixture
                        .database
                        .task_evidence()
                        .add(AddEvidenceInput {
                            id: None,
                            task_id: started.task.id.clone(),
                            source_run_id: Some(fixture.run.run.id.clone()),
                            evidence_type: EvidenceType::ToolCall,
                            ref_kind: EvidenceReferenceKind::ToolCall,
                            ref_id: old_tool.unwrap().id,
                            summary: "fresh wrapper around an old ToolCall".to_owned(),
                            metadata: json!({ "validationCheckType": "inspection" }),
                            trace_id: None,
                            span_id: None,
                        })
                        .unwrap();
                    assert_eq!(
                        fixture
                            .database
                            .task_evidence()
                            .validate(&evidence.id)
                            .unwrap(),
                        EvidenceValidityStatus::Valid
                    );
                    evidence
                }
                "foreign" => {
                    let other_run_id = foreign_run_id.expect("foreign Lead fixture");
                    thread::sleep(std::time::Duration::from_millis(2));
                    let tool_id = "foreign-proof-tool".to_owned();
                    fixture
                        .database
                        .with_connection(|connection| {
                            connection.execute(
                                "INSERT INTO tool_calls(
                                    id, runtime_tool_call_id, run_id, conversation_id, tool_name,
                                    input_json, result_json, status, execution_location,
                                    requires_approval, started_at, updated_at
                                 ) VALUES (?1, 'foreign-proof-call', ?2, ?3, 'git_read',
                                           '{\"operation\":\"status\"}', '{\"ok\":true}',
                                           'completed', 'host', 0, ?4, ?4)",
                                rusqlite::params![
                                    tool_id.as_str(),
                                    other_run_id.as_str(),
                                    fixture.conversation_id.as_str(),
                                    "9999-01-01T00:00:00.000Z",
                                ],
                            )?;
                            Ok::<_, rusqlite::Error>(())
                        })
                        .expect("insert completed foreign ToolCall fixture");
                    let evidence = fixture
                        .database
                        .task_evidence()
                        .add(AddEvidenceInput {
                            id: None,
                            task_id: started.task.id.clone(),
                            source_run_id: Some(other_run_id),
                            evidence_type: EvidenceType::ToolCall,
                            ref_kind: EvidenceReferenceKind::ToolCall,
                            ref_id: tool_id,
                            summary: "another Lead cannot prove this Attempt".to_owned(),
                            metadata: json!({ "validationCheckType": "inspection" }),
                            trace_id: None,
                            span_id: None,
                        })
                        .unwrap();
                    assert_eq!(
                        fixture
                            .database
                            .task_evidence()
                            .validate(&evidence.id)
                            .unwrap(),
                        EvidenceValidityStatus::Valid
                    );
                    evidence
                }
                _ => unreachable!(),
            };
            let finish_input = create_node_finish_input(
                &fixture,
                &started,
                &format!("{case}-proof-finish"),
                vec![GraphCriterionEvidenceInput {
                    criterion: "result inspected".to_owned(),
                    evidence_ids: vec![evidence.id],
                }],
            );
            let error = fixture
                .database
                .finish_read_only_graph_node(finish_input)
                .expect_err("invalid ToolCall provenance must reject Graph finish");
            assert!(matches!(error, RepositoryError::InvalidReference(_)));
            let facts = fixture
                .database
                .with_connection(|connection| {
                    Ok::<_, rusqlite::Error>((
                        connection.query_row(
                            "SELECT COUNT(*) FROM work_graph_node_finishes",
                            [],
                            |row| row.get::<_, i64>(0),
                        )?,
                        connection.query_row(
                            "SELECT status FROM task_attempts WHERE id = ?1",
                            [started.attempt.id.as_str()],
                            |row| row.get::<_, String>(0),
                        )?,
                    ))
                })
                .unwrap();
            assert_eq!(facts, (0, "running".to_owned()));
            cleanup(fixture);
        }
    }

    #[test]
    fn exact_activation_replays_and_conflicting_activation_is_rejected() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let first = activate(&fixture).expect("first activation");
        let replay = activate(&fixture).expect("exact replay");
        assert!(replay.replayed);
        assert_eq!(replay.snapshot.spec_hash, first.snapshot.spec_hash);
        let conflict_call = create_activation_tool_call(&fixture, "activate-conflict");
        let conflict = activate_with_tool_call(&fixture, &conflict_call).expect_err("conflict");
        assert!(matches!(conflict, RepositoryError::ConstraintViolation(_)));
        cleanup(fixture);
    }

    #[test]
    fn graph_node_start_atomically_owns_attempt_child_scope_profile_and_exact_replay() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "start-only-node",
            "attempt-only-node",
        );

        let first = fixture
            .database
            .start_ready_read_only_graph_node(input.clone())
            .expect("start runnable Graph node");
        assert!(first.created);
        assert!(!first.replayed);
        assert_eq!(first.started.run.id, first.child_run.child_run_id);
        assert_eq!(first.task.status, WorkTaskStatus::InProgress);
        assert_eq!(first.attempt.status, TaskAttemptStatus::Running);
        assert_eq!(first.attempt.run_id, fixture.run.run.id);
        assert_eq!(
            first.child_run.allowed_tools,
            Some(vec![
                "read".to_owned(),
                "ls".to_owned(),
                "find".to_owned(),
                "grep".to_owned(),
            ])
        );
        assert_eq!(first.child_run.budget, graph_child_budget());
        assert_eq!(
            first.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Active
        );

        let (mapped_attempt, allowed_tools_json, profile_id, snapshot_json): (
            Option<String>,
            Option<String>,
            String,
            String,
        ) = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT delegations.graph_task_attempt_id, delegations.allowed_tools_json,
                            profiles.profile_id, profiles.snapshot_json
                     FROM child_run_delegations delegations
                     JOIN run_execution_profiles profiles ON profiles.run_id = delegations.child_run_id
                     WHERE delegations.child_run_id = ?1",
                    [first.child_run.child_run_id.as_str()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
            })
            .expect("load persisted Graph delegation");
        assert_eq!(mapped_attempt.as_deref(), Some("attempt-only-node"));
        assert_eq!(
            allowed_tools_json.as_deref(),
            Some(r#"["read","ls","find","grep"]"#)
        );
        assert_eq!(profile_id, "durable_v2");
        assert_eq!(
            serde_json::from_str::<Value>(&snapshot_json).expect("profile JSON"),
            json!({ "schemaVersion": 1, "id": "durable_v2" })
        );

        let mut conflicting_replay = input.clone();
        conflicting_replay.budget.max_tool_calls = 5;
        let conflict = fixture
            .database
            .start_ready_read_only_graph_node(conflicting_replay)
            .expect_err("replay with a different budget must conflict");
        assert!(
            matches!(conflict, RepositoryError::ConstraintViolation(message) if message.contains("exactly bind") || message.contains("conflicts"))
        );

        let replay = fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect("exact node-start replay");
        assert!(!replay.created);
        assert!(replay.replayed);
        assert_eq!(replay.child_run.id, first.child_run.id);
        assert_eq!(replay.attempt.id, first.attempt.id);
        let fact_counts = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM task_attempts WHERE id = 'attempt-only-node'),
                        (SELECT COUNT(*) FROM child_run_delegations WHERE graph_task_attempt_id = 'attempt-only-node')",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
            })
            .expect("count exact replay facts");
        assert_eq!(fact_counts, (1, 1));
        cleanup(fixture);
    }

    #[test]
    fn graph_node_start_rejects_waiting_blocked_and_skipped_nodes_without_facts() {
        let waiting = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["root inspected"] },
                { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
            ]),
            "durable_v2",
        );
        let waiting_snapshot = activate(&waiting).expect("activate waiting graph").snapshot;
        let waiting_input = create_node_start_input(
            &waiting,
            &waiting_snapshot.nodes[1],
            &waiting.run.run.id,
            "waiting-node-start",
            "waiting-node-attempt",
        );
        assert!(matches!(
            waiting
                .database
                .start_ready_read_only_graph_node(waiting_input),
            Err(RepositoryError::ConstraintViolation(message)) if message.contains("not runnable")
        ));
        cleanup(waiting);

        let blocked = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let blocked_snapshot = activate(&blocked).expect("activate blocked graph").snapshot;
        blocked
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks
                     SET status = 'blocked', blocked_reason = 'host_pause', version = version + 1
                     WHERE id = ?1",
                    [blocked_snapshot.nodes[0].task_id.as_str()],
                )?;
                Ok(())
            })
            .expect("record blocked durable Task");
        let blocked_projection = blocked
            .database
            .read_only_graph_snapshot(&blocked.goal_id)
            .expect("read blocked projection")
            .expect("blocked Graph");
        assert_eq!(
            blocked_projection.nodes[0].readiness,
            GraphLeadNodeReadiness::Blocked
        );
        let blocked_input = create_node_start_input(
            &blocked,
            &blocked_snapshot.nodes[0],
            &blocked.run.run.id,
            "blocked-node-start",
            "blocked-node-attempt",
        );
        assert!(matches!(
            blocked
                .database
                .start_ready_read_only_graph_node(blocked_input),
            Err(RepositoryError::ConstraintViolation(message)) if message.contains("not runnable")
        ));
        cleanup(blocked);

        let skipped = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let skipped_snapshot = activate(&skipped).expect("activate skipped graph").snapshot;
        skipped
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'skipped', version = version + 1 WHERE id = ?1",
                    [skipped_snapshot.nodes[0].task_id.as_str()],
                )?;
                Ok(())
            })
            .expect("record explicit skip");
        let skipped_input = create_node_start_input(
            &skipped,
            &skipped_snapshot.nodes[0],
            &skipped.run.run.id,
            "skipped-node-start",
            "skipped-node-attempt",
        );
        assert!(matches!(
            skipped
                .database
                .start_ready_read_only_graph_node(skipped_input),
            Err(RepositoryError::ConstraintViolation(message)) if message.contains("not runnable")
        ));
        let rejected_attempts = skipped
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE id IN (
                        'waiting-node-attempt', 'blocked-node-attempt', 'skipped-node-attempt'
                     )",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count rejected node Attempts");
        assert_eq!(rejected_attempts, 0);
        cleanup(skipped);
    }

    #[test]
    fn graph_parent_lineage_cannot_be_forged_before_node_start() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "pseudo-child-node-start",
            "pseudo-child-attempt",
        );
        let mutation_error = fixture
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE runs
                     SET run_kind = 'child', parent_run_id = id, root_run_id = id, depth = 1
                     WHERE id = ?1",
                    [fixture.run.run.id.as_str()],
                )?;
                Ok(())
            })
            .expect_err("immutable lineage must reject a Child masquerading as Lead");
        assert!(mutation_error.contains("lineage facts are immutable"));
        let started = fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect("the unchanged depth-zero primary Lead may still start the node");
        assert!(started.created);
        cleanup(fixture);
    }

    #[test]
    fn graph_node_start_rolls_back_attempt_when_child_creation_fails() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let mut input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "start-rollback-node-original",
            "attempt-rollback-node",
        );
        input.worker_agent_id = "missing-worker".to_owned();
        input.node_start_tool_call_id = fixture
            .database
            .create_host_tool_call(
                &fixture.run.run.id,
                "start-rollback-node",
                GRAPH_NODE_START_TOOL,
                &node_start_tool_input(&input),
                "running",
                false,
            )
            .expect("create exact rollback ToolCall")
            .id;
        fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect_err("unknown worker must roll back the whole start");
        let facts = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT tasks.status, tasks.version,
                            (SELECT COUNT(*) FROM task_attempts WHERE id = 'attempt-rollback-node'),
                            (SELECT COUNT(*) FROM child_run_delegations WHERE graph_task_attempt_id = 'attempt-rollback-node')
                     FROM work_tasks tasks WHERE tasks.id = ?1",
                    [activated.snapshot.nodes[0].task_id.as_str()],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
            })
            .expect("inspect rollback facts");
        assert_eq!(facts, ("queued".to_owned(), 1, 0, 0));
        cleanup(fixture);
    }

    #[test]
    fn graph_node_start_enforces_read_only_mvp_budget_before_writing() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        for (suffix, budget) in [
            (
                "duration",
                ChildRunBudget {
                    max_duration_ms: 45_001,
                    ..graph_child_budget()
                },
            ),
            (
                "tokens",
                ChildRunBudget {
                    max_total_tokens: 4_097,
                    ..graph_child_budget()
                },
            ),
            (
                "output",
                ChildRunBudget {
                    max_output_tokens: 1_025,
                    ..graph_child_budget()
                },
            ),
            (
                "tools",
                ChildRunBudget {
                    max_tool_calls: 7,
                    ..graph_child_budget()
                },
            ),
        ] {
            let input = create_node_start_input_with_budget(
                &fixture,
                &activated.snapshot.nodes[0],
                &fixture.run.run.id,
                &format!("budget-{suffix}"),
                &format!("budget-attempt-{suffix}"),
                budget,
            );
            let error = fixture
                .database
                .start_ready_read_only_graph_node(input)
                .expect_err("oversized Graph budget must fail");
            assert!(
                matches!(error, RepositoryError::InvalidInput(message) if message.contains("bounded MVP limits"))
            );
        }
        let attempt_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1",
                    [activated.snapshot.nodes[0].task_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count bounded-budget attempts");
        assert_eq!(attempt_count, 0);
        cleanup(fixture);
    }

    #[test]
    fn a_new_same_conversation_durable_lead_run_can_resume_the_persisted_graph() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let terminal_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "terminal-parent-node-start",
            "terminal-parent-attempt",
        );
        fixture
            .database
            .apply_runtime_event(&fixture.run.run.id, 2, &json!({ "type": "run.completed" }))
            .expect("complete activation Lead Run");
        let terminal_error = fixture
            .database
            .start_ready_read_only_graph_node(terminal_input)
            .expect_err("terminal activation Run cannot start a node");
        assert!(
            matches!(terminal_error, RepositoryError::ConstraintViolation(message) if message.contains("must be running"))
        );

        let legacy = create_running_lead_run(
            &fixture.database,
            &fixture.conversation_id,
            "durable_v2_shadow",
        );
        let legacy_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &legacy.run.id,
            "legacy-parent-node-start",
            "legacy-parent-attempt",
        );
        let legacy_error = fixture
            .database
            .start_ready_read_only_graph_node(legacy_input)
            .expect_err("non-canonical profile cannot resume Graph");
        assert!(matches!(
            legacy_error,
            RepositoryError::InvalidValidationPolicy(_)
        ));
        fixture
            .database
            .apply_runtime_event(&legacy.run.id, 2, &json!({ "type": "run.completed" }))
            .expect("complete legacy Lead Run");

        let other_conversation = fixture
            .database
            .create_conversation("fox-general", Some("Other"), None, None)
            .expect("create other conversation");
        let other_run =
            create_running_lead_run(&fixture.database, &other_conversation.id, "durable_v2");
        let cross_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &other_run.run.id,
            "cross-conversation-node-start",
            "cross-conversation-attempt",
        );
        assert!(matches!(
            fixture
                .database
                .start_ready_read_only_graph_node(cross_input),
            Err(RepositoryError::CrossConversationReference)
        ));

        let resumed =
            create_running_lead_run(&fixture.database, &fixture.conversation_id, "durable_v2");
        let resumed_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &resumed.run.id,
            "resumed-node-start",
            "resumed-attempt",
        );
        let result = fixture
            .database
            .start_ready_read_only_graph_node(resumed_input)
            .expect("same-conversation durable Lead Run resumes Graph");
        assert_eq!(result.attempt.run_id, resumed.run.id);
        assert_eq!(result.snapshot.activated_by_run_id, fixture.run.run.id);
        assert_eq!(
            result.snapshot.nodes[0].readiness,
            GraphLeadNodeReadiness::Active
        );
        cleanup(fixture);
    }

    #[test]
    fn newer_proposed_plan_pauses_graph_node_start_without_side_effects() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        fixture
            .database
            .create_plan_revision(
                &fixture.conversation_id,
                &fixture.goal_id,
                "New proposal",
                "Pause the active graph until reviewed",
                json!([{ "title": "Replacement", "detail": null, "ordinal": 0 }]),
                &fixture.run.run.id,
            )
            .expect("create newer proposed plan");
        let input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "paused-by-proposal",
            "paused-by-proposal-attempt",
        );
        let error = fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect_err("newer proposal pauses node start");
        assert!(
            matches!(error, RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision"))
        );
        let attempt_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE id = 'paused-by-proposal-attempt'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count paused attempt");
        assert_eq!(attempt_count, 0);
        cleanup(fixture);
    }

    #[test]
    fn newer_approved_plan_prevents_a_resumed_lead_from_starting_the_old_graph() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        fixture
            .database
            .apply_runtime_event(&fixture.run.run.id, 2, &json!({ "type": "run.completed" }))
            .expect("complete activation Lead Run before recovery");
        let resumed =
            create_running_lead_run(&fixture.database, &fixture.conversation_id, "durable_v2");
        let newer = fixture
            .database
            .create_plan_revision(
                &fixture.conversation_id,
                &fixture.goal_id,
                "Approved replacement",
                "Replace the immutable Graph with a newly approved plan",
                json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["replacement inspected"] }]),
                &fixture.run.run.id,
            )
            .expect("create newer plan");
        fixture
            .database
            .resolve_plan_revision(&fixture.conversation_id, &newer.id, "approved")
            .expect("approve newer plan");

        let input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &resumed.run.id,
            "paused-by-approved-plan",
            "paused-by-approved-plan-attempt",
        );
        let error = fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect_err("newer approved plan replaces the activated Graph authority");
        assert!(
            matches!(error, RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision") && message.contains("approved"))
        );

        let facts = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT tasks.status, tasks.version,
                            (SELECT COUNT(*) FROM task_attempts WHERE id = ?2),
                            (SELECT COUNT(*) FROM child_run_delegations WHERE graph_task_attempt_id = ?2)
                     FROM work_tasks tasks WHERE tasks.id = ?1",
                    params![
                        activated.snapshot.nodes[0].task_id,
                        "paused-by-approved-plan-attempt"
                    ],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, i64>(3)?,
                        ))
                    },
                )
            })
            .expect("load old Graph facts after rejected start");
        assert_eq!(facts, ("queued".to_owned(), 1, 0, 0));
        cleanup(fixture);
    }

    #[test]
    fn concurrent_graph_node_starts_have_one_atomic_winner() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let first_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "concurrent-start-a",
            "concurrent-attempt-a",
        );
        let second_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "concurrent-start-b",
            "concurrent-attempt-b",
        );
        let first_database = Database::open(fixture.path.clone()).expect("open first contender");
        let second_database = Database::open(fixture.path.clone()).expect("open second contender");
        let barrier = Arc::new(Barrier::new(3));
        let first_barrier = Arc::clone(&barrier);
        let first = thread::spawn(move || {
            first_barrier.wait();
            first_database.start_ready_read_only_graph_node(first_input)
        });
        let second_barrier = Arc::clone(&barrier);
        let second = thread::spawn(move || {
            second_barrier.wait();
            second_database.start_ready_read_only_graph_node(second_input)
        });
        barrier.wait();
        let results = [
            first.join().expect("first contender"),
            second.join().expect("second contender"),
        ];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let facts = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT
                        (SELECT COUNT(*) FROM task_attempts WHERE task_id = ?1),
                        (SELECT COUNT(*) FROM child_run_delegations WHERE graph_task_attempt_id IS NOT NULL)",
                    [activated.snapshot.nodes[0].task_id.as_str()],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
                )
            })
            .expect("count concurrent facts");
        assert_eq!(facts, (1, 1));
        cleanup(fixture);
    }

    #[test]
    fn active_node_attempt_and_child_snapshot_survive_database_restart() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "restart-node-start",
            "restart-node-attempt",
        );
        let started = fixture
            .database
            .start_ready_read_only_graph_node(input)
            .expect("start node before restart");
        let path = fixture.path.clone();
        let goal_id = fixture.goal_id.clone();
        let child_run_id = started.child_run.child_run_id.clone();
        drop(fixture);

        let reopened = Database::open(path.clone()).expect("reopen graph database");
        let snapshot = reopened
            .read_only_graph_snapshot(&goal_id)
            .expect("read Graph after restart")
            .expect("persisted Graph");
        assert_eq!(snapshot.nodes[0].readiness, GraphLeadNodeReadiness::Active);
        assert_eq!(
            snapshot.nodes[0].task_version, started.task.version,
            "reopened Graph snapshot must expose the authoritative CAS version"
        );
        let persisted = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT attempts.status, delegations.child_run_id, profiles.profile_id
                     FROM task_attempts attempts
                     JOIN child_run_delegations delegations
                       ON delegations.graph_task_attempt_id = attempts.id
                     JOIN run_execution_profiles profiles ON profiles.run_id = delegations.child_run_id
                     WHERE attempts.id = 'restart-node-attempt'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
            })
            .expect("load restarted node lineage");
        assert_eq!(
            persisted,
            ("running".to_owned(), child_run_id, "durable_v2".to_owned())
        );
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn activation_requires_the_running_host_graph_tool_contract() {
        let cases = [
            ("tool_name", "some_other_tool", "must use"),
            ("execution_location", "runtime", "execute on the Host"),
            ("status", "completed", "must still be running"),
            (
                "input_json",
                r#"{"planRevisionId":"another-plan"}"#,
                "exactly bind",
            ),
            ("requires_approval", "1", "cannot require approval"),
        ];
        for (column, value, expected_message) in cases {
            let fixture = fixture(
                json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
                "durable_v2",
            );
            fixture
                .database
                .with_connection(|connection| {
                    connection.execute(
                        &format!("UPDATE tool_calls SET {column} = ?2 WHERE id = ?1"),
                        params![fixture.activation_tool_call_id, value],
                    )?;
                    Ok(())
                })
                .expect("mutate activation source for fail-closed test");
            let error = activate(&fixture).expect_err("invalid activation source must fail");
            assert!(matches!(
                error,
                RepositoryError::ConstraintViolation(message)
                    if message.contains(expected_message)
            ));
            cleanup(fixture);
        }
    }

    #[test]
    fn an_explicitly_skipped_dependency_does_not_invent_a_skipped_dependent() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["root inspected"] },
                { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        fixture
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE work_tasks SET status = 'skipped', version = version + 1
                     WHERE id = ?1 AND status = 'queued'",
                    [activated.snapshot.nodes[0].task_id.as_str()],
                )?;
                Ok(())
            })
            .expect("record an explicit Host skip fact");
        let snapshot = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read graph after explicit skip")
            .expect("graph exists");
        assert_eq!(snapshot.nodes[0].readiness, GraphLeadNodeReadiness::Skipped);
        assert_eq!(snapshot.nodes[1].readiness, GraphLeadNodeReadiness::Waiting);
        assert!(snapshot.nodes[1]
            .blocker
            .as_deref()
            .is_some_and(|message| message.contains("explicitly skipped")));
        cleanup(fixture);
    }

    #[test]
    fn only_current_successful_attempt_with_live_evidence_unlocks_dependents() {
        let fixture = fixture(
            json!([
                { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["root inspected"] },
                { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let root_task_id = activated.snapshot.nodes[0].task_id.clone();
        let next_task_id = activated.snapshot.nodes[1].task_id.clone();

        let start_input = create_node_start_input(
            &fixture,
            &activated.snapshot.nodes[0],
            &fixture.run.run.id,
            "graph-root-start",
            "graph-root-execution",
        );
        let started = fixture
            .database
            .start_ready_read_only_graph_node(start_input)
            .expect("start root Graph node");
        assert_eq!(started.task.version, 2);
        let bypass_error = fixture
            .database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: "graph-root-execution".to_owned(),
                task_id: root_task_id.clone(),
                run_id: fixture.run.run.id.clone(),
                expected_task_version: 2,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Succeeded,
                failure_reason: None,
            })
            .expect_err("generic finish must not bypass Graph criterion acceptance");
        assert!(matches!(
            bypass_error,
            RepositoryError::ConstraintViolation(message)
                if message.contains("graph.readonly_dedicated_finish_required")
        ));
        let unchanged = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read graph after bypass rejection")
            .expect("graph exists");
        assert_eq!(unchanged.nodes[0].readiness, GraphLeadNodeReadiness::Active);
        assert_eq!(unchanged.nodes[1].task_id, next_task_id);
        assert_eq!(
            unchanged.nodes[1].readiness,
            GraphLeadNodeReadiness::Waiting
        );
        cleanup(fixture);
        return;

        /*
         * Historical repair assertions used the generic Attempt finish path. v37 deliberately
         * rejects that path for Graph Tasks, so the dedicated finish/readiness coverage now lives
         * in the graph_finish_* tests above. Keep this block temporarily as migration context while
         * the remaining Graph repair/cancel slice is still intentionally unopened.
        let finding = fixture
            .database
            .add_review_finding(
                &fixture.conversation_id,
                &fixture.goal_id,
                Some(&root_task_id),
                Some(&fixture.plan_revision_id),
                "medium",
                "validation",
                "Root output needs repair",
                "The root result must be checked again",
                "open",
                &fixture.run.run.id,
            )
            .expect("add blocking Finding");
        let blocked = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read blocked graph")
            .expect("graph exists");
        assert_eq!(blocked.nodes[0].readiness, GraphLeadNodeReadiness::Blocked);
        assert_eq!(blocked.nodes[1].readiness, GraphLeadNodeReadiness::Waiting);

        let repair = fixture
            .database
            .start_task_attempt(StartTaskAttemptInput {
                id: "graph-root-repair".to_owned(),
                task_id: root_task_id.clone(),
                run_id: fixture.run.run.id.clone(),
                expected_task_version: 3,
                kind: TaskAttemptKind::Repair,
                root_cause: Some("medium review finding".to_owned()),
                finding_ids: vec![finding.id.clone()],
            })
            .expect("start root repair");
        assert_eq!(repair.task.version, 4);
        fixture
            .database
            .resolve_review_finding(&finding.id, "resolved", &fixture.run.run.id)
            .expect("resolve repair Finding");
        let old_evidence_error = fixture
            .database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: "graph-root-repair".to_owned(),
                task_id: root_task_id.clone(),
                run_id: fixture.run.run.id.clone(),
                expected_task_version: 4,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Succeeded,
                failure_reason: None,
            })
            .expect_err("the previous Attempt's Evidence cannot accept the repair");
        assert!(matches!(
            old_evidence_error,
            RepositoryError::EvidenceRequired { .. }
                | RepositoryError::ValidationChecksMissing { .. }
                | RepositoryError::InvalidValidationPolicy(_)
        ));

        let repair_evidence = add_host_check_evidence(&fixture, &root_task_id, "repair");
        fixture
            .database
            .finish_task_attempt(FinishTaskAttemptInput {
                id: "graph-root-repair".to_owned(),
                task_id: root_task_id.clone(),
                run_id: fixture.run.run.id.clone(),
                expected_task_version: 4,
                expected_attempt_version: 1,
                status: TaskAttemptStatus::Succeeded,
                failure_reason: None,
            })
            .expect("finish root repair with fresh Evidence");
        let repaired = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read repaired graph")
            .expect("graph exists");
        assert_eq!(
            repaired.nodes[0].readiness,
            GraphLeadNodeReadiness::Accepted
        );
        assert_eq!(
            repaired.nodes[1].readiness,
            GraphLeadNodeReadiness::Runnable
        );
        assert_eq!(
            repaired.nodes[0].authoritative_attempt_id.as_deref(),
            Some("graph-root-repair")
        );
        assert_eq!(
            repaired.nodes[0].evidence_ids,
            vec![repair_evidence.id.clone()]
        );

        fixture
            .database
            .task_evidence()
            .mark_stale(&repair_evidence.id, "source changed".to_owned())
            .expect("mark current repair Evidence stale");
        let stale = fixture
            .database
            .read_only_graph_snapshot(&fixture.goal_id)
            .expect("read stale graph")
            .expect("graph exists");
        assert_eq!(stale.nodes[0].readiness, GraphLeadNodeReadiness::Blocked);
        assert_eq!(stale.nodes[1].readiness, GraphLeadNodeReadiness::Waiting);
        let stale_dependency_input = create_node_start_input(
            &fixture,
            &stale.nodes[1],
            &fixture.run.run.id,
            "stale-dependent-start",
            "stale-dependent-attempt",
        );
        let start_error = fixture
            .database
            .start_ready_read_only_graph_node(stale_dependency_input)
            .expect_err("stale dependency Evidence cannot unlock node start");
        assert!(
            matches!(start_error, RepositoryError::ConstraintViolation(message) if message.contains("not runnable"))
        );
        let stale_attempt_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM task_attempts WHERE id = 'stale-dependent-attempt'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count rejected stale dependent attempt");
        assert_eq!(stale_attempt_count, 0);
        cleanup(fixture);
        */
    }

    #[test]
    fn rejects_unknown_self_cycle_duplicate_and_depth_two_graphs() {
        let cases = [
            json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["missing"], "acceptanceCriteria": ["a accepted"] }]),
            json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["a"], "acceptanceCriteria": ["a accepted"] }]),
            json!([
                { "nodeKey": "same", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"] },
                { "nodeKey": "same", "title": "B", "ordinal": 1, "acceptanceCriteria": ["b accepted"] }
            ]),
            json!([
                { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"] },
                { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a", "a"], "acceptanceCriteria": ["b accepted"] }
            ]),
            json!([
                { "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["b"], "acceptanceCriteria": ["a accepted"] },
                { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a"], "acceptanceCriteria": ["b accepted"] }
            ]),
            json!([
                { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"] },
                { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a"], "acceptanceCriteria": ["b accepted"] },
                { "nodeKey": "c", "title": "C", "ordinal": 2, "dependsOn": ["b"], "acceptanceCriteria": ["c accepted"] }
            ]),
            json!([{ "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"] }]),
            json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": "a accepted" }]),
            json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["same", "same"] }]),
            json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"], "status": "completed" }]),
        ];
        for plan in cases {
            let fixture = fixture(plan, "durable_v2");
            assert!(activate(&fixture).is_err());
            cleanup(fixture);
        }
    }

    #[test]
    fn rejects_nodes_without_explicit_acceptance_criteria_before_sql_insert() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0 }]),
            "durable_v2",
        );
        let error = activate(&fixture).expect_err("missing criterion must be rejected");
        assert!(
            matches!(error, RepositoryError::InvalidInput(message) if message.contains("acceptanceCriteria"))
        );
        let stored_count = fixture
            .database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM work_graph_specs WHERE goal_id = ?1",
                    [fixture.goal_id.as_str()],
                    |row| row.get::<_, i64>(0),
                )
            })
            .expect("count stored graph specs");
        assert_eq!(stored_count, 0);
        cleanup(fixture);
    }

    #[test]
    fn rejects_non_durable_parent_profiles() {
        for profile in ["legacy", "durable_v2_shadow", "graph_readonly_preview"] {
            let fixture = fixture(
                json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
                profile,
            );
            let error = activate(&fixture).expect_err("profile must be rejected");
            assert!(matches!(error, RepositoryError::InvalidValidationPolicy(_)));
            cleanup(fixture);
        }
    }

    #[test]
    fn snapshot_survives_database_restart() {
        let fixture = fixture(
            json!([
                { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a accepted"] },
                { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a"], "acceptanceCriteria": ["b accepted"] }
            ]),
            "durable_v2",
        );
        let activated = activate(&fixture).expect("activate graph");
        let path = fixture.path.clone();
        let goal_id = fixture.goal_id.clone();
        drop(fixture);
        let reopened = Database::open(path.clone()).expect("reopen database");
        let snapshot = reopened
            .read_only_graph_snapshot(&goal_id)
            .expect("read graph snapshot")
            .expect("persisted graph");
        assert_eq!(snapshot.spec_hash, activated.snapshot.spec_hash);
        assert_eq!(snapshot.nodes, activated.snapshot.nodes);
        drop(reopened);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reviewer_inconclusive_and_invalid_outputs_are_canonicalized_fail_closed() {
        for raw in [
            Some(
                r#"{"criteria":[{"criterion":"x","status":"passed","toolCallIds":["runtime-1"]}],"findings":[{"criterion":"x","severity":"low","title":"note","detail":"detail","toolCallIds":["runtime-1"]}],"recommendation":"inconclusive","summary":"cannot decide"}"#,
            ),
            Some(
                r#"{"criteria":[],"findings":[],"recommendation":"unknown","summary":"unknown recommendation"}"#,
            ),
            Some("not-json"),
        ] {
            let (outcome, summary, decision_json) =
                authoritative_graph_review_decision("completed", raw, None);
            assert_eq!(outcome, GraphNodeReviewOutcome::Inconclusive);
            assert!(!summary.is_empty());
            let decision: Value = serde_json::from_str(&decision_json).unwrap();
            assert_eq!(decision["recommendation"], "inconclusive");
            assert_eq!(decision["criteria"], json!([]));
            assert_eq!(decision["findings"], json!([]));
            assert_eq!(decision.as_object().unwrap().len(), 4);
        }
        for status in ["failed", "cancelled", "interrupted"] {
            let (outcome, _, decision_json) = authoritative_graph_review_decision(
                status,
                Some(r#"{"recommendation":"pass"}"#),
                Some("terminal failure"),
            );
            assert_eq!(outcome, GraphNodeReviewOutcome::Inconclusive);
            let decision: Value = serde_json::from_str(&decision_json).unwrap();
            assert_eq!(decision["criteria"], json!([]));
            assert_eq!(decision["findings"], json!([]));
        }
    }

    #[test]
    fn generic_goal_acceptance_is_rejected_for_activated_graphs() {
        let fixture = fixture(
            json!([{ "nodeKey": "only", "title": "Only", "ordinal": 0, "acceptanceCriteria": ["result inspected"] }]),
            "durable_v2",
        );
        activate(&fixture).unwrap();
        let error = fixture
            .database
            .accept_goal_with_review(
                &fixture.conversation_id,
                &fixture.goal_id,
                &fixture.plan_revision_id,
                1,
                "generic acceptance must not bypass Graph provenance",
                &json!({}),
                &fixture.run.run.id,
            )
            .expect_err("generic acceptance must be fail-closed for Graph Goals");
        assert!(error
            .to_string()
            .contains("graph.readonly_dedicated_accept_required"));
        cleanup(fixture);
    }
}
