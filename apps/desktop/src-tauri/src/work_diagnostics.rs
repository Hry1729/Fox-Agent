use crate::database::{
    Database, EvidenceValidityStatus, GoalRecord, GoalStatus, TaskEvidenceRecord, WorkEventRecord,
    WorkTaskRecord, WorkTaskStatus, WORK_EVENT_TYPES,
};
use crate::maintenance::MaintenanceResult;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

const WORK_TRACE_EXPORT_VERSION: u32 = 1;
const WORK_EVENT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkStateRequest {
    pub conversation_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkStateCounts {
    pub goals: usize,
    pub tasks: usize,
    pub evidence: usize,
    pub work_events: usize,
    pub runs: usize,
    pub runtime_events: usize,
    pub tool_calls: usize,
    pub goal_statuses: BTreeMap<String, usize>,
    pub task_statuses: BTreeMap<String, usize>,
    pub evidence_validity: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkStateFinding {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub goal_id: Option<String>,
    pub task_id: Option<String>,
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkStateDiagnosticReport {
    pub schema_version: i64,
    pub event_schema_version: u32,
    pub conversation_id: String,
    pub checked_at: String,
    pub healthy: bool,
    pub counts: WorkStateCounts,
    pub findings: Vec<WorkStateFinding>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkTraceExport {
    export_version: u32,
    generated_at: String,
    conversation_id: String,
    diagnosis: WorkStateDiagnosticReport,
    goals: Vec<GoalRecord>,
    tasks: Vec<WorkTaskRecord>,
    evidence: Vec<TaskEvidenceRecord>,
    work_events: Vec<WorkEventRecord>,
    runs: Vec<TraceRunSummary>,
    runtime_events: Vec<TraceRuntimeEventSummary>,
    tool_calls: Vec<TraceToolCallSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceRunSummary {
    id: String,
    status: String,
    started_at: Option<i64>,
    finished_at: Option<i64>,
    last_seq: i64,
    trace_id: Option<String>,
    root_span_id: Option<String>,
    has_error: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceRuntimeEventSummary {
    run_id: String,
    seq: i64,
    event_type: String,
    created_at: i64,
    trace_id: Option<String>,
    span_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraceToolCallSummary {
    id: String,
    run_id: String,
    tool_name: String,
    status: String,
    execution_location: String,
    requires_approval: bool,
    started_at: i64,
    completed_at: Option<i64>,
    trace_id: Option<String>,
    span_id: Option<String>,
    has_error: bool,
}

pub fn diagnose_work_state(
    database: &Database,
    conversation_id: &str,
) -> Result<WorkStateDiagnosticReport, String> {
    let conversation_id = validated_conversation_id(conversation_id)?;
    let detail = database.load_conversation(conversation_id)?;
    let events = database.list_work_events(conversation_id)?;
    let (runs, runtime_events, tool_calls) =
        database.load_conversation_trace_records(conversation_id)?;
    let mut findings = Vec::new();

    let goals = detail
        .goals
        .iter()
        .map(|goal| (goal.id.as_str(), goal))
        .collect::<HashMap<_, _>>();
    let tasks = detail
        .tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect::<HashMap<_, _>>();
    let mut tasks_by_goal = HashMap::<&str, Vec<&WorkTaskRecord>>::new();
    for task in &detail.tasks {
        tasks_by_goal.entry(&task.goal_id).or_default().push(task);
    }
    let mut evidence_by_task = HashMap::<&str, Vec<&TaskEvidenceRecord>>::new();
    for evidence in &detail.evidence {
        evidence_by_task
            .entry(&evidence.task_id)
            .or_default()
            .push(evidence);
    }

    let active_goals = detail
        .goals
        .iter()
        .filter(|goal| matches!(goal.status, GoalStatus::Active | GoalStatus::Blocked))
        .count();
    if active_goals > 1 {
        findings.push(finding(
            "error",
            "goal.multiple_active",
            format!("同一会话存在 {active_goals} 个 active/blocked Goal"),
            None,
            None,
            None,
        ));
    }

    for goal in &detail.goals {
        let goal_tasks = tasks_by_goal
            .get(goal.id.as_str())
            .cloned()
            .unwrap_or_default();
        if matches!(goal.status, GoalStatus::Active | GoalStatus::Blocked) && goal_tasks.is_empty()
        {
            findings.push(finding(
                "warning",
                "goal.no_tasks",
                "非终态 Goal 尚未创建 Task".to_owned(),
                Some(&goal.id),
                None,
                None,
            ));
        }
        if goal.status == GoalStatus::Blocked
            && goal
                .blocked_reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            findings.push(finding(
                "error",
                "goal.blocked_without_reason",
                "blocked Goal 缺少 blockedReason".to_owned(),
                Some(&goal.id),
                None,
                None,
            ));
        }
        let in_progress = goal_tasks
            .iter()
            .filter(|task| task.status == WorkTaskStatus::InProgress)
            .count();
        if in_progress > 1 {
            findings.push(finding(
                "error",
                "task.multiple_in_progress",
                format!("Goal 下存在 {in_progress} 个 in_progress Task"),
                Some(&goal.id),
                None,
                None,
            ));
        }
        if goal.status == GoalStatus::Completed
            && goal_tasks.iter().any(|task| {
                !matches!(
                    task.status,
                    WorkTaskStatus::Completed | WorkTaskStatus::Skipped
                )
            })
        {
            findings.push(finding(
                "error",
                "goal.completed_with_open_tasks",
                "completed Goal 仍包含非终态 Task".to_owned(),
                Some(&goal.id),
                None,
                None,
            ));
        }
    }

    for task in &detail.tasks {
        if !goals.contains_key(task.goal_id.as_str()) {
            findings.push(finding(
                "error",
                "task.orphan_goal",
                "Task 引用的 Goal 不存在".to_owned(),
                None,
                Some(&task.id),
                None,
            ));
        }
        if task.status == WorkTaskStatus::Completed
            && evidence_by_task
                .get(task.id.as_str())
                .is_none_or(|items| items.is_empty())
        {
            findings.push(finding(
                "error",
                "task.completed_without_evidence",
                "completed Task 没有 Evidence".to_owned(),
                Some(&task.goal_id),
                Some(&task.id),
                None,
            ));
        }
        if task.status == WorkTaskStatus::Blocked
            && task
                .blocked_reason
                .as_deref()
                .is_none_or(|reason| reason.trim().is_empty())
        {
            findings.push(finding(
                "error",
                "task.blocked_without_reason",
                "blocked Task 缺少 blockedReason".to_owned(),
                Some(&task.goal_id),
                Some(&task.id),
                None,
            ));
        }
        if task.status == WorkTaskStatus::InProgress && task.owner_run_id.is_none() {
            findings.push(finding(
                "warning",
                "task.in_progress_without_owner",
                "in_progress Task 没有 ownerRunId，重启审计可能将其标记为 interrupted".to_owned(),
                Some(&task.goal_id),
                Some(&task.id),
                None,
            ));
        }
    }

    for evidence in &detail.evidence {
        if !tasks.contains_key(evidence.task_id.as_str()) {
            findings.push(finding(
                "error",
                "evidence.orphan_task",
                "Evidence 引用的 Task 不存在".to_owned(),
                None,
                Some(&evidence.task_id),
                Some(&evidence.id),
            ));
        }
        let (severity, code, message) = match evidence.validity_status {
            EvidenceValidityStatus::Unverified => {
                ("warning", "evidence.unverified", "Evidence 尚未校验")
            }
            EvidenceValidityStatus::Stale => {
                ("warning", "evidence.stale", "Evidence 已过期，需要重新验证")
            }
            EvidenceValidityStatus::Missing => {
                ("warning", "evidence.missing", "Evidence 引用目标缺失")
            }
            EvidenceValidityStatus::Invalid => (
                "warning",
                "evidence.invalid",
                "Evidence 已不能证明 Task 结论",
            ),
            EvidenceValidityStatus::Valid => continue,
        };
        findings.push(finding(
            severity,
            code,
            message.to_owned(),
            tasks
                .get(evidence.task_id.as_str())
                .map(|task| task.goal_id.as_str()),
            Some(&evidence.task_id),
            Some(&evidence.id),
        ));
    }

    let goal_ids = goals.keys().copied().collect::<HashSet<_>>();
    let task_ids = tasks.keys().copied().collect::<HashSet<_>>();
    let mut previous_sequence = None;
    let mut missing_trace_count = 0usize;
    for event in &events {
        if event.schema_version != WORK_EVENT_SCHEMA_VERSION {
            findings.push(finding(
                "error",
                "event.unsupported_schema",
                format!("Work Event schemaVersion={} 不受支持", event.schema_version),
                event.goal_id.as_deref(),
                event.task_id.as_deref(),
                None,
            ));
        }
        if !WORK_EVENT_TYPES.contains(&event.event_type.as_str()) {
            findings.push(finding(
                "error",
                "event.unknown_type",
                format!("未知 Work Event 类型：{}", event.event_type),
                event.goal_id.as_deref(),
                event.task_id.as_deref(),
                None,
            ));
        }
        if let Some(previous) = previous_sequence {
            if event.sequence <= previous {
                findings.push(finding(
                    "error",
                    "event.non_monotonic_sequence",
                    "Work Event sequence 未严格递增".to_owned(),
                    event.goal_id.as_deref(),
                    event.task_id.as_deref(),
                    None,
                ));
            } else if event.sequence > previous + 1 {
                findings.push(finding(
                    "warning",
                    "event.sequence_gap",
                    format!(
                        "Work Event sequence 在 {previous} 与 {} 之间存在缺口",
                        event.sequence
                    ),
                    event.goal_id.as_deref(),
                    event.task_id.as_deref(),
                    None,
                ));
            }
        }
        previous_sequence = Some(event.sequence);
        if event
            .goal_id
            .as_deref()
            .is_some_and(|goal_id| !goal_ids.contains(goal_id))
        {
            findings.push(finding(
                "error",
                "event.orphan_goal",
                "Work Event 引用的 Goal 不存在".to_owned(),
                event.goal_id.as_deref(),
                event.task_id.as_deref(),
                None,
            ));
        }
        if event
            .task_id
            .as_deref()
            .is_some_and(|task_id| !task_ids.contains(task_id))
        {
            findings.push(finding(
                "error",
                "event.orphan_task",
                "Work Event 引用的 Task 不存在".to_owned(),
                event.goal_id.as_deref(),
                event.task_id.as_deref(),
                None,
            ));
        }
        if event.run_id.is_some() && event.trace_id.is_none() {
            missing_trace_count += 1;
        }
    }
    if missing_trace_count > 0 {
        findings.push(finding(
            "warning",
            "trace.partial_coverage",
            format!("{missing_trace_count} 个带 runId 的 Work Event 没有 traceId；A0 允许旧 Runtime 留空"),
            None,
            None,
            None,
        ));
    }

    let counts = WorkStateCounts {
        goals: detail.goals.len(),
        tasks: detail.tasks.len(),
        evidence: detail.evidence.len(),
        work_events: events.len(),
        runs: runs.len(),
        runtime_events: runtime_events.len(),
        tool_calls: tool_calls.len(),
        goal_statuses: count_goals(&detail.goals),
        task_statuses: count_tasks(&detail.tasks),
        evidence_validity: count_evidence(&detail.evidence),
    };
    Ok(WorkStateDiagnosticReport {
        schema_version: database.schema_version()?,
        event_schema_version: WORK_EVENT_SCHEMA_VERSION,
        conversation_id: conversation_id.to_owned(),
        checked_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        healthy: !findings.iter().any(|item| item.severity == "error"),
        counts,
        findings,
    })
}

pub fn export_work_trace(
    database: &Database,
    data_dir: &Path,
    conversation_id: &str,
) -> Result<MaintenanceResult, String> {
    let conversation_id = validated_conversation_id(conversation_id)?;
    let detail = database.load_conversation(conversation_id)?;
    let diagnosis = diagnose_work_state(database, conversation_id)?;
    let (runs, runtime_events, tool_calls) =
        database.load_conversation_trace_records(conversation_id)?;
    let generated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let report = WorkTraceExport {
        export_version: WORK_TRACE_EXPORT_VERSION,
        generated_at,
        conversation_id: conversation_id.to_owned(),
        diagnosis,
        goals: detail.goals,
        tasks: detail.tasks,
        evidence: detail.evidence,
        work_events: database.list_work_events(conversation_id)?,
        runs: runs
            .into_iter()
            .map(|run| TraceRunSummary {
                id: run.id,
                status: run.status,
                started_at: run.started_at,
                finished_at: run.finished_at,
                last_seq: run.last_seq,
                trace_id: run.trace_id,
                root_span_id: run.root_span_id,
                has_error: run.error_code.is_some() || run.error_message.is_some(),
            })
            .collect(),
        runtime_events: runtime_events
            .into_iter()
            .map(|event| TraceRuntimeEventSummary {
                run_id: event.run_id,
                seq: event.seq,
                event_type: event.event_type,
                created_at: event.created_at,
                trace_id: event.trace_id,
                span_id: event.span_id,
            })
            .collect(),
        tool_calls: tool_calls
            .into_iter()
            .map(|tool| TraceToolCallSummary {
                id: tool.id,
                run_id: tool.run_id,
                tool_name: tool.tool_name,
                status: tool.status,
                execution_location: tool.execution_location,
                requires_approval: tool.requires_approval,
                started_at: tool.started_at,
                completed_at: tool.completed_at,
                trace_id: tool.trace_id,
                span_id: tool.span_id,
                has_error: tool.error_message.is_some(),
            })
            .collect(),
    };
    let bytes = serde_json::to_vec_pretty(&report).map_err(|error| error.to_string())?;
    let directory = data_dir.join("exports");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let safe_id = conversation_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(64)
        .collect::<String>();
    let safe_id = if safe_id.is_empty() {
        "conversation"
    } else {
        safe_id.as_str()
    };
    let path = directory.join(format!(
        "fox-work-trace-{safe_id}-{}.json",
        crate::database::now_ms()
    ));
    fs::write(&path, &bytes).map_err(|error| error.to_string())?;
    Ok(MaintenanceResult {
        path: Some(path.to_string_lossy().into_owned()),
        message: "A0 工作状态与事件 Trace 已导出；不包含对话正文、项目文件或凭证".to_owned(),
        files: 1,
        bytes: bytes.len() as u64,
        restart_required: false,
    })
}

fn validated_conversation_id(conversation_id: &str) -> Result<&str, String> {
    let conversation_id = conversation_id.trim();
    if conversation_id.is_empty() {
        return Err("conversationId is required".to_owned());
    }
    Ok(conversation_id)
}

fn finding(
    severity: &str,
    code: &str,
    message: String,
    goal_id: Option<&str>,
    task_id: Option<&str>,
    evidence_id: Option<&str>,
) -> WorkStateFinding {
    WorkStateFinding {
        severity: severity.to_owned(),
        code: code.to_owned(),
        message,
        goal_id: goal_id.map(str::to_owned),
        task_id: task_id.map(str::to_owned),
        evidence_id: evidence_id.map(str::to_owned),
    }
}

fn increment(counts: &mut BTreeMap<String, usize>, key: &str) {
    *counts.entry(key.to_owned()).or_default() += 1;
}

fn count_goals(goals: &[GoalRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for goal in goals {
        increment(
            &mut counts,
            match goal.status {
                GoalStatus::Proposed => "proposed",
                GoalStatus::Active => "active",
                GoalStatus::Blocked => "blocked",
                GoalStatus::Completed => "completed",
                GoalStatus::Cancelled => "cancelled",
            },
        );
    }
    counts
}

fn count_tasks(tasks: &[WorkTaskRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for task in tasks {
        increment(
            &mut counts,
            match task.status {
                WorkTaskStatus::Queued => "queued",
                WorkTaskStatus::InProgress => "in_progress",
                WorkTaskStatus::Completed => "completed",
                WorkTaskStatus::Blocked => "blocked",
                WorkTaskStatus::Interrupted => "interrupted",
                WorkTaskStatus::Skipped => "skipped",
            },
        );
    }
    counts
}

fn count_evidence(evidence: &[TaskEvidenceRecord]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for item in evidence {
        increment(
            &mut counts,
            match item.validity_status {
                EvidenceValidityStatus::Unverified => "unverified",
                EvidenceValidityStatus::Valid => "valid",
                EvidenceValidityStatus::Stale => "stale",
                EvidenceValidityStatus::Missing => "missing",
                EvidenceValidityStatus::Invalid => "invalid",
            },
        );
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{CreateGoalInput, CreateTaskInput};
    use crate::runtime_host::WORK_TOOLS;
    use uuid::Uuid;

    #[test]
    fn a0_reference_docs_match_code_contracts() {
        let docs = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs");
        let database_reference = fs::read_to_string(docs.join("04-技术参考/数据库结构.md"))
            .expect("read database reference");
        let runtime_reference = fs::read_to_string(docs.join("04-技术参考/Runtime协议.md"))
            .expect("read runtime reference");

        for tool in WORK_TOOLS {
            assert!(
                runtime_reference.contains(&format!("`{tool}`")),
                "Runtime reference is missing work tool {tool}"
            );
        }
        for event in WORK_EVENT_TYPES {
            assert!(
                runtime_reference.contains(&format!("`{event}`")),
                "Runtime reference is missing work event {event}"
            );
        }
        for contract in [
            "`proposed/active/blocked/completed/cancelled`",
            "`queued/in_progress/completed/blocked/interrupted/skipped`",
            "`tool_call/trace_span/test_result/file_diff/artifact/user_confirmation/external_reference`",
            "`tool_call/artifact/run_event/message/source`",
            "`unverified/valid/stale/missing/invalid`",
        ] {
            assert!(
                database_reference.contains(contract),
                "Database reference is missing enum contract {contract}"
            );
        }
    }

    #[test]
    fn diagnoses_and_exports_a0_work_state() {
        let root = std::env::temp_dir().join(format!("fox-work-diagnostics-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).expect("create diagnostics root");
        let database_path = root.join("fox.db");
        let database = Database::open(database_path).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("diagnostics"), None, None)
            .expect("create conversation");
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "诊断 A0".to_owned(),
                objective: "确认工作状态与事件可导出".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: "test".to_owned(),
            })
            .expect("create goal");
        let run = database
            .create_run(&conversation.id, "diagnose work state", None)
            .expect("create run")
            .run;
        database
            .work_tasks()
            .create_many(vec![CreateTaskInput {
                id: None,
                goal_id: goal.id.clone(),
                parent_task_id: None,
                ordinal: 0,
                title: "检查状态".to_owned(),
                detail: None,
            }])
            .expect("create task");
        database
            .append_work_event(
                "goal.activated",
                &conversation.id,
                Some(&goal.id),
                None,
                Some(&run.id),
                serde_json::json!({ "goal": goal }),
            )
            .expect("append event");

        let diagnosis = diagnose_work_state(&database, &conversation.id).expect("diagnose");
        assert!(diagnosis.healthy);
        assert_eq!(diagnosis.schema_version, 15);
        assert_eq!(diagnosis.counts.goals, 1);
        assert_eq!(diagnosis.counts.tasks, 1);
        assert_eq!(diagnosis.counts.work_events, 1);
        assert_eq!(diagnosis.counts.runs, 1);

        let exported = export_work_trace(&database, &root, &conversation.id).expect("export");
        let path = exported.path.expect("export path");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read exported trace"))
                .expect("parse exported trace");
        assert_eq!(value["exportVersion"], 1);
        assert_eq!(value["diagnosis"]["healthy"], true);
        assert_eq!(value["workEvents"].as_array().unwrap().len(), 1);
        assert_eq!(value["runs"].as_array().unwrap().len(), 1);
        assert!(value["runs"][0].get("model").is_none());
        assert!(value["runtimeEvents"]
            .as_array()
            .unwrap()
            .iter()
            .all(|event| event.get("event").is_none()));

        drop(database);
        fs::remove_dir_all(root).expect("remove diagnostics root");
    }
}
