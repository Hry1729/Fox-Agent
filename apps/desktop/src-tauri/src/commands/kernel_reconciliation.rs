use super::*;
use crate::database::{ReconciliationRequest, ReconciliationView};

#[tauri::command]
pub fn kernel_reconciliation_load(
    state: State<'_, AppState>,
    request: ReconciliationRequest,
) -> ApiResponse<ReconciliationView> {
    match state
        .database
        .kernel_reconciliation_view(&request.conversation_id, &request.run_id)
    {
        Ok(view) => ApiResponse::success(view),
        Err(error) => ApiResponse::failure("kernel.reconciliation_unavailable", error, false),
    }
}
#[tauri::command]
pub async fn kernel_reconciliation_options(
    state: State<'_, AppState>,
    request: ReconciliationRequest,
) -> Result<ApiResponse<serde_json::Value>, String> {
    let db = state.database.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || {
            crate::kernel_reconciliation::options(&db, &request)
        })
        .await
        {
            Ok(Ok(value)) => ApiResponse::success(value),
            _ => ApiResponse::failure(
                "kernel.query_unavailable",
                "原连接器不可用、定义变化或未提供明确的只读查询工具。请在外部系统核对后填写依据。",
                false,
            ),
        },
    )
}
#[tauri::command]
pub async fn kernel_reconciliation_query(
    state: State<'_, AppState>,
    request: ReconciliationRequest,
) -> Result<ApiResponse<ReconciliationView>, String> {
    let db = state.database.clone();
    Ok(
        match tauri::async_runtime::spawn_blocking(move || {
            crate::kernel_reconciliation::query(&db, &request)
        })
        .await
        {
            Ok(Ok(value)) => ApiResponse::success(value),
            _ => ApiResponse::failure(
                "kernel.query_failed",
                "查询未完成或核对记录已变化。请刷新；未确认原操作结果。",
                false,
            ),
        },
    )
}
#[tauri::command]
pub fn kernel_reconciliation_confirm(
    state: State<'_, AppState>,
    request: ReconciliationRequest,
) -> ApiResponse<ReconciliationView> {
    match state.database.kernel_confirm_reconciliation(&request) {
        Ok(value) => ApiResponse::success(value),
        Err(error) => ApiResponse::failure("kernel.confirmation_failed", error, false),
    }
}
#[tauri::command]
pub fn kernel_reconciliation_resume(
    app: AppHandle,
    state: State<'_, AppState>,
    request: ReconciliationRequest,
) -> ApiResponse<StartRunResult> {
    let started = match state.database.kernel_create_recovery_run(&request) {
        Ok(value) => value,
        Err(error) => return ApiResponse::failure("kernel.recovery_failed", error, false),
    };
    let prompt = started.user_message.content.clone();
    // Host startup honors the new, transactionally frozen permission mode and
    // engine. Reapproval mode carries no old grants; the source Run stays terminal.
    dispatch_started_run(&app, &state, started, prompt, true)
}
