//! Unified Host lifecycle for background compute jobs (#7).
//!
//! This is deliberately a *lifecycle*, not another run/permission system. A job
//! belongs to an existing Run and conversation, so every authorization decision
//! still comes from that Run's frozen scope; the job only adds durable
//! bookkeeping: what was asked for, whether it is still running, how far it got,
//! what it produced, and who owns the process doing it.
//!
//! Four properties matter and are enforced here, not by convention:
//!
//! * **Start is idempotent.** `(run_id, idempotency_key)` is unique and the
//!   params are hashed. A repeat with the same parameters returns the existing
//!   job; the same key with different parameters is a conflict, never a second
//!   launch.
//! * **Status is a pure read.** It never executes work, so polling cannot cause
//!   side effects.
//! * **Cancel is a request until acknowledged.** Running work records a durable
//!   request first; the owning attempt writes `cancelled` only after stopping.
//! * **A restart can tell "gone" from "running".** `owner_pid` plus a start
//!   timestamp decide whether a `running` row is ours; an orphaned job becomes
//!   `paused` (resumable) instead of being silently replayed.

use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The states a job can be in. `Paused` is a pause of the work, not a failure:
/// it is the only non-terminal state from which a resume is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Queued,
    Running,
    Paused,
    Cancelled,
    Failed,
    Completed,
}

impl JobState {
    pub fn as_str(self) -> &'static str {
        match self {
            JobState::Queued => "queued",
            JobState::Running => "running",
            JobState::Paused => "paused",
            JobState::Cancelled => "cancelled",
            JobState::Failed => "failed",
            JobState::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<JobState> {
        Some(match value {
            "queued" => JobState::Queued,
            "running" => JobState::Running,
            "paused" => JobState::Paused,
            "cancelled" => JobState::Cancelled,
            "failed" => JobState::Failed,
            "completed" => JobState::Completed,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Cancelled | JobState::Failed | JobState::Completed
        )
    }
}

/// What the Host asks for. `params` participates in the idempotency comparison,
/// so it must be canonical for a given logical request.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobStartRequest {
    pub run_id: String,
    pub kind: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub deadline_ms: Option<i64>,
    #[serde(default)]
    pub progress_total: Option<u64>,
}

/// What the Host and the UI read back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub job_id: String,
    pub run_id: String,
    pub conversation_id: String,
    pub kind: String,
    pub state: JobState,
    pub cursor: Option<String>,
    pub progress_done: u64,
    pub progress_total: Option<u64>,
    pub result_ref: Option<String>,
    pub result_bytes: Option<u64>,
    pub result_sha256: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub finished_at: Option<i64>,
    pub attempts: u32,
    /// True when the caller may resume: only a paused job, or one whose owning
    /// process is gone.
    pub resumable: bool,
    /// The Host that owns the process, when one is still registered.
    pub owner_pid: Option<i64>,
    pub cancel_requested_at: Option<i64>,
    pub cancel_acknowledged_at: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobStartOutcome {
    /// A new job row was created; the caller must launch it.
    Created(JobSnapshot),
    /// The same logical request already exists; nothing new was launched.
    Existing(JobSnapshot),
}

impl JobStartOutcome {
    pub fn snapshot(&self) -> &JobSnapshot {
        match self {
            JobStartOutcome::Created(snapshot) | JobStartOutcome::Existing(snapshot) => snapshot,
        }
    }
}

fn invalid(message: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(message.into())
}

/// Canonical parameter hash: object keys are sorted so an equal request always
/// hashes equally, and key order can never fake a new job.
fn params_hash(params: &Value) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for key in keys {
                    out.insert(key.clone(), canonical(&map[key]));
                }
                Value::Object(out)
            }
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            other => other.clone(),
        }
    }
    let encoded = canonical(params).to_string();
    format!("sha256:{}", hex::encode(Sha256::digest(encoded.as_bytes())))
}

/// Whether a `running` row belongs to this process.
fn owned_by_current_process(owner_pid: Option<i64>, owner_started_at: Option<i64>) -> bool {
    let (Some(pid), Some(started_at)) = (owner_pid, owner_started_at) else {
        return false;
    };
    if pid != i64::from(std::process::id()) {
        return false;
    }
    // Same pid but a different process start time means the pid was reused after
    // a restart; that is a different process and must not be treated as ours.
    match process_started_at() {
        Some(current) => current == started_at,
        None => false,
    }
}

/// Best-effort process start time, used only to tell pid reuse apart from the
/// same live process. `None` never means "mine".
fn process_started_at() -> Option<i64> {
    crate::runtime_host::process_start_marker()
}

/// Host command keys carry a bounded, canonical dispatch identity. Other
/// background job families retain their historical 200-byte key limit.
fn validate_job_idempotency_key(run_id: &str, key: &str) -> Result<(), String> {
    if key.starts_with("job:tool-dispatch") {
        // 4 decimal digits are sufficient for the frozen 1024-byte component bound.
        const MAX_HOST_KEY_BYTES: usize = "job:tool-dispatch:".len() + 4 + 1 + 1024 + 1 + 1024;
        if key.len() > MAX_HOST_KEY_BYTES {
            return Err("job idempotency key exceeds its bound".into());
        }
        let encoded = key.strip_prefix("job:").ok_or("invalid Host job identity")?;
        let (encoded_run, call_id) = fox_engine_protocol::decode_dispatch_id(encoded)?;
        if encoded_run != run_id || fox_engine_protocol::encode_dispatch_id(&encoded_run, &call_id)? != encoded {
            return Err("credential_mismatch: Host job identity differs from its run".into());
        }
    } else if key.len() > 200 {
        return Err("job idempotency key exceeds its bound".into());
    }
    Ok(())
}

impl Database {
    /// Start (or return) a background job.
    pub fn kernel_job_start(&self, request: &JobStartRequest) -> Result<JobStartOutcome, String> {
        if request.run_id.trim().is_empty()
            || request.kind.trim().is_empty()
            || request.idempotency_key.trim().is_empty()
        {
            return Err("job identity fields must be non-empty".into());
        }
        if request.kind.len() > 64 {
            return Err("job kind exceeds its bound".into());
        }
        validate_job_idempotency_key(&request.run_id, &request.idempotency_key)?;
        if request.deadline_ms.is_some_and(|deadline| deadline <= 0) {
            return Err("job deadline must be positive".into());
        }
        let hash = params_hash(&request.params);
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let conversation_id: Option<String> = transaction
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1",
                    params![request.run_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(conversation_id) = conversation_id else {
                return Err(invalid("unknown run for job"));
            };
            let existing: Option<(String, String)> = transaction
                .query_row(
                    "SELECT job_id, params_hash FROM kernel_jobs
                      WHERE run_id = ?1 AND idempotency_key = ?2",
                    params![request.run_id, request.idempotency_key],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((job_id, stored_hash)) = existing {
                if stored_hash != hash {
                    // Same key, different request: refuse rather than run twice.
                    return Err(invalid("job_idempotency_conflict"));
                }
                let snapshot = read_snapshot(&transaction, &job_id)?;
                if snapshot.kind != request.kind {return Err(invalid("job_idempotency_conflict"));}
                transaction.commit()?;
                return Ok(JobStartOutcome::Existing(snapshot));
            }
            let job_id = format!("job:{}", Uuid::new_v4());
            transaction.execute(
                "INSERT INTO kernel_jobs
                 (job_id, run_id, conversation_id, kind, idempotency_key, params_hash, params_json,
                  state, cursor, progress_done, progress_total, result_ref, result_bytes,
                  result_sha256, error_code, error_message, attempts, deadline_ms, owner_pid,
                  owner_started_at, created_at, updated_at, finished_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', NULL, 0, ?8, NULL, NULL, NULL,
                         NULL, NULL, 0, ?9, CASE WHEN ?4='command' THEN ?11 ELSE NULL END, CASE WHEN ?4='command' THEN ?12 ELSE NULL END, ?10, ?10, NULL)",
                params![
                    job_id,
                    request.run_id,
                    conversation_id,
                    request.kind,
                    request.idempotency_key,
                    hash,
                    request.params.to_string(),
                    request.progress_total.map(|total| total as i64),
                    request.deadline_ms,
                    now,
                    i64::from(std::process::id()),
                    process_started_at()
                ],
            )?;
            let snapshot = read_snapshot(&transaction, &job_id)?;
            transaction.commit()?;
            Ok(JobStartOutcome::Created(snapshot))
        })
    }

    /// Pure read. Never executes work and never mutates the job.
    pub fn kernel_job_snapshot(&self, job_id: &str) -> Result<JobSnapshot, String> {
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// The job store as seen from one run, newest first.
    pub fn kernel_jobs_for_run(&self, run_id: &str) -> Result<Vec<JobSnapshot>, String> {
        let ids: Vec<String> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT job_id FROM kernel_jobs WHERE run_id = ?1 ORDER BY created_at, job_id",
            )?;
            let rows = statement.query_map(params![run_id], |row| row.get::<_, String>(0))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            Ok(ids)
        })?;
        let mut out = Vec::new();
        for job_id in ids {
            out.push(self.kernel_job_snapshot(&job_id)?);
        }
        Ok(out)
    }

    /// Original unfinished compute deadlines for a prospective Kernel park.
    /// The atomic park transaction re-reads this set; this read only prepares
    /// the candidate and never authorizes a state transition on its own.
    pub(crate) fn kernel_waiting_job_facts(
        &self, conversation_id: &str, run_id: &str,
    ) -> Result<Vec<crate::kernel::WaitingJobFact>, String> {
        let rows: Vec<(String,String,i64,Option<i64>)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT job_id,kind,attempts,deadline_ms FROM kernel_jobs
                 WHERE conversation_id=?1 AND run_id=?2 AND state IN ('queued','running','paused')
                 ORDER BY job_id")?;
            let rows = statement.query_map(params![conversation_id,run_id], |row| {
                Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))
            })?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        rows.into_iter().map(|(job_id,kind,attempt,deadline)| {
            if kind != "attachment_compute" || attempt < 0 || deadline.is_none_or(|value| value <= 0) {
                return Err("kernel.job_deadline_missing: unfinished compute Job has no original deadline or valid identity".into());
            }
            Ok(crate::kernel::WaitingJobFact {
                job_id, attempt, deadline_wall_ms: deadline.unwrap(),
            })
        }).collect()
    }

    /// Mark a queued job as running and hand it to this process.
    #[cfg(test)] // Legacy fixture setup only; production must claim a fenced attempt.
    pub fn kernel_job_mark_running(&self, job_id: &str) -> Result<JobSnapshot, String> {
        let now = now_ms();
        let pid = i64::from(std::process::id());
        let started_at = process_started_at();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let affected = transaction.execute(
                "UPDATE kernel_jobs
                    SET state='running', owner_pid=?2, owner_started_at=?3,
                        attempts=attempts+1, updated_at=?4
                  WHERE job_id=?1 AND state IN ('queued','paused')",
                params![job_id, pid, started_at, now],
            )?;
            if affected != 1 {
                return Err(invalid("job is not startable in its current state"));
            }
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// Record monotonic progress. Progress never moves backwards.
    #[cfg(test)]
    pub fn kernel_job_progress(
        &self,
        job_id: &str,
        done: u64,
        total: Option<u64>,
        cursor: Option<&str>,
    ) -> Result<JobSnapshot, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let affected = transaction.execute(
                "UPDATE kernel_jobs
                    SET progress_done=MAX(progress_done, ?2),
                        progress_total=COALESCE(?3, progress_total),
                        cursor=COALESCE(?4, cursor),
                        updated_at=?5
                  WHERE job_id=?1 AND state IN ('queued','running')",
                params![job_id, done as i64, total.map(|t| t as i64), cursor, now],
            )?;
            if affected != 1 {
                return Err(invalid("progress on a job that is not running"));
            }
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// Record a terminal outcome. Terminal is written once.
    #[cfg(test)]
    pub fn kernel_job_settle(
        &self,
        job_id: &str,
        state: JobState,
        result: Option<(String, u64, String)>,
        error: Option<(String, String)>,
    ) -> Result<JobSnapshot, String> {
        if !state.is_terminal() {
            return Err("job settlement must be a terminal state".into());
        }
        if state == JobState::Completed && result.is_none() {
            return Err("a completed job must carry a retrievable result reference".into());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let (result_ref, result_bytes, result_sha256) = match &result {
                Some((reference, bytes, sha)) => {
                    (Some(reference.clone()), Some(*bytes as i64), Some(sha.clone()))
                }
                None => (None, None, None),
            };
            let (error_code, error_message) = match &error {
                Some((code, message)) => (Some(code.clone()), Some(message.clone())),
                None => (None, None),
            };
            let affected = transaction.execute(
                "UPDATE kernel_jobs
                    SET state=?2, result_ref=?3, result_bytes=?4, result_sha256=?5,
                        error_code=?6, error_message=?7, updated_at=?8, finished_at=?8,
                        owner_pid=NULL, owner_started_at=NULL
                  WHERE job_id=?1 AND state NOT IN ('completed','failed','cancelled')",
                params![
                    job_id,
                    state.as_str(),
                    result_ref,
                    result_bytes,
                    result_sha256,
                    error_code,
                    error_message,
                    now
                ],
            )?;
            if affected != 1 {
                let snapshot = read_snapshot(&transaction, job_id)?;
                transaction.commit()?;
                // Idempotent settle: the first terminal state wins.
                return Ok(snapshot);
            }
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// Cancel a job. Idempotent; the executor stops the process tree it owns.
    #[cfg(test)]
    pub fn kernel_job_cancel(&self, job_id: &str) -> Result<JobSnapshot, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let affected = transaction.execute(
                "UPDATE kernel_jobs
                    SET state='cancelled', updated_at=?2, finished_at=?2,
                        owner_pid=NULL, owner_started_at=NULL
                  WHERE job_id=?1 AND state NOT IN ('completed','failed','cancelled')",
                params![job_id, now],
            )?;
            if affected != 1 {
                // Already terminal (including already cancelled): report the fact.
                let snapshot = read_snapshot(&transaction, job_id)?;
                transaction.commit()?;
                return Ok(snapshot);
            }
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// Pause a job so a continuation can offer it later. Used when the owning
    /// process disappears (a Host restart) rather than re-running the work.
    #[cfg(test)]
    pub fn kernel_job_pause(&self, job_id: &str, reason: &str) -> Result<JobSnapshot, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let affected = transaction.execute(
                "UPDATE kernel_jobs
                    SET state='paused', error_code='job.owner_gone', error_message=?2,
                        updated_at=?3, owner_pid=NULL, owner_started_at=NULL
                  WHERE job_id=?1 AND state IN ('queued','running')",
                params![job_id, reason, now],
            )?;
            if affected != 1 {
                return Err(invalid("job is not pausable in its current state"));
            }
            let snapshot = read_snapshot(&transaction, job_id)?;
            transaction.commit()?;
            Ok(snapshot)
        })
    }

    /// Reconcile jobs left `running` by a process that no longer exists. Returns
    /// the jobs that became resumable. Work is never replayed here.
    pub fn kernel_jobs_reconcile_orphans(&self) -> Result<Vec<JobSnapshot>, String> {
        let running: Vec<(String, Option<i64>, Option<i64>)> = self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT job_id, owner_pid, owner_started_at FROM kernel_jobs WHERE state='running' OR (kind='command' AND state='queued')",
            )?;
            let rows = statement.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                out.push(row?);
            }
            Ok(out)
        })?;
        let mut paused = Vec::new();
        for (job_id, pid, started_at) in running {
            if !crate::runtime_host::job_owner_is_gone(pid, started_at) {
                continue;
            }
            let changed=self.with_connection(|connection| {
                let tx=connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
                let changed=tx.execute("UPDATE kernel_jobs SET state=CASE WHEN kind='command' THEN 'failed' ELSE 'paused' END,owner_pid=NULL,owner_started_at=NULL,
                    error_code=CASE WHEN kind='command' THEN 'job.interrupted_unknown' ELSE 'job.owner_missing' END,
                    finished_at=CASE WHEN kind='command' THEN ?5 ELSE finished_at END,
                    error_message=CASE WHEN kind='command' THEN 'Command owner ended; effects are uncertain and the command must not be replayed.' ELSE ?4 END,updated_at=?5,uncertain_at=?5
                    WHERE job_id=?1 AND (state='running' OR (kind='command' AND state='queued')) AND owner_pid IS ?2 AND owner_started_at IS ?3",
                    params![job_id,pid,started_at,"The owning process ended. Resume re-verifies permission before a new attempt.",now_ms()])?;
                let snapshot=if changed==1 {Some(read_snapshot(&tx,&job_id)?)}else{None};
                tx.commit()?;Ok(snapshot)
            })?;
            if let Some(snapshot)=changed {paused.push(snapshot);}
        }
        Ok(paused)
    }

    /// Resume a paused job: the caller re-verifies authorization first, then
    /// marks the job running again under this process.
    pub fn kernel_job_resume(&self, job_id: &str) -> Result<JobSnapshot, String> {
        let snapshot = self.kernel_job_snapshot(job_id)?;
        if snapshot.kind == "command" { return Err("Commands cannot be resumed or replayed".into()); }
        if snapshot.state != JobState::Paused { return Err("job is not paused".into()); }
        Ok(snapshot)
    }

    /// The declared parameters of a job, for an executor that must re-run it.
    pub fn kernel_job_params(&self, job_id: &str) -> Result<Value, String> {
        self.with_connection(|connection| {
            let body: String = connection.query_row(
                "SELECT params_json FROM kernel_jobs WHERE job_id=?1",
                params![job_id],
                |row| row.get(0),
            )?;
            serde_json::from_str(&body)
                .map_err(|error| rusqlite::Error::InvalidParameterName(format!("invalid job params: {error}")))
        })
    }
}

pub(super) fn read_snapshot(
    transaction: &rusqlite::Transaction<'_>,
    job_id: &str,
) -> rusqlite::Result<JobSnapshot> {
    transaction.query_row(
        "SELECT job_id, run_id, conversation_id, kind, state, cursor, progress_done,
                progress_total, result_ref, result_bytes, result_sha256, error_code,
                error_message, created_at, updated_at, finished_at, attempts, owner_pid,
                owner_started_at, cancel_requested_at, cancel_acknowledged_at
           FROM kernel_jobs WHERE job_id = ?1",
        params![job_id],
        |row| {
            let state: String = row.get(4)?;
            let owner_pid: Option<i64> = row.get(17)?;
            let owner_started_at: Option<i64> = row.get(18)?;
            let state = JobState::parse(&state).unwrap_or(JobState::Failed);
            Ok(JobSnapshot {
                job_id: row.get(0)?,
                run_id: row.get(1)?,
                conversation_id: row.get(2)?,
                kind: row.get(3)?,
                state,
                cursor: row.get(5)?,
                progress_done: row.get::<_, i64>(6)?.max(0) as u64,
                progress_total: row.get::<_, Option<i64>>(7)?.map(|total| total.max(0) as u64),
                result_ref: row.get(8)?,
                result_bytes: row.get::<_, Option<i64>>(9)?.map(|bytes| bytes.max(0) as u64),
                result_sha256: row.get(10)?,
                error_code: row.get(11)?,
                error_message: row.get(12)?,
                started_at: row.get(13)?,
                updated_at: row.get(14)?,
                finished_at: row.get(15)?,
                attempts: row.get::<_, i64>(16)?.max(0) as u32,
                resumable: row.get::<_,String>(3)? != "command" && (state == JobState::Paused
                    || (state == JobState::Running
                        && !owned_by_current_process(owner_pid, owner_started_at))),
                owner_pid,
                cancel_requested_at: row.get(19)?,
                cancel_acknowledged_at: row.get(20)?,
            })
        },
    )
}

/// The events the Host publishes when a job changes, so the UI can follow a job
/// without polling the database.
pub fn job_change_payload(snapshot: &JobSnapshot) -> Value {
    json!({
        "jobId": snapshot.job_id,
        "runId": snapshot.run_id,
        "conversationId": snapshot.conversation_id,
        "kind": snapshot.kind,
        "state": snapshot.state.as_str(),
        "progressDone": snapshot.progress_done,
        "progressTotal": snapshot.progress_total,
        "resultRef": snapshot.result_ref,
        "errorCode": snapshot.error_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> (Database, std::path::PathBuf, String) {
        let root = std::env::temp_dir().join(format!("fox-jobs-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let db = Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, None, None)
            .unwrap();
        let run_id = db.create_run(&conversation.id, "test", None).unwrap().run.id;
        (db, root, run_id)
    }

    fn start_request(run_id: &str, key: &str, params: Value) -> JobStartRequest {
        JobStartRequest {
            run_id: run_id.into(),
            kind: "attachment_compute".into(),
            idempotency_key: key.into(),
            params,
            deadline_ms: None,
            progress_total: Some(10),
        }
    }

    #[test]
    fn starting_the_same_request_twice_returns_one_job() {
        let (db, root, run_id) = database();
        let request = start_request(&run_id, "key-1", json!({"code": "1+1"}));
        let first = db.kernel_job_start(&request).unwrap();
        let job_id = first.snapshot().job_id.clone();
        assert!(matches!(first, JobStartOutcome::Created(_)));
        let second = db.kernel_job_start(&request).unwrap();
        assert!(matches!(second, JobStartOutcome::Existing(_)));
        assert_eq!(second.snapshot().job_id, job_id);
        assert_eq!(second.snapshot().attempts, 0, "a repeat must not re-run");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reusing_a_key_with_different_params_is_a_conflict_not_a_second_run() {
        let (db, root, run_id) = database();
        db.kernel_job_start(&start_request(&run_id, "key-1", json!({"code": "1+1"})))
            .unwrap();
        let conflict = db
            .kernel_job_start(&start_request(&run_id, "key-1", json!({"code": "2+2"})))
            .unwrap_err();
        assert!(conflict.contains("idempotency_conflict"), "{conflict}");
        assert_eq!(db.kernel_jobs_for_run(&run_id).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn key_order_does_not_fake_a_new_job() {
        let (db, root, run_id) = database();
        let first = start_request(&run_id, "key-1", json!({"a": 1, "b": 2}));
        let second = start_request(&run_id, "key-1", json!({"b": 2, "a": 1}));
        assert_eq!(first.params, json!({"a": 1, "b": 2}));
        db.kernel_job_start(&first).unwrap();
        assert!(matches!(
            db.kernel_job_start(&second).unwrap(),
            JobStartOutcome::Existing(_)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cancel_is_idempotent_and_terminal_state_is_written_once() {
        let (db, root, run_id) = database();
        let started = db
            .kernel_job_start(&start_request(&run_id, "key-1", json!({})))
            .unwrap();
        let job_id = started.snapshot().job_id.clone();
        db.kernel_job_mark_running(&job_id).unwrap();
        let cancelled = db.kernel_job_cancel(&job_id).unwrap();
        assert_eq!(cancelled.state, JobState::Cancelled);
        let cancelled_again = db.kernel_job_cancel(&job_id).unwrap();
        assert_eq!(cancelled_again.state, JobState::Cancelled);
        // A late successful settle cannot resurrect a cancelled job.
        let settled = db
            .kernel_job_settle(
                &job_id,
                JobState::Completed,
                Some(("result:1".into(), 4, "sha256:aa".into())),
                None,
            )
            .unwrap();
        assert_eq!(settled.state, JobState::Cancelled);
        assert!(settled.result_ref.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_dispatch_keys_accept_the_full_utf8_bound_and_bind_the_run() {
        let run = "r".repeat(1024);
        let call = "c".repeat(1024);
        let key = format!("job:{}", fox_engine_protocol::encode_dispatch_id(&run, &call).unwrap());
        validate_job_idempotency_key(&run, &key).unwrap();
        assert!(validate_job_idempotency_key("different-run", &key).is_err());
        assert!(validate_job_idempotency_key("r", "job:tool-dispatch:01:r:c").is_err());
        assert!(validate_job_idempotency_key("r", "job:tool-dispatch:1:r:").is_err());
        assert!(validate_job_idempotency_key("r", &"legacy".repeat(40)).is_err());
        validate_job_idempotency_key("r", &"x".repeat(200)).unwrap();

        let (db, root, run_id) = database();
        let key = format!("job:{}", fox_engine_protocol::encode_dispatch_id(&run_id, &"调用:".repeat(140)).unwrap());
        let first = db.kernel_job_start(&start_request(&run_id, &key, json!({}))).unwrap();
        let second = db.kernel_job_start(&start_request(&run_id, &key, json!({}))).unwrap();
        assert!(matches!(first, JobStartOutcome::Created(_)));
        assert!(matches!(second, JobStartOutcome::Existing(_)));
        assert_eq!(first.snapshot().job_id, second.snapshot().job_id);

        let job_id = first.snapshot().job_id.clone();
        drop(db);
        let reopened = Database::open(root.join("facts.db")).unwrap();
        let after_reopen = reopened
            .kernel_job_start(&start_request(&run_id, &key, json!({})))
            .unwrap();
        assert!(matches!(after_reopen, JobStartOutcome::Existing(_)));
        assert_eq!(after_reopen.snapshot().job_id, job_id);
        let v79_count: i64 = reopened
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version=79",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(v79_count, 1);
        drop(reopened);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn progress_is_monotonic_and_only_while_running() {
        let (db, root, run_id) = database();
        let started = db
            .kernel_job_start(&start_request(&run_id, "key-1", json!({})))
            .unwrap();
        let job_id = started.snapshot().job_id.clone();
        db.kernel_job_mark_running(&job_id).unwrap();
        let progressed = db.kernel_job_progress(&job_id, 4, Some(10), Some("c4")).unwrap();
        assert_eq!(progressed.progress_done, 4);
        assert_eq!(progressed.cursor.as_deref(), Some("c4"));
        // A lower reading never rewinds durable progress.
        let rewound = db.kernel_job_progress(&job_id, 2, None, None).unwrap();
        assert_eq!(rewound.progress_done, 4);
        db.kernel_job_cancel(&job_id).unwrap();
        assert!(db.kernel_job_progress(&job_id, 5, None, None).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_completed_job_must_carry_a_retrievable_result() {
        let (db, root, run_id) = database();
        let started = db
            .kernel_job_start(&start_request(&run_id, "key-1", json!({})))
            .unwrap();
        let job_id = started.snapshot().job_id.clone();
        db.kernel_job_mark_running(&job_id).unwrap();
        assert!(db
            .kernel_job_settle(&job_id, JobState::Completed, None, None)
            .is_err());
        let settled = db
            .kernel_job_settle(
                &job_id,
                JobState::Completed,
                Some(("fox-result://x".into(), 12, "sha256:bb".into())),
                None,
            )
            .unwrap();
        assert_eq!(settled.state, JobState::Completed);
        assert_eq!(settled.result_ref.as_deref(), Some("fox-result://x"));
        // A non-terminal settle is refused.
        assert!(db.kernel_job_settle(&job_id, JobState::Running, None, None).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_orphaned_running_job_becomes_resumable_instead_of_replaying() {
        let (db, root, run_id) = database();
        let started = db
            .kernel_job_start(&start_request(&run_id, "key-1", json!({})))
            .unwrap();
        let job_id = started.snapshot().job_id.clone();
        db.kernel_job_mark_running(&job_id).unwrap();
        // Simulate a foreign process owning the job.
        db.with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_jobs SET owner_pid=999999, owner_started_at=1 WHERE job_id=?1",
                params![job_id],
            )?;
            Ok(())
        })
        .unwrap();
        let reconciled = db.kernel_jobs_reconcile_orphans().unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].state, JobState::Paused);
        assert!(reconciled[0].resumable);
        // Resume puts it back under this process; nothing was re-executed.
        let resumed = db.kernel_job_resume(&job_id).unwrap();
        assert_eq!(resumed.state, JobState::Paused);
        assert_eq!(resumed.attempts, 1);
        let claimed = db.kernel_job_claim_attempt(&resumed.job_id, 2).unwrap();
        assert_eq!(claimed.state, JobState::Running);
        assert_eq!(claimed.attempts, 2);
        let _ = std::fs::remove_dir_all(root);
    }
}
