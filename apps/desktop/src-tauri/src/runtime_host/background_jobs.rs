//! Production orchestration shared by Kernel tools, Legacy tools and desktop commands.
use super::attachment_compute::host_lifecycle::{self, HostJobLaunch, JobTokenRegistry};
use crate::{database::{Database, JobStartRequest, JobState}, kernel::CancellationToken};
use serde_json::{json, Value};
use std::{path::Path, sync::{Arc, OnceLock}, time::{Duration, Instant}};

pub(super) const TOOLS: &[&str] = &["compute_job_start", "compute_job_status", "compute_job_cancel", "compute_job_result"];
fn registry() -> &'static JobTokenRegistry { static REGISTRY: OnceLock<JobTokenRegistry> = OnceLock::new(); REGISTRY.get_or_init(Default::default) }

/// Parent terminalization requests cancellation for every unfinished compute
/// Job owned by that exact Run. A running executor still owns its terminal
/// write; this request never presents it as already stopped.
pub(super) fn cancel_unfinished_for_terminal(database: &Database, run_id: &str,
    conversation_id: &str) -> Result<(), String> {
    for job in database.kernel_jobs_for_run(run_id)? {
        if job.kind != "attachment_compute" || job.state.is_terminal() { continue; }
        if job.run_id != run_id || job.conversation_id != conversation_id {
            return Err("terminal Job reconciliation crossed the frozen Run".into());
        }
        host_lifecycle::cancel_host_job(database, registry(), &job.job_id)?;
    }
    Ok(())
}

fn allowed_fields(input: &Value, fields: &[&str]) -> Result<(), String> {
    let object=input.as_object().ok_or("job arguments must be an object")?;
    if object.keys().any(|k| !fields.contains(&k.as_str())) { return Err("unknown job argument; authorization fields cannot be supplied by the model".into()); }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(database: &Database, attachments: &Path, sessions: &Path,
    run_id: &str, tool: &str, input: &Value, parent: Option<CancellationToken>, budget: Duration,
    on_terminal: Option<Arc<dyn Fn(&str) + Send + Sync>>) -> Result<Value,String> {
    if !TOOLS.contains(&tool) { return Err("unsupported job operation".into()); }
    let conversation=super::run_bound_conversation(database,run_id,None)?;
    let result=match tool {
        "compute_job_start" => {
            allowed_fields(input,&["idempotencyKey","params","jobId"])?;
            // A queued row is not permission: reacquire the frozen Run on every launch.
            let binding=database.run_control_binding(run_id)?.ok_or("job needs a frozen Run")?;
            if binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative {
                database.kernel_validate_resource_acquisition(run_id)?;
                if !database.kernel_host_scope(run_id)?.tool_names.contains(tool) { return Err("job tool is outside frozen scope".into()); }
            }
            if budget.is_zero() { return Err("job execution budget exhausted".into()); }
            let (snapshot,params)=if let Some(id)=input["jobId"].as_str() {
                if input.get("params").is_some()||input.get("idempotencyKey").is_some() {return Err("resume accepts jobId only".into());}
                let row=database.kernel_job_snapshot(id)?;
                if row.conversation_id!=conversation||row.run_id!=run_id {return Err("job resume is outside this Run".into());}
                (database.kernel_job_resume(id)?,database.kernel_job_parameters(&conversation,id)?)
            } else {
                let key=input["idempotencyKey"].as_str().filter(|s| !s.trim().is_empty()).ok_or("idempotencyKey is required")?;
                let params=input["params"].clone();
                allowed_fields(&params,&["attachmentIds","artifactIds","projectPaths","code","processing","profile"])?;
                if params.get("processing").is_some_and(|v|v!="chunked") {return Err("background jobs require processing=chunked and onChunk/onFinish; use attachment_compute for whole mode".into());}
                if params["code"].as_str().is_none_or(|v|v.is_empty()||v.len()>131072) {return Err("code must be nonempty JavaScript within 128 KiB".into());}
                let row=database.kernel_job_start(&JobStartRequest {run_id:run_id.into(),kind:"attachment_compute".into(),idempotency_key:key.into(),params:params.clone(),deadline_ms:Some(crate::database::now_ms()+budget.as_millis().min(i64::MAX as u128) as i64),progress_total:None})?;
                (row.snapshot().clone(),params)
            };
            if matches!(snapshot.state,JobState::Queued|JobState::Paused)&&!registry().is_running_locally(&snapshot.job_id) {
                let launched=host_lifecycle::start_host_job(HostJobLaunch {database,attachments_dir:attachments,sessions_dir:sessions,registry:registry(),snapshot:snapshot.clone(),params,deadline:Some(Instant::now()+budget),default_budget:budget,cancellation:None,parent,on_terminal});
                if let Err(error)=launched { if !registry().is_running_locally(&snapshot.job_id) {return Err(error);} }
            }
            serde_json::to_value(database.kernel_job_snapshot(&snapshot.job_id)?).map_err(|e|e.to_string())?
        },
        _ => {
            allowed_fields(input,if tool=="compute_job_result" {&["jobId","offset","limit"]} else if tool=="compute_job_status" {&["jobId","waitMs"]} else {&["jobId"]})?;
            let id=input["jobId"].as_str().ok_or("jobId is required")?;
            let scoped_to_run = match database.run_control_binding(run_id)? {
                Some(binding) if binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative =>
                    database.compute_job_notice_enabled(run_id)?,
                _ => false,
            };
            let row=database.kernel_job_snapshot(id)?;
            if row.conversation_id!=conversation {return Err("job is outside the authorized conversation".into());}
            if scoped_to_run && row.run_id!=run_id {return Err("job is outside this Run".into());}
            match tool {
                "compute_job_cancel" => serde_json::to_value(host_lifecycle::cancel_host_job(database,registry(),id)?).map_err(|e|e.to_string())?,
                "compute_job_result" => {
                    let text=database.kernel_job_result_value(&conversation,id)?.to_string();
                    let offset=input.get("offset").map(|v|v.as_u64().ok_or("invalid offset")).transpose()?.unwrap_or(0) as usize;
                    let limit=input.get("limit").map(|v|v.as_u64().ok_or("invalid limit")).transpose()?.unwrap_or(16384) as usize;
                    if !(4..=65536).contains(&limit)||offset>text.len()||!text.is_char_boundary(offset) {return Err("invalid UTF-8 result range; limit must be 4..65536".into());}
                    let mut end=offset.saturating_add(limit).min(text.len());while !text.is_char_boundary(end) {end-=1;}
                    json!({"jobId":id,"reference":row.result_ref,"content":&text[offset..end],"offset":offset,"nextOffset":if end<text.len(){Some(end)}else{None},"complete":end==text.len(),"originalBytes":text.len(),"sha256":row.result_sha256})
                },
                _ => {
                    let wait=input.get("waitMs").map(|v|v.as_u64().ok_or("invalid waitMs")).transpose()?.unwrap_or(0).min(5000);
                    let until=Instant::now()+Duration::from_millis(wait);
                    let mut current=row;
                    while !current.state.is_terminal()&&Instant::now()<until {
                        if let Some(ref token)=parent {token.check()?;}
                        std::thread::sleep(Duration::from_millis(50));current=database.kernel_job_snapshot(id)?;
                    }
                    serde_json::to_value(current).map_err(|e|e.to_string())?
                }
            }
        }
    };
    Ok(json!({"content":[{"type":"text","text":result.to_string()}],"details":{}}))
}

impl Database {
    pub(crate) fn kernel_job_parameters(&self, conversation:&str,id:&str)->Result<Value,String> {
        self.with_connection(|c| {
            let text:String=c.query_row("SELECT params_json FROM kernel_jobs WHERE job_id=?1 AND conversation_id=?2",rusqlite::params![id,conversation],|r|r.get(0))?;
            serde_json::from_str(&text).map_err(|_|rusqlite::Error::InvalidQuery)
        })
    }
}
