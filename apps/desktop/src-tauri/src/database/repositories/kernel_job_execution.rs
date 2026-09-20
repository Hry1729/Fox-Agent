//! Attempt-fenced worker writes and atomic publication in the existing blob store.
use super::{Database, now_ms};
use super::kernel_jobs::{JobSnapshot, JobState, read_snapshot};
use rusqlite::{params, Transaction, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn invalid(message: &str) -> rusqlite::Error { rusqlite::Error::InvalidParameterName(message.into()) }
fn owner_started() -> i64 { crate::runtime_host::process_start_marker().unwrap_or(0) }
fn owned(tx: &Transaction<'_>, id: &str, attempt: u32) -> rusqlite::Result<JobSnapshot> {
    let valid: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_jobs
        WHERE job_id=?1 AND attempts=?2 AND owner_pid=?3 AND owner_started_at=?4 AND state='running')",
        params![id, attempt, std::process::id(), owner_started()], |r| r.get(0))?;
    if !valid { return Err(invalid("job.attempt_conflict")); }
    read_snapshot(tx, id)
}

impl Database {
    pub(crate) fn kernel_command_job_checkpoint(&self,id:&str,attempt:u32,value:&Value)->Result<(),String> {
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if owned(&tx,id,attempt)?.kind!="command" {return Err(invalid("not a command job"));}
            tx.execute("UPDATE kernel_jobs SET cursor=?2,updated_at=?3 WHERE job_id=?1",params![id,value.to_string(),now_ms()])?;
            tx.commit()
        })
    }
    /// Publish success AND failure diagnostics atomically; a failure never becomes completed.
    pub(crate) fn kernel_command_job_publish(&self,id:&str,attempt:u32,value:&Value)->Result<(),String> {
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let job=owned(&tx,id,attempt)?;
            if job.kind!="command" {return Err(invalid("not a command job"));}
            let mut body_value=value.clone();
            let mut state=value["state"].as_str().ok_or_else(||invalid("missing command terminal state"))?;
            if !["completed","failed","cancelled"].contains(&state) {return Err(invalid("command is not terminal"));}
            if job.cancel_requested_at.is_some() && state=="completed" {
                state="cancelled";body_value["state"]=Value::from(state);body_value["errorCode"]=Value::from("tool.cancelled");
            }
            let body=body_value.to_string();let sha=format!("sha256:{}",hex::encode(Sha256::digest(body.as_bytes())));
            tx.execute("INSERT OR IGNORE INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at) VALUES(?1,?2,?3,?4)",params![sha,body,body.len() as i64,now_ms()])?;
            let stored:String=tx.query_row("SELECT body_json FROM tool_call_result_blobs WHERE sha256=?1",[&sha],|r|r.get(0))?;
            if stored!=body {return Err(invalid("command result integrity mismatch"));}
            tx.execute("UPDATE kernel_jobs SET state=?2,result_ref=?3,result_bytes=?4,result_sha256=?5,
                error_code=?6,error_message=?7,updated_at=?8,finished_at=?8,owner_pid=NULL,owner_started_at=NULL,cursor=NULL,
                cancel_acknowledged_at=CASE WHEN ?2='cancelled' THEN ?8 ELSE cancel_acknowledged_at END WHERE job_id=?1",
                params![id,state,format!("fox-job-result://{id}"),body.len() as i64,sha,body_value["errorCode"].as_str(),body_value["errorMessage"].as_str(),now_ms()])?;
            tx.commit()
        })
    }
    pub(crate) fn kernel_job_parent_stopped(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|conn| conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM kernel_jobs j JOIN runs r ON r.id=j.run_id
             WHERE j.job_id=?1 AND r.status IN ('completed','failed','cancelled','interrupted'))",
            [id], |row| row.get(0)))
    }
    pub(crate) fn kernel_job_claim_attempt(&self, id: &str, attempt: u32) -> Result<JobSnapshot, String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let n = tx.execute("UPDATE kernel_jobs SET state='running', attempts=?2, owner_pid=?3,
                owner_started_at=?4, updated_at=?5, cancel_requested_at=NULL, cancel_acknowledged_at=NULL,
                cancelled_by=NULL, uncertain_at=NULL
                WHERE job_id=?1 AND attempts=?6 AND state IN ('queued','paused')",
                params![id, attempt, std::process::id(), owner_started(), now_ms(), i64::from(attempt)-1])?;
            if n != 1 { return Err(invalid("job.attempt_conflict")); }
            let row = read_snapshot(&tx,id)?; tx.commit()?; Ok(row)
        })
    }
    pub(crate) fn kernel_job_progress_attempt(&self, id: &str, attempt: u32, done: u64, total: Option<u64>) -> Result<(), String> {
        self.with_connection(|conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            owned(&tx,id,attempt)?;
            tx.execute("UPDATE kernel_jobs SET progress_done=MAX(progress_done,?2),
                progress_total=COALESCE(?3,progress_total),updated_at=?4 WHERE job_id=?1 AND attempts=?5",
                params![id, done as i64, total.map(|n|n as i64), now_ms(), attempt])?;
            tx.commit()
        })
    }
    pub(crate) fn kernel_job_settle_attempt(&self, id: &str, attempt: u32, state: JobState, error: Option<(&str,&str)>) -> Result<(), String> {
        if !matches!(state,JobState::Cancelled|JobState::Failed) { return Err("completion requires atomic result storage".into()); }
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            owned(&tx,id,attempt)?;
            let now=now_ms();
            tx.execute("UPDATE kernel_jobs SET state=?2,error_code=?3,error_message=?4,updated_at=?5,
                finished_at=?5,owner_pid=NULL,owner_started_at=NULL,
                cancel_acknowledged_at=CASE WHEN ?2='cancelled' THEN ?5 ELSE cancel_acknowledged_at END
                WHERE job_id=?1 AND attempts=?6",
                params![id,state.as_str(),error.map(|x|x.0),error.map(|x|x.1),now,attempt])?;
            tx.commit()
        })
    }
    pub(crate) fn kernel_job_complete_attempt(&self,id:&str,attempt:u32,conversation:&str,result:&Value)->Result<(),String> {
        let body=serde_json::to_string(result).map_err(|e|format!("job.result_storage_failed: {e}"))?;
        let sha=format!("sha256:{}",hex::encode(Sha256::digest(body.as_bytes())));
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let job=owned(&tx,id,attempt)?;
            if job.conversation_id!=conversation {return Err(invalid("job.conversation_mismatch"));}
            if job.cancel_requested_at.is_some() {return Err(invalid("job.cancel_requested"));}
            tx.execute("INSERT OR IGNORE INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at)
                VALUES(?1,?2,?3,?4)",params![sha,body,body.len() as i64,now_ms()])?;
            let stored:(String,i64)=tx.query_row("SELECT body_json,byte_size FROM tool_call_result_blobs WHERE sha256=?1",
                [&sha],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if stored.0!=body||stored.1!=body.len() as i64 {return Err(invalid("job.result_storage_failed: blob integrity mismatch"));}
            tx.execute("UPDATE kernel_jobs SET state='completed',result_ref=?2,result_bytes=?3,result_sha256=?4,
                updated_at=?5,finished_at=?5,owner_pid=NULL,owner_started_at=NULL WHERE job_id=?1 AND attempts=?6",
                params![id,format!("fox-job-result://{id}"),body.len() as i64,sha,now_ms(),attempt])?;
            tx.commit()
        })
    }
    pub(crate) fn kernel_job_result_value(&self,conversation:&str,id:&str)->Result<Value,String> {
        self.with_connection(|conn| {
            let row:Option<(String,String,i64)>=conn.query_row("SELECT b.body_json,j.result_sha256,j.result_bytes
                FROM kernel_jobs j JOIN tool_call_result_blobs b ON b.sha256=j.result_sha256
                WHERE j.job_id=?1 AND j.conversation_id=?2 AND (j.state='completed' OR (j.kind='command' AND j.state IN ('failed','cancelled')))",
                params![id,conversation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let (body,sha,len)=row.ok_or_else(||invalid("job result unavailable in this conversation"))?;
            if body.len() as i64!=len||format!("sha256:{}",hex::encode(Sha256::digest(body.as_bytes())))!=sha {
                return Err(invalid("job result integrity mismatch"));
            }
            serde_json::from_str(&body).map_err(|_|invalid("invalid stored job result"))
        })
    }
    pub(crate) fn kernel_job_request_cancel(&self,conversation:&str,id:&str)->Result<JobSnapshot,String> {
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let row=read_snapshot(&tx,id)?;
            if row.conversation_id!=conversation {return Err(invalid("job.conversation_mismatch"));}
            if !row.state.is_terminal() {
                let now=now_ms();
                tx.execute("UPDATE kernel_jobs SET cancel_requested_at=COALESCE(cancel_requested_at,?2),
                    cancelled_by='user',updated_at=?2 WHERE job_id=?1",params![id,now])?;
                if matches!(row.state,JobState::Queued|JobState::Paused) {
                    tx.execute("UPDATE kernel_jobs SET state='cancelled',cancel_acknowledged_at=?2,
                        finished_at=?2,owner_pid=NULL,owner_started_at=NULL WHERE job_id=?1",params![id,now])?;
                }
            }
            let row=read_snapshot(&tx,id)?;tx.commit()?;Ok(row)
        })
    }
}
