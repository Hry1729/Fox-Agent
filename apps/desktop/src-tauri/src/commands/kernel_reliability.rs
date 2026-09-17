//! Desktop commands over the same orchestration as model tools.
use super::*;
use crate::database::JobSnapshot;
use serde_json::{json,Value};

#[derive(serde::Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ReliabilityRequest {conversation_id:String}

#[tauri::command]
pub fn kernel_reliability_snapshot(state:State<'_,AppState>,request:ReliabilityRequest)->ApiResponse<Value> {
    let result=(||->Result<Value,String>{
        use rusqlite::OptionalExtension;
        let (run,jobs)=state.database.with_connection(|c| {
            let run:Option<String>=c.query_row("SELECT id FROM runs WHERE conversation_id=?1 ORDER BY created_at DESC,rowid DESC LIMIT 1",[&request.conversation_id],|r|r.get(0)).optional()?;
            let mut q=c.prepare("SELECT job_id FROM kernel_jobs WHERE conversation_id=?1 ORDER BY created_at DESC LIMIT 20")?;
            let jobs=q.query_map([&request.conversation_id],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
            Ok((run,jobs))
        })?;
        let rows=jobs.iter().map(|id|state.database.kernel_job_snapshot(id)).collect::<Result<Vec<_>,_>>()?;
        let context=run.as_ref().map(|id|state.database.kernel_context_budget(&request.conversation_id,id)).transpose()?.flatten();
        let continuable=state.database.kernel_list_continuable_runs(&request.conversation_id)?.into_iter().filter(|r|r.resumed_by_run_id.is_none()).collect::<Vec<_>>();
        Ok(json!({"runId":run,"contextBudget":context,"jobs":rows,"continuable":continuable}))
    })();
    match result {Ok(value)=>ApiResponse::success(value),Err(e)=>ApiResponse::failure("kernel.reliability_unavailable",e,false)}
}

#[derive(serde::Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct JobOperationRequest {conversation_id:String,run_id:String,tool:String,input:Value}
#[tauri::command]
pub fn kernel_job_execute(state:State<'_,AppState>,request:JobOperationRequest)->ApiResponse<Value> {
    match state.runtime_host.execute_job_operation(&request.conversation_id,&request.run_id,&request.tool,&request.input) {
        Ok(v)=>ApiResponse::success(v),Err(e)=>ApiResponse::failure("kernel.job_operation_failed",e,false)
    }
}

fn row_response(result:Result<Value,String>)->ApiResponse<JobSnapshot> {
    let result=result.and_then(|value|serde_json::from_str(value["content"][0]["text"].as_str().ok_or("missing job response")?).map_err(|e|e.to_string()));
    match result {Ok(row)=>ApiResponse::success(row),Err(e)=>ApiResponse::failure("kernel.job_operation_failed",e,false)}
}
fn owned_job(state:&AppState,conversation:&str,id:&str)->Result<JobSnapshot,String> {
    let row=state.database.kernel_job_snapshot(id)?;
    if row.conversation_id!=conversation {return Err("job is outside this conversation".into());}Ok(row)
}
#[tauri::command]
pub fn kernel_job_start(state:State<'_,AppState>,request:crate::database::JobStartRequest)->ApiResponse<JobSnapshot> {
    row_response((||{
        if request.kind!="attachment_compute" {return Err("unsupported job kind".into());}
        let conversation=state.database.run_conversation(&request.run_id)?.ok_or("unknown Run")?;
        state.runtime_host.execute_job_operation(&conversation,&request.run_id,"compute_job_start",&json!({"idempotencyKey":request.idempotency_key,"params":request.params}))
    })())
}
#[tauri::command]
pub fn kernel_job_status(state:State<'_,AppState>,conversation_id:String,job_id:String)->ApiResponse<JobSnapshot> {
    match owned_job(&state,&conversation_id,&job_id) {Ok(row)=>ApiResponse::success(row),Err(e)=>ApiResponse::failure("kernel.job_unknown",e,false)}
}
#[tauri::command]
pub fn kernel_job_list(state:State<'_,AppState>,conversation_id:String,run_id:String)->ApiResponse<Vec<JobSnapshot>> {
    if state.database.run_conversation(&run_id).ok().flatten().as_deref()!=Some(conversation_id.as_str()) {return ApiResponse::failure("kernel.job_scope","Run is outside this conversation",false);}
    match state.database.kernel_jobs_for_run(&run_id) {Ok(rows)=>ApiResponse::success(rows),Err(e)=>ApiResponse::failure("kernel.job_unknown",e,false)}
}
#[tauri::command]
pub fn kernel_job_cancel(state:State<'_,AppState>,conversation_id:String,job_id:String)->ApiResponse<JobSnapshot> {
    row_response(owned_job(&state,&conversation_id,&job_id).and_then(|row|state.runtime_host.execute_job_operation(&conversation_id,&row.run_id,"compute_job_cancel",&json!({"jobId":job_id}))))
}
#[tauri::command]
pub fn kernel_job_resume(state:State<'_,AppState>,conversation_id:String,job_id:String)->ApiResponse<JobSnapshot> {
    row_response(owned_job(&state,&conversation_id,&job_id).and_then(|row|state.runtime_host.execute_job_operation(&conversation_id,&row.run_id,"compute_job_start",&json!({"jobId":job_id}))))
}
#[tauri::command]
pub fn kernel_job_reconcile_orphans(state:State<'_,AppState>)->ApiResponse<Vec<JobSnapshot>> {
    match state.database.kernel_jobs_reconcile_orphans(){Ok(rows)=>ApiResponse::success(rows),Err(e)=>ApiResponse::failure("kernel.job_reconcile_failed",e,false)}
}
