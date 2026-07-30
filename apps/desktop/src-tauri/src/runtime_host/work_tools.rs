use crate::database::{
    AddEvidenceInput, CreateTaskInput, Database, EvidenceReferenceKind, EvidenceType, GoalStatus,
    WorkEventRecord, WorkTaskStatus,
};
use crate::work_mode_gate;
use serde_json::{json, Value};

pub const WORK_TOOLS: [&str; 7] = [
    "work_snapshot_get",
    "goal_propose",
    "goal_activate",
    "task_create_many",
    "task_update",
    "task_evidence_add",
    "task_evidence_validate",
];

#[derive(Debug)]
pub struct WorkToolOutcome {
    pub result: Value,
    pub events: Vec<WorkEventRecord>,
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
) -> Result<WorkToolOutcome, String> {
    let mut events = Vec::new();
    let details = match tool {
        "work_snapshot_get" => {
            let requested_conversation = required_string(input, "conversationId")?;
            if requested_conversation != conversation_id {
                return Err(
                    "work snapshot conversation does not match the current conversation".to_owned(),
                );
            }
            snapshot(database, conversation_id)?
        }
        "goal_propose" => {
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
        "goal_activate" => {
            let _ = required_string(input, "goalId")?;
            let _ = required_version(input)?;
            return Err(
                "goal activation is controlled by the Host; resolve the work mode confirmation instead"
                    .to_owned(),
            );
        }
        "task_create_many" => {
            let goal_id = required_string(input, "goalId")?;
            let goal = require_goal(database, conversation_id, &goal_id)?;
            if matches!(goal.status, GoalStatus::Completed | GoalStatus::Cancelled) {
                return Err("tasks cannot be added to a terminal goal".to_owned());
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
        _ => return Err(format!("unsupported work tool: {tool}")),
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

fn snapshot(database: &Database, conversation_id: &str) -> Result<Value, String> {
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
        return Ok(json!({ "goal": null, "tasks": [], "evidence": [] }));
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
    Ok(json!({ "goal": goal, "tasks": tasks, "evidence": evidence }))
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
    use std::time::{Duration, Instant};
    use uuid::Uuid;

    fn setup() -> (Database, std::path::PathBuf, String, String) {
        let path = std::env::temp_dir().join(format!("fox-work-tools-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("work tools"), None, None)
            .expect("create conversation");
        let run = database
            .create_run(&conversation.id, "exercise work tools", None)
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
        let activation_error = execute(
            &database,
            &conversation_id,
            &run_id,
            "goal_activate",
            &json!({ "goalId": goal_id, "expectedVersion": 1 }),
        )
        .expect_err("runtime must not activate a goal directly");
        assert!(activation_error.contains("controlled by the Host"));
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
        let snapshot = execute(
            &database,
            &conversation_id,
            &run_id,
            "work_snapshot_get",
            &json!({ "conversationId": conversation_id }),
        )
        .expect("snapshot");
        assert_eq!(
            snapshot.result["details"]["tasks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            snapshot.result["details"]["evidence"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(database.list_work_events(&conversation_id).unwrap().len() >= 6);
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
        database
            .goals()
            .complete(&goal_id, 1)
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
    fn rejects_cross_conversation_access() {
        let (database, path, conversation_id, run_id) = setup();
        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .expect("create second conversation");
        let error = execute(
            &database,
            &conversation_id,
            &run_id,
            "work_snapshot_get",
            &json!({ "conversationId": other.id }),
        )
        .expect_err("cross-conversation snapshot must fail");
        assert!(error.contains("current conversation"));
        drop(database);
        let _ = std::fs::remove_file(path);
    }
}
