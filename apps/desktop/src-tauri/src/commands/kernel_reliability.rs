//! Desktop commands over the same orchestration as model tools.
use super::*;
use crate::database::{Database, JobSnapshot};
use serde_json::{json, Value};

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReliabilityRequest {
    conversation_id: String,
}

#[tauri::command]
pub fn kernel_reliability_snapshot(
    state: State<'_, AppState>,
    request: ReliabilityRequest,
) -> ApiResponse<Value> {
    let result = (|| -> Result<Value, String> {
        use rusqlite::OptionalExtension;
        let (run,jobs)=state.database.with_connection(|c| {
            let run:Option<String>=c.query_row("SELECT id FROM runs WHERE conversation_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 1",[&request.conversation_id],|r|r.get(0)).optional()?;
            let mut q=c.prepare("SELECT job_id FROM kernel_jobs WHERE conversation_id=?1 ORDER BY created_at DESC LIMIT 20")?;
            let jobs=q.query_map([&request.conversation_id],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            Ok((run,jobs))
        })?;
        let rows = jobs
            .iter()
            .map(|id| state.database.kernel_job_snapshot(id))
            .collect::<Result<Vec<_>, _>>()?;
        let context = run
            .as_ref()
            .map(|id| {
                state
                    .database
                    .kernel_context_budget(&request.conversation_id, id)
            })
            .transpose()?
            .flatten();
        let context_window = run.as_deref().map(|id| state.database.runtime_context_window(id))
            .transpose()?.flatten();
        Ok(json!({"runId":run,"contextBudget":context,"contextWindow":context_window,"jobs":rows}))
    })();
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(e) => ApiResponse::failure("kernel.reliability_unavailable", e, false),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobOperationRequest {
    conversation_id: String,
    run_id: String,
    tool: String,
    input: Value,
}
#[tauri::command]
pub fn kernel_job_execute(
    state: State<'_, AppState>,
    request: JobOperationRequest,
) -> ApiResponse<Value> {
    match state.runtime_host.execute_job_operation(
        &request.conversation_id,
        &request.run_id,
        &request.tool,
        &request.input,
    ) {
        Ok(v) => ApiResponse::success(v),
        Err(e) => ApiResponse::failure("kernel.job_operation_failed", e, false),
    }
}

fn row_response(result: Result<Value, String>) -> ApiResponse<JobSnapshot> {
    let result = result.and_then(|value| {
        serde_json::from_str(
            value["content"][0]["text"]
                .as_str()
                .ok_or("missing job response")?,
        )
        .map_err(|e| e.to_string())
    });
    match result {
        Ok(row) => ApiResponse::success(row),
        Err(e) => ApiResponse::failure("kernel.job_operation_failed", e, false),
    }
}
fn owned_job(state: &AppState, conversation: &str, id: &str) -> Result<JobSnapshot, String> {
    let row = state.database.kernel_job_snapshot(id)?;
    if row.conversation_id != conversation {
        return Err("job is outside this conversation".into());
    }
    Ok(row)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryFile {
    id: String,
    display_name: String,
    created_at: i64,
    updated_at: i64,
}

#[tauri::command]
pub fn conversation_summary_files(
    state: State<'_, AppState>,
    conversation_id: String,
) -> ApiResponse<Vec<SummaryFile>> {
    let result = state.database.with_connection(|connection| {
        let mut statement = connection.prepare(
            "SELECT id, display_name, created_at, updated_at FROM artifacts WHERE conversation_id = ?1",
        )?;
        let rows = statement.query_map([conversation_id], |row| {
            Ok(SummaryFile {
                id: row.get(0)?,
                display_name: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
            })
        })?;
        rows.collect()
    });
    match result {
        Ok(files) => ApiResponse::success(files),
        Err(error) => ApiResponse::failure("conversation.summary_files_unavailable", error, false),
    }
}

const COMMAND_CALL_FILTER: &str = "lower(t.tool_name) IN (
    'run_command','test_run','code_check','format_code','shell','command','bash',
    'shell_command','exec_command','local_shell'
) AND (lower(t.tool_name) <> 'run_command'
    OR COALESCE(json_extract(t.input_json, '$.action'), 'sync') = 'sync'
    OR (json_extract(t.input_json, '$.action') = 'start' AND t.status = 'failed'
        AND json_extract(t.result_json, '$.details.jobId') IS NULL))";

#[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandTaskKind {
    Job,
    Tool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandTask {
    id: String,
    kind: CommandTaskKind,
    run_id: String,
    runtime_call_id: Option<String>,
    command: String,
    title: Option<String>,
    cwd: Option<String>,
    status: String,
    started_at: i64,
    finished_at: Option<i64>,
    stoppable: bool,
    stopping: bool,
    error_message: Option<String>,
}

fn command_task_response<T: serde::Serialize>(result: Result<T, String>) -> ApiResponse<T> {
    match result {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::failure("kernel.command_task_unavailable", error, false),
    }
}

#[tauri::command]
pub fn conversation_command_tasks(
    state: State<'_, AppState>,
    conversation_id: String,
) -> ApiResponse<Vec<CommandTask>> {
    command_task_response(list_command_tasks(&state.database, &conversation_id))
}

fn list_command_tasks(database: &Database, conversation_id: &str) -> Result<Vec<CommandTask>, String> {
    database.with_connection(|connection| {
        let mut statement = connection.prepare(&format!(
            "SELECT job_id, 'job', run_id, NULL, json_extract(params_json, '$.command'),
                    json_extract(params_json, '$.cwd'), state, created_at, finished_at,
                    state IN ('queued','running'), cancel_requested_at IS NOT NULL, error_message, NULL
             FROM kernel_jobs WHERE conversation_id = ?1 AND kind = 'command'
             UNION ALL
             SELECT t.id, 'tool', t.run_id, t.runtime_tool_call_id,
                    CAST(COALESCE(json_extract(t.input_json, '$.command'), json_extract(t.input_json, '$.cmd'),
                                  json_extract(t.input_json, '$.commandLine'), '') AS TEXT),
                    CAST(json_extract(t.input_json, '$.cwd') AS TEXT),
                    CASE WHEN t.status = 'running' AND r.finished_at IS NOT NULL
                         THEN CASE WHEN r.status = 'cancelled' THEN 'cancelled' ELSE 'interrupted' END
                         WHEN t.status = 'failed' AND t.error_message LIKE '[tool.cancelled]%'
                         THEN 'cancelled'
                         ELSE t.status END,
                    t.started_at, COALESCE(t.completed_at, r.finished_at),
                    t.status = 'running' AND r.finished_at IS NULL AND t.execution_location = 'host'
                        AND t.tool_name = 'run_command',
                    0, t.error_message, CAST(json_extract(t.input_json, '$.title') AS TEXT)
             FROM tool_calls t JOIN runs r ON r.id = t.run_id
             WHERE t.conversation_id = ?1 AND ({COMMAND_CALL_FILTER})
               AND t.status IN ('running','completed','failed','cancelled','interrupted')
             ORDER BY 8 DESC, 1 DESC"
        ))?;
        let rows = statement.query_map([conversation_id], |row| {
            Ok(CommandTask {
                id: row.get(0)?,
                kind: if row.get::<_, String>(1)? == "job" {
                    CommandTaskKind::Job
                } else {
                    CommandTaskKind::Tool
                },
                run_id: row.get(2)?,
                runtime_call_id: row.get(3)?,
                command: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                cwd: row.get(5)?,
                status: row.get(6)?,
                started_at: row.get(7)?,
                finished_at: row.get(8)?,
                stoppable: row.get(9)?,
                stopping: row.get(10)?,
                error_message: row.get(11)?,
                title: row.get(12)?,
            })
        })?;
        rows.collect()
    })
}

fn owned_command_job(state: &AppState, conversation_id: &str, task_id: &str) -> Result<JobSnapshot, String> {
    let job = owned_job(state, conversation_id, task_id)?;
    if job.kind != "command" {
        return Err("job is not a command".into());
    }
    Ok(job)
}

#[tauri::command]
pub fn conversation_command_task_output(
    state: State<'_, AppState>,
    conversation_id: String,
    task_id: String,
    kind: CommandTaskKind,
) -> ApiResponse<Value> {
    command_task_response((|| {
        if matches!(kind, CommandTaskKind::Tool) {
            let result: Option<String> = state.database.with_connection(|connection| {
                connection.query_row(
                    &format!("SELECT t.result_json FROM tool_calls t
                              WHERE t.conversation_id = ?1 AND t.id = ?2 AND ({COMMAND_CALL_FILTER})"),
                    [&conversation_id, &task_id],
                    |row| row.get(0),
                )
            })?;
            return result.as_deref().map(serde_json::from_str).transpose()
                .map(|value| value.unwrap_or(Value::Null)).map_err(|error| error.to_string());
        }
        let job = owned_command_job(&state, &conversation_id, &task_id)?;
        let snapshot = crate::runtime_host::command_jobs::command_snapshot(&state.database, &job)?;
        let output = |stream: &str| {
            let value = &snapshot[stream];
            let bytes = crate::runtime_host::command_jobs::command_stream_bytes(value)?;
            let dropped = value["droppedBytes"].as_u64().unwrap_or(0) > 0;
            Ok::<_, String>((crate::tool_host::decode_child_bytes(&bytes).0, dropped))
        };
        let (stdout, stdout_dropped) = output("stdout")?;
        let (stderr, stderr_dropped) = output("stderr")?;
        Ok(json!({"stdout": stdout, "stderr": stderr, "exitCode": snapshot["exitCode"],
                  "truncated": stdout_dropped || stderr_dropped}))
    })())
}

#[tauri::command]
pub fn conversation_command_task_stop(
    state: State<'_, AppState>,
    conversation_id: String,
    task_id: String,
    kind: CommandTaskKind,
) -> ApiResponse<()> {
    command_task_response((|| {
        if matches!(kind, CommandTaskKind::Job) {
            owned_command_job(&state, &conversation_id, &task_id)?;
            state.database.kernel_job_request_cancel(&conversation_id, &task_id)?;
            return Ok(());
        }
        let (run_id, call_id): (String, String) = state.database.with_connection(|connection| {
            connection.query_row(
                "SELECT t.run_id, t.runtime_tool_call_id FROM tool_calls t JOIN runs r ON r.id = t.run_id
                 WHERE t.conversation_id = ?1 AND t.id = ?2 AND r.finished_at IS NULL
                   AND t.status = 'running' AND t.execution_location = 'host' AND t.tool_name = 'run_command'
                   AND COALESCE(json_extract(t.input_json, '$.action'), 'sync') = 'sync'",
                [&conversation_id, &task_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })?;
        state.runtime_host.stop_running_command(&run_id, &call_id)
    })())
}

#[tauri::command]
pub fn kernel_job_start(
    state: State<'_, AppState>,
    request: crate::database::JobStartRequest,
) -> ApiResponse<JobSnapshot> {
    row_response((|| {
        if request.kind != "attachment_compute" {
            return Err("unsupported job kind".into());
        }
        let conversation = state
            .database
            .run_conversation(&request.run_id)?
            .ok_or("unknown Run")?;
        state.runtime_host.execute_job_operation(
            &conversation,
            &request.run_id,
            "compute_job_start",
            &json!({"idempotencyKey":request.idempotency_key,"params":request.params}),
        )
    })())
}
#[tauri::command]
pub fn kernel_job_status(
    state: State<'_, AppState>,
    conversation_id: String,
    job_id: String,
) -> ApiResponse<JobSnapshot> {
    match owned_job(&state, &conversation_id, &job_id) {
        Ok(row) => ApiResponse::success(row),
        Err(e) => ApiResponse::failure("kernel.job_unknown", e, false),
    }
}
#[tauri::command]
pub fn kernel_job_list(
    state: State<'_, AppState>,
    conversation_id: String,
    run_id: String,
) -> ApiResponse<Vec<JobSnapshot>> {
    if state
        .database
        .run_conversation(&run_id)
        .ok()
        .flatten()
        .as_deref()
        != Some(conversation_id.as_str())
    {
        return ApiResponse::failure(
            "kernel.job_scope",
            "Run is outside this conversation",
            false,
        );
    }
    match state.database.kernel_jobs_for_run(&run_id) {
        Ok(rows) => ApiResponse::success(rows),
        Err(e) => ApiResponse::failure("kernel.job_unknown", e, false),
    }
}
#[tauri::command]
pub fn kernel_job_cancel(
    state: State<'_, AppState>,
    conversation_id: String,
    job_id: String,
) -> ApiResponse<JobSnapshot> {
    row_response(
        owned_job(&state, &conversation_id, &job_id).and_then(|row| {
            state.runtime_host.execute_job_operation(
                &conversation_id,
                &row.run_id,
                "compute_job_cancel",
                &json!({"jobId":job_id}),
            )
        }),
    )
}
#[tauri::command]
pub fn kernel_job_resume(
    state: State<'_, AppState>,
    conversation_id: String,
    job_id: String,
) -> ApiResponse<JobSnapshot> {
    row_response(
        owned_job(&state, &conversation_id, &job_id).and_then(|row| {
            state.runtime_host.execute_job_operation(
                &conversation_id,
                &row.run_id,
                "compute_job_start",
                &json!({"jobId":job_id}),
            )
        }),
    )
}
#[tauri::command]
pub fn kernel_job_reconcile_orphans(state: State<'_, AppState>) -> ApiResponse<Vec<JobSnapshot>> {
    match state.database.kernel_jobs_reconcile_orphans() {
        Ok(rows) => ApiResponse::success(rows),
        Err(e) => ApiResponse::failure("kernel.job_reconcile_failed", e, false),
    }
}

#[cfg(test)]
mod command_task_tests {
    use super::*;

    #[test]
    fn command_task_list_counts_durable_jobs_and_real_tool_executions_once() {
        let database = Database::open(":memory:".into()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "commands", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let job = database
            .kernel_job_start(&crate::database::JobStartRequest {
                run_id: run.id.clone(),
                kind: "command".into(),
                idempotency_key: "ui-command-test".into(),
                params: json!({"command":"echo background","cwd":"project"}),
                deadline_ms: None,
                progress_total: None,
            })
            .unwrap();
        let foreground = database
            .create_host_tool_call(
                &run.id, "foreground", "run_command", &json!({"command":"echo foreground"}), "running", false,
            )
            .unwrap();
        database
            .create_host_tool_call(
                &run.id, "launch", "run_command", &json!({"action":"start","command":"echo background"}), "running", false,
            )
            .unwrap();
        let job_id = &job.snapshot().job_id;
        database
            .complete_host_tool_call(&run.id, "launch", Some(&json!({"details":{"jobId":job_id}})), None)
            .unwrap();
        database.create_host_tool_call(
            &run.id, "poll", "run_command", &json!({"action":"status","jobId":job_id}), "running", false,
        ).unwrap();
        database.create_host_tool_call(
            &run.id, "read", "read_file", &json!({"path":"input.md"}), "running", false,
        ).unwrap();

        let tasks = list_command_tasks(&database, &conversation.id).unwrap();
        assert_eq!(tasks.len(), 2);
        let background = tasks.iter().find(|task| matches!(task.kind, CommandTaskKind::Job)).unwrap();
        assert_eq!((&background.id, background.status.as_str()), (job_id, "queued"));
        assert_eq!(background.command, "echo background");
        let front = tasks.iter().find(|task| task.id == foreground.id).unwrap();
        assert!(front.stoppable);
        assert_eq!(front.status, "running");
        assert!(list_command_tasks(&database, "other-conversation").unwrap().is_empty());

        database.kernel_job_request_cancel(&conversation.id, job_id).unwrap();
        let tasks = list_command_tasks(&database, &conversation.id).unwrap();
        let cancelled = tasks.iter().find(|task| &task.id == job_id).unwrap();
        assert_eq!(cancelled.status, "cancelled");
        assert!(!cancelled.stoppable);
        assert!(cancelled.finished_at.is_some());
    }
}
