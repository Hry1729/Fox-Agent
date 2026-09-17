//! #6 continuation entry point: list what can be continued and start the next
//! attempt. The source run stays terminal; this creates a new, re-verified run.
use super::*;
use crate::database::{ContinuableRun, ContinuationRequest};

#[tauri::command]
pub fn kernel_run_continuable(
    state: State<'_, AppState>,
    request: ContinuationRequest,
) -> ApiResponse<ContinuableRun> {
    match state
        .database
        .kernel_continuable_run(&request.conversation_id, &request.source_run_id)
    {
        Ok(run) => ApiResponse::success(run),
        Err(error) => ApiResponse::failure("kernel.continuation_unavailable", error, false),
    }
}

#[tauri::command]
pub fn kernel_run_continuable_list(
    state: State<'_, AppState>,
    conversation_id: String,
) -> ApiResponse<Vec<ContinuableRun>> {
    match state
        .database
        .kernel_list_continuable_runs(&conversation_id)
    {
        Ok(runs) => ApiResponse::success(runs),
        Err(error) => ApiResponse::failure("kernel.continuation_unavailable", error, false),
    }
}

#[tauri::command]
pub fn kernel_run_continue(
    state: State<'_, AppState>,
    request: ContinuationRequest,
) -> ApiResponse<StartRunResult> {
    // R2: re-verify, resolve the real model config/input/scope, insert the
    // attempt with those frozen values, then start it through the ordinary
    // Kernel run-start path. The previous version inserted placeholder values and
    // never froze the artifacts, so the run could not start at all.
    match state.runtime_host.continue_kernel_run(&request) {
        Ok(started) => ApiResponse::success(started),
        Err(error) => ApiResponse::failure("kernel.continuation_failed", error, false),
    }
}
