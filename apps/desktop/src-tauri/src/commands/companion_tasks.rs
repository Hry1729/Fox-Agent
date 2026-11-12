use crate::{
    app_state::AppState,
    database::{
        canonical_timezone, resolved_device_timezone, ApiResponse, CreateDigitalColleagueInput,
        DigitalColleagueScheduleSpec, DigitalColleagueTriggerRecord, OwnedCompanionTask,
        OwnedCompanionTaskUpdate,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTaskRequest {
    companion_id: String,
    task_id: Option<String>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTaskCreateRequest {
    companion_id: String,
    name: String,
    objective: String,
    schedule: DigitalColleagueScheduleSpec,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTaskUpdateRequest {
    companion_id: String,
    task_id: String,
    name: Option<String>,
    objective: Option<String>,
    schedule: Option<DigitalColleagueScheduleSpec>,
    enabled: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionTaskRunNow {
    task: OwnedCompanionTask,
    trigger: crate::database::DigitalColleagueTriggerRecord,
    run_id: Option<String>,
}

fn response<T: Serialize>(value: Result<T, String>) -> ApiResponse<T> {
    value.map(ApiResponse::success).unwrap_or_else(|error| {
        let code = error
            .split(|c: char| c.is_whitespace() || c == ':' || c == '：')
            .find(|part| part.starts_with("companion.") || part.starts_with("digital_colleague."))
            .unwrap_or("companion.task_failed")
            .to_owned();
        ApiResponse::failure(&code, error, false)
    })
}

fn required(value: &str, field: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("companion.task_{field}_required"))
    } else {
        Ok(value.to_owned())
    }
}

fn task_id(value: Option<&str>) -> Result<&str, String> {
    value.filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "companion.task_id_required".to_owned())
}

#[tauri::command]
pub(crate) fn companion_task_list(
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<Vec<OwnedCompanionTask>> {
    response(state.database.list_owned_companion_tasks(&request.companion_id))
}

/// Recent run history for an owned task. `limit` bounds only the newest
/// completed-or-skipped tail; still-active triggers are always included, so
/// the returned list may be longer than `limit`.
#[tauri::command]
pub(crate) fn companion_task_history(
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<Vec<DigitalColleagueTriggerRecord>> {
    response(task_id(request.task_id.as_deref()).and_then(|task_id| {
        state.database.list_owned_companion_task_history(
            &request.companion_id,
            task_id,
            request.limit.unwrap_or(20).clamp(1, 100),
        )
    }))
}

#[tauri::command]
pub(crate) fn companion_task_create(
    state: State<'_, AppState>,
    request: CompanionTaskCreateRequest,
) -> ApiResponse<OwnedCompanionTask> {
    response((|| {
        let input = CreateDigitalColleagueInput {
            name: required(&request.name, "name")?,
            objective: required(&request.objective, "objective")?,
            expert_id: String::new(),
            project_id: None,
            project_root: None,
            knowledge_references: Vec::new(),
            max_runs_per_day: 8,
            max_tokens_per_day: 128_000,
            max_duration_ms: 300_000,
            max_output_tokens: 4_096,
            max_tool_calls: 16,
            companion_id: Some(request.companion_id),
        };
        // The browser resolves its device zone on create. Keep the same Host
        // fallback for callers that explicitly persist an empty zone.
        let schedule = match request.schedule {
            DigitalColleagueScheduleSpec::Once { at_ms, timezone } => DigitalColleagueScheduleSpec::Once {
                at_ms,
                timezone: if timezone.trim().is_empty() { resolved_device_timezone()? } else { canonical_timezone(&timezone)? },
            },
            DigitalColleagueScheduleSpec::Interval { interval_seconds, timezone } => DigitalColleagueScheduleSpec::Interval {
                interval_seconds,
                timezone: if timezone.trim().is_empty() { resolved_device_timezone()? } else { canonical_timezone(&timezone)? },
            },
            DigitalColleagueScheduleSpec::Daily { weekdays, local_time, timezone } => DigitalColleagueScheduleSpec::Daily {
                weekdays,
                local_time,
                timezone: if timezone.trim().is_empty() { resolved_device_timezone()? } else { canonical_timezone(&timezone)? },
            },
            DigitalColleagueScheduleSpec::Weekly { weekdays, local_time, timezone } => DigitalColleagueScheduleSpec::Weekly {
                weekdays,
                local_time,
                timezone: if timezone.trim().is_empty() { resolved_device_timezone()? } else { canonical_timezone(&timezone)? },
            },
        };
        state.database.create_owned_companion_task(&input, schedule, request.enabled.unwrap_or(true))
    })())
}

#[tauri::command]
pub(crate) fn companion_task_update(
    state: State<'_, AppState>,
    request: CompanionTaskUpdateRequest,
) -> ApiResponse<OwnedCompanionTask> {
    response(state.database.update_owned_companion_task(
        &request.companion_id,
        &request.task_id,
        OwnedCompanionTaskUpdate {
            name: request.name,
            objective: request.objective,
            knowledge_references: None,
            schedule: request.schedule,
            enabled: request.enabled,
        },
    ))
}

#[tauri::command]
pub(crate) fn companion_task_pause(
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<OwnedCompanionTask> {
    response(task_id(request.task_id.as_deref()).and_then(|task_id| {
        state.database.update_owned_companion_task(
            &request.companion_id,
            task_id,
            OwnedCompanionTaskUpdate { enabled: Some(false), ..Default::default() },
        )
    }))
}

#[tauri::command]
pub(crate) fn companion_task_resume(
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<OwnedCompanionTask> {
    response(task_id(request.task_id.as_deref()).and_then(|task_id| {
        state.database.update_owned_companion_task(
            &request.companion_id,
            task_id,
            OwnedCompanionTaskUpdate { enabled: Some(true), ..Default::default() },
        )
    }))
}

#[tauri::command]
pub(crate) fn companion_task_run_now(
    app: AppHandle,
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<CompanionTaskRunNow> {
    response((|| {
        let task_id = task_id(request.task_id.as_deref())?;
        let before = state.database.owned_companion_task(&request.companion_id, task_id)?
            .ok_or_else(|| "digital_colleague.not_found".to_owned())?;
        let acceptance = state.database.accept_digital_colleague_trigger(
            task_id,
            "manual",
            None,
            &format!("companion-task:{}", Uuid::new_v4()),
            &json!({"objective": before.task.objective, "source": "companion_task_run_now"}),
            None,
            "user",
        )?;
        let trigger = acceptance.trigger.clone();
        if let Some(prepared) = acceptance.prepared {
            app.state::<AppState>().runtime_host.dispatch_digital_colleague_trigger(prepared)?;
        }
        let task = state.database.owned_companion_task(&request.companion_id, task_id)?
            .ok_or_else(|| "digital_colleague.not_found".to_owned())?;
        Ok(CompanionTaskRunNow { task, trigger: trigger.clone(), run_id: trigger.run_id })
    })())
}

#[tauri::command]
pub(crate) fn companion_task_delete(
    state: State<'_, AppState>,
    request: CompanionTaskRequest,
) -> ApiResponse<Value> {
    response(task_id(request.task_id.as_deref()).and_then(|task_id| {
        state.database.owned_companion_task(&request.companion_id, task_id)?
            .ok_or_else(|| "digital_colleague.not_found".to_owned())?;
        serde_json::to_value(state.database.revoke_digital_colleague(task_id, "companion_task_delete")?)
            .map_err(|error| error.to_string())
    }))
}
