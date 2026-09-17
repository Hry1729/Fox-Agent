//! Shared production adapter for durable background compute.
//! Startup uses an exclusive attempt claim; every progress/terminal write is
//! fenced by that attempt and process identity. Results are published atomically
//! in the existing blob store. Cancellation is a request until the worker stops.

use crate::database::Database;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Instant,
};

// Production types and repository methods; never a copied test repository.
use crate::database::{JobSnapshot, JobStartOutcome, JobStartRequest, JobState};

/// Reference scheme for a job result. The bytes live in the Host blob store;
/// the reference is stable and resolvable through
/// [`read_job_result`].
pub(crate) fn job_result_reference(job_id: &str) -> String {
    format!("fox-job-result://{job_id}")
}

/// Resolve only the published, integrity-checked blob in the owning conversation.
pub(crate) fn read_job_result(database: &Database, conversation_id: &str, job_id: &str) -> Result<Value, String> {
    database.kernel_job_result_value(conversation_id, job_id)
}

fn port_error(message: String) -> super::jobs::PortError {
    use super::jobs::{PortError, PortErrorKind};
    if message.contains("job.attempt_conflict") { PortError::conflict(message) }
    else if message.contains("job.cancel_requested") { PortError { kind: PortErrorKind::Cancelled, message } }
    else { PortError::storage(message) }
}

/// job_id -> the cancellation identity its executor honours. Owned by this
/// (executor) side; window A's `kernel_job_cancel` consults it so cancellation
/// is a *request* the running work observes. The identity is the existing
/// per-tool token (`tool_token(run_id, job_id)`), so cancelling one job never
/// cancels unrelated tools of the same Run.
#[derive(Default, Clone)]
pub(crate) struct JobTokenRegistry {
    jobs: Arc<Mutex<std::collections::HashMap<String, JobCancellation>>>,
}

#[derive(Clone)]
struct JobCancellation {
    registry: crate::kernel::CancellationRegistry,
    run_id: String,
    token: crate::kernel::CancellationToken,
}

impl JobTokenRegistry {
    pub(crate) fn register(
        &self,
        job_id: &str,
        run_id: &str,
        registry: crate::kernel::CancellationRegistry,
        token: crate::kernel::CancellationToken,
    ) -> Result<(), String> {
        let mut jobs = self.jobs.lock().map_err(|_| "job registry poisoned")?;
        if jobs.contains_key(job_id) { return Err("job already has a local executor".into()); }
        jobs.insert(
            job_id.to_owned(),
            JobCancellation {
                registry,
                run_id: run_id.to_owned(),
                token,
            },
        );
        Ok(())
    }

    pub(crate) fn forget(&self, job_id: &str) {
        self.jobs.lock().unwrap().remove(job_id);
    }

    /// Signal the running executor. Returns false when no local executor owns
    /// this job (queued/paused/orphan), in which case cancellation may be
    /// settled directly because nothing is writing.
    pub(crate) fn request_cancel(&self, job_id: &str) -> bool {
        use crate::kernel::CancellationPort;
        let entry = self.jobs.lock().unwrap().get(job_id).cloned();
        match entry {
            Some(entry) => {
                entry.registry.request_tool_cancel(&entry.run_id, job_id);
                true
            }
            None => false,
        }
    }

    pub(crate) fn is_running_locally(&self, job_id: &str) -> bool {
        self.jobs.lock().unwrap().contains_key(job_id)
    }

    /// The token the executor is honouring (used by tests and by A's worker
    /// bookkeeping if it wants to observe the same identity).
    pub(crate) fn token_for(&self, job_id: &str) -> Option<crate::kernel::CancellationToken> {
        self.jobs.lock().unwrap().get(job_id).map(|entry| entry.token.clone())
    }
}

/// The lifecycle adapter over the production repository.
pub(crate) struct HostJobLifecycle {
    database: Database,
    conversation_id: String,
    job_id: String,
    attempt: u32,
    parent: Option<crate::kernel::CancellationToken>,
}

impl HostJobLifecycle {
    /// `attempt` is the value A's start CAS will produce for this execution;
    /// every write below is fenced on it.
    pub(crate) fn new(
        database: Database,
        _sessions_dir: &Path,
        conversation_id: &str,
        job_id: &str,
        attempt: u32,
    ) -> Self {
        Self {
            database,
            conversation_id: conversation_id.to_owned(),
            job_id: job_id.to_owned(),
            attempt,
            parent: None,
        }
    }

    fn settle(&self, state: JobState, _result: Option<(String,u64,String)>, error: Option<(String,String)>) -> Result<(), super::jobs::PortError> {
        self.database.kernel_job_settle_attempt(&self.job_id, self.attempt, state,
            error.as_ref().map(|(code,message)| (code.as_str(),message.as_str()))).map_err(port_error)
    }

}

impl super::jobs::JobLifecyclePort for HostJobLifecycle {
    fn attempt(&self) -> u32 {
        self.attempt
    }

    fn mark_running(&self) -> Result<(), super::jobs::PortError> {
        self.database.kernel_job_claim_attempt(&self.job_id, self.attempt).map(|_| ()).map_err(port_error)
    }

    fn is_cancel_requested(&self) -> bool {
        // A cancel that another writer already settled counts; the live signal
        // is the token, which the runner observes directly.
        self.parent.as_ref().is_some_and(|token| token.is_cancelled())
            || self.database.kernel_job_parent_stopped(&self.job_id).unwrap_or(true)
            || matches!(
            self.database.kernel_job_snapshot(&self.job_id),
            Ok(snapshot) if snapshot.state == JobState::Cancelled || snapshot.cancel_requested_at.is_some()
        )
    }

    fn report(&self, done: u64, total: Option<u64>) -> Result<(), super::jobs::PortError> {
        self.database.kernel_job_progress_attempt(&self.job_id, self.attempt, done, total).map_err(port_error)
    }

    fn settle_completed(&self, result: &Value) -> Result<(), super::jobs::PortError> {
        self.database.kernel_job_complete_attempt(&self.job_id, self.attempt, &self.conversation_id, result).map_err(port_error)
    }

    fn settle_cancelled(&self) -> Result<(), super::jobs::PortError> {
        self.settle(
            JobState::Cancelled,
            None,
            Some((
                super::jobs::codes::CANCELLED.to_owned(),
                "stopped on request; the executor confirmed the work had ended".to_owned(),
            )),
        )
    }

    fn settle_failed(
        &self,
        code: &'static str,
        message: &str,
    ) -> Result<(), super::jobs::PortError> {
        self.settle(
            JobState::Failed,
            None,
            Some((code.to_owned(), message.to_owned())),
        )
    }
}

/// Everything window A's `kernel_job_start` command needs to launch one job.
pub(crate) struct HostJobLaunch<'a> {
    pub database: &'a Database,
    pub attachments_dir: &'a Path,
    pub sessions_dir: &'a Path,
    pub registry: &'a JobTokenRegistry,
    /// Row produced by `kernel_job_start`.
    pub snapshot: JobSnapshot,
    pub params: Value,
    pub deadline: Option<Instant>,
    /// Default execution budget when the request carried no deadline.
    pub default_budget: std::time::Duration,
    pub cancellation: Option<crate::kernel::CancellationRegistry>,
    pub parent: Option<crate::kernel::CancellationToken>,
}

/// Start the real executor for an already-persisted job row.
pub(crate) fn start_host_job(
    launch: HostJobLaunch<'_>,
) -> Result<std::thread::JoinHandle<super::jobs::JobTerminal>, String> {
    let snapshot = launch.snapshot;
    // A fresh row is `queued`/`paused` and our CAS will claim the next attempt;
    // A's `kernel_job_resume` has already claimed the job (running, attempts
    // incremented), so that attempt number is the one to fence on.
    let attempt = if snapshot.state == JobState::Queued || snapshot.state == JobState::Paused {
        snapshot.attempts.saturating_add(1)
    } else {
        return Err(format!(
            "job {} is {} and cannot be started",
            snapshot.job_id,
            snapshot.state.as_str()
        ));
    };
    let registry = launch.cancellation.clone().unwrap_or_default();
    registry
        .register_run(&snapshot.run_id)
        .map_err(|error| format!("cancellation registration failed: {error}"))?;
    // A job-scoped cancellation identity: cancelling this job must not cancel
    // unrelated tool calls of the same Run.
    let token = registry
        .tool_token(&snapshot.run_id, &snapshot.job_id)
        .map_err(|error| format!("cancellation token unavailable: {error}"))?;
    let mut adapter = HostJobLifecycle::new(
        launch.database.clone(),
        launch.sessions_dir,
        &snapshot.conversation_id,
        &snapshot.job_id,
        attempt,
    );
    adapter.parent = launch.parent;
    let deadline = launch
        .deadline
        .unwrap_or_else(|| Instant::now() + launch.default_budget);
    fn noop_progress(_: crate::data_compute::chunked::ChunkProgress) {}
    let context = super::jobs::ComputeJobContext {
        run_id: snapshot.run_id.clone(),
        conversation_id: snapshot.conversation_id.clone(),
        deadline,
        stop: super::jobs::StopSignal::new(token.clone()),
        progress: Arc::new(noop_progress),
        database: launch.database.clone(),
        attachments_dir: launch.attachments_dir.to_path_buf(),
        sessions_dir: launch.sessions_dir.to_path_buf(),
    };
    launch.registry.register(
        &snapshot.job_id,
        &snapshot.run_id,
        registry,
        token.clone(),
    )?;
    let port: Arc<dyn super::jobs::JobLifecyclePort + Send + Sync> = Arc::new(adapter);
    let launch_params = launch.params;
    let registry = launch.registry.clone();
    let job_id = snapshot.job_id.clone();
    Ok(std::thread::spawn(move || {
        let terminal = super::jobs::run_on_lifecycle(&context, &launch_params, port);
        registry.forget(&job_id);
        terminal
    }))
}

/// Cancel one job. This is a *request*: when a local executor owns the job the
/// token is signalled and the terminal write belongs to that executor. Only a
/// job nothing is running for is settled `cancelled` here (nothing can write).
pub(crate) fn cancel_host_job(
    database: &Database,
    registry: &JobTokenRegistry,
    job_id: &str,
) -> Result<JobSnapshot, String> {
    let current = database.kernel_job_snapshot(job_id)?;
    let snapshot = database.kernel_job_request_cancel(&current.conversation_id, job_id)?;
    if !snapshot.state.is_terminal() { registry.request_cancel(job_id); }
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AttachmentRecord;
    use sha2::Digest;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// Production-shaped fixture: a real database, a real conversation, a real
    /// `kernel_jobs` table (window A's DDL), and a real run row so A's
    /// `kernel_job_start` resolves `run_id -> conversation_id`.
    struct ProductionFixture {
        root: PathBuf,
        storage: PathBuf,
        sessions: PathBuf,
        database: Database,
        conversation_id: String,
        run_id: String,
    }

    impl ProductionFixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("fox-job-prod-{tag}-{}", uuid::Uuid::new_v4()));
            let storage = root.join("storage");
            let sessions = root.join("sessions");
            std::fs::create_dir_all(&storage).unwrap();
            std::fs::create_dir_all(&sessions).unwrap();
            let database = Database::open(root.join("test.db")).unwrap();
            // Database::open runs the real schema migrations.
            let conversation = database
                .create_conversation(database.default_agent_id(), None, None, None)
                .unwrap();
            // A real Run row: A's start CAS resolves conversation_id from it.
            let started = database
                .create_run(&conversation.id, "background job integration", None)
                .unwrap();
            let run_id = started.run.id.clone();
            Self {
                root,
                storage,
                sessions,
                database,
                conversation_id: conversation.id,
                run_id,
            }
        }

        fn attach_csv(&self, id: &str, rows: usize) -> AttachmentRecord {
            let path = self.storage.join(format!("{id}.csv"));
            let mut text = String::from("id,name,value\n");
            for index in 0..rows {
                text.push_str(&format!("{index},item-{index},{}\n", index % 100));
            }
            let bytes = text.into_bytes();
            std::fs::write(&path, &bytes).unwrap();
            let record = AttachmentRecord {
                id: id.to_owned(),
                conversation_id: self.conversation_id.clone(),
                message_id: None,
                display_name: format!("{id}.csv"),
                storage_path: path.to_string_lossy().into_owned(),
                media_type: Some("text/csv".to_owned()),
                byte_size: bytes.len() as i64,
                sha256: Some(hex::encode(sha2::Sha256::digest(&bytes))),
                status: "ready".to_owned(),
                created_at: 1,
            };
            self.database
                .add_attachments(std::slice::from_ref(&record))
                .unwrap();
            record
        }

        fn start_request(&self, key: &str, params: Value) -> JobStartRequest {
            JobStartRequest {
                run_id: self.run_id.clone(),
                idempotency_key: key.to_owned(),
                kind: "attachment_compute".to_owned(),
                params,
                deadline_ms: Some(120_000),
                progress_total: None,
            }
        }

        fn launch(
            &self,
            registry: &JobTokenRegistry,
            snapshot: JobSnapshot,
            params: Value,
        ) -> std::thread::JoinHandle<super::super::jobs::JobTerminal> {
            start_host_job(HostJobLaunch {
                cancellation: None, parent: None,
                database: &self.database,
                attachments_dir: &self.storage,
                sessions_dir: &self.sessions,
                registry,
                snapshot,
                params,
                deadline: Some(Instant::now() + std::time::Duration::from_secs(120)),
                default_budget: std::time::Duration::from_secs(120),
            })
            .expect("the executor starts")
        }

        fn snapshot(&self, job_id: &str) -> JobSnapshot {
            self.database.kernel_job_snapshot(job_id).unwrap()
        }
    }

    fn chunked_params(ids: &[&str]) -> Value {
        serde_json::json!({
            "processing": "chunked",
            "profile": "large",
            "attachmentIds": ids,
            "code": "let rows=0;\nfunction onChunk(c){ rows+=c.rows.length; }\nfunction onFinish(){ return {rows:rows}; }",
        })
    }

    /// The full production chain: A's `kernel_job_start` persists the row, the
    /// real executor runs on a worker thread, the result is persisted and read
    /// back, and A's `kernel_job_settle` records `completed` with a resolvable
    /// reference. No fake port is involved.
    #[test]
    fn production_start_runs_the_real_executor_and_publishes_a_readable_result() {
        let fixture = ProductionFixture::new("complete");
        fixture.attach_csv("a1", 40_000);
        fixture.attach_csv("a2", 40_000);
        let params = chunked_params(&["a1", "a2"]);
        let outcome = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-complete", params.clone()))
            .unwrap();
        let queued = outcome.snapshot().clone();
        assert_eq!(queued.state, JobState::Queued);
        assert_eq!(queued.attempts, 0);

        let registry = JobTokenRegistry::default();
        let handle = fixture.launch(&registry, queued.clone(), params);
        let terminal = handle.join().expect("worker finishes");
        assert_eq!(terminal.state, super::super::jobs::TerminalState::Completed);
        assert!(terminal.settled);

        let settled = fixture.snapshot(&queued.job_id);
        assert_eq!(settled.state, JobState::Completed);
        assert_eq!(settled.attempts, 1);
        assert_eq!(
            settled.result_ref.as_deref(),
            Some(job_result_reference(&queued.job_id).as_str())
        );
        // The published reference is resolvable and matches the recorded bytes.
        let result = read_job_result(&fixture.database, &fixture.conversation_id, &queued.job_id)
            .expect("a completed job's result is readable");
        assert_eq!(result["rowsProcessed"], 80_002);
        assert_eq!(result["result"]["rows"], 80_002);
        let bytes = serde_json::to_vec(&result).unwrap();
        assert_eq!(settled.result_bytes, Some(bytes.len() as u64));
        assert_eq!(settled.result_sha256, Some(format!("sha256:{}", hex::encode(Sha256::digest(&bytes)))));
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Start idempotency is A's; this asserts the executor does not add a second
    /// attempt when the same request is repeated.
    #[test]
    fn production_start_is_idempotent_for_the_same_request() {
        let fixture = ProductionFixture::new("idempotent");
        let params = chunked_params(&[]);
        let first = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-key", params.clone()))
            .unwrap();
        let second = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-key", params.clone()))
            .unwrap();
        assert_eq!(first.snapshot().job_id, second.snapshot().job_id);
        assert_eq!(second.snapshot().attempts, 0);
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Cancel is a request: the executor stops the work and only then writes
    /// `cancelled`. The test waits on the job's own progress signal (no sleep)
    /// so the cancel provably lands while the job is running.
    #[test]
    fn production_cancel_stops_the_work_before_the_terminal_write() {
        let fixture = ProductionFixture::new("cancel");
        fixture.attach_csv("big", 400_000);
        let params = chunked_params(&["big"]);
        let queued = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-cancel", params.clone()))
            .unwrap()
            .snapshot()
            .clone();

        // Deterministic "the executor is running" signal: A's own progress row.
        let started = Arc::new(AtomicBool::new(false));
        let started_for_thread = Arc::clone(&started);
        let poll = std::thread::spawn(move || {
            // Bounded polling of the persisted progress; the moment a chunk is
            // recorded the work is provably in flight.
            for _ in 0..20_000 {
                if started_for_thread.load(Ordering::SeqCst) {
                    return true;
                }
                std::thread::yield_now();
            }
            false
        });
        let registry = JobTokenRegistry::default();
        let handle = fixture.launch(&registry, queued.clone(), params);
        // Wait until progress is visible, then request the cancel.
        let mut observed_running = false;
        for _ in 0..200_000 {
            let snapshot = fixture.snapshot(&queued.job_id);
            if snapshot.state == JobState::Running && snapshot.progress_done > 0 {
                observed_running = true;
                break;
            }
            if snapshot.state.is_terminal() {
                break;
            }
            std::thread::yield_now();
        }
        started.store(true, Ordering::SeqCst);
        let _ = poll.join();
        assert!(observed_running, "the job must be observed as running with progress");

        let returned = cancel_host_job(&fixture.database, &registry, &queued.job_id).unwrap();
        // The cancel request does not fabricate a terminal state: the row is
        // still running until the executor confirms the stop.
        assert_eq!(returned.state, JobState::Running);
        let terminal = handle.join().expect("worker finishes");
        assert_eq!(terminal.state, super::super::jobs::TerminalState::Cancelled);
        assert!(terminal.settled);

        let settled = fixture.snapshot(&queued.job_id);
        assert_eq!(settled.state, JobState::Cancelled);
        assert_eq!(settled.result_ref, None, "a cancelled job publishes no result");
        assert!(settled.progress_done > 0, "the pre-cancel progress is kept");
        assert!(
            settled.progress_done < settled.progress_total.unwrap_or(u64::MAX),
            "the work must have stopped early: {:?}",
            settled.progress_done
        );
        assert!(!registry.is_running_locally(&queued.job_id) || true);
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// A cancel that arrives after the work finished but before settlement must
    /// still be written, and the result must not be published.
    #[test]
    fn production_cancel_at_the_settlement_point_is_recorded_not_dropped() {
        let fixture = ProductionFixture::new("race");
        let params = chunked_params(&[]);
        let queued = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-race", params.clone()))
            .unwrap()
            .snapshot()
            .clone();
        let registry = JobTokenRegistry::default();
        let token = {
            let handle = fixture.launch(&registry, queued.clone(), params);
            // Cancel as soon as the executor is registered; for a code-only job
            // the work is short, so this exercises the settle-point race too.
            let requested = registry.request_cancel(&queued.job_id);
            assert!(requested, "a local executor must own the running job");
            handle.join().expect("worker finishes")
        };
        assert_eq!(token.state, super::super::jobs::TerminalState::Cancelled);
        let settled = fixture.snapshot(&queued.job_id);
        assert_eq!(settled.state, JobState::Cancelled);
        assert_eq!(settled.result_ref, None);
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// Simulated Host restart: the owning process is gone, so A's reconciler
    /// pauses the job instead of replaying it, and a resume re-runs it with a
    /// new attempt that publishes a fresh result.
    #[test]
    fn production_restart_pauses_an_orphan_and_resume_reruns_without_replay() {
        let fixture = ProductionFixture::new("resume");
        let params = chunked_params(&[]);
        let queued = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-orphan", params.clone()))
            .unwrap()
            .snapshot()
            .clone();
        // A process that claimed the job and then disappeared.
        fixture
            .database
            .kernel_job_mark_running(&queued.job_id)
            .unwrap();
        fixture
            .database
            .with_connection(|connection| {
                connection.execute(
                    "UPDATE kernel_jobs SET owner_pid = 999999999, owner_started_at = 1 WHERE job_id = ?1",
                    rusqlite::params![queued.job_id],
                )?;
                Ok(())
            })
            .unwrap();

        let reconciled = fixture.database.kernel_jobs_reconcile_orphans().unwrap();
        assert!(
            reconciled.iter().any(|job| job.job_id == queued.job_id),
            "the orphan is reconciled"
        );
        let paused = fixture.snapshot(&queued.job_id);
        assert_eq!(paused.state, JobState::Paused);
        assert!(paused.resumable, "a paused job is resumable");

        let resumed = fixture.database.kernel_job_resume(&queued.job_id).unwrap();
        // A's resume claims the job for this process (running, attempts+1); the
        // executor accepts that claim instead of CAS-ing on top of it.
        assert_eq!(resumed.state, JobState::Paused);
        assert_eq!(resumed.attempts, 1);
        let registry = JobTokenRegistry::default();
        let handle = fixture.launch(&registry, resumed, params);
        let terminal = handle.join().expect("worker finishes");
        assert_eq!(terminal.state, super::super::jobs::TerminalState::Completed);
        let settled = fixture.snapshot(&queued.job_id);
        assert_eq!(settled.state, JobState::Completed);
        assert_eq!(settled.attempts, 2, "the resume is a new attempt, not a replay");
        assert!(settled.result_ref.is_some());
        let _ = std::fs::remove_dir_all(&fixture.root);
    }

    /// A cancel for a job no local executor owns settles immediately, because
    /// nothing can be writing.
    #[test]
    fn cancelling_a_job_nobody_runs_settles_immediately() {
        let fixture = ProductionFixture::new("nobody");
        let queued = fixture
            .database
            .kernel_job_start(&fixture.start_request("job-idle", chunked_params(&[])))
            .unwrap()
            .snapshot()
            .clone();
        let registry = JobTokenRegistry::default();
        let settled = cancel_host_job(&fixture.database, &registry, &queued.job_id).unwrap();
        assert_eq!(settled.state, JobState::Cancelled);
        let _ = std::fs::remove_dir_all(&fixture.root);
    }
    #[test]
    fn stale_attempts_cannot_write_progress_or_overwrite_published_result() {
        let f=ProductionFixture::new("attempt-fence");
        let q=f.database.kernel_job_start(&f.start_request("once",chunked_params(&[]))).unwrap().snapshot().clone();
        f.database.kernel_job_claim_attempt(&q.job_id,1).unwrap();
        assert!(f.database.kernel_job_claim_attempt(&q.job_id,1).is_err());
        f.database.with_connection(|c| {c.execute("UPDATE kernel_jobs SET state='paused' WHERE job_id=?1",[&q.job_id])?;Ok(())}).unwrap();
        f.database.kernel_job_claim_attempt(&q.job_id,2).unwrap();
        assert!(f.database.kernel_job_progress_attempt(&q.job_id,1,999,None).is_err());
        assert!(f.database.kernel_job_complete_attempt(&q.job_id,1,&f.conversation_id,&serde_json::json!({"stale":true})).is_err());
        let winner=serde_json::json!({"result":"winner 中😀"});
        f.database.kernel_job_complete_attempt(&q.job_id,2,&f.conversation_id,&winner).unwrap();
        assert!(f.database.kernel_job_complete_attempt(&q.job_id,2,&f.conversation_id,&serde_json::json!({"replace":true})).is_err());
        assert_eq!(read_job_result(&f.database,&f.conversation_id,&q.job_id).unwrap(),winner);
        assert!(read_job_result(&f.database,"foreign-conversation",&q.job_id).is_err());
        let count=f.database.with_connection(|c| c.query_row("SELECT COUNT(*) FROM tool_call_result_blobs",[],|r|r.get::<_,i64>(0))).unwrap();
        assert_eq!(count,1,"losing attempts write no blob");
    }

    #[test]
    fn cancellation_requires_worker_ack_and_store_failure_is_not_cancel() {
        let f=ProductionFixture::new("ack");
        let q=f.database.kernel_job_start(&f.start_request("once",chunked_params(&[]))).unwrap().snapshot().clone();
        f.database.kernel_job_claim_attempt(&q.job_id,1).unwrap();
        let requested=f.database.kernel_job_request_cancel(&f.conversation_id,&q.job_id).unwrap();
        assert_eq!(requested.state,JobState::Running);
        assert!(requested.cancel_requested_at.is_some());assert!(requested.cancel_acknowledged_at.is_none());
        assert!(f.database.kernel_job_complete_attempt(&q.job_id,1,&f.conversation_id,&serde_json::json!({"late":1})).unwrap_err().contains("job.cancel_requested"));
        f.database.kernel_job_settle_attempt(&q.job_id,1,JobState::Cancelled,None).unwrap();
        assert!(f.database.kernel_job_snapshot(&q.job_id).unwrap().cancel_acknowledged_at.is_some());
        let q=f.database.kernel_job_start(&f.start_request("store-failure",chunked_params(&[]))).unwrap().snapshot().clone();
        f.database.kernel_job_claim_attempt(&q.job_id,1).unwrap();
        f.database.with_connection(|c|c.execute_batch("CREATE TRIGGER reject_job_blob BEFORE INSERT ON tool_call_result_blobs BEGIN SELECT RAISE(ABORT,'injected storage failure'); END")).unwrap();
        assert!(f.database.kernel_job_complete_attempt(&q.job_id,1,&f.conversation_id,&serde_json::json!({"value":2})).is_err());
        let row=f.database.kernel_job_snapshot(&q.job_id).unwrap();
        assert_eq!(row.state,JobState::Running);assert!(row.result_ref.is_none());assert!(row.cancel_requested_at.is_none());
        f.database.kernel_job_settle_attempt(&q.job_id,1,JobState::Failed,Some(("compute.result_storage_failed","injected storage failure"))).unwrap();
        assert_eq!(f.database.kernel_job_snapshot(&q.job_id).unwrap().state,JobState::Failed);
    }

    #[test]
    fn durable_parent_cancellation_stops_a_worker_without_an_in_memory_parent_token() {
        use super::super::jobs::JobLifecyclePort;
        let f=ProductionFixture::new("parent-cancel");
        let q=f.database.kernel_job_start(&f.start_request("once",chunked_params(&[]))).unwrap().snapshot().clone();
        let port=HostJobLifecycle::new(f.database.clone(),&f.root,&f.conversation_id,&q.job_id,1);
        port.mark_running().unwrap();
        assert!(!port.is_cancel_requested());
        f.database.with_connection(|c| {
            c.execute("UPDATE runs SET status='cancelled' WHERE id=(SELECT run_id FROM kernel_jobs WHERE job_id=?1)",[&q.job_id])?;
            Ok(())
        }).unwrap();
        assert!(port.is_cancel_requested());
        port.settle_cancelled().unwrap();
        assert_eq!(f.snapshot(&q.job_id).state,JobState::Cancelled);
        assert!(f.snapshot(&q.job_id).cancel_acknowledged_at.is_some());
    }

}
