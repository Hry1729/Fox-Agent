use crate::database::{
    AddEvidenceInput, CreateTaskInput, Database, EvidenceReferenceKind, EvidenceType,
    EvidenceValidityStatus, GoalStatus, WorkEventRecord, WorkTaskStatus,
};
use crate::work_mode_gate;
use serde_json::{json, Value};
use std::fmt;

pub const WORK_TOOLS: [&str; 11] = [
    "work_snapshot_get",
    "goal_propose",
    "goal_complete",
    "task_create_many",
    "task_update",
    "task_evidence_add",
    "task_evidence_validate",
    "plan_revision_create",
    "review_finding_add",
    "review_finding_resolve",
    "acceptance_submit",
];

#[derive(Debug)]
pub struct WorkToolOutcome {
    pub result: Value,
    pub events: Vec<WorkEventRecord>,
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
    let mut events = Vec::new();
    let details = match tool {
        "work_snapshot_get" => snapshot(database, conversation_id)?,
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
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err(WorkToolError::new(
                    "goal.not_active",
                    "only an active goal can be completed",
                    json!({ "goalId": goal_id, "status": goal.status }),
                ));
            }
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
            let tasks = input
                .get("tasks")
                .and_then(Value::as_array)
                .filter(|tasks| !tasks.is_empty())
                .ok_or_else(|| "tasks must be a non-empty array".to_owned())?
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
                .create_many(tasks)
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
            json!({ "tasks": tasks })
        }
        "task_update" => {
            let task_id = required_string(input, "taskId")?;
            let (task, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before updating tasks".into());
            }
            let status = parse_task_status(
                input
                    .get("status")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "status is required".to_owned())?,
            )?;
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
        "task_evidence_add" => {
            let task_id = required_string(input, "taskId")?;
            let (_, goal_id) = require_task(database, conversation_id, &task_id)?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err("the goal is paused; resume it before adding evidence".into());
            }
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
                    metadata: json!({}),
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
            let tasks = input
                .get("tasks")
                .cloned()
                .filter(Value::is_array)
                .ok_or_else(|| "tasks must be an array".to_owned())?;
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
                let _ = require_task(database, conversation_id, task_id)?;
            }
            let severity = required_string(input, "severity")?;
            if !["critical", "high", "medium", "low", "info"].contains(&severity.as_str()) {
                return Err("unsupported review severity".into());
            }
            let status = optional_string(input, "status")?.unwrap_or_else(|| "open".to_owned());
            if !["open", "resolved", "waived"].contains(&status.as_str()) {
                return Err("unsupported review status".into());
            }
            let (_, goal_tasks, goal_evidence) = database
                .load_work_graph_snapshot(conversation_id)
                .map_err(|error| error.to_string())?;
            let goal_task_ids = goal_tasks
                .iter()
                .filter(|task| task.goal_id == goal_id)
                .map(|task| task.id.as_str())
                .collect::<std::collections::HashSet<_>>();
            if goal_evidence.iter().any(|evidence| {
                goal_task_ids.contains(evidence.task_id.as_str())
                    && evidence.source_run_id.as_deref() == Some(run_id)
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
                optional_string(input, "planRevisionId")?.as_deref(),
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
            let finding = database
                .resolve_review_finding(&finding_id, &status)?
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
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if goal.status != GoalStatus::Active {
                return Err(WorkToolError::new(
                    "acceptance.goal_not_active",
                    "final acceptance requires an active goal",
                    json!({ "goalId": goal_id, "status": goal.status }),
                ));
            }
            let checks = validate_goal_for_completion(database, conversation_id, &goal_id)?;
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
            let goal_findings = findings
                .iter()
                .filter(|finding| finding.goal_id == goal_id)
                .collect::<Vec<_>>();
            if goal_findings.is_empty() {
                return Err(WorkToolError::new(
                    "acceptance.review_required",
                    "an independent review record is required before final acceptance",
                    json!({ "goalId": goal_id }),
                ));
            }
            let blockers = goal_findings.iter().filter(|finding| finding.status == "open" && ["critical", "high", "medium"].contains(&finding.severity.as_str())).map(|finding| json!({ "findingId": finding.id, "severity": finding.severity, "title": finding.title })).collect::<Vec<_>>();
            if !blockers.is_empty() {
                return Err(WorkToolError::new(
                    "acceptance.review_blocked",
                    "open review findings block final acceptance",
                    json!({ "goalId": goal_id, "findings": blockers }),
                ));
            }
            let acceptance = database.create_acceptance(
                conversation_id,
                &goal_id,
                Some(&plan.id),
                "accepted",
                &required_string(input, "summary")?,
                checks,
                &required_string(input, "reviewer")?,
            )?;
            let goal = database
                .goals()
                .complete(&goal_id, required_version(input)?)
                .map_err(|error| error.to_string())?;
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
        _ => return Err(format!("unsupported work tool: {tool}").into()),
    };
    let mut details = details;
    if let Some(object) = details.as_object_mut() {
        object.insert("workEvents".to_owned(), json!(events));
    }
    Ok(WorkToolOutcome {
        result: tool_result(details),
        events,
    })
}

pub fn snapshot(database: &Database, conversation_id: &str) -> Result<Value, String> {
    let (goals, tasks, evidence) = database
        .load_work_graph_snapshot(conversation_id)
        .map_err(|error| error.to_string())?;
    let goal = goals.into_iter().find(|goal| {
        matches!(
            goal.status,
            GoalStatus::Active | GoalStatus::Blocked | GoalStatus::Proposed
        )
    });
    let Some(goal) = goal else {
        return Ok(
            json!({ "goal": null, "tasks": [], "evidence": [], "planRevisions": [], "reviewFindings": [], "acceptances": [] }),
        );
    };
    let tasks = tasks
        .into_iter()
        .filter(|task| task.goal_id == goal.id)
        .collect::<Vec<_>>();
    let task_ids = tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let evidence = evidence
        .into_iter()
        .filter(|item| task_ids.contains(item.task_id.as_str()))
        .collect::<Vec<_>>();
    let (plans, findings, acceptances) = database.load_a1_snapshot(conversation_id)?;
    let plan_revisions = plans
        .into_iter()
        .filter(|item| item.goal_id == goal.id)
        .collect::<Vec<_>>();
    let review_findings = findings
        .into_iter()
        .filter(|item| item.goal_id == goal.id)
        .collect::<Vec<_>>();
    let acceptances = acceptances
        .into_iter()
        .filter(|item| item.goal_id == goal.id)
        .collect::<Vec<_>>();
    Ok(
        json!({ "goal": goal, "tasks": tasks, "evidence": evidence, "planRevisions": plan_revisions, "reviewFindings": review_findings, "acceptances": acceptances }),
    )
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
    Ok(
        json!({ "taskCount": goal_tasks.len(), "completed": goal_tasks.iter().filter(|task| task.status == WorkTaskStatus::Completed).count(), "skipped": goal_tasks.iter().filter(|task| task.status == WorkTaskStatus::Skipped).count(), "validEvidence": evidence.iter().filter(|item| item.validity_status == EvidenceValidityStatus::Valid).count() }),
    )
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
            .create_run(&conversation.id, "创建目标并制定计划，修复跨文件问题", None)
            .expect("create run")
            .run;
        (database, path, conversation.id, run.id)
    }

    fn percentile_95(mut samples: Vec<Duration>) -> Duration {
        samples.sort_unstable();
        samples[(samples.len() * 95).div_ceil(100).saturating_sub(1)]
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
        let completed = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal_id, "expectedVersion": 1 }),
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

        let error = execute(&database, &conversation_id, &run_id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "done", "reviewer": "fox-reviewer"
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
            "status": "resolved", "reviewer": "fox-reviewer"
        })).expect("record independent review");
        let accepted = execute(&database, &conversation_id, &review_run.id, "acceptance_submit", &json!({
            "goalId": goal_id, "expectedVersion": 1, "summary": "plan, review, tasks, and evidence accepted",
            "reviewer": "fox-reviewer"
        })).expect("accept goal");
        assert_eq!(accepted.result["details"]["goal"]["status"], "completed");
        assert_eq!(
            database
                .load_conversation(&conversation_id)
                .unwrap()
                .acceptances
                .len(),
            1
        );
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
        execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_complete",
            &json!({ "goalId": goal_id, "expectedVersion": 1 }),
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
}
