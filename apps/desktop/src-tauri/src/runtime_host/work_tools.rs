use crate::database::{
    ActivateReadOnlyGraphInput, AddEvidenceInput, ChildRunBudget, ChildRunRecord,
    CreateGraphNodeReviewRequestInput, CreateReadOnlyGraphAcceptanceInput, CreateTaskInput,
    Database, EvidenceReferenceKind, EvidenceType, EvidenceValidityStatus,
    FinishReadOnlyGraphNodeInput, FinishTaskAttemptInput, GoalStatus, GraphCriterionEvidenceInput,
    PreflightTaskRepairOverrideInput, RepositoryError, StartReadyReadOnlyGraphNodeInput,
    StartRunResult, StartTaskAttemptInput, StartTaskRepairOverrideInput, TaskAttemptKind,
    TaskAttemptStatus, WorkEventRecord, WorkTaskStatus,
};
use crate::work_mode_gate;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
};

pub const WORK_TOOLS: [&str; 28] = [
    "work_snapshot_get",
    "graph_readonly_activate",
    "graph_readonly_snapshot_get",
    "graph_readonly_node_start",
    "graph_readonly_node_finish",
    "graph_readonly_node_cancel",
    "graph_readonly_node_review",
    "graph_readonly_accept",
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
    "workflow_cancel",
];

const DURABLE_UNINTEGRATED_WORKFLOW_MUTATORS: &[&str] = &[
    "workflow_start",
    "workflow_stage_start",
    "workflow_stage_complete",
    "workflow_stage_fail",
    "workflow_cancel",
];

const DURABLE_V2_PERSISTENT_GRAPH_TOOLS: &[&str] = &[
    "graph_readonly_activate",
    "graph_readonly_snapshot_get",
    "graph_readonly_node_start",
    "graph_readonly_node_finish",
    "graph_readonly_node_cancel",
    "graph_readonly_node_review",
    "graph_readonly_accept",
];

const GRAPH_CHILD_MAX_DURATION_MS: i64 = 45_000;
const GRAPH_CHILD_MAX_TOTAL_TOKENS: i64 = 4_096;
const GRAPH_CHILD_MAX_OUTPUT_TOKENS: i64 = 1_024;
const GRAPH_CHILD_MAX_TOOL_CALLS: i64 = 6;

#[derive(Debug)]
pub struct WorkToolOutcome {
    pub result: Value,
    pub events: Vec<WorkEventRecord>,
    pub post_finalize: Option<WorkToolPostFinalizeDirective>,
}

#[derive(Debug)]
pub enum WorkToolPostFinalizeDirective {
    StartChild(WorkToolChildDispatch),
    CancelGraphChild(WorkToolGraphCancelDirective),
    StartGraphReviewer(WorkToolGraphReviewDirective),
    AcceptReadOnlyGraph(WorkToolGraphAcceptanceDirective),
}

#[derive(Debug)]
pub struct WorkToolChildDispatch {
    pub started: StartRunResult,
    pub child_run: ChildRunRecord,
}

#[derive(Debug)]
pub struct WorkToolGraphCancelDirective {
    pub intent_id: String,
    pub child_run_id: String,
}

#[derive(Debug)]
pub struct WorkToolGraphReviewDirective {
    pub request_id: String,
}

#[derive(Debug)]
pub struct WorkToolGraphAcceptanceDirective {
    pub acceptance_id: String,
}

#[derive(Debug)]
pub struct WorkToolError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl WorkToolError {
    fn new(code: impl Into<String>, message: impl Into<String>, details: Value) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details,
        }
    }
}

impl fmt::Display for WorkToolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for WorkToolError {}

impl From<String> for WorkToolError {
    fn from(message: String) -> Self {
        Self::new("work_tool.failed", message, json!({}))
    }
}

impl From<&str> for WorkToolError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

pub fn is_work_tool(tool: &str) -> bool {
    WORK_TOOLS.contains(&tool)
}

pub fn execute(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    tool: &str,
    input: &Value,
) -> Result<WorkToolOutcome, WorkToolError> {
    execute_with_optional_host_tool_call_id(database, conversation_id, run_id, None, tool, input)
}

pub fn execute_with_host_tool_call_id(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    host_tool_call_id: &str,
    tool: &str,
    input: &Value,
) -> Result<WorkToolOutcome, WorkToolError> {
    execute_with_optional_host_tool_call_id(
        database,
        conversation_id,
        run_id,
        Some(host_tool_call_id),
        tool,
        input,
    )
}

fn execute_with_optional_host_tool_call_id(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    host_tool_call_id: Option<&str>,
    tool: &str,
    input: &Value,
) -> Result<WorkToolOutcome, WorkToolError> {
    if DURABLE_V2_PERSISTENT_GRAPH_TOOLS.contains(&tool) {
        require_durable_v2_graph_profile(database, run_id, tool)?;
    }
    if DURABLE_UNINTEGRATED_WORKFLOW_MUTATORS.contains(&tool) {
        let policy_id = database
            .validation_policy_id_for_run(run_id, None)
            .map_err(repository_work_tool_error)?;
        if policy_id != "legacy_v1" {
            return Err(WorkToolError::new(
                "workflow.durable_attempt_required",
                "durable execution cannot use workflow mutators until workflow stages share the Host Attempt, Evidence, Review, and Acceptance transaction boundary",
                json!({ "tool": tool, "validationPolicyId": policy_id }),
            ));
        }
    }
    let mut events = Vec::new();
    let mut post_finalize = None;
    let details = match tool {
        "work_snapshot_get" => snapshot(database, conversation_id)?,
        "graph_readonly_activate" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_activation_source_missing",
                    "read-only Graph activation requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let activated = database
                .activate_read_only_graph(ActivateReadOnlyGraphInput {
                    plan_revision_id: required_string(input, "planRevisionId")?,
                    parent_run_id: run_id.to_owned(),
                    activation_tool_call_id: host_tool_call_id.to_owned(),
                })
                .map_err(|error| graph_repository_work_tool_error("activation", error))?;
            json!({
                "activated": true,
                "replayed": activated.replayed,
                "graph": activated.snapshot,
            })
        }
        "graph_readonly_snapshot_get" => {
            let goal_id = required_string(input, "goalId")?;
            require_goal(database, conversation_id, &goal_id)?;
            let graph = database
                .read_only_graph_snapshot(&goal_id)
                .map_err(|error| graph_repository_work_tool_error("snapshot", error))?;
            json!({
                "activated": graph.is_some(),
                "graph": graph,
            })
        }
        "graph_readonly_node_start" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_node_start_source_missing",
                    "read-only Graph node start requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let request = parse_graph_node_start_request(input)?;
            let started = database
                .start_ready_read_only_graph_node(StartReadyReadOnlyGraphNodeInput {
                    goal_id: request.goal_id,
                    task_id: request.task_id,
                    parent_run_id: run_id.to_owned(),
                    node_start_tool_call_id: host_tool_call_id.to_owned(),
                    expected_task_version: request.expected_task_version,
                    attempt_id: request.attempt_id,
                    worker_agent_id: request.worker_agent_id,
                    objective: request.objective,
                    context: request.context,
                    budget: request.budget,
                })
                .map_err(graph_node_start_repository_error)?;
            if started.created && !started.replayed && started.child_run.status == "queued" {
                post_finalize = Some(WorkToolPostFinalizeDirective::StartChild(
                    WorkToolChildDispatch {
                        started: started.started.clone(),
                        child_run: started.child_run.clone(),
                    },
                ));
            }
            json!({
                "created": started.created,
                "replayed": started.replayed,
                "childRun": started.child_run,
                "task": started.task,
                "attempt": started.attempt,
                "graph": started.snapshot,
            })
        }
        "graph_readonly_node_finish" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_node_finish_source_missing",
                    "read-only Graph node finish requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let request = parse_graph_node_finish_request(input)?;
            let finished = database
                .finish_read_only_graph_node(FinishReadOnlyGraphNodeInput {
                    goal_id: request.goal_id,
                    task_id: request.task_id,
                    attempt_id: request.attempt_id,
                    parent_run_id: run_id.to_owned(),
                    node_finish_tool_call_id: host_tool_call_id.to_owned(),
                    expected_task_version: request.expected_task_version,
                    expected_attempt_version: request.expected_attempt_version,
                    criterion_evidence: request.criterion_evidence,
                    summary: request.summary,
                })
                .map_err(graph_node_finish_repository_error)?;
            if !finished.replayed {
                if let Ok(event) = database.append_work_event(
                    "task.completed",
                    conversation_id,
                    Some(&finished.finish.task.goal_id),
                    Some(&finished.finish.task.id),
                    Some(run_id),
                    json!({
                        "task": finished.finish.task,
                        "attempt": finished.finish.attempt,
                        "criterionEvidence": finished.criterion_evidence,
                        "childRunId": finished.child_run_id,
                        "childResultHash": finished.child_result_hash,
                        "graphNodeFinish": true,
                    }),
                ) {
                    events.push(event);
                }
            }
            json!({
                "replayed": finished.replayed,
                "childRunId": finished.child_run_id,
                "childResultHash": finished.child_result_hash,
                "criterionEvidence": finished.criterion_evidence,
                "task": finished.finish.task,
                "attempt": finished.finish.attempt,
                "graph": finished.snapshot,
                "goalAcceptanceCreated": false,
            })
        }
        "graph_readonly_node_cancel" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_node_cancel_source_missing",
                    "read-only Graph node cancel requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let request = parse_graph_node_cancel_request(input)?;
            let cancellation = database
                .create_graph_node_cancel_intent(
                    crate::database::CreateGraphNodeCancelIntentInput {
                        goal_id: request.goal_id,
                        task_id: request.task_id,
                        attempt_id: request.attempt_id,
                        parent_run_id: run_id.to_owned(),
                        node_cancel_tool_call_id: host_tool_call_id.to_owned(),
                        expected_task_version: request.expected_task_version,
                        expected_attempt_version: request.expected_attempt_version,
                        reason: request.reason,
                    },
                )
                .map_err(graph_node_cancel_repository_error)?;
            if cancellation.pending_activation {
                post_finalize = Some(WorkToolPostFinalizeDirective::CancelGraphChild(
                    WorkToolGraphCancelDirective {
                        intent_id: cancellation.intent_id.clone(),
                        child_run_id: cancellation.child_run_id.clone(),
                    },
                ));
            }
            json!({
                "created": cancellation.created,
                "replayed": cancellation.replayed,
                "pendingActivation": cancellation.pending_activation,
                "intentId": cancellation.intent_id,
                "childRunId": cancellation.child_run_id,
                "task": cancellation.task,
                "attempt": cancellation.attempt,
                "graph": cancellation.snapshot,
                "cancelCompleted": false,
            })
        }
        "graph_readonly_node_review" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_node_review_source_missing",
                    "read-only Graph node review requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let request = parse_graph_node_review_request(input)?;
            let review = database
                .create_graph_node_review_request(CreateGraphNodeReviewRequestInput {
                    goal_id: request.goal_id,
                    task_id: request.task_id,
                    attempt_id: request.attempt_id,
                    parent_run_id: run_id.to_owned(),
                    review_tool_call_id: host_tool_call_id.to_owned(),
                    expected_task_version: request.expected_task_version,
                    expected_attempt_version: request.expected_attempt_version,
                    criterion_evidence: request.criterion_evidence,
                    summary: request.summary,
                })
                .map_err(graph_node_review_repository_error)?;
            if review.created && review.pending_activation {
                post_finalize = Some(WorkToolPostFinalizeDirective::StartGraphReviewer(
                    WorkToolGraphReviewDirective {
                        request_id: review.request_id.clone(),
                    },
                ));
            }
            json!({
                "created": review.created,
                "replayed": review.replayed,
                "pendingActivation": review.pending_activation,
                "requestId": review.request_id,
                "implementationChildRunId": review.implementation_child_run_id,
                "candidateHash": review.candidate_hash,
                "graph": review.snapshot,
                "reviewCompleted": false,
            })
        }
        "graph_readonly_accept" => {
            let host_tool_call_id = host_tool_call_id.ok_or_else(|| {
                WorkToolError::new(
                    "graph.readonly_accept_source_missing",
                    "read-only Graph acceptance requires the current Host ToolCall identity",
                    json!({ "tool": tool }),
                )
            })?;
            let request = parse_graph_acceptance_request(input)?;
            let acceptance = database
                .create_read_only_graph_acceptance(CreateReadOnlyGraphAcceptanceInput {
                    goal_id: request.goal_id,
                    submitter_run_id: run_id.to_owned(),
                    accept_tool_call_id: host_tool_call_id.to_owned(),
                    expected_goal_version: request.expected_goal_version,
                    summary: request.summary,
                })
                .map_err(graph_acceptance_repository_error)?;
            if acceptance.created && acceptance.pending_activation {
                post_finalize = Some(WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(
                    WorkToolGraphAcceptanceDirective {
                        acceptance_id: acceptance.acceptance_id.clone(),
                    },
                ));
            }
            json!({
                "created": acceptance.created,
                "replayed": acceptance.replayed,
                "pendingActivation": acceptance.pending_activation,
                "acceptanceId": acceptance.acceptance_id,
                "goalAccepted": false,
            })
        }
        "goal_propose" => {
            let user_request = database.run_user_message_content(conversation_id, run_id)?;
            let user_evaluation = user_request.as_deref().map(|request| {
                work_mode_gate::evaluate(work_mode_gate::WorkModeGateInput::user_request(request))
            });
            if let Some(evaluation) = user_evaluation.filter(|evaluation| {
                evaluation.decision == work_mode_gate::WorkModeDecision::StayConversation
            }) {
                json!({
                    "goal": null,
                    "gateDecision": evaluation.decision,
                    "gateReasons": evaluation.reasons,
                    "message": "The current user message is conversational content. Continue with a normal answer and do not create a Goal.",
                })
            } else {
                let applied = work_mode_gate::apply_runtime_proposal(
                    database,
                    conversation_id,
                    run_id,
                    required_string(input, "title")?,
                    required_string(input, "objective")?,
                    optional_string(input, "acceptanceSummary")?,
                )?;
                if let Some(event) = applied.event {
                    events.push(event);
                }
                json!({
                    "goal": applied.goal,
                    "gateDecision": applied.evaluation.decision,
                    "gateReasons": applied.evaluation.reasons,
                })
            }
        }
        "goal_complete" => {
            let goal_id = required_string(input, "goalId")?;
            if database
                .read_only_graph_snapshot(&goal_id)
                .map_err(repository_work_tool_error)?
                .is_some()
            {
                return Err(WorkToolError::new(
                    "graph.readonly_dedicated_accept_required",
                    "an activated read-only Graph can complete its Goal only through graph_readonly_accept",
                    json!({ "goalId": goal_id, "tool": "graph_readonly_accept" }),
                ));
            }
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err(WorkToolError::new(
                    "goal.not_active",
                    "only an active goal can be completed",
                    json!({ "goalId": goal_id, "status": goal.status }),
                ));
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let (_, tasks, evidence) = database
                .load_work_graph_snapshot(conversation_id)
                .map_err(|error| error.to_string())?;
            let goal_tasks = tasks
                .iter()
                .filter(|task| task.goal_id == goal_id)
                .collect::<Vec<_>>();
            if goal_tasks.is_empty() {
                return Err(WorkToolError::new(
                    "goal.tasks_required",
                    "a goal cannot complete before at least one task is recorded",
                    json!({ "goalId": goal_id }),
                ));
            }
            let unfinished = goal_tasks
                .iter()
                .filter(|task| {
                    !matches!(
                        task.status,
                        WorkTaskStatus::Completed | WorkTaskStatus::Skipped
                    )
                })
                .map(
                    |task| json!({ "taskId": task.id, "title": task.title, "status": task.status }),
                )
                .collect::<Vec<_>>();
            if !unfinished.is_empty() {
                return Err(WorkToolError::new(
                    "goal.tasks_unfinished",
                    "a goal cannot complete while required tasks remain unfinished",
                    json!({ "goalId": goal_id, "tasks": unfinished }),
                ));
            }
            let validation_policies = goal_tasks
                .iter()
                .map(|task| database.task_validation_policy(&task.id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(repository_work_tool_error)?;
            let durable_skipped = goal_tasks
                .iter()
                .zip(validation_policies.iter())
                .filter(|(task, policy)| {
                    task.status == WorkTaskStatus::Skipped
                        && policy.snapshot.id != "legacy_v1"
                })
                .map(|(task, policy)| {
                    json!({ "taskId": task.id, "validationPolicy": policy.snapshot })
                })
                .collect::<Vec<_>>();
            if !durable_skipped.is_empty() {
                return Err(WorkToolError::new(
                    "task.skip_not_authorized",
                    "durable Tasks cannot complete a Goal through model-selected skip",
                    json!({ "goalId": goal_id, "tasks": durable_skipped }),
                ));
            }
            let tasks_without_valid_evidence = goal_tasks
                .iter()
                .filter(|task| task.status == WorkTaskStatus::Completed)
                .filter(|task| {
                    !evidence.iter().any(|item| {
                        item.task_id == task.id
                            && item.validity_status == EvidenceValidityStatus::Valid
                    })
                })
                .map(|task| {
                    let evidence_statuses = evidence
                        .iter()
                        .filter(|item| item.task_id == task.id)
                        .map(|item| {
                            json!({
                                "evidenceId": item.id,
                                "validityStatus": item.validity_status,
                                "invalidReason": item.invalid_reason,
                            })
                        })
                        .collect::<Vec<_>>();
                    json!({
                        "taskId": task.id,
                        "title": task.title,
                        "evidence": evidence_statuses,
                    })
                })
                .collect::<Vec<_>>();
            if !tasks_without_valid_evidence.is_empty() {
                return Err(WorkToolError::new(
                    "goal.valid_evidence_required",
                    "each completed task needs at least one valid evidence record before the goal can complete",
                    json!({ "goalId": goal_id, "tasks": tasks_without_valid_evidence }),
                ));
            }
            let acceptance_required = validation_policies
                .into_iter()
                .filter(|policy| policy.snapshot.completion_requires_acceptance)
                .collect::<Vec<_>>();
            if !acceptance_required.is_empty() {
                return Err(WorkToolError::new(
                    "goal.acceptance_required",
                    "one or more frozen ValidationPolicies require acceptance_submit; goal_complete cannot bypass review and acceptance",
                    json!({ "goalId": goal_id, "validationPolicies": acceptance_required }),
                ));
            }
            let goal = database
                .goals()
                .complete(&goal_id, required_version(input)?)
                .map_err(|error| error.to_string())?;
            if let Ok(event) = database.append_work_event(
                "goal.completed",
                conversation_id,
                Some(&goal_id),
                None,
                Some(run_id),
                json!({ "goal": goal }),
            ) {
                events.push(event);
            }
            json!({ "goal": goal })
        }
        "task_create_many" => {
            let goal_id = required_string(input, "goalId")?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("tasks can only be added after Fox Host activates the goal".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let submitted_tasks = input
                .get("tasks")
                .and_then(Value::as_array)
                .filter(|tasks| !tasks.is_empty())
                .ok_or_else(|| "tasks must be a non-empty array".to_owned())?;
            if submitted_tasks.len() > 64 {
                return Err("tasks cannot contain more than 64 items".into());
            }
            let policy_ids = submitted_tasks
                .iter()
                .map(|task| {
                    let risk_hint = task.get("riskLevel").and_then(Value::as_str);
                    database
                        .validation_policy_id_for_run(run_id, risk_hint)
                        .map(str::to_owned)
                        .map_err(|error| error.to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            let tasks = submitted_tasks
                .iter()
                .map(|task| {
                    Ok(CreateTaskInput {
                        id: None,
                        goal_id: goal_id.clone(),
                        parent_task_id: None,
                        ordinal: task
                            .get("ordinal")
                            .and_then(Value::as_i64)
                            .filter(|ordinal| *ordinal >= 0)
                            .ok_or_else(|| {
                                "task ordinal must be a non-negative integer".to_owned()
                            })?,
                        title: required_string(task, "title")?,
                        detail: optional_string(task, "detail")?,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let tasks = database
                .work_tasks()
                .create_many_with_policy_ids(tasks, policy_ids)
                .map_err(|error| error.to_string())?;
            for task in &tasks {
                if let Ok(event) = database.append_work_event(
                    "task.created",
                    conversation_id,
                    Some(&goal_id),
                    Some(&task.id),
                    Some(run_id),
                    json!({ "task": task }),
                ) {
                    events.push(event);
                }
            }
            let policies = tasks
                .iter()
                .map(|task| database.task_validation_policy(&task.id))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            json!({ "tasks": tasks, "validationPolicies": policies })
        }
        "task_update" => {
            let task_id = required_string(input, "taskId")?;
            let (task, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before updating tasks".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let status = parse_task_status(
                input
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "status is required".to_owned())?,
            )?;
            let policy = database
                .task_validation_policy(&task.id)
                .map_err(|error| error.to_string())?;
            if policy.snapshot.id != "legacy_v1"
                && matches!(
                    status,
                    WorkTaskStatus::InProgress
                        | WorkTaskStatus::Completed
                        | WorkTaskStatus::Blocked
                        | WorkTaskStatus::Interrupted
                        | WorkTaskStatus::Skipped
                )
            {
                return Err(WorkToolError::new(
                    "task.attempt_required",
                    "durable Tasks must change execution state through task_attempt_start, task_attempt_finish, or task_repair_start",
                    json!({ "taskId": task.id, "validationPolicy": policy.snapshot }),
                ));
            }
            let blocked_reason = optional_string(input, "blockedReason")?;
            let owner_run_id = (status == WorkTaskStatus::InProgress).then_some(run_id);
            let task = database
                .work_tasks()
                .update(
                    &task.id,
                    required_version(input)?,
                    status.clone(),
                    owner_run_id,
                    blocked_reason,
                )
                .map_err(|error| error.to_string())?;
            if let Some(event_type) = task_event_type(&status) {
                if let Ok(event) = database.append_work_event(
                    event_type,
                    conversation_id,
                    Some(&goal_id),
                    Some(&task.id),
                    Some(run_id),
                    json!({ "task": task }),
                ) {
                    events.push(event);
                }
            }
            json!({ "task": task })
        }
        "task_attempt_start" => {
            let task_id = required_string(input, "taskId")?;
            let (_, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before starting an Attempt".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let result = database
                .start_task_attempt(StartTaskAttemptInput {
                    id: required_string(input, "attemptId")?,
                    task_id: task_id.clone(),
                    run_id: run_id.to_owned(),
                    expected_task_version: required_version(input)?,
                    kind: TaskAttemptKind::Execution,
                    root_cause: None,
                    finding_ids: Vec::new(),
                })
                .map_err(repository_work_tool_error)?;
            if let Some(event) = database
                .append_work_event(
                    "task.started",
                    conversation_id,
                    Some(&goal_id),
                    Some(&task_id),
                    Some(run_id),
                    json!({ "task": result.task, "attempt": result.attempt }),
                )
                .ok()
            {
                events.push(event);
            }
            json!({ "task": result.task, "attempt": result.attempt })
        }
        "task_repair_start" => {
            let task_id = required_string(input, "taskId")?;
            let (_, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before starting a repair".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let finding_ids = required_string_array(input, "findingIds", 32)?;
            let result = database
                .start_task_attempt(StartTaskAttemptInput {
                    id: required_string(input, "attemptId")?,
                    task_id: task_id.clone(),
                    run_id: run_id.to_owned(),
                    expected_task_version: required_version(input)?,
                    kind: TaskAttemptKind::Repair,
                    root_cause: Some(required_string(input, "rootCause")?),
                    finding_ids,
                })
                .map_err(repository_work_tool_error)?;
            if result.budget_exhausted {
                if let Ok(event) = database.append_work_event(
                    "task.blocked",
                    conversation_id,
                    Some(&goal_id),
                    Some(&task_id),
                    Some(run_id),
                    json!({
                        "task": result.task,
                        "reasonCode": "repair_budget_exhausted",
                        "humanEscalationRequired": true
                    }),
                ) {
                    events.push(event);
                }
                return Err(WorkToolError::new(
                    "task.repair_budget_exhausted",
                    "the frozen ValidationPolicy repair budget is exhausted; the Task is now genuinely blocked for human escalation",
                    json!({ "task": result.task, "humanEscalationRequired": true }),
                ));
            }
            if let Ok(event) = database.append_work_event(
                "task.started",
                conversation_id,
                Some(&goal_id),
                Some(&task_id),
                Some(run_id),
                json!({ "task": result.task, "attempt": result.attempt, "repair": true }),
            ) {
                events.push(event);
            }
            json!({ "task": result.task, "attempt": result.attempt })
        }
        "task_repair_escalate_start" => {
            return Err(WorkToolError::new(
                "task.repair_override_approval_required",
                "task_repair_escalate_start can execute only through the Host-bound allow_once approval path",
                json!({}),
            ));
        }
        "task_attempt_finish" => {
            let task_id = required_string(input, "taskId")?;
            let (_, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before finishing an Attempt".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let attempt_status = parse_attempt_status(&required_string(input, "status")?)?;
            let result = database
                .finish_task_attempt(FinishTaskAttemptInput {
                    id: required_string(input, "attemptId")?,
                    task_id: task_id.clone(),
                    run_id: run_id.to_owned(),
                    expected_task_version: required_version(input)?,
                    expected_attempt_version: input
                        .get("expectedAttemptVersion")
                        .and_then(Value::as_i64)
                        .filter(|version| *version >= 1)
                        .ok_or_else(|| "expectedAttemptVersion must be at least 1".to_owned())?,
                    status: attempt_status,
                    failure_reason: optional_string(input, "failureReason")?,
                })
                .map_err(repository_work_tool_error)?;
            if let Some(event_type) = task_event_type(&result.task.status) {
                if let Ok(event) = database.append_work_event(
                    event_type,
                    conversation_id,
                    Some(&goal_id),
                    Some(&task_id),
                    Some(run_id),
                    json!({ "task": result.task, "attempt": result.attempt }),
                ) {
                    events.push(event);
                }
            }
            json!({ "task": result.task, "attempt": result.attempt })
        }
        "task_evidence_add" => {
            let task_id = required_string(input, "taskId")?;
            let (_, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before adding evidence".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let evidence_type: EvidenceType = serde_json::from_value(
                input
                    .get("evidenceType")
                    .cloned()
                    .ok_or_else(|| "evidenceType is required".to_owned())?,
            )
            .map_err(|_| "unsupported evidenceType".to_owned())?;
            let ref_kind: EvidenceReferenceKind = serde_json::from_value(
                input
                    .get("refKind")
                    .cloned()
                    .ok_or_else(|| "refKind is required".to_owned())?,
            )
            .map_err(|_| "unsupported refKind".to_owned())?;
            let policy = database
                .task_validation_policy(&task_id)
                .map_err(repository_work_tool_error)?;
            let validation_check_type = optional_string(input, "validationCheckType")?;
            if let Some(check_type) = validation_check_type.as_deref() {
                if !policy
                    .snapshot
                    .allowed_check_types
                    .iter()
                    .any(|allowed| allowed == check_type)
                {
                    return Err(WorkToolError::new(
                        "task.validation_check_type_not_allowed",
                        "the frozen ValidationPolicy does not allow this validation check type",
                        json!({ "taskId": task_id, "checkType": check_type, "validationPolicy": policy.snapshot }),
                    ));
                }
            }
            let evidence = database
                .task_evidence()
                .add(AddEvidenceInput {
                    id: None,
                    task_id: task_id.clone(),
                    source_run_id: Some(run_id.to_owned()),
                    evidence_type,
                    ref_kind,
                    ref_id: required_string(input, "refId")?,
                    summary: required_string(input, "summary")?,
                    metadata: validation_check_type
                        .map(|check_type| json!({ "validationCheckType": check_type }))
                        .unwrap_or_else(|| json!({})),
                    trace_id: None,
                    span_id: None,
                })
                .map_err(|error| error.to_string())?;
            if let Ok(event) = database.append_work_event(
                "evidence.added",
                conversation_id,
                Some(&goal_id),
                Some(&task_id),
                Some(run_id),
                json!({ "evidence": evidence }),
            ) {
                events.push(event);
            }
            json!({ "evidence": evidence })
        }
        "task_evidence_validate" => {
            let evidence_id = required_string(input, "evidenceId")?;
            let evidence = database
                .task_evidence()
                .get(&evidence_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("task evidence '{evidence_id}' was not found"))?;
            let (_, goal_id) = require_task(database, conversation_id, &evidence.task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before validating evidence".into());
            }
            require_no_pending_plan(database, conversation_id, &goal_id)?;
            let status = database
                .task_evidence()
                .validate(&evidence_id)
                .map_err(|error| error.to_string())?;
            let evidence = database
                .task_evidence()
                .get(&evidence_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("task evidence '{evidence_id}' was not found"))?;
            if let Ok(event) = database.append_work_event(
                "evidence.validated",
                conversation_id,
                Some(&goal_id),
                Some(&evidence.task_id),
                Some(run_id),
                json!({ "evidenceId": evidence_id, "validityStatus": status }),
            ) {
                events.push(event);
            }
            json!({ "evidence": evidence, "validityStatus": status })
        }
        "plan_revision_create" => {
            let goal_id = required_string(input, "goalId")?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err(WorkToolError::new(
                    "plan.goal_not_active",
                    "a plan revision requires an active goal",
                    json!({ "goalId": goal_id, "status": goal.status }),
                ));
            }
            let tasks = validate_plan_tasks(input, &goal_id)?;
            let revision = database.create_plan_revision(
                conversation_id,
                &goal_id,
                &required_string(input, "title")?,
                &required_string(input, "summary")?,
                tasks,
                run_id,
            )?;
            if let Ok(event) = database.append_work_event(
                "plan.revised",
                conversation_id,
                Some(&goal_id),
                None,
                Some(run_id),
                json!({ "planRevision": revision }),
            ) {
                events.push(event);
            }
            json!({ "planRevision": revision })
        }
        "review_finding_add" => {
            let goal_id = required_string(input, "goalId")?;
            let _goal = require_goal(database, conversation_id, &goal_id)?;
            let task_id = optional_string(input, "taskId")?;
            if let Some(task_id) = task_id.as_deref() {
                let (_, task_goal_id) = require_task(database, conversation_id, task_id)?;
                if task_goal_id != goal_id {
                    return Err(WorkToolError::new(
                        "review.task_goal_mismatch",
                        "review task must belong to the reviewed goal",
                        json!({ "goalId": goal_id, "taskId": task_id }),
                    ));
                }
            }
            let severity = required_string(input, "severity")?;
            if !["critical", "high", "medium", "low", "info"].contains(&severity.as_str()) {
                return Err("unsupported review severity".into());
            }
            let status = optional_string(input, "status")?.unwrap_or_else(|| "open".to_owned());
            if !["open", "resolved", "waived"].contains(&status.as_str()) {
                return Err("unsupported review status".into());
            }
            let plan_revision_id = required_string(input, "planRevisionId")?;
            let (plans, _, _) = database.load_a1_snapshot(conversation_id)?;
            let reviewed_plan = plans
                .iter()
                .find(|plan| plan.id == plan_revision_id && plan.goal_id == goal_id)
                .ok_or_else(|| {
                    WorkToolError::new(
                        "review.plan_not_found",
                        "the reviewed PlanRevision was not found for this goal",
                        json!({ "goalId": goal_id, "planRevisionId": plan_revision_id }),
                    )
                })?;
            if reviewed_plan.status != "approved" {
                return Err(WorkToolError::new(
                    "review.plan_not_approved",
                    "independent review requires a user-approved PlanRevision",
                    json!({ "goalId": goal_id, "planRevisionId": plan_revision_id, "status": reviewed_plan.status }),
                ));
            }
            if let Some(pending) = plans.iter().find(|candidate| {
                candidate.goal_id == goal_id
                    && candidate.status == "proposed"
                    && candidate.revision > reviewed_plan.revision
            }) {
                return Err(WorkToolError::new(
                    "review.plan_pending",
                    "review cannot finalize while a newer PlanRevision awaits user approval",
                    json!({ "goalId": goal_id, "reviewedPlanRevisionId": reviewed_plan.id, "pendingPlanRevisionId": pending.id }),
                ));
            }
            let (_, goal_tasks, _) = database
                .load_work_graph_snapshot(conversation_id)
                .map_err(|error| error.to_string())?;
            let reviewed_task_ids = goal_tasks
                .iter()
                .filter(|task| task.goal_id == goal_id)
                .filter(|task| task_id.as_deref().is_none_or(|id| task.id == id))
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>();
            if reviewed_task_ids.iter().any(|task_id| {
                database
                    .task_has_attempt_run(task_id, run_id)
                    .unwrap_or(true)
            }) {
                return Err(WorkToolError::new(
                    "review.not_independent",
                    "independent review must run after implementation in a different Fox run",
                    json!({ "goalId": goal_id, "implementationRunId": run_id }),
                ));
            }
            let finding = database.add_review_finding(
                conversation_id,
                &goal_id,
                task_id.as_deref(),
                Some(&plan_revision_id),
                &severity,
                &required_string(input, "category")?,
                &required_string(input, "title")?,
                &required_string(input, "detail")?,
                &status,
                run_id,
            )?;
            if let Ok(event) = database.append_work_event(
                "review.finding_added",
                conversation_id,
                Some(&goal_id),
                task_id.as_deref(),
                Some(run_id),
                json!({ "finding": finding }),
            ) {
                events.push(event);
            }
            json!({ "finding": finding })
        }
        "review_finding_resolve" => {
            let finding_id = required_string(input, "findingId")?;
            let status = required_string(input, "status")?;
            if !["resolved", "waived"].contains(&status.as_str()) {
                return Err("status must be resolved or waived".into());
            }
            let (_, findings, _) = database.load_a1_snapshot(conversation_id)?;
            let existing = findings
                .iter()
                .find(|finding| finding.id == finding_id)
                .ok_or_else(|| "review finding was not found in this conversation".to_owned())?;
            if ["critical", "high", "medium"].contains(&existing.severity.as_str()) {
                let affected_tasks = if let Some(task_id) = existing.task_id.as_deref() {
                    vec![require_task(database, conversation_id, task_id)?.0]
                } else {
                    database
                        .work_tasks()
                        .list_by_goal(&existing.goal_id)
                        .map_err(|error| error.to_string())?
                };
                let implementation_owned_high_risk = affected_tasks.iter().any(|task| {
                    database
                        .task_validation_policy(&task.id)
                        .is_ok_and(|policy| policy.snapshot.reviewer_policy == "independent")
                        && database
                            .task_has_attempt_run(&task.id, run_id)
                            .unwrap_or(true)
                });
                if implementation_owned_high_risk {
                    return Err(WorkToolError::new(
                        "review.not_independent",
                        "the implementation run cannot provide final resolution for a high-risk critical/high finding",
                        json!({ "findingId": finding_id, "implementationRunId": run_id }),
                    ));
                }
            }
            let finding = database
                .resolve_review_finding(&finding_id, &status, run_id)?
                .ok_or_else(|| "review finding was not found".to_owned())?;
            if let Ok(event) = database.append_work_event(
                "review.finding_resolved",
                conversation_id,
                Some(&existing.goal_id),
                existing.task_id.as_deref(),
                Some(run_id),
                json!({ "finding": finding }),
            ) {
                events.push(event);
            }
            json!({ "finding": finding })
        }
        "acceptance_submit" => {
            let goal_id = required_string(input, "goalId")?;
            if database
                .read_only_graph_snapshot(&goal_id)
                .map_err(repository_work_tool_error)?
                .is_some()
            {
                return Err(WorkToolError::new(
                    "graph.readonly_dedicated_accept_required",
                    "an activated read-only Graph requires Host-derived graph_readonly_accept instead of model-supplied acceptance checks",
                    json!({ "goalId": goal_id, "tool": "graph_readonly_accept" }),
                ));
            }
            require_goal(database, conversation_id, &goal_id)?;
            let task_summary = validate_goal_for_completion(database, conversation_id, &goal_id)?;
            let (plans, findings, _) = database.load_a1_snapshot(conversation_id)?;
            let plan = plans
                .iter()
                .filter(|plan| plan.goal_id == goal_id && plan.status == "approved")
                .max_by_key(|plan| plan.revision)
                .ok_or_else(|| {
                    WorkToolError::new(
                        "acceptance.plan_required",
                        "an approved PlanRevision is required before final acceptance",
                        json!({ "goalId": goal_id }),
                    )
                })?;
            if let Some(pending) = plans.iter().find(|candidate| {
                candidate.goal_id == goal_id
                    && candidate.status == "proposed"
                    && candidate.revision > plan.revision
            }) {
                return Err(WorkToolError::new(
                    "acceptance.plan_pending",
                    "a newer PlanRevision is still awaiting user approval",
                    json!({ "goalId": goal_id, "approvedPlanRevisionId": plan.id, "pendingPlanRevisionId": pending.id }),
                ));
            }
            let goal_findings = findings
                .iter()
                .filter(|finding| {
                    finding.goal_id == goal_id
                        && finding.plan_revision_id.as_deref() == Some(plan.id.as_str())
                })
                .collect::<Vec<_>>();
            if goal_findings.is_empty() {
                return Err(WorkToolError::new(
                    "acceptance.review_required",
                    "an independent review record is required before final acceptance",
                    json!({ "goalId": goal_id }),
                ));
            }
            let reviewer_findings = goal_findings
                .iter()
                .filter(|finding| finding.created_by == run_id)
                .collect::<Vec<_>>();
            if reviewer_findings.is_empty() {
                return Err(WorkToolError::new(
                    "acceptance.reviewer_run_required",
                    "final acceptance must be submitted by the run that reviewed the approved plan",
                    json!({ "goalId": goal_id, "planRevisionId": plan.id, "reviewerRunId": run_id }),
                ));
            }
            let (_, goal_tasks, _) = database
                .load_work_graph_snapshot(conversation_id)
                .map_err(|error| error.to_string())?;
            let goal_task_ids = goal_tasks
                .iter()
                .filter(|task| task.goal_id == goal_id)
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>();
            if goal_task_ids.iter().any(|task_id| {
                database
                    .task_has_attempt_run(task_id, run_id)
                    .unwrap_or(true)
            }) {
                return Err(WorkToolError::new(
                    "acceptance.reviewer_not_independent",
                    "the acceptance reviewer run must be different from every implementation evidence run",
                    json!({ "goalId": goal_id, "reviewerRunId": run_id }),
                ));
            }
            let blockers = findings.iter().filter(|finding| {
                finding.goal_id == goal_id
                    && finding.status == "open"
                    && ["critical", "high", "medium"].contains(&finding.severity.as_str())
            }).map(|finding| json!({ "findingId": finding.id, "severity": finding.severity, "title": finding.title })).collect::<Vec<_>>();
            if !blockers.is_empty() {
                return Err(WorkToolError::new(
                    "acceptance.review_blocked",
                    "open review findings block final acceptance",
                    json!({ "goalId": goal_id, "findings": blockers }),
                ));
            }
            let checks = validate_acceptance_checks(
                database,
                conversation_id,
                &goal_id,
                run_id,
                input,
                task_summary,
            )?;
            let summary = required_string(input, "summary")?;
            let (acceptance, goal) = database
                .accept_goal_with_review(
                    conversation_id,
                    &goal_id,
                    &plan.id,
                    required_version(input)?,
                    &summary,
                    &checks,
                    run_id,
                )
                .map_err(repository_work_tool_error)?;
            if let Ok(event) = database.append_work_event(
                "acceptance.completed",
                conversation_id,
                Some(&goal_id),
                None,
                Some(run_id),
                json!({ "acceptance": acceptance, "goal": goal }),
            ) {
                events.push(event);
            }
            json!({ "acceptance": acceptance, "goal": goal })
        }
        tool if crate::expert_workflows::is_workflow_tool(tool) => {
            crate::expert_workflows::execute_tool(database, conversation_id, run_id, tool, input)?
        }
        _ => return Err(format!("unsupported work tool: {tool}").into()),
    };
    let mut details = details;
    if let Some(object) = details.as_object_mut() {
        object.insert("workEvents".to_owned(), json!(events));
    }
    Ok(WorkToolOutcome {
        result: tool_result(details),
        events,
        post_finalize,
    })
}

pub(crate) fn validate_task_repair_override_request(input: &Value) -> Result<(), WorkToolError> {
    parse_task_repair_override_request(input).map(|_| ())
}

pub(crate) fn canonicalize_task_repair_override_request(
    input: &Value,
) -> Result<Value, WorkToolError> {
    Ok(parse_task_repair_override_request(input)?.canonical_value())
}

pub(crate) fn preflight_task_repair_override(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    input: &Value,
) -> Result<(), WorkToolError> {
    let request = parse_task_repair_override_request(input)?;
    database
        .preflight_task_repair_override(PreflightTaskRepairOverrideInput {
            task_id: request.task_id,
            attempt_id: request.attempt_id,
            run_id: run_id.to_owned(),
            conversation_id: conversation_id.to_owned(),
            expected_task_version: request.expected_version,
            root_cause: request.root_cause,
            finding_ids: request.finding_ids,
            escalation_reason: request.escalation_reason,
        })
        .map_err(repository_work_tool_error)
}

pub(crate) fn execute_approved_task_repair_override(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    tool_call_id: &str,
    approval_id: &str,
    input: &Value,
) -> Result<WorkToolOutcome, WorkToolError> {
    let request = parse_task_repair_override_request(input)?;
    let result = database
        .start_task_repair_override(StartTaskRepairOverrideInput {
            task_id: request.task_id.clone(),
            attempt_id: request.attempt_id,
            run_id: run_id.to_owned(),
            conversation_id: conversation_id.to_owned(),
            tool_call_id: tool_call_id.to_owned(),
            approval_id: approval_id.to_owned(),
            expected_task_version: request.expected_version,
            root_cause: request.root_cause,
            finding_ids: request.finding_ids,
            escalation_reason: request.escalation_reason,
        })
        .map_err(repository_work_tool_error)?;
    let mut events = Vec::new();
    if let Ok(event) = database.append_work_event(
        "task.started",
        conversation_id,
        Some(&result.task.goal_id),
        Some(&result.task.id),
        Some(run_id),
        json!({
            "task": result.task,
            "attempt": result.attempt,
            "repair": true,
            "humanRepairBudgetOverride": result.override_event,
        }),
    ) {
        events.push(event);
    }
    let details = json!({
        "task": result.task,
        "attempt": result.attempt,
        "repairOverride": result.override_event,
        "workEvents": events,
    });
    Ok(WorkToolOutcome {
        result: tool_result(details),
        events,
        post_finalize: None,
    })
}

#[derive(Debug)]
struct TaskRepairOverrideRequest {
    task_id: String,
    attempt_id: String,
    expected_version: i64,
    root_cause: String,
    finding_ids: Vec<String>,
    escalation_reason: String,
}

impl TaskRepairOverrideRequest {
    fn canonical_value(&self) -> Value {
        json!({
            "taskId": self.task_id,
            "attemptId": self.attempt_id,
            "expectedVersion": self.expected_version,
            "rootCause": self.root_cause,
            "findingIds": self.finding_ids,
            "escalationReason": self.escalation_reason,
        })
    }
}

#[derive(Debug)]
struct GraphNodeStartRequest {
    goal_id: String,
    task_id: String,
    expected_task_version: i64,
    attempt_id: String,
    worker_agent_id: String,
    objective: String,
    context: String,
    budget: ChildRunBudget,
}

#[derive(Debug)]
struct GraphNodeFinishRequest {
    goal_id: String,
    task_id: String,
    attempt_id: String,
    expected_task_version: i64,
    expected_attempt_version: i64,
    criterion_evidence: Vec<GraphCriterionEvidenceInput>,
    summary: String,
}

#[derive(Debug)]
struct GraphNodeCancelRequest {
    goal_id: String,
    task_id: String,
    attempt_id: String,
    expected_task_version: i64,
    expected_attempt_version: i64,
    reason: String,
}

type GraphNodeReviewRequest = GraphNodeFinishRequest;

#[derive(Debug)]
struct GraphAcceptanceRequest {
    goal_id: String,
    expected_goal_version: i64,
    summary: String,
}

fn parse_graph_node_start_request(input: &Value) -> Result<GraphNodeStartRequest, WorkToolError> {
    const FIELDS: &[&str] = &[
        "goalId",
        "taskId",
        "expectedTaskVersion",
        "attemptId",
        "workerAgentId",
        "objective",
        "context",
        "budget",
    ];
    const BUDGET_FIELDS: &[&str] = &[
        "maxDurationMs",
        "maxTotalTokens",
        "maxOutputTokens",
        "maxToolCalls",
    ];
    let invalid = |message: String, details: Value| {
        WorkToolError::new("graph.readonly_node_start_invalid_input", message, details)
    };
    let invalid_budget = |message: String, details: Value| {
        WorkToolError::new("graph.readonly_node_budget_invalid", message, details)
    };
    let object = input.as_object().ok_or_else(|| {
        invalid(
            "read-only Graph node-start input must be an object".to_owned(),
            json!({}),
        )
    })?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(invalid(
            format!("read-only Graph node-start input contains unsupported field '{field}'"),
            json!({ "field": field }),
        ));
    }
    let text = |field: &str, maximum: usize| -> Result<String, WorkToolError> {
        let value = input.get(field).and_then(Value::as_str).ok_or_else(|| {
            invalid(
                format!("{field} must be a string"),
                json!({ "field": field }),
            )
        })?;
        if has_noncanonical_surrounding_whitespace(value) {
            return Err(invalid(
                format!("{field} must be non-empty canonical text without surrounding whitespace"),
                json!({ "field": field }),
            ));
        }
        if value.chars().count() > maximum {
            return Err(invalid(
                format!("{field} cannot exceed {maximum} characters"),
                json!({ "field": field, "maximumChars": maximum }),
            ));
        }
        Ok(value.to_owned())
    };
    let expected_task_version = input
        .get("expectedTaskVersion")
        .and_then(Value::as_i64)
        .filter(|version| *version >= 1)
        .ok_or_else(|| {
            invalid(
                "expectedTaskVersion must be an integer of at least 1".to_owned(),
                json!({ "field": "expectedTaskVersion" }),
            )
        })?;
    let budget_value = input
        .get("budget")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            invalid_budget(
                "budget must be an object".to_owned(),
                json!({ "field": "budget" }),
            )
        })?;
    if let Some(field) = budget_value
        .keys()
        .find(|field| !BUDGET_FIELDS.contains(&field.as_str()))
    {
        return Err(invalid_budget(
            format!("budget contains unsupported field '{field}'"),
            json!({ "field": format!("budget.{field}") }),
        ));
    }
    let budget_integer = |field: &str, minimum: i64, maximum: i64| {
        budget_value
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| (minimum..=maximum).contains(value))
            .ok_or_else(|| {
                invalid_budget(
                    format!("budget.{field} must be an integer between {minimum} and {maximum}"),
                    json!({
                        "field": format!("budget.{field}"),
                        "minimum": minimum,
                        "maximum": maximum,
                    }),
                )
            })
    };
    let budget = ChildRunBudget {
        max_duration_ms: budget_integer("maxDurationMs", 1_000, GRAPH_CHILD_MAX_DURATION_MS)?,
        max_total_tokens: budget_integer("maxTotalTokens", 256, GRAPH_CHILD_MAX_TOTAL_TOKENS)?,
        max_output_tokens: budget_integer("maxOutputTokens", 64, GRAPH_CHILD_MAX_OUTPUT_TOKENS)?,
        max_tool_calls: budget_integer("maxToolCalls", 0, GRAPH_CHILD_MAX_TOOL_CALLS)?,
    };
    if budget.max_output_tokens > budget.max_total_tokens {
        return Err(invalid_budget(
            "budget.maxOutputTokens cannot exceed budget.maxTotalTokens".to_owned(),
            json!({ "field": "budget.maxOutputTokens" }),
        ));
    }
    Ok(GraphNodeStartRequest {
        goal_id: text("goalId", 200)?,
        task_id: text("taskId", 200)?,
        expected_task_version,
        attempt_id: text("attemptId", 200)?,
        worker_agent_id: text("workerAgentId", 200)?,
        objective: text("objective", 8_000)?,
        context: text("context", 12_000)?,
        budget,
    })
}

fn parse_graph_node_finish_request(input: &Value) -> Result<GraphNodeFinishRequest, WorkToolError> {
    const FIELDS: &[&str] = &[
        "goalId",
        "taskId",
        "attemptId",
        "expectedTaskVersion",
        "expectedAttemptVersion",
        "criterionEvidence",
        "summary",
    ];
    const CRITERION_FIELDS: &[&str] = &["criterion", "evidenceIds"];
    let invalid = |message: String, details: Value| {
        WorkToolError::new("graph.readonly_node_finish_invalid_input", message, details)
    };
    let object = input.as_object().ok_or_else(|| {
        invalid(
            "read-only Graph node-finish input must be an object".to_owned(),
            json!({}),
        )
    })?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(invalid(
            format!("read-only Graph node-finish input contains unsupported field '{field}'"),
            json!({ "field": field }),
        ));
    }
    let text = |field: &str, maximum: usize| -> Result<String, WorkToolError> {
        let value = input.get(field).and_then(Value::as_str).ok_or_else(|| {
            invalid(
                format!("{field} must be a string"),
                json!({ "field": field }),
            )
        })?;
        if has_noncanonical_surrounding_whitespace(value) || value.chars().count() > maximum {
            return Err(invalid(
                format!("{field} must be non-empty canonical text of at most {maximum} characters"),
                json!({ "field": field, "maximumChars": maximum }),
            ));
        }
        Ok(value.to_owned())
    };
    let version = |field: &str| -> Result<i64, WorkToolError> {
        input
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 1)
            .ok_or_else(|| {
                invalid(
                    format!("{field} must be an integer of at least 1"),
                    json!({ "field": field }),
                )
            })
    };
    let criterion_values = input
        .get("criterionEvidence")
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty() && values.len() <= 8)
        .ok_or_else(|| {
            invalid(
                "criterionEvidence must contain between 1 and 8 entries".to_owned(),
                json!({ "field": "criterionEvidence" }),
            )
        })?;
    let mut criteria = HashSet::new();
    let mut total_bindings = 0usize;
    let mut criterion_evidence = Vec::with_capacity(criterion_values.len());
    for (index, value) in criterion_values.iter().enumerate() {
        let entry = value.as_object().ok_or_else(|| {
            invalid(
                "every criterionEvidence entry must be an object".to_owned(),
                json!({ "index": index }),
            )
        })?;
        if let Some(field) = entry
            .keys()
            .find(|field| !CRITERION_FIELDS.contains(&field.as_str()))
        {
            return Err(invalid(
                format!("criterionEvidence[{index}] contains unsupported field '{field}'"),
                json!({ "index": index, "field": field }),
            ));
        }
        let criterion = entry
            .get("criterion")
            .and_then(Value::as_str)
            .filter(|criterion| {
                !has_noncanonical_surrounding_whitespace(criterion)
                    && criterion.chars().count() <= 1_000
            })
            .ok_or_else(|| {
                invalid(
                    "criterion must be canonical non-empty text of at most 1000 characters"
                        .to_owned(),
                    json!({ "index": index, "field": "criterion" }),
                )
            })?
            .to_owned();
        if !criteria.insert(criterion.clone()) {
            return Err(invalid(
                "criterionEvidence cannot repeat a criterion".to_owned(),
                json!({ "index": index, "field": "criterion" }),
            ));
        }
        let evidence_values = entry
            .get("evidenceIds")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty() && values.len() <= 16)
            .ok_or_else(|| {
                invalid(
                    "every criterion requires between 1 and 16 Evidence IDs".to_owned(),
                    json!({ "index": index, "field": "evidenceIds" }),
                )
            })?;
        total_bindings = total_bindings.saturating_add(evidence_values.len());
        if total_bindings > 100 {
            return Err(invalid(
                "criterionEvidence cannot contain more than 100 total bindings".to_owned(),
                json!({ "field": "criterionEvidence" }),
            ));
        }
        let mut unique = HashSet::new();
        let evidence_ids = evidence_values
            .iter()
            .map(|evidence_id| {
                let evidence_id = evidence_id.as_str().filter(|evidence_id| {
                    !has_noncanonical_surrounding_whitespace(evidence_id)
                        && evidence_id.chars().count() <= 200
                });
                let evidence_id = evidence_id.ok_or_else(|| {
                    invalid(
                        "Evidence IDs must be canonical non-empty strings of at most 200 characters"
                            .to_owned(),
                        json!({ "index": index, "field": "evidenceIds" }),
                    )
                })?;
                if !unique.insert(evidence_id.to_owned()) {
                    return Err(invalid(
                        "a criterion cannot repeat an Evidence ID".to_owned(),
                        json!({ "index": index, "field": "evidenceIds" }),
                    ));
                }
                Ok(evidence_id.to_owned())
            })
            .collect::<Result<Vec<_>, WorkToolError>>()?;
        criterion_evidence.push(GraphCriterionEvidenceInput {
            criterion,
            evidence_ids,
        });
    }
    Ok(GraphNodeFinishRequest {
        goal_id: text("goalId", 200)?,
        task_id: text("taskId", 200)?,
        attempt_id: text("attemptId", 200)?,
        expected_task_version: version("expectedTaskVersion")?,
        expected_attempt_version: version("expectedAttemptVersion")?,
        criterion_evidence,
        summary: text("summary", 4_000)?,
    })
}

fn parse_graph_node_review_request(input: &Value) -> Result<GraphNodeReviewRequest, WorkToolError> {
    parse_graph_node_finish_request(input).map_err(|error| {
        WorkToolError::new(
            "graph.readonly_node_review_invalid_input",
            error.message.replace("node-finish", "node-review"),
            error.details,
        )
    })
}

fn parse_graph_acceptance_request(input: &Value) -> Result<GraphAcceptanceRequest, WorkToolError> {
    const FIELDS: &[&str] = &["goalId", "expectedGoalVersion", "summary"];
    let invalid = |message: String, details: Value| {
        WorkToolError::new("graph.readonly_accept_invalid_input", message, details)
    };
    let object = input.as_object().ok_or_else(|| {
        invalid(
            "read-only Graph acceptance input must be an object".to_owned(),
            json!({}),
        )
    })?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(invalid(
            format!("read-only Graph acceptance input contains unsupported field '{field}'"),
            json!({ "field": field }),
        ));
    }
    let text = |field: &str, maximum: usize| -> Result<String, WorkToolError> {
        let value = input.get(field).and_then(Value::as_str).ok_or_else(|| {
            invalid(
                format!("{field} must be a string"),
                json!({ "field": field }),
            )
        })?;
        if has_noncanonical_surrounding_whitespace(value) || value.chars().count() > maximum {
            return Err(invalid(
                format!("{field} must be non-empty canonical text of at most {maximum} characters"),
                json!({ "field": field, "maximumChars": maximum }),
            ));
        }
        Ok(value.to_owned())
    };
    let expected_goal_version = input
        .get("expectedGoalVersion")
        .and_then(Value::as_i64)
        .filter(|version| *version >= 1)
        .ok_or_else(|| {
            invalid(
                "expectedGoalVersion must be an integer of at least 1".to_owned(),
                json!({ "field": "expectedGoalVersion" }),
            )
        })?;
    Ok(GraphAcceptanceRequest {
        goal_id: text("goalId", 200)?,
        expected_goal_version,
        summary: text("summary", 4_000)?,
    })
}

fn parse_graph_node_cancel_request(input: &Value) -> Result<GraphNodeCancelRequest, WorkToolError> {
    const FIELDS: &[&str] = &[
        "goalId",
        "taskId",
        "attemptId",
        "expectedTaskVersion",
        "expectedAttemptVersion",
        "reason",
    ];
    let invalid = |message: String, details: Value| {
        WorkToolError::new("graph.readonly_node_cancel_invalid_input", message, details)
    };
    let object = input.as_object().ok_or_else(|| {
        invalid(
            "read-only Graph node-cancel input must be an object".to_owned(),
            json!({}),
        )
    })?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(invalid(
            format!("read-only Graph node-cancel input contains unsupported field '{field}'"),
            json!({ "field": field }),
        ));
    }
    let text = |field: &str, maximum: usize| -> Result<String, WorkToolError> {
        let value = input.get(field).and_then(Value::as_str).ok_or_else(|| {
            invalid(
                format!("{field} must be a string"),
                json!({ "field": field }),
            )
        })?;
        if has_noncanonical_surrounding_whitespace(value) || value.chars().count() > maximum {
            return Err(invalid(
                format!("{field} must be non-empty canonical text of at most {maximum} characters"),
                json!({ "field": field, "maximumChars": maximum }),
            ));
        }
        Ok(value.to_owned())
    };
    let version = |field: &str| -> Result<i64, WorkToolError> {
        input
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| *value >= 1)
            .ok_or_else(|| {
                invalid(
                    format!("{field} must be an integer of at least 1"),
                    json!({ "field": field }),
                )
            })
    };
    Ok(GraphNodeCancelRequest {
        goal_id: text("goalId", 200)?,
        task_id: text("taskId", 200)?,
        attempt_id: text("attemptId", 200)?,
        expected_task_version: version("expectedTaskVersion")?,
        expected_attempt_version: version("expectedAttemptVersion")?,
        reason: text("reason", 2_000)?,
    })
}

fn parse_task_repair_override_request(
    input: &Value,
) -> Result<TaskRepairOverrideRequest, WorkToolError> {
    const FIELDS: &[&str] = &[
        "taskId",
        "attemptId",
        "expectedVersion",
        "rootCause",
        "findingIds",
        "escalationReason",
    ];
    let object = input.as_object().ok_or_else(|| {
        WorkToolError::new(
            "task.repair_override_invalid_input",
            "task repair override input must be an object",
            json!({}),
        )
    })?;
    if let Some(field) = object
        .keys()
        .find(|field| !FIELDS.contains(&field.as_str()))
    {
        return Err(WorkToolError::new(
            "task.repair_override_invalid_input",
            format!("task repair override input contains unsupported field '{field}'"),
            json!({ "field": field }),
        ));
    }
    let expected_version = input
        .get("expectedVersion")
        .and_then(Value::as_i64)
        .filter(|version| *version >= 1)
        .ok_or_else(|| "expectedVersion must be at least 1".to_owned())?;
    Ok(TaskRepairOverrideRequest {
        task_id: canonical_bounded_required_string(input, "taskId", 200)?,
        attempt_id: canonical_bounded_required_string(input, "attemptId", 200)?,
        expected_version,
        root_cause: canonical_bounded_required_string(input, "rootCause", 4_000)?,
        finding_ids: canonical_required_string_array(input, "findingIds", 32)?,
        escalation_reason: canonical_bounded_required_string(input, "escalationReason", 2_000)?,
    })
}

fn canonical_bounded_required_string(
    input: &Value,
    key: &str,
    max_chars: usize,
) -> Result<String, WorkToolError> {
    let raw = input
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is required"))?;
    if has_noncanonical_surrounding_whitespace(raw) {
        return Err(format!(
            "{key} must be non-empty canonical text without surrounding whitespace"
        )
        .into());
    }
    if raw.chars().count() > max_chars {
        return Err(format!("{key} cannot exceed {max_chars} characters").into());
    }
    Ok(raw.to_owned())
}

fn canonical_required_string_array(
    input: &Value,
    key: &str,
    limit: usize,
) -> Result<Vec<String>, WorkToolError> {
    let values = input
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| format!("{key} must be a non-empty array"))?;
    if values.len() > limit {
        return Err(format!("{key} cannot contain more than {limit} values").into());
    }
    let mut unique = HashSet::new();
    values
        .iter()
        .map(|value| {
            let raw = value
                .as_str()
                .ok_or_else(|| format!("{key} must contain strings"))?;
            if has_noncanonical_surrounding_whitespace(raw) {
                return Err(format!(
                    "{key} values must be non-empty canonical text without surrounding whitespace"
                ));
            }
            if raw.chars().count() > 200 {
                return Err(format!("{key} values cannot exceed 200 characters"));
            }
            if !unique.insert(raw.to_owned()) {
                return Err(format!("{key} cannot contain duplicate values"));
            }
            Ok(raw.to_owned())
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(WorkToolError::from)
}

fn has_noncanonical_surrounding_whitespace(value: &str) -> bool {
    value.is_empty()
        || value != value.trim()
        || value.starts_with('\u{feff}')
        || value.ends_with('\u{feff}')
}

pub fn snapshot(database: &Database, conversation_id: &str) -> Result<Value, String> {
    let (
        goal,
        tasks,
        evidence,
        plan_revisions,
        review_findings,
        acceptances,
        validation_policies,
        task_attempts,
        task_ledger,
        history_summary,
    ) = database
        .load_work_snapshot_v2(conversation_id)
        .map_err(|error| error.to_string())?;
    let generated_at = task_ledger.projection_meta.generated_at;
    let source_high_watermark = work_snapshot_source_high_watermark(
        &task_ledger,
        &plan_revisions,
        &review_findings,
        &acceptances,
    );
    let source_hash = work_snapshot_source_hash(
        conversation_id,
        &goal,
        &tasks,
        &evidence,
        &plan_revisions,
        &review_findings,
        &acceptances,
        &validation_policies,
        &task_attempts,
        &task_ledger,
        &history_summary,
        &source_high_watermark,
    )?;
    Ok(json!({
        "schemaVersion": 2,
        "generatedAt": generated_at,
        "sourceHighWatermark": source_high_watermark,
        "sourceHash": source_hash,
        "goal": goal,
        "tasks": tasks,
        "evidence": evidence,
        "planRevisions": plan_revisions,
        "reviewFindings": review_findings,
        "acceptances": acceptances,
        "validationPolicies": validation_policies,
        "taskAttempts": task_attempts,
        "taskLedger": task_ledger,
        "historySummary": history_summary
    }))
}

fn work_snapshot_source_high_watermark(
    task_ledger: &crate::database::TaskLedgerProjection,
    plans: &[crate::database::PlanRevisionRecord],
    findings: &[crate::database::ReviewFindingRecord],
    acceptances: &[crate::database::AcceptanceRecord],
) -> Value {
    let plan = plans
        .iter()
        .max_by_key(|record| record.revision)
        .map(|record| {
            json!({
                "id": record.id,
                "revision": record.revision,
                "createdAt": record.created_at,
                "approvedAt": record.approved_at
            })
        });
    let finding = findings
        .iter()
        .max_by(|left, right| {
            left.resolved_at
                .as_deref()
                .unwrap_or(left.created_at.as_str())
                .cmp(
                    right
                        .resolved_at
                        .as_deref()
                        .unwrap_or(right.created_at.as_str()),
                )
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|record| {
            json!({
                "id": record.id,
                "createdAt": record.created_at,
                "resolvedAt": record.resolved_at
            })
        });
    let acceptance = acceptances
        .iter()
        .max_by(|left, right| {
            left.resolved_at
                .as_deref()
                .unwrap_or(left.created_at.as_str())
                .cmp(
                    right
                        .resolved_at
                        .as_deref()
                        .unwrap_or(right.created_at.as_str()),
                )
                .then_with(|| left.id.cmp(&right.id))
        })
        .map(|record| {
            json!({
                "id": record.id,
                "createdAt": record.created_at,
                "resolvedAt": record.resolved_at
            })
        });
    json!({
        "selectedGoalId": task_ledger.goal.as_ref().map(|goal| goal.id.as_str()),
        "taskLedger": task_ledger.projection_meta.source_high_watermark,
        "planRevision": plan,
        "reviewFinding": finding,
        "acceptance": acceptance
    })
}

#[allow(clippy::too_many_arguments)]
fn work_snapshot_source_hash(
    conversation_id: &str,
    goal: &Option<crate::database::GoalRecord>,
    tasks: &[crate::database::WorkTaskRecord],
    evidence: &[crate::database::TaskEvidenceRecord],
    plans: &[crate::database::PlanRevisionRecord],
    findings: &[crate::database::ReviewFindingRecord],
    acceptances: &[crate::database::AcceptanceRecord],
    validation_policies: &[crate::database::TaskValidationPolicyRecord],
    task_attempts: &[crate::database::TaskAttemptRecord],
    task_ledger: &crate::database::TaskLedgerProjection,
    history_summary: &Value,
    source_high_watermark: &Value,
) -> Result<String, String> {
    let mut ledger = serde_json::to_value(task_ledger).map_err(|error| error.to_string())?;
    if let Some(meta) = ledger
        .get_mut("projectionMeta")
        .and_then(Value::as_object_mut)
    {
        meta.remove("generatedAt");
    }
    let material = json!({
        "schemaVersion": 2,
        "conversationId": conversation_id,
        "sourceHighWatermark": source_high_watermark,
        "goal": goal,
        "tasks": tasks,
        "evidence": evidence,
        "planRevisions": plans,
        "reviewFindings": findings,
        "acceptances": acceptances,
        "validationPolicies": validation_policies,
        "taskAttempts": task_attempts,
        "taskLedger": ledger,
        "historySummary": history_summary
    });
    let encoded = serde_json::to_vec(&material).map_err(|error| error.to_string())?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

fn validate_goal_for_completion(
    database: &Database,
    conversation_id: &str,
    goal_id: &str,
) -> Result<Value, WorkToolError> {
    let (_, tasks, evidence) = database
        .load_work_graph_snapshot(conversation_id)
        .map_err(|error| error.to_string())?;
    let goal_tasks = tasks
        .iter()
        .filter(|task| task.goal_id == goal_id)
        .collect::<Vec<_>>();
    if goal_tasks.is_empty() {
        return Err(WorkToolError::new(
            "goal.tasks_required",
            "a goal cannot complete before at least one task is recorded",
            json!({ "goalId": goal_id }),
        ));
    }
    let unfinished = goal_tasks
        .iter()
        .filter(|task| {
            !matches!(
                task.status,
                WorkTaskStatus::Completed | WorkTaskStatus::Skipped
            )
        })
        .map(|task| json!({ "taskId": task.id, "title": task.title, "status": task.status }))
        .collect::<Vec<_>>();
    if !unfinished.is_empty() {
        return Err(WorkToolError::new(
            "goal.tasks_unfinished",
            "a goal cannot complete while required tasks remain unfinished",
            json!({ "goalId": goal_id, "tasks": unfinished }),
        ));
    }
    let durable_skipped = goal_tasks
        .iter()
        .filter(|task| task.status == WorkTaskStatus::Skipped)
        .filter_map(|task| {
            database
                .task_validation_policy(&task.id)
                .ok()
                .filter(|policy| policy.snapshot.id != "legacy_v1")
                .map(|policy| json!({ "taskId": task.id, "validationPolicy": policy.snapshot }))
        })
        .collect::<Vec<_>>();
    if !durable_skipped.is_empty() {
        return Err(WorkToolError::new(
            "task.skip_not_authorized",
            "durable Tasks cannot be treated as verified through model-selected skip",
            json!({ "goalId": goal_id, "tasks": durable_skipped }),
        ));
    }
    let missing = goal_tasks
        .iter()
        .filter(|task| {
            task.status == WorkTaskStatus::Completed
                && !evidence.iter().any(|item| {
                    item.task_id == task.id && item.validity_status == EvidenceValidityStatus::Valid
                })
        })
        .map(|task| json!({ "taskId": task.id, "title": task.title }))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(WorkToolError::new("goal.valid_evidence_required", "each completed task needs at least one valid evidence record before the goal can complete", json!({ "goalId": goal_id, "tasks": missing })));
    }
    let goal_task_ids = goal_tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    Ok(
        json!({ "taskCount": goal_tasks.len(), "completed": goal_tasks.iter().filter(|task| task.status == WorkTaskStatus::Completed).count(), "skipped": goal_tasks.iter().filter(|task| task.status == WorkTaskStatus::Skipped).count(), "validEvidence": evidence.iter().filter(|item| goal_task_ids.contains(item.task_id.as_str()) && item.validity_status == EvidenceValidityStatus::Valid).count() }),
    )
}

fn require_no_pending_plan(
    database: &Database,
    conversation_id: &str,
    goal_id: &str,
) -> Result<(), WorkToolError> {
    let (plans, _, _) = database.load_a1_snapshot(conversation_id)?;
    if let Some(plan) = plans
        .iter()
        .find(|plan| plan.goal_id == goal_id && plan.status == "proposed")
    {
        return Err(WorkToolError::new(
            "plan.approval_required",
            "state-changing work is paused until the user approves or rejects the proposed PlanRevision",
            json!({ "goalId": goal_id, "planRevisionId": plan.id, "revision": plan.revision }),
        ));
    }
    Ok(())
}

fn validate_plan_tasks(input: &Value, goal_id: &str) -> Result<Value, WorkToolError> {
    const SERIAL_MAX_TASKS: usize = 64;
    const GRAPH_MAX_NODES: usize = 3;
    const GRAPH_MAX_DEPTH: usize = 1;
    const NODE_KEY_MAX_CHARS: usize = 100;
    const SERIAL_TITLE_MAX_CHARS: usize = 500;
    const GRAPH_TITLE_MAX_CHARS: usize = 300;
    const DETAIL_MAX_CHARS: usize = 4_000;
    const ACCEPTANCE_MAX_ITEMS: usize = 8;
    const ACCEPTANCE_MAX_CHARS: usize = 1_000;

    let submitted = input
        .get("tasks")
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .ok_or_else(|| {
            WorkToolError::new(
                "plan.tasks_required",
                "a PlanRevision requires a non-empty ordered task list",
                json!({ "goalId": goal_id }),
            )
        })?;
    let graph_mode = submitted.iter().any(|task| {
        task.as_object().is_some_and(|object| {
            ["nodeKey", "dependsOn", "acceptanceCriteria"]
                .iter()
                .any(|field| object.contains_key(*field))
        })
    });
    let item_limit = if graph_mode {
        GRAPH_MAX_NODES
    } else {
        SERIAL_MAX_TASKS
    };
    if submitted.len() > item_limit {
        return Err(WorkToolError::new(
            if graph_mode {
                "plan.graph_too_many_nodes"
            } else {
                "plan.too_many_tasks"
            },
            format!(
                "{} cannot contain more than {item_limit} items",
                if graph_mode {
                    "read-only Graph"
                } else {
                    "plan"
                }
            ),
            json!({ "goalId": goal_id, "actualCount": submitted.len(), "maximum": item_limit }),
        ));
    }

    let allowed_fields: HashSet<&str> = if graph_mode {
        [
            "nodeKey",
            "title",
            "detail",
            "ordinal",
            "dependsOn",
            "acceptanceCriteria",
        ]
        .into_iter()
        .collect()
    } else {
        ["title", "detail", "ordinal"].into_iter().collect()
    };
    let mut tasks = submitted
        .iter()
        .map(|task| {
            let object = task.as_object().ok_or_else(|| {
                WorkToolError::new(
                    "plan.task_invalid",
                    "every plan task must be an object",
                    json!({ "goalId": goal_id, "task": task }),
                )
            })?;
            let unsupported = object
                .keys()
                .filter(|key| !allowed_fields.contains(key.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !unsupported.is_empty() {
                return Err(WorkToolError::new(
                    "plan.task_fields_unsupported",
                    "plan tasks cannot contain runtime state or other unsupported fields",
                    json!({ "goalId": goal_id, "fields": unsupported }),
                ));
            }
            let ordinal = task
                .get("ordinal")
                .and_then(Value::as_i64)
                .filter(|ordinal| *ordinal >= 0)
                .ok_or_else(|| {
                    WorkToolError::new(
                        "plan.task_ordinal_invalid",
                        "every plan task requires a non-negative ordinal",
                        json!({ "goalId": goal_id, "task": task }),
                    )
                })?;
            let detail = strict_bounded_optional_plan_string(
                task.get("detail"),
                "detail",
                DETAIL_MAX_CHARS,
                goal_id,
            )?;
            let normalized = if graph_mode {
                let title = strict_bounded_plan_string(
                    task.get("title"),
                    "title",
                    GRAPH_TITLE_MAX_CHARS,
                    goal_id,
                )?;
                let node_key = strict_bounded_plan_string(
                    task.get("nodeKey"),
                    "nodeKey",
                    NODE_KEY_MAX_CHARS,
                    goal_id,
                )?;
                validate_plan_node_key(&node_key, goal_id)?;
                let depends_on = bounded_plan_string_array(
                    task.get("dependsOn"),
                    "dependsOn",
                    GRAPH_MAX_NODES,
                    NODE_KEY_MAX_CHARS,
                    true,
                    goal_id,
                )?;
                for dependency in &depends_on {
                    validate_plan_node_key(dependency, goal_id)?;
                }
                let acceptance_criteria = bounded_plan_string_array(
                    task.get("acceptanceCriteria"),
                    "acceptanceCriteria",
                    ACCEPTANCE_MAX_ITEMS,
                    ACCEPTANCE_MAX_CHARS,
                    false,
                    goal_id,
                )?;
                json!({
                    "nodeKey": node_key,
                    "title": title,
                    "detail": detail,
                    "ordinal": ordinal,
                    "dependsOn": depends_on,
                    "acceptanceCriteria": acceptance_criteria,
                })
            } else {
                let title = strict_bounded_plan_string(
                    task.get("title"),
                    "title",
                    SERIAL_TITLE_MAX_CHARS,
                    goal_id,
                )?;
                json!({
                    "title": title,
                    "detail": detail,
                    "ordinal": ordinal,
                })
            };
            Ok((ordinal, normalized))
        })
        .collect::<Result<Vec<_>, WorkToolError>>()?;
    tasks.sort_by_key(|(ordinal, _)| *ordinal);
    for (expected, (ordinal, _)) in tasks.iter().enumerate() {
        if *ordinal != expected as i64 {
            return Err(WorkToolError::new(
                "plan.task_ordinals_not_contiguous",
                "plan task ordinals must be unique and contiguous from zero",
                json!({ "goalId": goal_id, "expectedOrdinal": expected, "actualOrdinal": ordinal }),
            ));
        }
    }
    if graph_mode {
        validate_read_only_plan_graph(&tasks, goal_id, GRAPH_MAX_DEPTH)?;
    }
    Ok(Value::Array(
        tasks.into_iter().map(|(_, task)| task).collect(),
    ))
}

fn strict_bounded_plan_string(
    value: Option<&Value>,
    field: &str,
    maximum_chars: usize,
    goal_id: &str,
) -> Result<String, WorkToolError> {
    let value = value.and_then(Value::as_str).ok_or_else(|| {
        WorkToolError::new(
            "plan.task_field_invalid",
            format!("{field} must be a string"),
            json!({ "goalId": goal_id, "field": field }),
        )
    })?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed != value {
        return Err(WorkToolError::new(
            "plan.task_field_invalid",
            format!("{field} must be non-empty and cannot start or end with whitespace"),
            json!({ "goalId": goal_id, "field": field }),
        ));
    }
    if value.chars().count() > maximum_chars {
        return Err(WorkToolError::new(
            "plan.task_field_too_long",
            format!("{field} cannot exceed {maximum_chars} characters"),
            json!({ "goalId": goal_id, "field": field, "maximumChars": maximum_chars }),
        ));
    }
    Ok(value.to_owned())
}

fn strict_bounded_optional_plan_string(
    value: Option<&Value>,
    field: &str,
    maximum_chars: usize,
    goal_id: &str,
) -> Result<Option<String>, WorkToolError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(value) => {
            strict_bounded_plan_string(Some(value), field, maximum_chars, goal_id).map(Some)
        }
    }
}

fn bounded_plan_string_array(
    value: Option<&Value>,
    field: &str,
    maximum_items: usize,
    maximum_chars: usize,
    optional: bool,
    goal_id: &str,
) -> Result<Vec<String>, WorkToolError> {
    let Some(value) = value else {
        if optional {
            return Ok(Vec::new());
        }
        return Err(WorkToolError::new(
            "plan.graph_field_required",
            format!("every Graph node requires {field}"),
            json!({ "goalId": goal_id, "field": field }),
        ));
    };
    let values = value.as_array().ok_or_else(|| {
        WorkToolError::new(
            "plan.graph_field_invalid",
            format!("{field} must be an array"),
            json!({ "goalId": goal_id, "field": field }),
        )
    })?;
    if (!optional && values.is_empty()) || values.len() > maximum_items {
        return Err(WorkToolError::new(
            "plan.graph_field_invalid",
            format!(
                "{field} must contain {} to {maximum_items} items",
                if optional { 0 } else { 1 }
            ),
            json!({ "goalId": goal_id, "field": field, "actualCount": values.len(), "maximum": maximum_items }),
        ));
    }
    let mut unique = HashSet::new();
    values
        .iter()
        .map(|value| {
            let value = strict_bounded_plan_string(Some(value), field, maximum_chars, goal_id)?;
            if !unique.insert(value.clone()) {
                return Err(WorkToolError::new(
                    "plan.graph_value_duplicate",
                    format!("{field} cannot contain duplicate values"),
                    json!({ "goalId": goal_id, "field": field, "value": value }),
                ));
            }
            Ok(value)
        })
        .collect()
}

fn validate_plan_node_key(value: &str, goal_id: &str) -> Result<(), WorkToolError> {
    if value
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    {
        return Ok(());
    }
    Err(WorkToolError::new(
        "plan.graph_node_key_invalid",
        "nodeKey and dependsOn values may contain only ASCII letters, digits, dot, dash, and underscore",
        json!({ "goalId": goal_id, "nodeKey": value }),
    ))
}

fn validate_read_only_plan_graph(
    tasks: &[(i64, Value)],
    goal_id: &str,
    maximum_depth: usize,
) -> Result<(), WorkToolError> {
    let keys = tasks
        .iter()
        .map(|(_, task)| task["nodeKey"].as_str().expect("normalized nodeKey"))
        .collect::<HashSet<_>>();
    if keys.len() != tasks.len() {
        return Err(WorkToolError::new(
            "plan.graph_node_key_duplicate",
            "Graph nodeKey values must be unique",
            json!({ "goalId": goal_id }),
        ));
    }
    let mut dependencies = HashMap::<String, Vec<String>>::new();
    for (_, task) in tasks {
        let node_key = task["nodeKey"].as_str().expect("normalized nodeKey");
        let node_dependencies = task["dependsOn"]
            .as_array()
            .expect("normalized dependsOn")
            .iter()
            .map(|value| value.as_str().expect("normalized dependency").to_owned())
            .collect::<Vec<_>>();
        for dependency in &node_dependencies {
            if dependency == node_key {
                return Err(WorkToolError::new(
                    "plan.graph_self_dependency",
                    "a Graph node cannot depend on itself",
                    json!({ "goalId": goal_id, "nodeKey": node_key }),
                ));
            }
            if !keys.contains(dependency.as_str()) {
                return Err(WorkToolError::new(
                    "plan.graph_dependency_unknown",
                    "dependsOn must reference another nodeKey in the same PlanRevision",
                    json!({ "goalId": goal_id, "nodeKey": node_key, "dependency": dependency }),
                ));
            }
        }
        dependencies.insert(node_key.to_owned(), node_dependencies);
    }

    let mut depths = HashMap::<String, usize>::new();
    while depths.len() < tasks.len() {
        let mut advanced = false;
        for (node_key, node_dependencies) in &dependencies {
            if depths.contains_key(node_key)
                || !node_dependencies
                    .iter()
                    .all(|dependency| depths.contains_key(dependency))
            {
                continue;
            }
            let depth = node_dependencies
                .iter()
                .filter_map(|dependency| depths.get(dependency))
                .max()
                .map_or(0, |depth| depth + 1);
            if depth > maximum_depth {
                return Err(WorkToolError::new(
                    "plan.graph_depth_exceeded",
                    format!("read-only Graph depth cannot exceed {maximum_depth}"),
                    json!({ "goalId": goal_id, "nodeKey": node_key, "depth": depth, "maximumDepth": maximum_depth }),
                ));
            }
            depths.insert(node_key.clone(), depth);
            advanced = true;
        }
        if !advanced {
            return Err(WorkToolError::new(
                "plan.graph_cycle",
                "read-only Graph dependencies must be acyclic",
                json!({ "goalId": goal_id }),
            ));
        }
    }
    Ok(())
}

fn validate_acceptance_checks(
    database: &Database,
    conversation_id: &str,
    goal_id: &str,
    reviewer_run_id: &str,
    input: &Value,
    task_summary: Value,
) -> Result<Value, WorkToolError> {
    let submitted = input
        .get("checks")
        .and_then(Value::as_array)
        .filter(|checks| !checks.is_empty())
        .ok_or_else(|| {
            WorkToolError::new(
                "acceptance.checks_required",
                "final acceptance requires at least one explicit criterion check",
                json!({ "goalId": goal_id }),
            )
        })?;
    let (_, tasks, evidence) = database
        .load_work_graph_snapshot(conversation_id)
        .map_err(|error| error.to_string())?;
    let goal_tasks = tasks
        .iter()
        .filter(|task| task.goal_id == goal_id)
        .collect::<Vec<_>>();
    let goal_task_ids = goal_tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    let evidence_by_id = evidence
        .iter()
        .filter(|item| goal_task_ids.contains(item.task_id.as_str()))
        .map(|item| (item.id.as_str(), item))
        .collect::<HashMap<_, _>>();
    let mut criteria = Vec::with_capacity(submitted.len());
    let mut criterion_names = HashSet::new();
    let mut covered_task_ids = HashSet::new();

    for check in submitted {
        let criterion = required_string(check, "criterion")?;
        if !criterion_names.insert(criterion.to_lowercase()) {
            return Err(WorkToolError::new(
                "acceptance.criterion_duplicate",
                "acceptance criteria must be unique",
                json!({ "goalId": goal_id, "criterion": criterion }),
            ));
        }
        let method = required_string(check, "method")?;
        if !["test", "inspection", "review", "manual", "other"].contains(&method.as_str()) {
            return Err(WorkToolError::new(
                "acceptance.method_unsupported",
                "acceptance method must be test, inspection, review, manual, or other",
                json!({ "goalId": goal_id, "criterion": criterion, "method": method }),
            ));
        }
        let status = required_string(check, "status")?;
        if !["passed", "not_applicable"].contains(&status.as_str()) {
            return Err(WorkToolError::new(
                "acceptance.check_not_passed",
                "final acceptance only permits passed or justified not_applicable criteria",
                json!({ "goalId": goal_id, "criterion": criterion, "status": status }),
            ));
        }
        let detail = optional_string(check, "detail")?;
        if status == "not_applicable" && detail.is_none() {
            return Err(WorkToolError::new(
                "acceptance.not_applicable_reason_required",
                "a not_applicable criterion requires a detail explaining why",
                json!({ "goalId": goal_id, "criterion": criterion }),
            ));
        }
        let evidence_ids = match check.get("evidenceIds") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    item.as_str()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| "evidenceIds must contain non-empty strings".to_owned())
                })
                .collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err("evidenceIds must be an array".into()),
        };
        if status == "passed" && evidence_ids.is_empty() {
            return Err(WorkToolError::new(
                "acceptance.evidence_required",
                "each passed criterion requires at least one valid evidence record",
                json!({ "goalId": goal_id, "criterion": criterion }),
            ));
        }
        for evidence_id in &evidence_ids {
            let item = evidence_by_id.get(evidence_id.as_str()).ok_or_else(|| {
                WorkToolError::new(
                    "acceptance.evidence_not_found",
                    "acceptance evidence must belong to this goal",
                    json!({ "goalId": goal_id, "criterion": criterion, "evidenceId": evidence_id }),
                )
            })?;
            if item.validity_status != EvidenceValidityStatus::Valid {
                return Err(WorkToolError::new(
                    "acceptance.evidence_not_valid",
                    "acceptance evidence must be Host-validated",
                    json!({ "goalId": goal_id, "criterion": criterion, "evidenceId": evidence_id, "validityStatus": item.validity_status }),
                ));
            }
            covered_task_ids.insert(item.task_id.as_str());
        }
        criteria.push(json!({
            "criterion": criterion,
            "method": method,
            "status": status,
            "evidenceIds": evidence_ids,
            "detail": detail,
        }));
    }

    let uncovered_tasks = goal_tasks
        .iter()
        .filter(|task| task.status == WorkTaskStatus::Completed)
        .filter(|task| !covered_task_ids.contains(task.id.as_str()))
        .map(|task| json!({ "taskId": task.id, "title": task.title }))
        .collect::<Vec<_>>();
    if !uncovered_tasks.is_empty() {
        return Err(WorkToolError::new(
            "acceptance.task_coverage_required",
            "acceptance criteria must cite evidence covering every completed task",
            json!({ "goalId": goal_id, "tasks": uncovered_tasks }),
        ));
    }

    Ok(json!({
        "criteria": criteria,
        "taskSummary": task_summary,
        "reviewer": { "runId": reviewer_run_id },
    }))
}

fn require_goal(
    database: &Database,
    conversation_id: &str,
    goal_id: &str,
) -> Result<crate::database::GoalRecord, String> {
    let goal = database
        .goals()
        .get(goal_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("goal '{goal_id}' was not found"))?;
    if goal.conversation_id != conversation_id {
        return Err("goal belongs to a different conversation".to_owned());
    }
    Ok(goal)
}

fn require_task(
    database: &Database,
    conversation_id: &str,
    task_id: &str,
) -> Result<(crate::database::WorkTaskRecord, String), String> {
    let task = database
        .work_tasks()
        .get(task_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("work task '{task_id}' was not found"))?;
    let goal = require_goal(database, conversation_id, &task.goal_id)?;
    Ok((task, goal.id))
}

fn required_string(input: &Value, key: &str) -> Result<String, String> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{key} is required"))
}

fn optional_string(input: &Value, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.trim().to_owned())),
        Some(Value::String(_)) => Err(format!("{key} cannot be empty")),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn required_version(input: &Value) -> Result<i64, String> {
    input
        .get("expectedVersion")
        .and_then(Value::as_i64)
        .filter(|version| *version >= 1)
        .ok_or_else(|| "expectedVersion must be an integer of at least 1".to_owned())
}

fn required_string_array(input: &Value, key: &str, limit: usize) -> Result<Vec<String>, String> {
    let values = input
        .get(key)
        .and_then(Value::as_array)
        .filter(|values| !values.is_empty())
        .ok_or_else(|| format!("{key} must be a non-empty array"))?;
    if values.len() > limit {
        return Err(format!("{key} cannot contain more than {limit} values"));
    }
    let mut unique = HashSet::new();
    values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| format!("{key} must contain non-empty strings"))?;
            if value.chars().count() > 200 {
                return Err(format!("{key} values cannot exceed 200 characters"));
            }
            if !unique.insert(value.to_owned()) {
                return Err(format!("{key} cannot contain duplicate values"));
            }
            Ok(value.to_owned())
        })
        .collect()
}

fn parse_attempt_status(value: &str) -> Result<TaskAttemptStatus, String> {
    match value {
        "succeeded" => Ok(TaskAttemptStatus::Succeeded),
        "failed" => Ok(TaskAttemptStatus::Failed),
        "blocked" => Ok(TaskAttemptStatus::Blocked),
        "cancelled" => Ok(TaskAttemptStatus::Cancelled),
        _ => Err(format!("unsupported terminal Attempt status: {value}")),
    }
}

fn repository_work_tool_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let code = if rendered.contains("[graph.readonly_dedicated_start_required]") {
        "graph.readonly_dedicated_start_required"
    } else if rendered.contains("[graph.readonly_dedicated_finish_required]") {
        "graph.readonly_dedicated_finish_required"
    } else if rendered.contains("[child_run.tool_budget_exceeded]") {
        "child_run.tool_budget_exceeded"
    } else if rendered.contains("[digital_colleague.tool_budget_exceeded]") {
        "digital_colleague.tool_budget_exceeded"
    } else if rendered.contains("[child_run.budget_exceeded]") {
        "child_run.budget_exceeded"
    } else if rendered.contains("[digital_colleague.budget_exceeded]") {
        "digital_colleague.budget_exceeded"
    } else {
        match &error {
            RepositoryError::InvalidValidationPolicy(_) => {
                "runtime.validation_policy.invalid_snapshot"
            }
            RepositoryError::ValidationChecksMissing { .. } => "task.validation_checks_missing",
            RepositoryError::ReviewBlocked { .. } => "task.review_blocked",
            RepositoryError::IndependentReviewRequired { .. } => "task.independent_review_required",
            RepositoryError::EvidenceRequired { .. } => "task.valid_evidence_required",
            RepositoryError::OptimisticLockFailed { .. } => "task.optimistic_lock_failed",
            _ => "work_tool.failed",
        }
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({ "repositoryError": rendered }),
    )
}

fn require_durable_v2_graph_profile(
    database: &Database,
    run_id: &str,
    tool: &str,
) -> Result<(), WorkToolError> {
    let profile = database
        .frozen_run_execution_profile_id(run_id)
        .map_err(|error| graph_repository_work_tool_error("profile", error))?;
    if profile.as_deref() != Some("durable_v2") {
        return Err(WorkToolError::new(
            "graph.readonly_profile_required",
            "persistent read-only Graph tools require the current Run to freeze durable_v2",
            json!({
                "tool": tool,
                "runId": run_id,
                "frozenExecutionProfile": profile,
                "requiredExecutionProfile": "durable_v2",
            }),
        ));
    }
    Ok(())
}

fn graph_repository_work_tool_error(
    operation: &'static str,
    error: RepositoryError,
) -> WorkToolError {
    let code = match &error {
        RepositoryError::NotFound {
            entity: "plan revision",
            ..
        } => "graph.readonly_plan_not_found",
        RepositoryError::NotFound {
            entity: "activation tool call",
            ..
        }
        | RepositoryError::CrossConversationReference => "graph.readonly_activation_source_invalid",
        RepositoryError::InvalidValidationPolicy(_) => "graph.readonly_profile_invalid",
        RepositoryError::InvalidInput(_) | RepositoryError::InvalidReference(_) => {
            "graph.readonly_invalid_spec"
        }
        RepositoryError::ConstraintViolation(_) | RepositoryError::InvalidTransition { .. } => {
            "graph.readonly_activation_rejected"
        }
        RepositoryError::SqliteBusyExhausted { .. }
        | RepositoryError::Database { busy: true, .. } => "graph.readonly_repository_busy",
        _ => "graph.readonly_repository_failed",
    };
    let rendered = error.to_string();
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": operation,
            "repositoryError": rendered,
        }),
    )
}

fn graph_node_start_repository_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let (code, retryable) = match &error {
        RepositoryError::NotFound {
            entity: "read-only graph",
            ..
        } => ("graph.readonly_not_activated", false),
        RepositoryError::NotFound {
            entity: "graph task",
            ..
        }
        | RepositoryError::InvalidReference(_) => ("graph.readonly_node_not_found", false),
        RepositoryError::NotFound {
            entity: "Graph node-start tool call",
            ..
        }
        | RepositoryError::CrossConversationReference => {
            ("graph.readonly_node_start_source_invalid", false)
        }
        RepositoryError::NotFound {
            entity: "parent run",
            ..
        }
        | RepositoryError::InvalidValidationPolicy(_) => ("graph.readonly_lead_run_invalid", false),
        RepositoryError::OptimisticLockFailed { .. } => {
            ("graph.readonly_node_version_conflict", true)
        }
        RepositoryError::InvalidInput(message) if message.contains("budget") => {
            ("graph.readonly_node_budget_invalid", false)
        }
        RepositoryError::InvalidInput(_) => ("graph.readonly_node_start_invalid_input", false),
        RepositoryError::InvalidTransition { .. } => ("graph.readonly_node_not_runnable", false),
        RepositoryError::ConstraintViolation(message) if message.contains("not runnable") => {
            ("graph.readonly_node_not_runnable", false)
        }
        RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision") => {
            ("graph.readonly_plan_changed", true)
        }
        RepositoryError::ConstraintViolation(message) if message.contains("budget") => {
            ("graph.readonly_node_budget_invalid", false)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("ToolCall")
                || message.contains("Lead Run")
                || message.contains("depth-zero primary") =>
        {
            ("graph.readonly_node_start_source_invalid", false)
        }
        RepositoryError::SqliteBusyExhausted { .. }
        | RepositoryError::Database { busy: true, .. } => ("graph.readonly_repository_busy", true),
        _ => ("graph.readonly_node_start_rejected", false),
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": "node_start",
            "repositoryError": rendered,
            "retryable": retryable,
        }),
    )
}

fn graph_node_finish_repository_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let (code, retryable) = match &error {
        RepositoryError::ConstraintViolation(message)
            if message.contains("[graph.readonly_review_candidate_mismatch]") =>
        {
            ("graph.readonly_review_candidate_mismatch", true)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("[graph.readonly_review_required]") =>
        {
            ("graph.readonly_review_required", false)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("[graph.readonly_child_result_required]") =>
        {
            ("graph.readonly_child_result_required", false)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("before its authoritative Child Run completes") =>
        {
            ("graph.readonly_child_not_completed", true)
        }
        RepositoryError::NotFound {
            entity: "read-only graph",
            ..
        } => ("graph.readonly_not_activated", false),
        RepositoryError::NotFound {
            entity: "Graph node-finish tool call",
            ..
        }
        | RepositoryError::CrossConversationReference => {
            ("graph.readonly_node_finish_source_invalid", false)
        }
        RepositoryError::NotFound {
            entity: "graph task",
            ..
        }
        | RepositoryError::NotFound {
            entity: "graph task attempt",
            ..
        }
        | RepositoryError::NotFound {
            entity: "criterion Evidence",
            ..
        }
        | RepositoryError::InvalidReference(_) => {
            ("graph.readonly_criterion_evidence_invalid", true)
        }
        RepositoryError::OptimisticLockFailed { .. } => {
            ("graph.readonly_node_version_conflict", true)
        }
        RepositoryError::InvalidValidationPolicy(_) => {
            ("graph.readonly_validation_policy_invalid", false)
        }
        RepositoryError::InvalidInput(_) => ("graph.readonly_node_finish_invalid_input", false),
        RepositoryError::InvalidTransition { .. } => ("graph.readonly_node_not_active", false),
        RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision") => {
            ("graph.readonly_plan_changed", true)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("ToolCall")
                || message.contains("parent Lead")
                || message.contains("depth-zero primary") =>
        {
            ("graph.readonly_node_finish_source_invalid", false)
        }
        RepositoryError::EvidenceRequired { .. }
        | RepositoryError::ValidationChecksMissing { .. } => {
            ("graph.readonly_criterion_evidence_invalid", true)
        }
        RepositoryError::ReviewBlocked { .. }
        | RepositoryError::IndependentReviewRequired { .. } => {
            ("graph.readonly_review_required", false)
        }
        RepositoryError::SqliteBusyExhausted { .. }
        | RepositoryError::Database { busy: true, .. } => ("graph.readonly_repository_busy", true),
        _ => ("graph.readonly_node_finish_rejected", false),
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": "node_finish",
            "repositoryError": rendered,
            "retryable": retryable,
        }),
    )
}

fn graph_node_cancel_repository_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let (code, retryable) = match &error {
        RepositoryError::NotFound {
            entity: "read-only graph",
            ..
        } => ("graph.readonly_not_activated", false),
        RepositoryError::NotFound {
            entity: "Graph node-cancel tool call",
            ..
        }
        | RepositoryError::CrossConversationReference => {
            ("graph.readonly_node_cancel_source_invalid", false)
        }
        RepositoryError::NotFound {
            entity: "graph task",
            ..
        }
        | RepositoryError::NotFound {
            entity: "graph task attempt",
            ..
        }
        | RepositoryError::InvalidReference(_) => {
            ("graph.readonly_node_cancel_target_invalid", false)
        }
        RepositoryError::OptimisticLockFailed { .. } => {
            ("graph.readonly_node_version_conflict", true)
        }
        RepositoryError::InvalidValidationPolicy(_) => {
            ("graph.readonly_validation_policy_invalid", false)
        }
        RepositoryError::InvalidInput(_) => ("graph.readonly_node_cancel_invalid_input", false),
        RepositoryError::InvalidTransition { .. } => ("graph.readonly_node_not_active", false),
        RepositoryError::ConstraintViolation(message) if message.contains("newer PlanRevision") => {
            ("graph.readonly_plan_changed", true)
        }
        RepositoryError::ConstraintViolation(message)
            if message.contains("ToolCall")
                || message.contains("parent Lead")
                || message.contains("depth-zero primary") =>
        {
            ("graph.readonly_node_cancel_source_invalid", false)
        }
        RepositoryError::SqliteBusyExhausted { .. }
        | RepositoryError::Database { busy: true, .. } => ("graph.readonly_repository_busy", true),
        _ => ("graph.readonly_node_cancel_rejected", false),
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": "node_cancel",
            "repositoryError": rendered,
            "retryable": retryable,
        }),
    )
}

fn graph_node_review_repository_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let (code, retryable) = if rendered.contains("[graph.readonly_review_in_progress]") {
        ("graph.readonly_review_in_progress", true)
    } else if rendered.contains("[graph.readonly_child_result_required]") {
        ("graph.readonly_child_result_required", false)
    } else if rendered.contains("[graph.readonly_plan_changed]")
        || rendered.contains("newer PlanRevision")
    {
        ("graph.readonly_plan_changed", true)
    } else {
        match &error {
            RepositoryError::NotFound {
                entity: "read-only graph",
                ..
            } => ("graph.readonly_not_activated", false),
            RepositoryError::NotFound {
                entity: "Graph node-review tool call",
                ..
            }
            | RepositoryError::CrossConversationReference => {
                ("graph.readonly_node_review_source_invalid", false)
            }
            RepositoryError::NotFound {
                entity: "graph task",
                ..
            }
            | RepositoryError::NotFound {
                entity: "graph task attempt",
                ..
            }
            | RepositoryError::NotFound {
                entity: "criterion Evidence",
                ..
            }
            | RepositoryError::InvalidReference(_) => {
                ("graph.readonly_criterion_evidence_invalid", true)
            }
            RepositoryError::OptimisticLockFailed { .. } => {
                ("graph.readonly_node_version_conflict", true)
            }
            RepositoryError::InvalidValidationPolicy(_) => {
                ("graph.readonly_validation_policy_invalid", false)
            }
            RepositoryError::InvalidInput(_) => ("graph.readonly_node_review_invalid_input", false),
            RepositoryError::InvalidTransition { .. } => {
                ("graph.readonly_node_not_reviewable", false)
            }
            RepositoryError::EvidenceRequired { .. }
            | RepositoryError::ValidationChecksMissing { .. } => {
                ("graph.readonly_criterion_evidence_invalid", true)
            }
            RepositoryError::SqliteBusyExhausted { .. }
            | RepositoryError::Database { busy: true, .. } => {
                ("graph.readonly_repository_busy", true)
            }
            _ => ("graph.readonly_node_review_rejected", false),
        }
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": "node_review",
            "repositoryError": rendered,
            "retryable": retryable,
        }),
    )
}

fn graph_acceptance_repository_error(error: RepositoryError) -> WorkToolError {
    let rendered = error.to_string();
    let (code, retryable) = if rendered.contains("[graph.readonly_findings_open]") {
        ("graph.readonly_findings_open", false)
    } else if rendered.contains("[graph.readonly_accept_not_ready]") {
        ("graph.readonly_accept_not_ready", true)
    } else if rendered.contains("[graph.readonly_plan_changed]")
        || rendered.contains("newer PlanRevision")
    {
        ("graph.readonly_plan_changed", true)
    } else {
        match &error {
            RepositoryError::NotFound {
                entity: "read-only graph",
                ..
            } => ("graph.readonly_not_activated", false),
            RepositoryError::NotFound {
                entity: "Graph acceptance tool call",
                ..
            }
            | RepositoryError::CrossConversationReference => {
                ("graph.readonly_accept_source_invalid", false)
            }
            RepositoryError::OptimisticLockFailed { .. } => {
                ("graph.readonly_goal_version_conflict", true)
            }
            RepositoryError::InvalidValidationPolicy(_) => {
                ("graph.readonly_validation_policy_invalid", false)
            }
            RepositoryError::InvalidInput(_) => ("graph.readonly_accept_invalid_input", false),
            RepositoryError::InvalidTransition { .. }
            | RepositoryError::EvidenceRequired { .. }
            | RepositoryError::ValidationChecksMissing { .. }
            | RepositoryError::ReviewBlocked { .. }
            | RepositoryError::IndependentReviewRequired { .. } => {
                ("graph.readonly_accept_not_ready", true)
            }
            RepositoryError::SqliteBusyExhausted { .. }
            | RepositoryError::Database { busy: true, .. } => {
                ("graph.readonly_repository_busy", true)
            }
            _ => ("graph.readonly_accept_rejected", false),
        }
    };
    WorkToolError::new(
        code,
        rendered.clone(),
        json!({
            "operation": "graph_accept",
            "repositoryError": rendered,
            "retryable": retryable,
        }),
    )
}

fn parse_task_status(value: &str) -> Result<WorkTaskStatus, String> {
    match value {
        "queued" => Ok(WorkTaskStatus::Queued),
        "in_progress" => Ok(WorkTaskStatus::InProgress),
        "completed" => Ok(WorkTaskStatus::Completed),
        "blocked" => Ok(WorkTaskStatus::Blocked),
        "interrupted" => Ok(WorkTaskStatus::Interrupted),
        "skipped" => Ok(WorkTaskStatus::Skipped),
        _ => Err(format!("unsupported task status: {value}")),
    }
}

fn task_event_type(status: &WorkTaskStatus) -> Option<&'static str> {
    match status {
        WorkTaskStatus::InProgress => Some("task.started"),
        WorkTaskStatus::Completed => Some("task.completed"),
        WorkTaskStatus::Blocked => Some("task.blocked"),
        WorkTaskStatus::Interrupted => Some("task.interrupted"),
        WorkTaskStatus::Queued | WorkTaskStatus::Skipped => None,
    }
}

fn tool_result(details: Value) -> Value {
    let text = serde_json::to_string_pretty(&details).unwrap_or_else(|_| details.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "details": details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::CreateGoalInput;
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    fn setup() -> (Database, std::path::PathBuf, String, String) {
        let path = std::env::temp_dir().join(format!("fox-work-tools-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("work tools"), None, None)
            .expect("create conversation");
        let run = database
            .create_run(
                &conversation.id,
                "/goal 创建目标并制定计划，修复跨文件问题",
                None,
            )
            .expect("create run")
            .run;
        (database, path, conversation.id, run.id)
    }

    fn percentile_95(mut samples: Vec<Duration>) -> Duration {
        samples.sort_unstable();
        samples[(samples.len() * 95).div_ceil(100).saturating_sub(1)]
    }

    #[test]
    fn graph_node_finish_request_is_strict_bounded_and_allows_cross_criterion_reuse() {
        let input = json!({
            "goalId": "goal",
            "taskId": "task",
            "attemptId": "attempt",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "criterionEvidence": [
                { "criterion": "report inspected", "evidenceIds": ["evidence"] },
                { "criterion": "scope inspected", "evidenceIds": ["evidence"] }
            ],
            "summary": "Every frozen criterion was revalidated."
        });
        let parsed = parse_graph_node_finish_request(&input).expect("parse exact finish input");
        assert_eq!(parsed.criterion_evidence.len(), 2);
        assert_eq!(parsed.criterion_evidence[0].evidence_ids, vec!["evidence"]);
        assert_eq!(parsed.criterion_evidence[1].evidence_ids, vec!["evidence"]);

        let mut leaked_authority = input.clone();
        leaked_authority["parentRunId"] = json!("model-must-not-choose-authority");
        let error = parse_graph_node_finish_request(&leaked_authority)
            .expect_err("Host authority fields are never model input");
        assert_eq!(error.code, "graph.readonly_node_finish_invalid_input");

        let mut duplicate = input;
        duplicate["criterionEvidence"][0]["evidenceIds"] = json!(["evidence", "evidence"]);
        let error = parse_graph_node_finish_request(&duplicate)
            .expect_err("one criterion cannot repeat the same Evidence ID");
        assert_eq!(error.code, "graph.readonly_node_finish_invalid_input");
    }

    #[test]
    fn graph_node_review_request_reuses_finish_bounds_without_model_authority() {
        let input = json!({
            "goalId": "goal",
            "taskId": "task",
            "attemptId": "attempt",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "criterionEvidence": [
                { "criterion": "report inspected", "evidenceIds": ["evidence"] }
            ],
            "summary": "Request an independent review of the frozen Attempt."
        });
        let parsed = parse_graph_node_review_request(&input).expect("parse exact review input");
        assert_eq!(parsed.criterion_evidence.len(), 1);

        for field in [
            "parentRunId",
            "toolCallId",
            "reviewerAgentId",
            "reviewerRunId",
            "childRunId",
            "validationPolicyId",
            "authority",
            "status",
        ] {
            let mut leaked = input.clone();
            leaked[field] = json!("model-spoof");
            let error = parse_graph_node_review_request(&leaked)
                .expect_err("review authority is injected by Host");
            assert_eq!(error.code, "graph.readonly_node_review_invalid_input");
        }
    }

    #[test]
    fn graph_acceptance_request_is_exact_and_bounded() {
        let input = json!({
            "goalId": "goal",
            "expectedGoalVersion": 3,
            "summary": "All current Graph nodes are accepted."
        });
        let parsed = parse_graph_acceptance_request(&input).expect("parse exact acceptance input");
        assert_eq!(parsed.goal_id, "goal");
        assert_eq!(parsed.expected_goal_version, 3);

        for field in [
            "planRevisionId",
            "checks",
            "evidenceIds",
            "reviewerRunId",
            "status",
            "authority",
        ] {
            let mut leaked = input.clone();
            leaked[field] = json!("model-spoof");
            let error = parse_graph_acceptance_request(&leaked)
                .expect_err("acceptance authority is derived by Host");
            assert_eq!(error.code, "graph.readonly_accept_invalid_input");
        }
        let mut oversized = input;
        oversized["summary"] = json!("s".repeat(4_001));
        assert!(parse_graph_acceptance_request(&oversized).is_err());
    }

    #[test]
    fn graph_review_and_accept_repository_errors_are_stable() {
        let candidate = graph_node_finish_repository_error(RepositoryError::ConstraintViolation(
            "[graph.readonly_review_candidate_mismatch] finish must reuse the exact reviewed input"
                .to_owned(),
        ));
        assert_eq!(candidate.code, "graph.readonly_review_candidate_mismatch");
        assert_eq!(candidate.details["retryable"], true);

        let review = graph_node_review_repository_error(RepositoryError::ConstraintViolation(
            "[graph.readonly_review_in_progress] authoritative review is already active".to_owned(),
        ));
        assert_eq!(review.code, "graph.readonly_review_in_progress");
        assert_eq!(review.details["retryable"], true);

        let findings = graph_acceptance_repository_error(RepositoryError::ConstraintViolation(
            "[graph.readonly_findings_open] every Graph-linked finding must be closed".to_owned(),
        ));
        assert_eq!(findings.code, "graph.readonly_findings_open");
        assert_eq!(findings.details["retryable"], false);

        let stale = graph_acceptance_repository_error(RepositoryError::OptimisticLockFailed {
            id: "goal".to_owned(),
            expected_version: 7,
        });
        assert_eq!(stale.code, "graph.readonly_goal_version_conflict");
        assert_eq!(stale.details["retryable"], true);
    }

    #[test]
    fn graph_node_cancel_request_is_exact_bounded_and_host_owned() {
        let input = json!({
            "goalId": "goal",
            "taskId": "task",
            "attemptId": "attempt",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "reason": "The bounded inspection is no longer needed."
        });
        let parsed = parse_graph_node_cancel_request(&input).expect("parse exact cancel input");
        assert_eq!(parsed.goal_id, "goal");
        assert_eq!(parsed.expected_task_version, 2);
        assert_eq!(parsed.expected_attempt_version, 1);

        for forbidden in [
            "childRunId",
            "parentRunId",
            "status",
            "authority",
            "profile",
        ] {
            let mut leaked = input.clone();
            leaked[forbidden] = json!("model-controlled");
            let error = parse_graph_node_cancel_request(&leaked)
                .expect_err("Host authority cannot be model input");
            assert_eq!(error.code, "graph.readonly_node_cancel_invalid_input");
        }

        let mut invalid = input.clone();
        invalid["reason"] = json!(" reason");
        assert!(parse_graph_node_cancel_request(&invalid).is_err());
        invalid = input;
        invalid["expectedAttemptVersion"] = json!(0);
        assert!(parse_graph_node_cancel_request(&invalid).is_err());
    }

    #[test]
    fn plan_revision_tasks_keep_serial_compatibility_and_canonicalize_read_only_graphs() {
        let serial = validate_plan_tasks(
            &json!({
                "tasks": [
                    { "title": "Second", "detail": "serial detail", "ordinal": 1 },
                    { "title": "First", "ordinal": 0 }
                ]
            }),
            "goal-serial",
        )
        .expect("validate serial plan");
        assert_eq!(
            serial,
            json!([
                { "title": "First", "detail": null, "ordinal": 0 },
                { "title": "Second", "detail": "serial detail", "ordinal": 1 }
            ])
        );

        let graph = validate_plan_tasks(
            &json!({
                "tasks": [
                    {
                        "nodeKey": "right", "title": "Right", "ordinal": 2,
                        "dependsOn": ["root"], "acceptanceCriteria": ["right report inspected"]
                    },
                    {
                        "nodeKey": "root", "title": "Root", "ordinal": 0,
                        "acceptanceCriteria": ["root report is non-empty"]
                    },
                    {
                        "nodeKey": "left", "title": "Left", "ordinal": 1,
                        "dependsOn": ["root"], "acceptanceCriteria": ["left report inspected"]
                    }
                ]
            }),
            "goal-graph",
        )
        .expect("validate read-only Graph plan");
        assert_eq!(graph[0]["nodeKey"], "root");
        assert_eq!(graph[0]["dependsOn"], json!([]));
        assert_eq!(graph[1]["nodeKey"], "left");
        assert_eq!(graph[2]["nodeKey"], "right");
        assert_eq!(
            graph[2]["acceptanceCriteria"],
            json!(["right report inspected"])
        );

        validate_plan_tasks(
            &json!({ "tasks": [{ "title": "x".repeat(500), "ordinal": 0 }] }),
            "goal-serial",
        )
        .expect("serial title limit remains 500");
        let error = validate_plan_tasks(
            &json!({ "tasks": [{ "title": "x".repeat(501), "ordinal": 0 }] }),
            "goal-serial",
        )
        .expect_err("serial title over 500 must fail");
        assert_eq!(error.code, "plan.task_field_too_long");
        validate_plan_tasks(
            &json!({ "tasks": [{
                "nodeKey": "root", "title": "x".repeat(300), "ordinal": 0,
                "acceptanceCriteria": ["accepted"]
            }] }),
            "goal-graph",
        )
        .expect("Graph title limit matches Repository at 300");
        let error = validate_plan_tasks(
            &json!({ "tasks": [{
                "nodeKey": "root", "title": "x".repeat(301), "ordinal": 0,
                "acceptanceCriteria": ["accepted"]
            }] }),
            "goal-graph",
        )
        .expect_err("Graph title over Repository limit must fail");
        assert_eq!(error.code, "plan.task_field_too_long");
    }

    #[test]
    fn persistent_graph_work_tools_activate_and_serialize_a_bounded_snapshot() {
        let (database, path, conversation_id, run_id) = setup();
        database
            .apply_runtime_event(&run_id, 1, &json!({ "type": "run.started" }))
            .expect("start parent Run");
        database
            .freeze_run_execution_profile(&run_id, "durable_v2")
            .expect("freeze durable_v2");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "Persistent graph".to_owned(),
                objective: "Activate a bounded read-only graph".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .expect("create Goal");
        database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![
                    CreateTaskInput {
                        id: None,
                        goal_id: goal.id.clone(),
                        parent_task_id: None,
                        ordinal: 0,
                        title: "Root".to_owned(),
                        detail: None,
                    },
                    CreateTaskInput {
                        id: None,
                        goal_id: goal.id.clone(),
                        parent_task_id: None,
                        ordinal: 1,
                        title: "Next".to_owned(),
                        detail: None,
                    },
                ],
                vec!["standard_v1".to_owned(), "standard_v1".to_owned()],
            )
            .expect("create durable Tasks");
        let plan = database
            .create_plan_revision(
                &conversation_id,
                &goal.id,
                "Persistent read-only graph",
                "One root and one dependent",
                json!([
                    { "nodeKey": "root", "title": "Root", "ordinal": 0, "acceptanceCriteria": ["root inspected"] },
                    { "nodeKey": "next", "title": "Next", "ordinal": 1, "dependsOn": ["root"], "acceptanceCriteria": ["next inspected"] }
                ]),
                &run_id,
            )
            .expect("create PlanRevision");
        database
            .resolve_plan_revision(&conversation_id, &plan.id, "approved")
            .expect("approve PlanRevision");

        let before = execute(
            &database,
            &conversation_id,
            &run_id,
            "graph_readonly_snapshot_get",
            &json!({ "goalId": goal.id }),
        )
        .expect("read stable unactivated snapshot result");
        assert_eq!(before.result["details"]["activated"], false);
        assert_eq!(before.result["details"]["graph"], Value::Null);

        let missing_plan_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-missing-plan",
                "graph_readonly_activate",
                &json!({ "planRevisionId": "missing-plan" }),
                "running",
                false,
            )
            .expect("create missing-plan Host ToolCall");
        let missing_plan_error = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &missing_plan_call.id,
            "graph_readonly_activate",
            &json!({ "planRevisionId": "missing-plan" }),
        )
        .expect_err("missing PlanRevision must expose a stable WorkTool error");
        assert_eq!(missing_plan_error.code, "graph.readonly_plan_not_found");
        serde_json::to_string(&missing_plan_error.details)
            .expect("Graph Repository error details serialize");

        let activation_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-activate",
                "graph_readonly_activate",
                &json!({ "planRevisionId": plan.id }),
                "running",
                false,
            )
            .expect("create activation Host ToolCall");
        let activated = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &activation_call.id,
            "graph_readonly_activate",
            &json!({ "planRevisionId": plan.id }),
        )
        .expect("activate persistent Graph");
        assert_eq!(activated.result["details"]["activated"], true);
        assert_eq!(activated.result["details"]["replayed"], false);
        assert_eq!(activated.result["details"]["graph"]["graphId"], goal.id);
        assert_eq!(
            activated.result["details"]["graph"]["nodes"]
                .as_array()
                .map(Vec::len),
            Some(2)
        );
        assert_eq!(
            activated.result["details"]["graph"]["nodes"][0]["readiness"],
            "runnable"
        );
        assert_eq!(
            activated.result["details"]["graph"]["nodes"][1]["readiness"],
            "waiting"
        );
        serde_json::to_string(&activated.result).expect("Graph WorkTool result serializes");

        let root_task_id = activated.result["details"]["graph"]["nodes"][0]["taskId"]
            .as_str()
            .expect("root Task ID")
            .to_owned();
        let waiting_task_id = activated.result["details"]["graph"]["nodes"][1]["taskId"]
            .as_str()
            .expect("waiting Task ID")
            .to_owned();
        let root_version = activated.result["details"]["graph"]["nodes"][0]["taskVersion"]
            .as_i64()
            .expect("root Task version must be projected for CAS");
        let node_start_input = |task_id: &str, attempt_id: &str, expected_task_version: i64| {
            json!({
                "goalId": goal.id,
                "taskId": task_id,
                "expectedTaskVersion": expected_task_version,
                "attemptId": attempt_id,
                "workerAgentId": database.default_agent_id(),
                "objective": "Inspect the bounded Graph node inputs.",
                "context": "Use only read, ls, find, and grep; return a bounded report.",
                "budget": {
                    "maxDurationMs": 45_000,
                    "maxTotalTokens": 4_096,
                    "maxOutputTokens": 1_024,
                    "maxToolCalls": 6,
                },
            })
        };

        let stale_input = node_start_input(
            &root_task_id,
            "persistent-graph-stale-attempt",
            root_version + 1,
        );
        let stale_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-stale-start",
                "graph_readonly_node_start",
                &stale_input,
                "running",
                false,
            )
            .expect("create stale node-start Host ToolCall");
        let stale = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &stale_call.id,
            "graph_readonly_node_start",
            &stale_input,
        )
        .expect_err("stale CAS must reject node start");
        assert_eq!(stale.code, "graph.readonly_node_version_conflict");
        assert_eq!(stale.details["retryable"], true);

        let waiting_input =
            node_start_input(&waiting_task_id, "persistent-graph-waiting-attempt", 1);
        let waiting_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-waiting-start",
                "graph_readonly_node_start",
                &waiting_input,
                "running",
                false,
            )
            .expect("create waiting node-start Host ToolCall");
        let waiting = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &waiting_call.id,
            "graph_readonly_node_start",
            &waiting_input,
        )
        .expect_err("waiting dependency must reject node start");
        assert_eq!(waiting.code, "graph.readonly_node_not_runnable");
        assert_eq!(waiting.details["retryable"], false);

        let mut oversized_budget_input = node_start_input(
            &root_task_id,
            "persistent-graph-budget-attempt",
            root_version,
        );
        oversized_budget_input["budget"]["maxToolCalls"] = json!(7);
        let oversized_budget_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-budget-start",
                "graph_readonly_node_start",
                &oversized_budget_input,
                "running",
                false,
            )
            .expect("create oversized-budget node-start Host ToolCall");
        let oversized_budget = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &oversized_budget_call.id,
            "graph_readonly_node_start",
            &oversized_budget_input,
        )
        .expect_err("oversized Graph Child budget must fail closed");
        assert_eq!(oversized_budget.code, "graph.readonly_node_budget_invalid");

        let start_input =
            node_start_input(&root_task_id, "persistent-graph-root-attempt", root_version);
        let start_call = database
            .create_host_tool_call(
                &run_id,
                "persistent-graph-root-start",
                "graph_readonly_node_start",
                &start_input,
                "running",
                false,
            )
            .expect("create root node-start Host ToolCall");
        let started = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &start_call.id,
            "graph_readonly_node_start",
            &start_input,
        )
        .expect("start runnable root Graph node");
        assert_eq!(started.result["details"]["created"], true);
        assert_eq!(started.result["details"]["replayed"], false);
        assert_eq!(
            started.result["details"]["graph"]["nodes"][0]["readiness"],
            "active"
        );
        assert!(matches!(
            started.post_finalize,
            Some(WorkToolPostFinalizeDirective::StartChild(_))
        ));

        let exact_node_replay = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &start_call.id,
            "graph_readonly_node_start",
            &start_input,
        )
        .expect("exact Repository node-start replay");
        assert_eq!(exact_node_replay.result["details"]["created"], false);
        assert_eq!(exact_node_replay.result["details"]["replayed"], true);
        assert!(
            exact_node_replay.post_finalize.is_none(),
            "Repository exact replay must never dispatch the Child twice"
        );

        let repository_replay = execute_with_host_tool_call_id(
            &database,
            &conversation_id,
            &run_id,
            &activation_call.id,
            "graph_readonly_activate",
            &json!({ "planRevisionId": plan.id }),
        )
        .expect("exact Repository activation replay");
        assert_eq!(repository_replay.result["details"]["replayed"], true);

        let after = execute(
            &database,
            &conversation_id,
            &run_id,
            "graph_readonly_snapshot_get",
            &json!({ "goalId": goal.id }),
        )
        .expect("read activated Graph snapshot");
        assert_eq!(after.result["details"]["activated"], true);
        assert_eq!(
            after.result["details"]["graph"]["specHash"],
            activated.result["details"]["graph"]["specHash"]
        );

        let legacy_conversation_id = database
            .create_conversation(
                database.default_agent_id(),
                Some("legacy graph request"),
                None,
                None,
            )
            .expect("create Legacy conversation")
            .id;
        let legacy_run = database
            .create_run(&legacy_conversation_id, "legacy graph request", None)
            .expect("create Legacy Run")
            .run;
        database
            .apply_runtime_event(&legacy_run.id, 1, &json!({ "type": "run.started" }))
            .expect("start Legacy Run");
        database
            .freeze_run_execution_profile(&legacy_run.id, "legacy")
            .expect("freeze Legacy profile");
        let profile_error = execute(
            &database,
            &legacy_conversation_id,
            &legacy_run.id,
            "graph_readonly_snapshot_get",
            &json!({ "goalId": goal.id }),
        )
        .expect_err("Legacy Run must not access persistent Graph tools");
        assert_eq!(profile_error.code, "graph.readonly_profile_required");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn plan_revision_graph_contract_rejects_partial_unsafe_or_unbounded_inputs() {
        let cases = [
            (
                "partial Graph",
                json!([{ "title": "Root", "ordinal": 0, "dependsOn": [] }]),
                "plan.task_field_invalid",
            ),
            (
                "too many nodes",
                json!([
                    { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a"] },
                    { "nodeKey": "b", "title": "B", "ordinal": 1, "acceptanceCriteria": ["b"] },
                    { "nodeKey": "c", "title": "C", "ordinal": 2, "acceptanceCriteria": ["c"] },
                    { "nodeKey": "d", "title": "D", "ordinal": 3, "acceptanceCriteria": ["d"] }
                ]),
                "plan.graph_too_many_nodes",
            ),
            (
                "duplicate key",
                json!([
                    { "nodeKey": "same", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a"] },
                    { "nodeKey": "same", "title": "B", "ordinal": 1, "acceptanceCriteria": ["b"] }
                ]),
                "plan.graph_node_key_duplicate",
            ),
            (
                "unknown dependency",
                json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["missing"], "acceptanceCriteria": ["a"] }]),
                "plan.graph_dependency_unknown",
            ),
            (
                "duplicate dependency",
                json!([
                    { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a"] },
                    { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a", "a"], "acceptanceCriteria": ["b"] }
                ]),
                "plan.graph_value_duplicate",
            ),
            (
                "self dependency",
                json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["a"], "acceptanceCriteria": ["a"] }]),
                "plan.graph_self_dependency",
            ),
            (
                "cycle",
                json!([
                    { "nodeKey": "a", "title": "A", "ordinal": 0, "dependsOn": ["b"], "acceptanceCriteria": ["a"] },
                    { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a"], "acceptanceCriteria": ["b"] }
                ]),
                "plan.graph_cycle",
            ),
            (
                "depth two",
                json!([
                    { "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a"] },
                    { "nodeKey": "b", "title": "B", "ordinal": 1, "dependsOn": ["a"], "acceptanceCriteria": ["b"] },
                    { "nodeKey": "c", "title": "C", "ordinal": 2, "dependsOn": ["b"], "acceptanceCriteria": ["c"] }
                ]),
                "plan.graph_depth_exceeded",
            ),
            (
                "missing criteria",
                json!([{ "nodeKey": "a", "title": "A", "ordinal": 0 }]),
                "plan.graph_field_required",
            ),
            (
                "empty criteria",
                json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": [] }]),
                "plan.graph_field_invalid",
            ),
            (
                "invalid key",
                json!([{ "nodeKey": "bad key", "title": "A", "ordinal": 0, "acceptanceCriteria": ["a"] }]),
                "plan.graph_node_key_invalid",
            ),
            (
                "title edge whitespace",
                json!([{ "nodeKey": "a", "title": " A", "ordinal": 0, "acceptanceCriteria": ["a"] }]),
                "plan.task_field_invalid",
            ),
            (
                "criterion unicode edge whitespace",
                json!([{ "nodeKey": "a", "title": "A", "ordinal": 0, "acceptanceCriteria": ["\u{a0}criterion"] }]),
                "plan.task_field_invalid",
            ),
            (
                "runtime state",
                json!([{ "title": "A", "ordinal": 0, "status": "completed" }]),
                "plan.task_fields_unsupported",
            ),
        ];

        for (label, tasks, expected_code) in cases {
            let error =
                validate_plan_tasks(&json!({ "tasks": tasks }), "goal-graph").expect_err(label);
            assert_eq!(error.code, expected_code, "{label}");
        }

        let too_many_criteria = (0..9)
            .map(|index| Value::String(format!("criterion-{index}")))
            .collect::<Vec<_>>();
        let error = validate_plan_tasks(
            &json!({ "tasks": [{
                "nodeKey": "a", "title": "A", "ordinal": 0,
                "acceptanceCriteria": too_many_criteria
            }] }),
            "goal-graph",
        )
        .expect_err("acceptance criteria must be bounded");
        assert_eq!(error.code, "plan.graph_field_invalid");
    }

    #[test]
    fn repair_override_host_schema_is_exact_bounded_and_never_directly_executable() {
        let valid = json!({
            "taskId": "task-1",
            "attemptId": "attempt-override-1",
            "expectedVersion": 7,
            "rootCause": "the ordinary bounded repairs did not remove the root cause",
            "findingIds": ["finding-1"],
            "escalationReason": "the operator reviewed the evidence and authorized one final repair"
        });
        validate_task_repair_override_request(&valid).expect("valid exact schema");
        assert!(is_work_tool("task_repair_escalate_start"));
        let padded = json!({
            "taskId": " task-1 ",
            "attemptId": " attempt-override-1 ",
            "expectedVersion": 7,
            "rootCause": "  the ordinary bounded repairs did not remove the root cause  ",
            "findingIds": [" finding-1 "],
            "escalationReason": "  the operator reviewed the evidence and authorized one final repair  "
        });
        assert!(
            canonicalize_task_repair_override_request(&padded).is_err(),
            "Host rejects non-canonical whitespace before Runtime projection promotion or approval"
        );
        for whitespace_only in [
            json!({
                "taskId": "task-1", "attemptId": "attempt-override-1",
                "expectedVersion": 7, "rootCause": "   ",
                "findingIds": ["finding-1"], "escalationReason": "operator approval"
            }),
            json!({
                "taskId": "task-1", "attemptId": "attempt-override-1",
                "expectedVersion": 7, "rootCause": "root",
                "findingIds": ["   "], "escalationReason": "operator approval"
            }),
            json!({
                "taskId": "task-1", "attemptId": "attempt-override-1",
                "expectedVersion": 7, "rootCause": "root",
                "findingIds": ["finding-1"], "escalationReason": "   "
            }),
        ] {
            assert!(canonicalize_task_repair_override_request(&whitespace_only).is_err());
        }
        for edge in ['\u{feff}', '\u{0085}'] {
            for field in ["taskId", "attemptId", "rootCause", "escalationReason"] {
                for value in [format!("{edge}value"), format!("value{edge}")] {
                    let mut candidate = valid.clone();
                    candidate[field] = Value::String(value);
                    assert!(
                        canonicalize_task_repair_override_request(&candidate).is_err(),
                        "Host must reject {field} with U+{:04X} at either boundary",
                        edge as u32
                    );
                }
            }
            for value in [format!("{edge}finding-1"), format!("finding-1{edge}")] {
                let mut candidate = valid.clone();
                candidate["findingIds"] = json!([value]);
                assert!(
                    canonicalize_task_repair_override_request(&candidate).is_err(),
                    "Host must reject findingIds items with U+{:04X} at either boundary",
                    edge as u32
                );
            }
        }
        let mut internal_whitespace = valid.clone();
        internal_whitespace["rootCause"] =
            Value::String("root\u{feff}cause\u{0085}detail".to_owned());
        canonicalize_task_repair_override_request(&internal_whitespace)
            .expect("internal FEFF/U+0085 are content, not surrounding whitespace");

        for invalid in [
            json!({
                "taskId": "task-1",
                "attemptId": "attempt-override-1",
                "expectedVersion": 7,
                "rootCause": "root",
                "findingIds": ["finding-1"],
                "escalationReason": "operator approval",
                "runId": "runtime-spoof"
            }),
            json!({
                "taskId": "task-1",
                "attemptId": "attempt-override-1",
                "expectedVersion": 7,
                "rootCause": "root",
                "findingIds": ["finding-1", "finding-1"],
                "escalationReason": "operator approval"
            }),
            json!({
                "taskId": "task-1",
                "attemptId": "attempt-override-1",
                "expectedVersion": 0,
                "rootCause": "root",
                "findingIds": ["finding-1"],
                "escalationReason": "operator approval"
            }),
            json!({
                "taskId": "t".repeat(201),
                "attemptId": "attempt-override-1",
                "expectedVersion": 7,
                "rootCause": "root",
                "findingIds": ["finding-1"],
                "escalationReason": "operator approval"
            }),
        ] {
            assert!(validate_task_repair_override_request(&invalid).is_err());
        }

        let (database, path, conversation_id, run_id) = setup();
        let direct = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_repair_escalate_start",
            &valid,
        )
        .expect_err("direct execution must not bypass Host approval lifecycle");
        assert_eq!(direct.code, "task.repair_override_approval_required");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn work_tools_enforce_host_owned_state_and_optimistic_versions() {
        let (database, path, conversation_id, run_id) = setup();
        let proposed = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_propose",
            &json!({ "title": "A0", "objective": "修复跨文件问题并运行测试" }),
        )
        .expect("propose goal");
        let goal_id = proposed.result["details"]["goal"]["id"]
            .as_str()
            .expect("goal id")
            .to_owned();
        assert!(!is_work_tool("goal_activate"));
        let activation_error = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_activate",
            &json!({ "goalId": goal_id, "expectedVersion": 1 }),
        )
        .expect_err("runtime must not activate a goal directly");
        assert!(activation_error
            .to_string()
            .contains("unsupported work tool"));
        let proposed_goal = database.goals().get(&goal_id).unwrap().unwrap();
        crate::work_mode_gate::resolve_confirmation(
            &database,
            &crate::work_mode_gate::ResolveWorkModeConfirmationRequest {
                conversation_id: conversation_id.clone(),
                goal_id: goal_id.clone(),
                approved: true,
                expected_version: proposed_goal.version,
            },
        )
        .expect("Host confirmation activates the proposed goal");
        assert_eq!(
            database.goals().get(&goal_id).unwrap().unwrap().status,
            GoalStatus::Active
        );
        let created = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal_id,
                "tasks": [{ "title": "Implement", "ordinal": 0 }]
            }),
        )
        .expect("create tasks");
        let task_id = created.result["details"]["tasks"][0]["id"]
            .as_str()
            .expect("task id")
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_id, "status": "in_progress", "expectedVersion": 1 }),
        )
        .expect("start task");
        assert!(execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_id, "status": "blocked", "expectedVersion": 1, "blockedReason": "stale" }),
        )
        .expect_err("stale task update must fail")
        .to_string()
        .contains("version"));
        let added = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task_id,
                "evidenceType": "external_reference",
                "refKind": "source",
                "refId": "https://example.com/proof",
                "summary": "proof"
            }),
        )
        .expect("add evidence");
        let evidence_id = added.result["details"]["evidence"]["id"]
            .as_str()
            .expect("evidence id")
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_validate",
            &json!({ "evidenceId": evidence_id }),
        )
        .expect("validate evidence");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_id, "status": "completed", "expectedVersion": 2 }),
        )
        .expect("complete task");
        let active_goal_version = database.goals().get(&goal_id).unwrap().unwrap().version;
        let completed = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal_id, "expectedVersion": active_goal_version }),
        )
        .expect("complete goal");
        assert_eq!(completed.result["details"]["goal"]["status"], "completed");
        assert!(completed.result["details"]["goal"]["completedAt"].is_string());
        assert!(completed.result["details"]["workEvents"]
            .as_array()
            .is_some_and(|events| events.iter().any(|event| event["type"] == "goal.completed")));
        let snapshot = execute(
            &database,
            &conversation_id,
            &run_id,
            "work_snapshot_get",
            &json!({}),
        )
        .expect("snapshot");
        assert!(snapshot.result["details"]["goal"].is_null());
        assert!(database.list_work_events(&conversation_id).unwrap().len() >= 7);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a1_acceptance_requires_plan_review_and_valid_evidence() {
        let (database, path, conversation_id, run_id) = setup();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "A1 workflow".to_owned(),
                objective: "implement, review, and accept a verified change".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .expect("create active goal");
        let goal_id = goal.id;
        let created = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal_id, "tasks": [{ "title": "Implement", "ordinal": 0 }]
            }),
        )
        .expect("create task");
        let task_id = created.result["details"]["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({
                "taskId": task_id, "status": "in_progress", "expectedVersion": 1
            }),
        )
        .expect("start task");
        let evidence = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task_id, "evidenceType": "external_reference", "refKind": "source",
                "refId": "https://example.com/a1", "summary": "verified result"
            }),
        )
        .expect("add evidence");
        let evidence_id = evidence.result["details"]["evidence"]["id"]
            .as_str()
            .unwrap();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_validate",
            &json!({ "evidenceId": evidence_id }),
        )
        .expect("validate evidence");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({
                "taskId": task_id, "status": "completed", "expectedVersion": 2
            }),
        )
        .expect("complete task");
        let plan = execute(
            &database,
            &conversation_id,
            &run_id,
            "plan_revision_create",
            &json!({
                "goalId": goal_id, "title": "Implementation plan", "summary": "one verified task",
                "tasks": [{ "title": "Implement", "ordinal": 0 }]
            }),
        )
        .expect("create plan revision");
        let plan_id = plan.result["details"]["planRevision"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(plan.result["details"]["planRevision"]["status"], "proposed");

        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal_id, "tasks": [{ "title": "must wait", "ordinal": 1 }]
            }),
        )
        .expect_err("a proposed plan must pause state-changing work");
        assert_eq!(error.code, "plan.approval_required");

        let error = execute(&database, &conversation_id, &run_id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "done",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id]
            }]
        })).expect_err("a proposed plan cannot be accepted");
        assert_eq!(error.code, "acceptance.plan_required");
        let approved_plan = database
            .resolve_plan_revision(&conversation_id, &plan_id, "approved")
            .expect("approve plan revision through Host boundary");
        assert_eq!(approved_plan.status, "approved");

        let error = execute(&database, &conversation_id, &run_id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "done",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id]
            }]
        })).expect_err("review is required");
        assert_eq!(error.code, "acceptance.review_required");

        database
            .apply_runtime_event(&run_id, 1, &json!({ "type": "run.completed" }))
            .expect("complete implementation run");
        let review_run = database
            .create_run(&conversation_id, "独立审查已完成任务和证据", None)
            .expect("create review run")
            .run;
        execute(&database, &conversation_id, &review_run.id, "review_finding_add", &json!({
            "goalId": goal_id, "planRevisionId": plan_id, "severity": "info", "category": "verification",
            "title": "Independent review completed", "detail": "No blocking finding after evidence review.",
            "status": "resolved"
        })).expect("record independent review");
        let stale = execute(&database, &conversation_id, &review_run.id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 99, "summary": "plan, review, tasks, and evidence accepted",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id], "detail": "reviewed the validated implementation evidence"
            }]
        })).expect_err("stale Goal CAS must not leave Acceptance behind");
        assert!(stale.message.contains("version"));
        assert!(database
            .load_conversation(&conversation_id)
            .unwrap()
            .acceptances
            .is_empty());
        let medium_blocker = database
            .add_review_finding(
                &conversation_id,
                &goal_id,
                Some(&task_id),
                Some(&plan_id),
                "medium",
                "correctness",
                "medium blocker",
                "must be resolved before acceptance",
                "open",
                &review_run.id,
            )
            .unwrap();
        let blocked = execute(&database, &conversation_id, &review_run.id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "plan, review, tasks, and evidence accepted",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id], "detail": "reviewed the validated implementation evidence"
            }]
        })).expect_err("open medium Finding must atomically block Acceptance");
        assert_eq!(blocked.code, "acceptance.review_blocked");
        assert!(database
            .load_conversation(&conversation_id)
            .unwrap()
            .acceptances
            .is_empty());
        database
            .resolve_review_finding(&medium_blocker.id, "resolved", &review_run.id)
            .unwrap();
        let accepted = execute(&database, &conversation_id, &review_run.id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "plan, review, tasks, and evidence accepted",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id], "detail": "reviewed the validated implementation evidence"
            }]
        })).expect("accept goal");
        assert_eq!(accepted.result["details"]["goal"]["status"], "completed");
        let repeated = execute(&database, &conversation_id, &review_run.id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "plan, review, tasks, and evidence accepted",
            "checks": [{
                "criterion": "implementation verified", "method": "inspection", "status": "passed",
                "evidenceIds": [evidence_id], "detail": "reviewed the validated implementation evidence"
            }]
        })).expect("identical Acceptance retry is idempotent");
        assert_eq!(
            repeated.result["details"]["acceptance"]["id"],
            accepted.result["details"]["acceptance"]["id"]
        );
        let acceptances = database
            .load_conversation(&conversation_id)
            .unwrap()
            .acceptances;
        assert_eq!(acceptances.len(), 1);
        assert_eq!(acceptances[0].reviewer, review_run.id);
        assert_eq!(acceptances[0].checks["criteria"][0]["method"], "inspection");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn conversational_markdown_cannot_create_a_runtime_goal() {
        let path = std::env::temp_dir().join(format!("fox-work-tools-chat-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("chat"), None, None)
            .expect("create conversation");
        let request = "### 3. 什么是数据泄漏\n\n如果训练过程看到了本不该获得的信息，离线分数会虚高。\n\n- 根据测试集分数反复选择参数；\n- 先做全数据特征选择，再交叉验证。\n\n正确顺序通常是：先划分，再只在训练集上 fit 预处理器；对验证/测试只调用 transform。";
        let run = database
            .create_run(&conversation.id, request, None)
            .expect("create run")
            .run;

        let outcome = execute(
            &database,
            &conversation.id,
            &run.id,
            "goal_propose",
            &json!({
                "title": "解释数据泄漏",
                "objective": "整理数据泄漏与训练测试划分"
            }),
        )
        .expect("ignore misplaced runtime proposal");

        assert!(outcome.result["details"]["goal"].is_null());
        assert_eq!(
            outcome.result["details"]["gateDecision"],
            "stay_conversation"
        );
        assert!(database
            .goals()
            .get_by_conversation(&conversation.id)
            .unwrap()
            .is_empty());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn goal_complete_rejects_open_tasks() {
        let (database, path, conversation_id, run_id) = setup();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "未完成目标".to_owned(),
                objective: "验证 Host 不接受模型提前完成".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .expect("create goal");
        database
            .work_tasks()
            .create_many(vec![CreateTaskInput {
                id: None,
                goal_id: goal.id.clone(),
                parent_task_id: None,
                ordinal: 0,
                title: "仍在排队".to_owned(),
                detail: None,
            }])
            .expect("create open task");

        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal.id, "expectedVersion": goal.version }),
        )
        .expect_err("open task must block goal completion");

        assert!(error.to_string().contains("unfinished"));
        assert_eq!(
            database.goals().get(&goal.id).unwrap().unwrap().status,
            GoalStatus::Active
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn goal_complete_requires_valid_evidence_for_each_completed_task() {
        let (database, path, conversation_id, run_id) = setup();
        database
            .apply_runtime_event(&run_id, 1, &json!({ "type": "run.started" }))
            .expect("start Run before creating verification ToolCalls");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "证据验收目标".to_owned(),
                objective: "验证 Host 只接受经过校验的完成证据".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .expect("create goal");
        let task = database
            .work_tasks()
            .create_many(vec![CreateTaskInput {
                id: None,
                goal_id: goal.id.clone(),
                parent_task_id: None,
                ordinal: 0,
                title: "执行并验证".to_owned(),
                detail: None,
            }])
            .expect("create task")
            .remove(0);
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task.id, "status": "in_progress", "expectedVersion": 1 }),
        )
        .expect("start task");
        let tool_call = database
            .create_host_tool_call(
                &run_id,
                "verify-evidence",
                "run_command",
                &json!({ "command": "cargo test" }),
                "running",
                false,
            )
            .expect("create verification call");
        database
            .complete_host_tool_call(
                &run_id,
                "verify-evidence",
                Some(&json!({ "passed": true })),
                None,
            )
            .expect("complete verification call");
        let evidence = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task.id,
                "evidenceType": "test_result",
                "refKind": "tool_call",
                "refId": tool_call.id,
                "summary": "验证命令通过"
            }),
        )
        .expect("add unverified evidence");
        let evidence_id = evidence.result["details"]["evidence"]["id"]
            .as_str()
            .expect("evidence id")
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task.id, "status": "completed", "expectedVersion": 2 }),
        )
        .expect("complete task");

        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal.id, "expectedVersion": goal.version }),
        )
        .expect_err("unverified evidence must block goal completion");
        assert_eq!(error.code, "goal.valid_evidence_required");
        assert_eq!(error.details["tasks"][0]["taskId"], task.id);
        assert_eq!(
            error.details["tasks"][0]["evidence"][0]["validityStatus"],
            "unverified"
        );

        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_validate",
            &json!({ "evidenceId": evidence_id }),
        )
        .expect("validate evidence");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal.id, "expectedVersion": goal.version }),
        )
        .expect("complete goal with valid evidence");

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn proposed_goal_cannot_receive_tasks_before_host_confirmation() {
        let (database, path, conversation_id, run_id) = setup();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "待确认目标".to_owned(),
                objective: "等待 Host 确认后再创建任务".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Proposed,
                created_by: "host:work-mode-gate".to_owned(),
            })
            .expect("create proposed goal");

        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal.id,
                "tasks": [{ "title": "不应落库", "ordinal": 0 }]
            }),
        )
        .expect_err("proposed goal must reject task creation");

        assert!(error.to_string().contains("activates the goal"));
        let (_, tasks, _) = database
            .load_work_graph_snapshot(&conversation_id)
            .expect("load work graph");
        assert!(tasks.iter().all(|task| task.goal_id != goal.id));
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cross_file_bug_workflow_restores_three_tasks_and_evidence_after_restart() {
        let (database, path, conversation_id, run_id) = setup();
        database
            .apply_runtime_event(&run_id, 1, &json!({ "type": "run.started" }))
            .expect("start Run before creating workflow ToolCalls");
        let artifact_root =
            std::env::temp_dir().join(format!("fox-a0-cross-file-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&artifact_root).expect("create artifact fixture directory");

        let proposed = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_propose",
            &json!({
                "title": "修复跨三文件状态恢复问题",
                "objective": "定位、修改三个文件并验证重启恢复"
            }),
        )
        .expect("create active goal");
        let goal_id = proposed.result["details"]["goal"]["id"]
            .as_str()
            .expect("goal id")
            .to_owned();
        let proposed_goal = database.goals().get(&goal_id).unwrap().unwrap();
        crate::work_mode_gate::resolve_confirmation(
            &database,
            &crate::work_mode_gate::ResolveWorkModeConfirmationRequest {
                conversation_id: conversation_id.clone(),
                goal_id: goal_id.clone(),
                approved: true,
                expected_version: proposed_goal.version,
            },
        )
        .expect("Host confirmation activates the proposed goal");
        let created = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal_id,
                "tasks": [
                    { "title": "定位", "ordinal": 0 },
                    { "title": "修改", "ordinal": 1 },
                    { "title": "验证", "ordinal": 2 }
                ]
            }),
        )
        .expect("create acceptance tasks");
        let task_ids = created.result["details"]["tasks"]
            .as_array()
            .expect("tasks")
            .iter()
            .map(|task| task["id"].as_str().expect("task id").to_owned())
            .collect::<Vec<_>>();

        let locate_call = database
            .create_host_tool_call(
                &run_id,
                "locate-cross-file-bug",
                "grep",
                &json!({ "pattern": "stale work graph" }),
                "running",
                false,
            )
            .expect("create locate tool call");
        database
            .complete_host_tool_call(
                &run_id,
                "locate-cross-file-bug",
                Some(&json!({ "matches": 3 })),
                None,
            )
            .expect("complete locate tool call");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[0], "status": "in_progress", "expectedVersion": 1 }),
        )
        .expect("start locate task");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task_ids[0],
                "evidenceType": "tool_call",
                "refKind": "tool_call",
                "refId": locate_call.id,
                "summary": "定位到状态模型、Host 查询和前端 Reducer 三处"
            }),
        )
        .expect("add locate evidence");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[0], "status": "completed", "expectedVersion": 2 }),
        )
        .expect("complete locate task");

        let fixture_files = ["state-model.rs", "runtime-host.rs", "goal-view.tsx"];
        for (index, name) in fixture_files.iter().enumerate() {
            let file = artifact_root.join(name);
            std::fs::write(&file, format!("fixed fixture {index}\n")).expect("write fixture");
            let runtime_tool_call_id = format!("edit-cross-file-{index}");
            database
                .create_host_tool_call(
                    &run_id,
                    &runtime_tool_call_id,
                    "edit_file",
                    &json!({ "path": file }),
                    "running",
                    false,
                )
                .expect("create edit tool call");
            database
                .complete_host_tool_call(
                    &run_id,
                    &runtime_tool_call_id,
                    Some(&json!({
                        "details": {
                            "path": file.to_string_lossy(),
                            "bytes": std::fs::metadata(&file).expect("fixture metadata").len()
                        }
                    })),
                    None,
                )
                .expect("complete edit tool call");
        }
        let artifacts = database
            .load_conversation(&conversation_id)
            .expect("load artifacts")
            .artifacts;
        assert_eq!(artifacts.len(), 3);
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[1], "status": "in_progress", "expectedVersion": 1 }),
        )
        .expect("start modify task");
        for artifact in &artifacts {
            execute(
                &database,
                &conversation_id,
                &run_id,
                "task_evidence_add",
                &json!({
                    "taskId": task_ids[1],
                    "evidenceType": "file_diff",
                    "refKind": "artifact",
                    "refId": artifact.id,
                    "summary": format!("修改 {}", artifact.display_name)
                }),
            )
            .expect("add file diff evidence");
        }
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[1], "status": "completed", "expectedVersion": 2 }),
        )
        .expect("complete modify task");

        let test_call = database
            .create_host_tool_call(
                &run_id,
                "verify-cross-file-fix",
                "shell_command",
                &json!({ "command": "cargo test && bun test" }),
                "running",
                false,
            )
            .expect("create verification tool call");
        database
            .complete_host_tool_call(
                &run_id,
                "verify-cross-file-fix",
                Some(&json!({ "passed": true })),
                None,
            )
            .expect("complete verification tool call");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[2], "status": "in_progress", "expectedVersion": 1 }),
        )
        .expect("start verify task");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task_ids[2],
                "evidenceType": "test_result",
                "refKind": "tool_call",
                "refId": test_call.id,
                "summary": "Rust、前端和 Runtime 测试全部通过"
            }),
        )
        .expect("add test evidence");
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_ids[2], "status": "completed", "expectedVersion": 2 }),
        )
        .expect("complete verify task");
        let evidence_before_restart = database
            .load_conversation(&conversation_id)
            .expect("load evidence before restart")
            .evidence;
        assert_eq!(evidence_before_restart.len(), 5);
        for item in evidence_before_restart {
            assert_eq!(
                database
                    .task_evidence()
                    .validate(&item.id)
                    .expect("validate acceptance evidence"),
                crate::database::EvidenceValidityStatus::Valid
            );
        }
        let active_goal_version = database.goals().get(&goal_id).unwrap().unwrap().version;
        execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal_id, "expectedVersion": active_goal_version }),
        )
        .expect("complete acceptance goal");

        drop(database);
        let restored = Database::open(path.clone()).expect("reopen database");
        let detail = restored
            .load_conversation(&conversation_id)
            .expect("load restored conversation");
        assert_eq!(detail.goals.len(), 1);
        assert_eq!(detail.goals[0].status, GoalStatus::Completed);
        assert_eq!(detail.tasks.len(), 3);
        assert!(detail
            .tasks
            .iter()
            .all(|task| task.status == WorkTaskStatus::Completed && task.attempt == 1));
        assert_eq!(detail.evidence.len(), 5);
        assert!(detail
            .evidence
            .iter()
            .all(|item| item.source_run_id.as_deref() == Some(&run_id)));
        assert!(detail.evidence.iter().all(|item| {
            item.validity_status == crate::database::EvidenceValidityStatus::Valid
        }));

        drop(restored);
        let _ = std::fs::remove_file(path);
        std::fs::remove_dir_all(artifact_root).expect("remove artifact fixture directory");
    }

    #[test]
    fn a0_snapshot_and_state_update_p95_stay_within_budget() {
        let (database, path, conversation_id, run_id) = setup();
        let goal = database
            .goals()
            .create(crate::database::CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "Performance baseline".to_owned(),
                objective: "Measure a 500-task snapshot".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .expect("create performance goal");
        let tasks = database
            .work_tasks()
            .create_many(
                (0..500)
                    .map(|ordinal| crate::database::CreateTaskInput {
                        id: None,
                        goal_id: goal.id.clone(),
                        parent_task_id: None,
                        ordinal,
                        title: format!("Task {ordinal}"),
                        detail: None,
                    })
                    .collect(),
            )
            .expect("create 500 tasks");

        database
            .load_conversation(&conversation_id)
            .expect("warm conversation snapshot");
        let snapshot_samples = (0..60)
            .map(|_| {
                let started = Instant::now();
                let detail = database
                    .load_conversation(&conversation_id)
                    .expect("load conversation snapshot");
                assert_eq!(detail.tasks.len(), 500);
                started.elapsed()
            })
            .collect::<Vec<_>>();
        let snapshot_p95 = percentile_95(snapshot_samples);
        assert!(
            snapshot_p95 < Duration::from_millis(50),
            "conversation snapshot P95 {snapshot_p95:?} exceeded 50ms"
        );

        let task_id = tasks[0].id.clone();
        let mut version = tasks[0].version;
        let mut transaction_samples = Vec::with_capacity(80);
        let started = Instant::now();
        let active = database
            .work_tasks()
            .update(
                &task_id,
                version,
                WorkTaskStatus::InProgress,
                Some(&run_id),
                None,
            )
            .expect("start measured task");
        transaction_samples.push(started.elapsed());
        version = active.version;
        for _ in 0..40 {
            let started = Instant::now();
            let blocked = database
                .work_tasks()
                .update(
                    &task_id,
                    version,
                    WorkTaskStatus::Blocked,
                    None,
                    Some("performance cycle".to_owned()),
                )
                .expect("block measured task");
            transaction_samples.push(started.elapsed());
            version = blocked.version;

            let started = Instant::now();
            let next = database
                .work_tasks()
                .update(
                    &task_id,
                    version,
                    WorkTaskStatus::InProgress,
                    Some(&run_id),
                    None,
                )
                .expect("resume measured task");
            transaction_samples.push(started.elapsed());
            version = next.version;
        }
        let transaction_p95 = percentile_95(transaction_samples);
        eprintln!(
            "A0 backend performance baseline: snapshot_p95={snapshot_p95:?}, transaction_p95={transaction_p95:?}"
        );
        assert!(
            transaction_p95 < Duration::from_millis(20),
            "state transaction P95 {transaction_p95:?} exceeded 20ms"
        );

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn snapshot_identity_is_owned_by_the_host_envelope() {
        let (database, path, conversation_id, run_id) = setup();
        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .expect("create second conversation");
        let other_goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: other.id.clone(),
                title: "其他对话目标".to_owned(),
                objective: "不能通过模型参数读取".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: "test".to_owned(),
            })
            .expect("create other goal");
        let snapshot = execute(
            &database,
            &conversation_id,
            &run_id,
            "work_snapshot_get",
            &json!({ "conversationId": other.id }),
        )
        .expect("snapshot uses envelope conversation");
        assert!(snapshot.result["details"]["goal"].is_null());
        assert_ne!(
            snapshot.result["details"]["goal"]["id"].as_str(),
            Some(other_goal.id.as_str())
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn work_snapshot_v2_source_identity_is_stable_and_covers_a1_facts() {
        let (database, path, conversation_id, _) = setup();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "atomic identity".to_owned(),
                objective: "hash the emitted facts".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: "test".to_owned(),
            })
            .unwrap();
        let first = snapshot(&database, &conversation_id).unwrap();
        let repeated = snapshot(&database, &conversation_id).unwrap();
        assert_eq!(first["schemaVersion"], 2);
        assert_eq!(first["sourceHash"], repeated["sourceHash"]);
        assert_eq!(first["sourceHash"].as_str().unwrap().len(), 64);
        assert_eq!(
            first["generatedAt"],
            first["taskLedger"]["projectionMeta"]["generatedAt"]
        );
        assert_eq!(first["sourceHighWatermark"]["selectedGoalId"], goal.id);

        database
            .create_plan_revision(
                &conversation_id,
                &goal.id,
                "plan",
                "new A1 fact",
                json!([]),
                "test",
            )
            .unwrap();
        let changed = snapshot(&database, &conversation_id).unwrap();
        assert_ne!(first["sourceHash"], changed["sourceHash"]);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn durable_host_tools_freeze_policy_and_close_attempt_with_validated_checks() {
        let (database, path, conversation_id, run_id) = setup();
        database
            .freeze_run_execution_profile(&run_id, "durable_v2")
            .unwrap();
        database
            .apply_runtime_event(
                &run_id,
                1,
                &json!({ "type": "run.started", "executionProfileId": "durable_v2" }),
            )
            .unwrap();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "durable validation".to_owned(),
                objective: "freeze and enforce Host validation".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .unwrap();
        let bad_risk = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal.id,
                "tasks": [{ "title": "bad", "ordinal": 0, "riskLevel": "mystery" }]
            }),
        )
        .expect_err("unknown risk must fail closed");
        assert!(bad_risk.message.contains("unsupported task risk level"));

        let created = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal.id,
                "tasks": [{ "title": "inspect", "ordinal": 0, "riskLevel": "standard" }]
            }),
        )
        .unwrap();
        let task_id = created.result["details"]["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            created.result["details"]["validationPolicies"][0]["snapshot"]["id"],
            "standard_v1"
        );
        let skip = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_update",
            &json!({ "taskId": task_id, "status": "skipped", "expectedVersion": 1 }),
        )
        .expect_err("durable Task cannot bypass validation through skip");
        assert_eq!(skip.code, "task.attempt_required");
        let workflow = execute(
            &database,
            &conversation_id,
            &run_id,
            "workflow_start",
            &json!({}),
        )
        .expect_err("durable workflow mutator cannot bypass Attempts");
        assert_eq!(workflow.code, "workflow.durable_attempt_required");
        let started = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_attempt_start",
            &json!({
                "taskId": task_id,
                "attemptId": "host-attempt-1",
                "expectedVersion": 1
            }),
        )
        .unwrap();
        assert_eq!(started.result["details"]["attempt"]["status"], "running");
        let inspection_call = database
            .create_host_tool_call(
                &run_id,
                "durable-inspection",
                "host_validation_check",
                &json!({}),
                "running",
                false,
            )
            .unwrap();
        database
            .complete_host_tool_call(
                &run_id,
                "durable-inspection",
                Some(&json!({ "passed": true })),
                None,
            )
            .unwrap();
        let added = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_add",
            &json!({
                "taskId": task_id,
                "evidenceType": "tool_call",
                "refKind": "tool_call",
                "refId": inspection_call.id,
                "summary": "Host inspection passed",
                "validationCheckType": "inspection"
            }),
        )
        .unwrap();
        let evidence_id = added.result["details"]["evidence"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_evidence_validate",
            &json!({ "evidenceId": evidence_id }),
        )
        .unwrap();
        let finished = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_attempt_finish",
            &json!({
                "taskId": task_id,
                "attemptId": "host-attempt-1",
                "expectedVersion": 2,
                "expectedAttemptVersion": 1,
                "status": "succeeded"
            }),
        )
        .unwrap();
        assert_eq!(finished.result["details"]["task"]["status"], "completed");
        let recovered = execute(
            &database,
            &conversation_id,
            &run_id,
            "work_snapshot_get",
            &json!({}),
        )
        .unwrap();
        assert_eq!(
            recovered.result["details"]["validationPolicies"][0]["snapshot"]["id"],
            "standard_v1"
        );
        assert_eq!(
            recovered.result["details"]["taskAttempts"][0]["id"],
            "host-attempt-1"
        );
        assert_eq!(
            recovered.result["details"]["taskAttempts"][0]["status"],
            "succeeded"
        );
        let bypass = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal.id, "expectedVersion": 1 }),
        )
        .expect_err("standard policy requires acceptance");
        assert_eq!(bypass.code, "goal.acceptance_required");
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn high_risk_implementation_run_cannot_resolve_its_own_repair_blocker() {
        let (database, path, conversation_id, run_id) = setup();
        database
            .freeze_run_execution_profile(&run_id, "durable_v2")
            .unwrap();
        database
            .apply_runtime_event(
                &run_id,
                1,
                &json!({ "type": "run.started", "executionProfileId": "durable_v2" }),
            )
            .unwrap();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation_id.clone(),
                title: "high-risk repair gate".to_owned(),
                objective: "require an independent final review".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run_id.clone(),
            })
            .unwrap();
        let created = execute(
            &database,
            &conversation_id,
            &run_id,
            "task_create_many",
            &json!({
                "goalId": goal.id,
                "tasks": [{ "title": "repair safely", "ordinal": 0, "riskLevel": "high" }]
            }),
        )
        .unwrap();
        let task_id = created.result["details"]["tasks"][0]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        execute(
            &database,
            &conversation_id,
            &run_id,
            "task_attempt_start",
            &json!({
                "taskId": task_id,
                "attemptId": "high-host-attempt",
                "expectedVersion": 1
            }),
        )
        .unwrap();
        let finding = database
            .add_review_finding(
                &conversation_id,
                &goal.id,
                Some(&task_id),
                None,
                "high",
                "correctness",
                "independent recheck required",
                "implementation cannot sign off its own repair",
                "open",
                "review-run",
            )
            .unwrap();
        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "review_finding_resolve",
            &json!({ "findingId": finding.id, "status": "resolved" }),
        )
        .expect_err("implementation run must not provide independent sign-off");
        assert_eq!(error.code, "review.not_independent");
        let (_, findings, _) = database.load_a1_snapshot(&conversation_id).unwrap();
        let retained = findings
            .iter()
            .find(|candidate| candidate.id == finding.id)
            .unwrap();
        assert_eq!(retained.status, "open");
        assert!(retained.resolved_by.is_none());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn graph_node_start_exposes_a_stable_retryable_plan_change_error() {
        let error = graph_node_start_repository_error(RepositoryError::ConstraintViolation(
            "read-only Graph node start is paused because a newer PlanRevision is approved"
                .to_owned(),
        ));
        assert_eq!(error.code, "graph.readonly_plan_changed");
        assert_eq!(error.details["retryable"], true);
        assert_eq!(error.details["operation"], "node_start");
    }
}
