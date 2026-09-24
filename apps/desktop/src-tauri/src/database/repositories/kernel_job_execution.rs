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

/// A durable Host fact, not a model message or an executable outbox effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KernelJobNotice {
    pub data_root_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub job_id: String,
    pub attempt: u32,
    pub terminal_state: JobState,
    pub finished_at: i64,
    pub result_ref: Option<String>,
    pub result_sha256: Option<String>,
    pub result_bytes: Option<u64>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    terminal_origin: String,
    owner_pid: Option<i64>,
    owner_started_at: Option<i64>,
}

fn notice_enabled(tx: &Transaction<'_>, job: &JobSnapshot) -> rusqlite::Result<bool> {
    if job.kind != "attachment_compute" { return Ok(false); }
    let frozen: Option<String> = tx.query_row(
        "SELECT k.frozen_config_json FROM kernel_runs k
         JOIN run_control_bindings b ON b.run_id=k.run_id
         JOIN runs r ON r.id=k.run_id
         WHERE k.run_id=?1 AND k.kernel_mode='authoritative'
           AND b.authority='authoritative' AND b.conversation_id=?2
           AND r.conversation_id=?2",
        params![job.run_id, job.conversation_id], |row| row.get(0),
    ).optional()?;
    let Some(frozen) = frozen else { return Ok(false); };
    let config: crate::kernel::RunFrozenConfig = serde_json::from_str(&frozen)
        .map_err(|_| invalid("invalid frozen Kernel Run for job notice"))?;
    Ok(config.experimental_compute_job_notice)
}

fn insert_notice(
    tx: &Transaction<'_>, root: &str, job: &JobSnapshot, state: JobState,
    finished_at: i64, result: Option<(&str, &str, i64)>, error: Option<(&str, &str)>,
    worker_owned: bool,
) -> rusqlite::Result<()> {
    let (result_ref, result_sha, result_bytes) = match result {
        Some((reference, sha, bytes)) => (Some(reference), Some(sha), Some(bytes)),
        None => (None, None, None),
    };
    tx.execute(
        "INSERT INTO kernel_job_notices
         (job_id,data_root_id,conversation_id,run_id,attempt,terminal_state,finished_at,
          result_ref,result_sha256,result_bytes,error_code,error_message,
          terminal_origin,owner_pid,owner_started_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
        params![job.job_id, root, job.conversation_id, job.run_id, job.attempts,
            state.as_str(), finished_at, result_ref, result_sha, result_bytes,
            error.map(|value| value.0), error.map(|value| value.1),
            if worker_owned { "worker" } else { "unowned" },
            if worker_owned { Some(i64::from(std::process::id())) } else { None },
            if worker_owned { Some(owner_started()) } else { None }],
    )?;
    Ok(())
}

fn read_notice(tx: &rusqlite::Connection, root: &str, conversation: &str, run: &str, id: &str)
    -> rusqlite::Result<Option<KernelJobNotice>> {
    tx.query_row(
        "SELECT n.data_root_id,n.conversation_id,n.run_id,n.job_id,n.attempt,
                n.terminal_state,n.finished_at,n.result_ref,n.result_sha256,n.result_bytes,
                n.error_code,n.error_message,n.terminal_origin,n.owner_pid,n.owner_started_at,
                blob.body_json,blob.byte_size
         FROM kernel_job_notices n
         JOIN kernel_jobs j ON j.job_id=n.job_id AND j.kind='attachment_compute'
             AND j.run_id=n.run_id
             AND j.conversation_id=n.conversation_id AND j.attempts=n.attempt
             AND j.state=n.terminal_state AND j.finished_at=n.finished_at
             AND j.result_ref IS n.result_ref AND j.result_sha256 IS n.result_sha256
             AND j.result_bytes IS n.result_bytes AND j.error_code IS n.error_code
             AND j.error_message IS n.error_message
         LEFT JOIN tool_call_result_blobs blob ON blob.sha256=n.result_sha256
         JOIN runs r ON r.id=n.run_id AND r.conversation_id=n.conversation_id
         JOIN kernel_runs k ON k.run_id=n.run_id AND k.kernel_mode='authoritative'
         JOIN run_control_bindings binding ON binding.run_id=n.run_id
             AND binding.authority='authoritative' AND binding.conversation_id=n.conversation_id
         WHERE n.data_root_id=?1 AND n.conversation_id=?2 AND n.run_id=?3 AND n.job_id=?4",
        params![root, conversation, run, id], |row| {
            let state: String = row.get(5)?;
            let state = JobState::parse(&state).ok_or_else(|| invalid("invalid notice state"))?;
            let attempt: i64 = row.get(4)?;
            let bytes: Option<i64> = row.get(9)?;
            let notice = KernelJobNotice {
                data_root_id: row.get(0)?, conversation_id: row.get(1)?,
                run_id: row.get(2)?, job_id: row.get(3)?,
                attempt: u32::try_from(attempt).map_err(|_| invalid("invalid notice attempt"))?,
                terminal_state: state, finished_at: row.get(6)?,
                result_ref: row.get(7)?, result_sha256: row.get(8)?,
                result_bytes: bytes.map(|value| u64::try_from(value)
                    .map_err(|_| invalid("invalid notice length"))).transpose()?,
                error_code: row.get(10)?, error_message: row.get(11)?,
                terminal_origin: row.get(12)?, owner_pid: row.get(13)?,
                owner_started_at: row.get(14)?,
            };
            if state == JobState::Completed {
                let body: Option<String> = row.get(15)?;
                let stored_bytes: Option<i64> = row.get(16)?;
                let Some(body) = body else { return Err(invalid("missing job notice result blob")); };
                let body_sha = format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())));
                if stored_bytes != Some(body.len() as i64)
                    || notice.result_bytes != Some(body.len() as u64)
                    || notice.result_sha256.as_deref() != Some(body_sha.as_str()) {
                    return Err(invalid("job notice result integrity mismatch"));
                }
            }
            Ok(notice)
        },
    ).optional()
}

fn same_worker_terminal(
    tx: &Transaction<'_>, root: &str, id: &str, attempt: u32,
    state: JobState, result: Option<(&str, &str, i64, &str)>,
    error: Option<(&str, &str)>, conversation: Option<&str>,
) -> rusqlite::Result<bool> {
    let Ok(job) = read_snapshot(tx, id) else { return Ok(false); };
    if job.attempts != attempt || job.state != state || !notice_enabled(tx, &job)?
        || conversation.is_some_and(|value| value != job.conversation_id.as_str()) {
        return Ok(false);
    }
    let Some(notice) = read_notice(tx, root, &job.conversation_id, &job.run_id, id)? else {
        return Ok(false);
    };
    if notice.terminal_origin != "worker"
        || notice.owner_pid != Some(i64::from(std::process::id()))
        || notice.owner_started_at != Some(owner_started())
        || notice.attempt != attempt || notice.terminal_state != state
        || notice.finished_at != job.finished_at.unwrap_or_default()
        || notice.error_code.as_deref() != error.map(|value| value.0)
        || notice.error_message.as_deref() != error.map(|value| value.1)
        || job.error_code.as_deref() != error.map(|value| value.0)
        || job.error_message.as_deref() != error.map(|value| value.1) {
        return Ok(false);
    }
    match result {
        Some((reference, sha, bytes, body)) => {
            if notice.result_ref.as_deref() != Some(reference)
                || notice.result_sha256.as_deref() != Some(sha)
                || notice.result_bytes != Some(bytes as u64)
                || job.result_ref.as_deref() != Some(reference)
                || job.result_sha256.as_deref() != Some(sha)
                || job.result_bytes != Some(bytes as u64) {
                return Ok(false);
            }
            let stored: Option<(String, i64)> = tx.query_row(
                "SELECT body_json,byte_size FROM tool_call_result_blobs WHERE sha256=?1",
                [sha], |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional()?;
            Ok(stored.is_some_and(|stored| stored.0 == body && stored.1 == bytes))
        }
        None => Ok(notice.result_ref.is_none() && notice.result_sha256.is_none()
            && notice.result_bytes.is_none() && job.result_ref.is_none()
            && job.result_sha256.is_none() && job.result_bytes.is_none()),
    }
}

impl Database {
    pub(crate) fn kernel_command_job_checkpoint(&self,id:&str,attempt:u32,value:&Value)->Result<(),String> {
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let row=owned(&tx,id,attempt)?;
            if row.kind!="command" {return Err(invalid("not a command job"));}
            let body=value.to_string();
            if row.cursor.as_deref()!=Some(body.as_str()) {
                tx.execute("UPDATE kernel_jobs SET cursor=?2,updated_at=?3 WHERE job_id=?1",params![id,body,now_ms()])?;
            }
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
    /// Supervision must not reload the potentially large output cursor every tick.
    pub(crate) fn kernel_command_job_stop_requested(&self, id: &str) -> Result<bool, String> {
        self.with_connection(|conn| conn.query_row(
            "SELECT j.cancel_requested_at IS NOT NULL OR j.state <> 'running'
                OR r.status IN ('completed','failed','cancelled','interrupted')
             FROM kernel_jobs j JOIN runs r ON r.id=j.run_id WHERE j.job_id=?1",
            [id], |row| row.get(0)))
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
            let job = match owned(&tx,id,attempt) {
                Ok(job) => job,
                Err(original) => {
                    if same_worker_terminal(&tx,&self.data_root_id,id,attempt,state,None,error,None)? {
                        return Ok(());
                    }
                    return Err(original);
                }
            };
            let emit_notice = notice_enabled(&tx,&job)?;
            let now=now_ms();
            tx.execute("UPDATE kernel_jobs SET state=?2,error_code=?3,error_message=?4,updated_at=?5,
                finished_at=?5,owner_pid=NULL,owner_started_at=NULL,
                cancel_acknowledged_at=CASE WHEN ?2='cancelled' THEN ?5 ELSE cancel_acknowledged_at END
                WHERE job_id=?1 AND attempts=?6",
                params![id,state.as_str(),error.map(|x|x.0),error.map(|x|x.1),now,attempt])?;
            if emit_notice {
                insert_notice(&tx,&self.data_root_id,&job,state,now,None,error,true)?;
            }
            tx.commit()
        })
    }
    pub(crate) fn kernel_job_complete_attempt(&self,id:&str,attempt:u32,conversation:&str,result:&Value)->Result<(),String> {
        let body=serde_json::to_string(result).map_err(|e|format!("job.result_storage_failed: {e}"))?;
        let sha=format!("sha256:{}",hex::encode(Sha256::digest(body.as_bytes())));
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let reference = format!("fox-job-result://{id}");
            let job=match owned(&tx,id,attempt) {
                Ok(job) => job,
                Err(original) => {
                    if same_worker_terminal(&tx,&self.data_root_id,id,attempt,JobState::Completed,
                        Some((reference.as_str(),sha.as_str(),body.len() as i64,body.as_str())),None,Some(conversation))? {
                        return Ok(());
                    }
                    return Err(original);
                }
            };
            if job.conversation_id!=conversation {return Err(invalid("job.conversation_mismatch"));}
            if job.cancel_requested_at.is_some() {return Err(invalid("job.cancel_requested"));}
            let emit_notice = notice_enabled(&tx,&job)?;
            tx.execute("INSERT OR IGNORE INTO tool_call_result_blobs(sha256,body_json,byte_size,created_at)
                VALUES(?1,?2,?3,?4)",params![sha,body,body.len() as i64,now_ms()])?;
            let stored:(String,i64)=tx.query_row("SELECT body_json,byte_size FROM tool_call_result_blobs WHERE sha256=?1",
                [&sha],|r|Ok((r.get(0)?,r.get(1)?)))?;
            if stored.0!=body||stored.1!=body.len() as i64 {return Err(invalid("job.result_storage_failed: blob integrity mismatch"));}
            tx.execute("UPDATE kernel_jobs SET state='completed',result_ref=?2,result_bytes=?3,result_sha256=?4,
                updated_at=?5,finished_at=?5,owner_pid=NULL,owner_started_at=NULL,
                error_code=CASE WHEN ?7 THEN NULL ELSE error_code END,
                error_message=CASE WHEN ?7 THEN NULL ELSE error_message END
                WHERE job_id=?1 AND attempts=?6",
                params![id,reference,body.len() as i64,sha,now_ms(),attempt,emit_notice])?;
            if emit_notice {
                let finished_at: i64 = tx.query_row("SELECT finished_at FROM kernel_jobs WHERE job_id=?1", [id], |row| row.get(0))?;
                insert_notice(&tx,&self.data_root_id,&job,JobState::Completed,finished_at,
                    Some((reference.as_str(),sha.as_str(),body.len() as i64)),None,true)?;
            }
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
                let emit_notice = matches!(row.state,JobState::Queued|JobState::Paused)
                    && notice_enabled(&tx,&row)?;
                let now=now_ms();
                tx.execute("UPDATE kernel_jobs SET cancel_requested_at=COALESCE(cancel_requested_at,?2),
                    cancelled_by='user',updated_at=?2 WHERE job_id=?1",params![id,now])?;
                if matches!(row.state,JobState::Queued|JobState::Paused) {
                    tx.execute("UPDATE kernel_jobs SET state='cancelled',cancel_acknowledged_at=?2,
                        finished_at=?2,owner_pid=NULL,owner_started_at=NULL,
                        error_code=CASE WHEN ?3 THEN NULL ELSE error_code END,
                        error_message=CASE WHEN ?3 THEN NULL ELSE error_message END
                        WHERE job_id=?1",params![id,now,emit_notice])?;
                    if emit_notice {
                        insert_notice(&tx,&self.data_root_id,&row,JobState::Cancelled,now,None,None,false)?;
                    }
                }
            }
            let row=read_snapshot(&tx,id)?;tx.commit()?;Ok(row)
        })
    }

    /// B1 internal scoped read. The caller supplies no root identity; a copied
    /// database opened from another Host data root cannot consume old facts.
    pub(crate) fn kernel_job_notice(&self, conversation:&str, run:&str, id:&str)
        -> Result<Option<KernelJobNotice>,String> {
        self.with_connection(|conn| read_notice(conn,&self.data_root_id,conversation,run,id))
    }

    /// Terminalize an expired job that has no executor. B2 will decide when to
    /// call this; B1 never starts or replays the work while doing so.
    pub(crate) fn kernel_job_expire_unowned(&self, conversation:&str, run:&str, id:&str)
        -> Result<JobSnapshot,String> {
        self.with_connection(|conn| {
            let tx=conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let job=read_snapshot(&tx,id)?;
            if job.conversation_id!=conversation || job.run_id!=run {
                return Err(invalid("job scope mismatch"));
            }
            if !matches!(job.state,JobState::Queued|JobState::Paused)
                || !notice_enabled(&tx,&job)? {
                return Err(invalid("job is not an experimental unowned compute job"));
            }
            let now=now_ms();
            let changed=tx.execute(
                "UPDATE kernel_jobs SET state='failed',error_code='job.deadline_exceeded',
                 error_message='compute job deadline exceeded before execution',
                 updated_at=?4,finished_at=?4,owner_pid=NULL,owner_started_at=NULL
                 WHERE job_id=?1 AND run_id=?2 AND conversation_id=?3
                   AND state IN ('queued','paused') AND owner_pid IS NULL
                   AND owner_started_at IS NULL AND deadline_ms > 0 AND deadline_ms <= ?4",
                params![id,run,conversation,now],
            )?;
            if changed!=1 { return Err(invalid("job is not expired and unowned")); }
            insert_notice(&tx,&self.data_root_id,&job,JobState::Failed,now,None,
                Some(("job.deadline_exceeded","compute job deadline exceeded before execution")),false)?;
            let settled=read_snapshot(&tx,id)?;
            tx.commit()?;
            Ok(settled)
        })
    }
}

#[cfg(test)]
mod notice_tests {
    use super::*;
    use crate::database::JobStartRequest;
    use fox_engine_protocol::TimeBudgets;
    use serde_json::json;
    use std::path::PathBuf;
    use uuid::Uuid;

    struct Fixture {
        root: PathBuf,
        db: Database,
        conversation: String,
        run: String,
    }

    impl Fixture {
        fn new(tag: &str, enabled: bool) -> Self {
            let root = std::env::temp_dir().join(format!("fox-b1-notice-{tag}-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let db = Database::open(root.join("facts.db")).unwrap();
            let conversation = db.create_conversation(db.default_agent_id(), None, None, None).unwrap().id;
            let run = db.create_run(&conversation, "B1 notice", None).unwrap().run.id;
            let binding = db.freeze_kernel_run_control(&run, "legacy", TimeBudgets::default()).unwrap();
            let frozen = crate::kernel::RunFrozenConfig {
                engine_id: binding.engine_id.clone(), kernel_mode: "authoritative".into(),
                capability_manifest_version: 2, capability_manifest_hash: "b1-manifest".into(),
                permission_snapshot_id: binding.permission_snapshot_id.clone(),
                execution_profile_id: binding.execution_profile_id.clone(),
                prompt_config_hash: "b1-prompt".into(),
                model_request_timeout_ms: binding.budgets.model_request_ms,
                model_first_response_ms: binding.budgets.model_first_response_ms,
                model_idle_ms: binding.budgets.model_idle_ms,
                tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
                run_execution_budget_ms: binding.budgets.run_execution_ms,
                run_execution_limited: binding.budgets.run_execution_limited,
                approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
                provider_max_retries: 2, turn_max_retries: 1,
                experimental_compute_job_notice: enabled,
            };
            db.kernel_create_run(&run, "pi", "authoritative", 2,
                &binding.permission_snapshot_id, &binding.execution_profile_id,
                "b1-prompt", &serde_json::to_string(&frozen).unwrap()).unwrap();
            Self { root, db, conversation, run }
        }

        fn start(&self, key: &str, kind: &str, deadline_ms: i64) -> String {
            self.db.kernel_job_start(&JobStartRequest {
                run_id: self.run.clone(), kind: kind.into(), idempotency_key: key.into(),
                params: json!({"case":key}), deadline_ms: Some(deadline_ms), progress_total: None,
            }).unwrap().snapshot().job_id.clone()
        }

        fn compute(&self, key: &str) -> String {
            self.start(key, "attachment_compute", now_ms() + 60_000)
        }

        fn finish(self) {
            let root = self.root.clone();
            drop(self);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    fn notice_count(db: &Database) -> i64 {
        db.with_connection(|conn| conn.query_row("SELECT COUNT(*) FROM kernel_job_notices", [], |r| r.get(0))).unwrap()
    }

    fn blob_count(db: &Database) -> i64 {
        db.with_connection(|conn| conn.query_row("SELECT COUNT(*) FROM tool_call_result_blobs", [], |r| r.get(0))).unwrap()
    }

    #[test]
    fn completed_notice_is_atomic_unique_scoped_and_exactly_idempotent() {
        let f=Fixture::new("success",true);
        let job=f.compute("success");
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        let result=json!({"answer":"done"});
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_jobs SET owner_pid=-1 WHERE job_id=?1",[&job])).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).is_err());
        assert_eq!(notice_count(&f.db),0);
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_jobs SET owner_pid=?2 WHERE job_id=?1",
            params![&job,i64::from(std::process::id())])).unwrap();
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).unwrap();
        let notice=f.db.kernel_job_notice(&f.conversation,&f.run,&job).unwrap().unwrap();
        assert_eq!(notice.terminal_state,JobState::Completed);
        assert_eq!(notice.attempt,1);
        assert_eq!(notice.result_sha256,f.db.kernel_job_snapshot(&job).unwrap().result_sha256);
        assert_eq!(notice_count(&f.db),1);
        let blobs=blob_count(&f.db);
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).unwrap();
        assert_eq!(notice_count(&f.db),1);
        assert_eq!(blob_count(&f.db),blobs);
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&json!({"answer":"other"})).is_err());
        assert!(f.db.kernel_job_settle_attempt(&job,1,JobState::Failed,Some(("compute.failed","late"))).is_err());
        assert!(f.db.kernel_job_complete_attempt(&job,0,&f.conversation,&result).is_err());
        assert!(f.db.kernel_job_complete_attempt(&job,1,"foreign",&result).is_err());
        assert!(f.db.kernel_job_notice("foreign",&f.run,&job).unwrap().is_none());
        assert!(f.db.kernel_job_notice(&f.conversation,"foreign-run",&job).unwrap().is_none());
        // Build a second persisted identity solely for this cross-Run read
        // negative; the product create_run flow is outside this test.
        let second_run=format!("{}-other",f.run);
        f.db.with_connection(|conn| conn.execute(
            "INSERT INTO runs(id,conversation_id,status,model,created_at)
             VALUES(?1,?2,'running','test',?3)",
            params![&second_run,&f.conversation,now_ms()],
        )).unwrap();
        assert!(f.db.kernel_job_notice(&f.conversation,&second_run,&job).unwrap().is_none());
        assert_eq!(f.db.kernel_job_result_value(&f.conversation,&job).unwrap(),result);
        f.finish();
    }

    #[test]
    fn failure_cancel_and_deadline_publish_only_real_terminal_facts() {
        let f=Fixture::new("other-terminals",true);
        let failed=f.compute("failed");
        f.db.kernel_job_claim_attempt(&failed,1).unwrap();
        f.db.kernel_job_settle_attempt(&failed,1,JobState::Failed,Some(("compute.failed","diagnostic"))).unwrap();
        f.db.kernel_job_settle_attempt(&failed,1,JobState::Failed,Some(("compute.failed","diagnostic"))).unwrap();
        assert!(f.db.kernel_job_settle_attempt(&failed,1,JobState::Failed,Some(("compute.failed","changed"))).is_err());
        assert!(f.db.kernel_job_settle_attempt(&failed,1,JobState::Cancelled,None).is_err());
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&failed).unwrap().unwrap().error_code.as_deref(),Some("compute.failed"));

        let running=f.compute("running-cancel");
        f.db.kernel_job_claim_attempt(&running,1).unwrap();
        assert_eq!(f.db.kernel_job_request_cancel(&f.conversation,&running).unwrap().state,JobState::Running);
        assert!(f.db.kernel_job_notice(&f.conversation,&f.run,&running).unwrap().is_none());
        f.db.kernel_job_settle_attempt(&running,1,JobState::Cancelled,None).unwrap();
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&running).unwrap().unwrap().terminal_state,JobState::Cancelled);

        let queued=f.compute("queued-cancel");
        assert_eq!(f.db.kernel_job_request_cancel(&f.conversation,&queued).unwrap().state,JobState::Cancelled);
        f.db.kernel_job_request_cancel(&f.conversation,&queued).unwrap();
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&queued).unwrap().unwrap().terminal_state,JobState::Cancelled);

        let paused=f.compute("paused-cancel");
        f.db.kernel_job_claim_attempt(&paused,1).unwrap();
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_jobs SET state='paused',owner_pid=NULL,
            owner_started_at=NULL,error_code='job.owner_gone',error_message='old error' WHERE job_id=?1",[&paused])).unwrap();
        f.db.kernel_job_request_cancel(&f.conversation,&paused).unwrap();
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&paused).unwrap().unwrap().terminal_state,JobState::Cancelled);

        let expired=f.start("expired","attachment_compute",now_ms()-1);
        assert_eq!(f.db.kernel_job_expire_unowned(&f.conversation,&f.run,&expired).unwrap().state,JobState::Failed);
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&expired).unwrap().unwrap().error_code.as_deref(),Some("job.deadline_exceeded"));
        assert!(f.db.kernel_job_expire_unowned(&f.conversation,&f.run,&expired).is_err());
        let paused_expired=f.start("paused-expired","attachment_compute",now_ms()-1);
        f.db.kernel_job_claim_attempt(&paused_expired,1).unwrap();
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_jobs SET state='paused',owner_pid=NULL,
            owner_started_at=NULL WHERE job_id=?1",[&paused_expired])).unwrap();
        assert_eq!(f.db.kernel_job_expire_unowned(&f.conversation,&f.run,&paused_expired).unwrap().state,JobState::Failed);
        assert_eq!(notice_count(&f.db),6);
        f.finish();
    }

    #[test]
    fn resumed_attempt_cannot_be_overwritten_by_stale_attempt() {
        let f=Fixture::new("stale",true);
        let job=f.compute("resume");
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_jobs SET state='paused',owner_pid=NULL,
            owner_started_at=NULL,error_code='job.owner_gone',error_message='old error' WHERE job_id=?1",[&job])).unwrap();
        f.db.kernel_job_claim_attempt(&job,2).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&json!({"stale":true})).is_err());
        assert_eq!(notice_count(&f.db),0);
        let result=json!({"winner":"attempt-2"});
        f.db.kernel_job_complete_attempt(&job,2,&f.conversation,&result).unwrap();
        assert_eq!(f.db.kernel_job_notice(&f.conversation,&f.run,&job).unwrap().unwrap().attempt,2);
        assert!(f.db.kernel_job_snapshot(&job).unwrap().error_code.is_none());
        assert!(f.db.kernel_job_resume(&job).is_err());
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).is_err());
        assert_eq!(f.db.kernel_job_result_value(&f.conversation,&job).unwrap(),result);
        f.finish();
    }

    #[test]
    fn notice_or_blob_insert_failure_rolls_back_the_whole_settlement() {
        let f=Fixture::new("rollback",true);
        let job=f.compute("notice-fault");
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        let original_blobs=blob_count(&f.db);
        f.db.with_connection(|conn| conn.execute_batch("CREATE TRIGGER b1_reject_notice BEFORE INSERT ON kernel_job_notices BEGIN SELECT RAISE(ABORT,'injected notice failure'); END")).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&json!({"value":1})).is_err());
        assert_eq!(f.db.kernel_job_snapshot(&job).unwrap().state,JobState::Running);
        assert_eq!(blob_count(&f.db),original_blobs);
        assert_eq!(notice_count(&f.db),0);
        f.db.with_connection(|conn| conn.execute_batch("DROP TRIGGER b1_reject_notice")).unwrap();
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&json!({"value":1})).unwrap();

        let failed=f.compute("settle-fault");
        f.db.kernel_job_claim_attempt(&failed,1).unwrap();
        f.db.with_connection(|conn| conn.execute_batch("CREATE TRIGGER b1_reject_notice BEFORE INSERT ON kernel_job_notices BEGIN SELECT RAISE(ABORT,'injected notice failure'); END")).unwrap();
        assert!(f.db.kernel_job_settle_attempt(&failed,1,JobState::Failed,Some(("compute.failed","fault"))).is_err());
        assert_eq!(f.db.kernel_job_snapshot(&failed).unwrap().state,JobState::Running);
        f.db.with_connection(|conn| conn.execute_batch("DROP TRIGGER b1_reject_notice")).unwrap();

        let queued=f.compute("cancel-fault");
        f.db.with_connection(|conn| conn.execute_batch("CREATE TRIGGER b1_reject_notice BEFORE INSERT ON kernel_job_notices BEGIN SELECT RAISE(ABORT,'injected notice failure'); END")).unwrap();
        assert!(f.db.kernel_job_request_cancel(&f.conversation,&queued).is_err());
        let pending=f.db.kernel_job_snapshot(&queued).unwrap();
        assert_eq!(pending.state,JobState::Queued);
        assert!(pending.cancel_requested_at.is_none());
        f.db.with_connection(|conn| conn.execute_batch("DROP TRIGGER b1_reject_notice")).unwrap();

        let blob=f.compute("blob-fault");
        f.db.kernel_job_claim_attempt(&blob,1).unwrap();
        f.db.with_connection(|conn| conn.execute_batch("CREATE TRIGGER b1_reject_blob BEFORE INSERT ON tool_call_result_blobs BEGIN SELECT RAISE(ABORT,'injected blob failure'); END")).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&blob,1,&f.conversation,&json!({"unique":"blob fault"})).is_err());
        assert_eq!(f.db.kernel_job_snapshot(&blob).unwrap().state,JobState::Running);
        assert!(f.db.kernel_job_notice(&f.conversation,&f.run,&blob).unwrap().is_none());
        f.db.with_connection(|conn| conn.execute_batch("DROP TRIGGER b1_reject_blob")).unwrap();
        f.finish();
    }

    #[test]
    fn old_flag_and_command_jobs_keep_their_rejection_semantics() {
        let f=Fixture::new("disabled",false);
        let job=f.compute("legacy");
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        let value=json!({"legacy":true});
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&value).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&value).is_err());
        assert!(f.db.kernel_job_notice(&f.conversation,&f.run,&job).unwrap().is_none());
        f.finish();

        let f=Fixture::new("command",true);
        let job=f.start("command","command",now_ms()+60_000);
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        let value=json!({"command":true});
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&value).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&value).is_err());
        assert_eq!(notice_count(&f.db),0);
        f.finish();
    }

    #[test]
    fn owner_conflicts_and_database_copies_cannot_consume_existing_notice() {
        let f=Fixture::new("root",true);
        let job=f.compute("owned");
        f.db.kernel_job_claim_attempt(&job,1).unwrap();
        let result=json!({"persisted":"original"});
        f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).unwrap();
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_job_notices SET owner_pid=-1 WHERE job_id=?1",[&job])).unwrap();
        assert!(f.db.kernel_job_complete_attempt(&job,1,&f.conversation,&result).is_err());
        f.db.with_connection(|conn| conn.execute("UPDATE kernel_job_notices SET owner_pid=?2 WHERE job_id=?1",params![&job,i64::from(std::process::id())])).unwrap();
        let root=f.root.clone();let source=root.join("facts.db");
        let conversation=f.conversation.clone();let run=f.run.clone();
        let original_root=f.db.data_root_id.clone();
        drop(f);
        let beside=root.join("copy.db");
        std::fs::copy(&source,&beside).unwrap();
        let other=root.join("other");std::fs::create_dir_all(&other).unwrap();
        let elsewhere=other.join("facts.db");std::fs::copy(&source,&elsewhere).unwrap();
        for copy in [&beside,&elsewhere] {
            let reopened=Database::open(copy.clone()).unwrap();
            assert_ne!(reopened.data_root_id,original_root);
            assert!(reopened.kernel_job_notice(&conversation,&run,&job).unwrap().is_none());
            drop(reopened);
        }
        let reopened=Database::open(source).unwrap();
        assert_eq!(reopened.data_root_id,original_root);
        assert!(reopened.kernel_job_notice(&conversation,&run,&job).unwrap().is_some());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_84_upgrades_v83_shape_and_memory_instances_are_distinct() {
        let first=Database::open(PathBuf::from(":memory:")).unwrap();
        let clone=first.clone();
        let second=Database::open(PathBuf::from(":memory:")).unwrap();
        assert_eq!(first.data_root_id,clone.data_root_id);
        assert_ne!(first.data_root_id,second.data_root_id);
        assert!(first.set_app_setting(super::super::DATA_ROOT_INSTANCE_UUID_KEY,"replacement").is_err());

        let root=std::env::temp_dir().join(format!("fox-b1-v83-{}",Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path=root.join("facts.db");
        let db=Database::open(path.clone()).unwrap();
        db.with_connection(|conn| conn.execute_batch(
            "DROP TABLE kernel_job_notices; DELETE FROM schema_migrations WHERE version=84;"
        )).unwrap();
        drop(db);
        for _ in 0..2 {
            let reopened=Database::open(path.clone()).unwrap();
            reopened.with_connection(|conn| {
                let version: i64=conn.query_row("SELECT MAX(version) FROM schema_migrations",[],|r|r.get(0))?;
                let violations: i64=conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check",[],|r|r.get(0))?;
                assert_eq!(version,84);assert_eq!(violations,0);
                Ok(())
            }).unwrap();
            drop(reopened);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
