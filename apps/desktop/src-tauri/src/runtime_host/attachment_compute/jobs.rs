//! Background-compute job executor (CONTRACTS §4, window C's executor side).
//!
//! The Host job lifecycle owned by window A keeps the state machine,
//! idempotency keys, persistence and recovery. This module owns only the
//! computation: it runs on the lifecycle's worker thread, inside the same
//! conversation authorization and QuickJS bounds as the synchronous tool,
//! reporting progress and honoring cancellation.
//!
//! Executor guarantees the lifecycle can rely on:
//!
//! * **No hidden replay.** Every execution snapshots inputs into a fresh
//!   isolated workspace (uuid) and writes outputs only there. Re-running a
//!   job never appends to a previous execution's outputs, and nothing here
//!   writes to the user's project or to the original attachments.
//! * **Cancellation is real and confirmed.** A cancel request stops the work
//!   first; only then does the executor write the `cancelled` terminal. A
//!   cancel that arrives while the work is already finishing still wins over
//!   publishing the result, and it is always recorded — never silently
//!   dropped.
//! * **Every path settles explicitly.** Start loss, pre-start cancel,
//!   in-flight cancel, post-success cancel race, result-storage failure,
//!   panic and late reports from a superseded attempt each produce a defined
//!   outcome, and no path reports a storage failure as a user cancellation.
//! * **Progress is monotonic.** `rows_done` only increases within one
//!   execution and `rows_total` is the measured extent before JavaScript
//!   starts.
//! * **Budget separation.** The lifecycle hands in the job's execution
//!   deadline; approval waiting and status polling never touch it.

use crate::database::Database;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Instant,
};

/// Stable error codes surfaced to the lifecycle for `kernel_jobs.error_code`.
pub(crate) mod codes {
    pub const CANCELLED: &str = "compute.cancelled";
    pub const TIMED_OUT: &str = "compute.timed_out";
    pub const INVALID_PARAMS: &str = "compute.invalid_params";
    pub const EXECUTION_FAILED: &str = "compute.execution_failed";
    /// The computation finished but its result could not be persisted and
    /// verified, so `completed` would be a lie. Never a user cancellation.
    pub const RESULT_STORAGE_FAILED: &str = "compute.result_storage_failed";
    /// This attempt no longer owned the job (a newer attempt or a concurrent
    /// terminal write won). The superseded attempt stops without settling.
    pub const SUPERSEDED: &str = "compute.superseded";
    /// The terminal write itself failed (usually a transient store failure);
    /// the job must not be reported as cancelled or completed.
    pub const SETTLE_FAILED: &str = "compute.settle_failed";
}

/// Why a lifecycle call failed. The runner must never confuse a storage or
/// database failure with a user cancellation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PortErrorKind {
    /// The job is not in a state this attempt owns: start CAS lost, another
    /// attempt took over, or the row is already terminal.
    Conflict,
    /// A cancel was already recorded for this job by another writer.
    Cancelled,
    /// The result could not be persisted and read back.
    Storage,
    /// Any other lifecycle failure (database busy, I/O, ...).
    Other,
}

#[derive(Debug)]
pub(crate) struct PortError {
    pub kind: PortErrorKind,
    pub message: String,
}

impl PortError {
    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self {
            kind: PortErrorKind::Conflict,
            message: message.into(),
        }
    }

    pub(crate) fn cancelled(message: impl Into<String>) -> Self {
        Self {
            kind: PortErrorKind::Cancelled,
            message: message.into(),
        }
    }

    pub(crate) fn storage(message: impl Into<String>) -> Self {
        Self {
            kind: PortErrorKind::Storage,
            message: message.into(),
        }
    }

    pub(crate) fn other(message: impl Into<String>) -> Self {
        Self {
            kind: PortErrorKind::Other,
            message: message.into(),
        }
    }
}

/// Stop conditions for one execution. Owns its handles so it can be moved into
/// the `'static` cancellation closure the compute engine requires.
#[derive(Clone)]
pub(crate) struct StopSignal {
    /// Host-visible user/Host cancellation (the token the lifecycle wires to
    /// `kernel_job_cancel`).
    pub cancellation: crate::kernel::CancellationToken,
    /// Runner-owned fence: true once this attempt learns it no longer owns the
    /// job. A fenced execution stops working and settles nothing, because the
    /// terminal state belongs to whoever superseded it.
    pub fence: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
    pub requested: Option<std::sync::Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl StopSignal {
    pub(crate) fn new(cancellation: crate::kernel::CancellationToken) -> Self {
        Self {
            cancellation,
            fence: None,
            requested: None,
        }
    }

    pub(crate) fn with_fence(
        cancellation: crate::kernel::CancellationToken,
        fence: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            cancellation,
            fence: Some(fence),
            requested: None,
        }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.cancellation.is_cancelled() || self.requested.as_ref().is_some_and(|read| read())
    }

    pub(crate) fn stopped(&self) -> bool {
        self.cancelled() || self.fence.as_ref().is_some_and(|fence| fence())
    }
}

/// Everything the executor needs from the lifecycle for one execution.
///
/// Owns its handles (database clone, owned paths, `Arc` progress sink) so the
/// lifecycle can move it into the worker thread without leaking or borrowing
/// Host state.
#[derive(Clone)]
pub(crate) struct ComputeJobContext {
    pub run_id: String,
    pub conversation_id: String,
    /// Hard execution deadline derived from `JobStartRequest.deadline_ms`
    /// (or the lifecycle's default for the job kind). Never extended
    /// silently: hitting it ends the execution as `compute.timed_out`.
    pub deadline: Instant,
    /// Cancellation / supersession signal.
    pub stop: StopSignal,
    /// Progress sink; the lifecycle persists the latest snapshot.
    pub progress: std::sync::Arc<dyn Fn(crate::data_compute::chunked::ChunkProgress) + Send + Sync>,
    /// Database + storage roots the executor is authorized to use.
    pub database: Database,
    pub attachments_dir: std::path::PathBuf,
    pub sessions_dir: std::path::PathBuf,
}

/// Classified outcome so the lifecycle can map failures onto `JobState`
/// without parsing messages.
#[derive(Debug)]
pub(crate) struct ComputeJobOutcome {
    pub result: Value,
    pub rows_processed: u64,
    pub duration_ms: u64,
}

#[derive(Debug)]
pub(crate) struct ComputeJobError {
    pub code: &'static str,
    pub message: String,
}

/// Terminal classification of one execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TerminalState {
    Completed,
    Failed,
    Cancelled,
    /// This attempt never owned the job (start CAS lost), so it wrote nothing
    /// and the row still belongs to the lifecycle.
    NotStarted,
    /// A newer attempt (or a concurrent terminal write) owns the outcome; this
    /// attempt stopped without settling.
    Superseded,
}

/// What the runner settled, for the lifecycle's ledger.
#[derive(Debug)]
pub(crate) struct JobTerminal {
    pub state: TerminalState,
    pub error_code: Option<&'static str>,
    pub error_message: Option<String>,
    /// True when this runner wrote the terminal state itself.
    pub settled: bool,
}

impl JobTerminal {
    fn completed() -> Self {
        Self {
            state: TerminalState::Completed,
            error_code: None,
            error_message: None,
            settled: true,
        }
    }

    fn cancelled() -> Self {
        Self {
            state: TerminalState::Cancelled,
            error_code: None,
            error_message: None,
            settled: true,
        }
    }

    fn failed(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            state: TerminalState::Failed,
            error_code: Some(code),
            error_message: Some(message.into()),
            settled: true,
        }
    }

    fn not_started(kind: PortErrorKind, message: impl Into<String>) -> Self {
        let code = match kind {
            PortErrorKind::Conflict => codes::SUPERSEDED,
            PortErrorKind::Cancelled => codes::CANCELLED,
            PortErrorKind::Storage | PortErrorKind::Other => codes::EXECUTION_FAILED,
        };
        Self {
            state: TerminalState::NotStarted,
            error_code: Some(code),
            error_message: Some(message.into()),
            settled: false,
        }
    }

    fn superseded(message: impl Into<String>) -> Self {
        Self {
            state: TerminalState::Superseded,
            error_code: Some(codes::SUPERSEDED),
            error_message: Some(message.into()),
            settled: false,
        }
    }

    /// The runner tried to settle but the store refused/failed; nothing was
    /// written by this attempt. Distinct from `Failed`, which means the
    /// failure *was* recorded.
    fn unsettled(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            state: TerminalState::Failed,
            error_code: Some(code),
            error_message: Some(message.into()),
            settled: false,
        }
    }
}

/// The executor-side half of the job lifecycle (CONTRACTS §4/§11). Window A's
/// `kernel_jobs` repository implements this port; the runner below guarantees
/// settle-once, monotonic progress, attempt fencing, and that a cancelled
/// execution's owned work stops before the terminal write. Cancellation of any
/// child process trees is the executor's duty (attachment compute runs
/// in-process, so the QuickJS interrupt covers it; process-spawning executors
/// must call `tool_host::terminate_process_tree`).
pub(crate) trait JobLifecyclePort: Send + Sync {
    /// Attempt fencing token for this execution. The adapter must fence every
    /// `report`/`settle` on `(job_id, attempt)` so a late report from an older
    /// attempt cannot touch a newer one.
    fn attempt(&self) -> u32;
    /// The start CAS: only the attempt that wins may run or settle. `Conflict`
    /// means another owner already has it.
    fn mark_running(&self) -> Result<(), PortError>;
    /// True once a cancel was requested for this job, whether through the
    /// token the lifecycle signalled or a flag the store recorded.
    fn is_cancel_requested(&self) -> bool;
    /// Monotonic progress; the port rejects regressions and fenced attempts.
    fn report(&self, done: u64, total: Option<u64>) -> Result<(), PortError>;
    /// Persist the result, verify it is readable, and only then write the
    /// `completed` terminal. Returns `Storage` when the result cannot be
    /// persisted/read back, `Conflict` when another writer already settled.
    fn settle_completed(&self, result: &Value) -> Result<(), PortError>;
    /// Confirm a cancellation that the executor has already stopped for.
    fn settle_cancelled(&self) -> Result<(), PortError>;
    /// Record a failure terminal (`compute.*` code).
    fn settle_failed(&self, code: &'static str, message: &str) -> Result<(), PortError>;
}

/// Deterministic seam for the "cancel arrives exactly at the settlement point"
/// tests. It runs on the execution thread after the work returned and before
/// the terminal decision, so a test can schedule the interleaving exactly
/// instead of sleeping and hoping.
#[cfg(test)]
pub(crate) mod test_hooks {
    use std::sync::Mutex;

    type Hook = Box<dyn FnMut() + Send>;

    static AT_SETTLE: Mutex<Option<Hook>> = Mutex::new(None);

    pub(crate) fn set_at_settle(hook: Option<Hook>) {
        *AT_SETTLE.lock().unwrap() = hook;
    }

    pub(crate) fn run_at_settle() {
        if let Some(hook) = AT_SETTLE.lock().unwrap().as_mut() {
            hook();
        }
    }
}

/// Run one job to its terminal state on the caller's thread.
///
/// Decision tree (every branch settles explicitly or explains why it must not):
///
/// 1. start CAS lost → `NotStarted`, nothing written;
/// 2. stop already requested before work → work never starts, `settle_cancelled`;
/// 3. work returns cancelled/timed out/other error → the matching terminal;
/// 4. work succeeds but a cancel was requested meanwhile → `settle_cancelled`
///    (the work has already stopped, so the confirmation is truthful);
/// 5. work succeeds → `settle_completed`; if the store reports `Conflict` a
///    concurrent winner keeps the row (`Superseded`), and if it reports
///    `Storage` the job is failed loudly — never reported as a cancellation;
/// 6. this attempt was fenced mid-flight → stop and write nothing;
/// 7. a panic inside the executor is caught and settled as a failure.
pub(crate) fn run_on_lifecycle(
    context: &ComputeJobContext,
    params: &Value,
    port: Arc<dyn JobLifecyclePort + Send + Sync>,
) -> JobTerminal {
    // 1) Start CAS first: only the winning attempt may run or settle.
    if let Err(error) = port.mark_running() {
        return JobTerminal::not_started(error.kind, error.message);
    }

    let fence = Arc::new(AtomicBool::new(false));
    let fence_reason = Arc::new(std::sync::Mutex::new(String::new()));
    // A refused report means this attempt no longer owns the job: stop the
    // work and never settle, so a late old attempt cannot overwrite the newer
    // attempt's outcome. Progress is both pushed to the lifecycle's UI sink and
    // persisted; only the persisted write can fence the attempt.
    let progress: Arc<dyn Fn(crate::data_compute::chunked::ChunkProgress) + Send + Sync> = {
        let port = Arc::clone(&port);
        let fence = Arc::clone(&fence);
        let fence_reason = Arc::clone(&fence_reason);
        let ui_sink = Arc::clone(&context.progress);
        Arc::new(move |chunk: crate::data_compute::chunked::ChunkProgress| {
            ui_sink(chunk);
            if let Err(error) = port.report(chunk.rows_done, chunk.rows_total) {
                match error.kind {
                    PortErrorKind::Conflict | PortErrorKind::Cancelled => {
                        *fence_reason.lock().unwrap() = error.message;
                        fence.store(true, Ordering::SeqCst);
                    }
                    // A transient progress failure is not a terminal fact: the
                    // final settle decides the outcome.
                    PortErrorKind::Storage | PortErrorKind::Other => {}
                }
            }
        })
    };
    let fence_fn: Arc<dyn Fn() -> bool + Send + Sync> = {
        let fence = Arc::clone(&fence);
        Arc::new(move || fence.load(Ordering::SeqCst))
    };
    let mut stop = StopSignal::with_fence(context.stop.cancellation.clone(), fence_fn);
    let cancellation_port = Arc::clone(&port);
    stop.requested = Some(Arc::new(move || cancellation_port.is_cancel_requested()));
    let run_context = ComputeJobContext {
        run_id: context.run_id.clone(),
        conversation_id: context.conversation_id.clone(),
        deadline: context.deadline,
        stop,
        progress,
        database: context.database.clone(),
        attachments_dir: context.attachments_dir.clone(),
        sessions_dir: context.sessions_dir.clone(),
    };
    let port: &dyn JobLifecyclePort = &*port;

    // 2) Stop already requested before any work: nothing is written at all.
    if context.stop.stopped() {
        return finish_cancelled(port, &fence);
    }

    let executed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        run(&run_context, params)
    }));
    let outcome = match executed {
        Ok(outcome) => outcome,
        Err(_) => Err(ComputeJobError {
            code: codes::EXECUTION_FAILED,
            message: "compute job panicked; the job was settled as failed".into(),
        }),
    };

    #[cfg(test)]
    test_hooks::run_at_settle();

    // 6) Fenced mid-flight: the row belongs to someone else now.
    if fence.load(Ordering::SeqCst) {
        let reason = fence_reason.lock().unwrap().clone();
        return JobTerminal::superseded(if reason.is_empty() {
            "a newer attempt owns this job".to_owned()
        } else {
            reason
        });
    }

    match outcome {
        Err(error) if error.code == codes::SUPERSEDED => JobTerminal::superseded(error.message),
        // 3) Cancellation observed by the work itself: the work has stopped.
        Err(error) if error.code == codes::CANCELLED => finish_cancelled(port, &fence),
        Err(error) => {
            finish_failed(port,error.code,error.message)
        }
        Ok(outcome) => {
            // 4) Success and cancellation racing: the user's cancel wins, but
            //    only after the work actually stopped (it has: `run` returned)
            //    and only by writing the terminal explicitly.
            if context.stop.cancelled() || port.is_cancel_requested() {
                return finish_cancelled(port, &fence);
            }
            // 5) Publish the result; `completed` requires a retrievable result.
            match port.settle_completed(&outcome.result) {
                Ok(()) => JobTerminal::completed(),
                Err(error) => match error.kind {
                    PortErrorKind::Conflict => {
                        JobTerminal::superseded(error.message)
                    }
                    PortErrorKind::Cancelled => finish_cancelled(port, &fence),
                    // The result is not retrievable, so `completed` would be a
                    // lie. Fail it explicitly; this must never surface as a
                    // user cancellation.
                    PortErrorKind::Storage | PortErrorKind::Other => {
                        finish_failed(port,codes::RESULT_STORAGE_FAILED,error.message)
                    }
                },
            }
        }
    }
}

/// Confirm a cancellation. Only a successful terminal write counts as a
/// confirmed cancel: if the store refuses because another writer already owns
/// the row, this attempt is superseded; if the write fails transiently, the
/// outcome is an *unsettled failure*, never a claimed cancellation.
fn finish_cancelled(port: &dyn JobLifecyclePort, fence: &AtomicBool) -> JobTerminal {
    if fence.load(Ordering::SeqCst) {
        return JobTerminal::superseded("a newer attempt owns this job");
    }
    match port.settle_cancelled() {
        Ok(()) => JobTerminal::cancelled(),
        Err(error) => match error.kind {
            PortErrorKind::Conflict | PortErrorKind::Cancelled => {
                JobTerminal::superseded(error.message)
            }
            PortErrorKind::Storage | PortErrorKind::Other => {
                JobTerminal::unsettled(codes::SETTLE_FAILED, error.message)
            }
        },
    }
}

/// Spawn the runner on a named worker thread. Window A's `kernel_job_start`
/// calls this after the job row exists; the thread owns exactly one execution
/// and settles at most once.
pub(crate) fn spawn_on_lifecycle(
    job_id: String,
    context: ComputeJobContext,
    params: Value,
    port: Arc<dyn JobLifecyclePort + Send + Sync>,
) -> std::thread::JoinHandle<JobTerminal> {
    std::thread::Builder::new()
        .name(format!("fox-compute-job-{job_id}"))
        .spawn(move || run_on_lifecycle(&context, &params, port))
        .expect("spawn compute job worker")
}

/// Run one background attachment computation on the caller's thread.
///
/// `params` is the same input object as the synchronous tool
/// (`code`, `attachmentIds`/`artifactIds`, optional `processing`/`profile`).
/// Background jobs default to `processing: "chunked"` — the whole point of a
/// job is work that outgrows the synchronous 15s/30s window — but an
/// explicit `processing: "whole"` is honored for short legacy calls.
pub(crate) fn run(
    context: &ComputeJobContext,
    params: &Value,
) -> Result<ComputeJobOutcome, ComputeJobError> {
    let started = Instant::now();
    if context.stop.stopped() {
        return Err(ComputeJobError {
            code: if context.stop.cancelled() {
                codes::CANCELLED
            } else {
                codes::SUPERSEDED
            },
            message: "compute job was stopped before it started".into(),
        });
    }
    let mut input = params.clone();
    if input.get("processing").is_none() {
        input["processing"] = json!("chunked");
    }
    if !input.get("code").and_then(Value::as_str).is_some_and(|c| !c.trim().is_empty()) {
        return Err(ComputeJobError {
            code: codes::INVALID_PARAMS,
            message: "compute job requires nonempty JavaScript code".into(),
        });
    }
    let progress = Arc::clone(&context.progress);
    let stop = context.stop.clone();
    let result = super::execute_with_options(
        &context.database,
        &context.attachments_dir,
        &context.sessions_dir,
        &context.conversation_id,
        &context.run_id,
        &input,
        move || stop.stopped(),
        super::ComputeOptions {
            deadline: Some(context.deadline),
            progress: Some(&*progress),
        },
    );
    match result {
        Ok(mut value) => {
            let rows = value["rowsProcessed"].as_u64().unwrap_or(0);
            value["durationMs"] = json!(started.elapsed().as_millis() as u64);
            Ok(ComputeJobOutcome {
                result: value,
                rows_processed: rows,
                duration_ms: started.elapsed().as_millis() as u64,
            })
        }
        Err(message) => {
            // Ordering matters: a real user cancel is reported as such; a stop
            // caused only by this attempt losing ownership is `superseded`, not
            // a cancellation the user asked for.
            let code = if context.stop.cancelled() {
                codes::CANCELLED
            } else if context.stop.fence.as_ref().is_some_and(|fence| fence()) {
                codes::SUPERSEDED
            } else if message.contains("cancelled") {
                codes::CANCELLED
            } else if message.contains("timed out") {
                codes::TIMED_OUT
            } else if is_javascript_execution_failure(&message) {
                // Provenance, not wording. The QuickJS interpreter prefixes every
                // error it raises with this marker and appends its own location
                // diagnostic ("near code line N"). Classifying on message text
                // instead made every executed exception look like a bad
                // parameter, because the diagnostic itself contains "code ".
                codes::EXECUTION_FAILED
            } else if is_parameter_validation_failure(&message) {
                // Only messages produced by the input-validation stage may reach
                // here, and each is matched by a phrase that describes the
                // parameter itself.
                codes::INVALID_PARAMS
            } else {
                codes::EXECUTION_FAILED
            };
            Err(ComputeJobError { code, message })
        }
    }
}

/// Marker every QuickJS failure carries out of the interpreter. It is emitted by
/// `data_compute::format_js_error` and `data_compute::chunked::format_js_error`,
/// which are the only producers of user-code execution errors on this path.
/// Kept in sync with that producer: if it ever changes its wording, this
/// predicate must change with it (and the regression test will fail loudly).
const JS_EXECUTION_FAILURE_MARKER: &str = "JavaScript execution failed";

fn is_javascript_execution_failure(message: &str) -> bool {
    message.contains(JS_EXECUTION_FAILURE_MARKER)
}

/// Phrases produced only by the input-validation stage of
/// `attachment_compute::execute_with_options`, before any execution work starts.
/// Deliberately narrow: a broad "does the message mention code/profile" test is
/// what caused the misclassification, because user exceptions and their source
/// excerpts can contain any of those words.
fn is_parameter_validation_failure(message: &str) -> bool {
    message.contains("processing must be")
        || message.contains("profile")
        || message.contains("档位的")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::AttachmentRecord;
    use crate::kernel::CancellationPort;
    use sha2::Digest;
    use std::sync::{
        atomic::AtomicU64,
        Arc, Mutex,
    };
    use std::time::Duration;

    /// Serializes the tests that install the process-wide at-settle hook.
    static HOOK_LOCK: Mutex<()> = Mutex::new(());

    fn lock_hooks() -> std::sync::MutexGuard<'static, ()> {
        HOOK_LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    struct Fixture {
        root: std::path::PathBuf,
        sessions: std::path::PathBuf,
        database: Database,
        conversation_id: String,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("fox-job-{tag}-{}", uuid::Uuid::new_v4()));
            let sessions = root.join("sessions");
            std::fs::create_dir_all(&sessions).unwrap();
            let database = Database::open(root.join("test.db")).unwrap();
            let conversation = database
                .create_conversation(database.default_agent_id(), None, None, None)
                .unwrap();
            Self {
                root,
                sessions,
                database,
                conversation_id: conversation.id,
            }
        }

        fn attach_csv(&self, id: &str, rows: usize) -> AttachmentRecord {
            let storage = self.root.join("storage");
            std::fs::create_dir_all(&storage).unwrap();
            let path = storage.join(format!("{id}.csv"));
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
    }

    fn token_for(run_id: &str) -> crate::kernel::CancellationToken {
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run(run_id).unwrap();
        registry.run_token(run_id).unwrap()
    }

    /// In-memory lifecycle port: every call is recorded, and each result can be
    /// injected so the runner's decision tree is exercised deterministically.
    #[derive(Default)]
    struct FakePort {
        attempt: u32,
        mark_running_error: Mutex<Option<(PortErrorKind, String)>>,
        settle_completed_error: Mutex<Option<(PortErrorKind, String)>>,
        settle_cancelled_error: Mutex<Option<(PortErrorKind, String)>>,
        report_error: Mutex<Option<(PortErrorKind, String, u64)>>,
        report_calls: AtomicU64,
        cancel_requested: AtomicBool,
        panic_in_report: AtomicBool,
        /// Set by the at-settle hook: proves the runner reached the settlement
        /// point only after the work returned.
        work_finished: AtomicBool,
        events: Mutex<Vec<String>>,
        stored_result: Mutex<Option<Value>>,
    }

    impl FakePort {
        fn new() -> Self {
            Self {
                attempt: 3,
                ..Default::default()
            }
        }

        fn record(&self, event: impl Into<String>) {
            self.events.lock().unwrap().push(event.into());
        }

        fn events(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }

        fn settles(&self) -> Vec<String> {
            self.events()
                .into_iter()
                .filter(|event| event.starts_with("settle"))
                .collect()
        }

        /// Settlement event names without the ordering-proof suffix, so tests
        /// can assert the exact sequence of terminal writes.
        fn settle_names(&self) -> Vec<String> {
            self.settles()
                .into_iter()
                .map(|event| match event.split_once(":work_finished") {
                    Some((head, _)) => head.to_owned(),
                    None => event,
                })
                .collect()
        }

        fn inject(slot: &Mutex<Option<(PortErrorKind, String)>>, error: PortError) {
            *slot.lock().unwrap() = Some((error.kind, error.message));
        }
    }

    impl JobLifecyclePort for FakePort {
        fn attempt(&self) -> u32 {
            self.attempt
        }

        fn mark_running(&self) -> Result<(), PortError> {
            self.record("mark_running");
            match self.mark_running_error.lock().unwrap().clone() {
                Some((kind, message)) => Err(PortError { kind, message }),
                None => Ok(()),
            }
        }

        fn is_cancel_requested(&self) -> bool {
            self.cancel_requested.load(Ordering::SeqCst)
        }

        fn report(&self, done: u64, total: Option<u64>) -> Result<(), PortError> {
            if self.panic_in_report.load(Ordering::SeqCst) {
                panic!("injected report panic");
            }
            let call = self.report_calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.record(format!("report:{done}/{total:?}"));
            match self.report_error.lock().unwrap().clone() {
                Some((kind, message, from_call)) if call >= from_call => {
                    Err(PortError { kind, message })
                }
                _ => Ok(()),
            }
        }

        fn settle_completed(&self, result: &Value) -> Result<(), PortError> {
            self.record(format!(
                "settle_completed:work_finished={}",
                self.work_finished.load(Ordering::SeqCst)
            ));
            match self.settle_completed_error.lock().unwrap().clone() {
                Some((kind, message)) => Err(PortError { kind, message }),
                None => {
                    *self.stored_result.lock().unwrap() = Some(result.clone());
                    Ok(())
                }
            }
        }

        fn settle_cancelled(&self) -> Result<(), PortError> {
            self.record(format!(
                "settle_cancelled:work_finished={}",
                self.work_finished.load(Ordering::SeqCst)
            ));
            match self.settle_cancelled_error.lock().unwrap().clone() {
                Some((kind, message)) => Err(PortError { kind, message }),
                None => Ok(()),
            }
        }

        fn settle_failed(&self, code: &'static str, _message: &str) -> Result<(), PortError> {
            self.record(format!("settle_failed:{code}"));
            Ok(())
        }
    }

    struct Harness {
        fixture: Fixture,
        port: Arc<FakePort>,
    }

    impl Harness {
        fn new(tag: &str) -> Self {
            Self {
                fixture: Fixture::new(tag),
                port: Arc::new(FakePort::new()),
            }
        }

        fn run_with(
            &self,
            run_id: &str,
            token: &crate::kernel::CancellationToken,
            params: Value,
        ) -> JobTerminal {
            let context = ComputeJobContext {
                run_id: run_id.to_owned(),
                conversation_id: self.fixture.conversation_id.clone(),
                deadline: Instant::now() + Duration::from_secs(60),
                stop: StopSignal::new(token.clone()),
                progress: Arc::new(|_| {}),
                database: self.fixture.database.clone(),
                attachments_dir: self.fixture.root.clone(),
                sessions_dir: self.fixture.sessions.clone(),
            };
            let port: Arc<dyn JobLifecyclePort + Send + Sync> = self.port.clone();
            run_on_lifecycle(&context, &params, port)
        }
    }

    fn code_only_params() -> Value {
        json!({
            "processing": "chunked",
            "code": "function onChunk(c){}\nfunction onFinish(){ return {ok: 1}; }",
        })
    }

    #[test]
    fn start_cas_loss_writes_nothing_and_reports_not_started() {
        let harness = Harness::new("cas");
        FakePort::inject(
            &harness.port.mark_running_error,
            PortError::conflict("job is not startable in its current state"),
        );
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::NotStarted);
        assert!(!terminal.settled, "a lost start CAS must not settle anything");
        assert_eq!(terminal.error_code, Some(codes::SUPERSEDED));
        assert!(harness.port.settles().is_empty(), "{:?}", harness.port.events());
        assert!(
            harness.port.events().iter().all(|event| event == "mark_running"),
            "no work may run after a lost CAS: {:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn pre_start_cancel_never_runs_work_and_confirms_cancellation_once() {
        let harness = Harness::new("precancel");
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("run-1").unwrap();
        registry.request_run_cancel("run-1");
        let token = registry.run_token("run-1").unwrap();
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::Cancelled);
        assert!(terminal.settled);
        assert_eq!(harness.port.settles().len(), 1, "{:?}", harness.port.events());
        assert!(harness.port.settles()[0].starts_with("settle_cancelled"));
        assert!(
            !harness.port.events().iter().any(|event| event.starts_with("report")),
            "cancelled before start must not execute work: {:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    /// The review's exact defect: the work finished, then a cancel arrived
    /// before settlement. The old runner returned `Cancelled` without writing
    /// any terminal, so the job stayed `running` forever.
    #[test]
    fn cancel_at_the_settlement_point_is_written_and_never_reports_completed() {
        let _guard = lock_hooks();
        let harness = Harness::new("race-store");
        // Deterministic interleaving: at the settlement point the store has
        // already recorded a cancel request for this job.
        let port = Arc::clone(&harness.port);
        test_hooks::set_at_settle(Some(Box::new(move || {
            port.cancel_requested.store(true, Ordering::SeqCst);
        })));
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, code_only_params());
        test_hooks::set_at_settle(None);
        assert_eq!(terminal.state, TerminalState::Cancelled);
        assert!(terminal.settled, "the cancel must be persisted, not dropped");
        assert_eq!(harness.port.settles().len(), 1, "{:?}", harness.port.events());
        assert!(
            harness.port.settles()[0].starts_with("settle_cancelled"),
            "{:?}",
            harness.port.events()
        );
        assert!(
            harness.port.events().iter().all(|event| !event.starts_with("settle_completed")),
            "a cancelled job must never publish a completed terminal: {:?}",
            harness.port.events()
        );
        assert!(harness.port.stored_result.lock().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    /// Same race, but the cancel arrives through the Host token rather than the
    /// store flag. The work has demonstrably stopped first.
    #[test]
    fn token_cancel_at_the_settlement_point_stops_work_before_the_terminal_write() {
        let _guard = lock_hooks();
        let harness = Harness::new("race-token");
        let registry = Arc::new(crate::kernel::CancellationRegistry::default());
        registry.register_run("run-1").unwrap();
        let token = registry.run_token("run-1").unwrap();
        let port = Arc::clone(&harness.port);
        let cancel_registry = Arc::clone(&registry);
        test_hooks::set_at_settle(Some(Box::new(move || {
            // Proves the runner reached the settlement point only after the
            // computation returned, then the user cancels.
            port.work_finished.store(true, Ordering::SeqCst);
            cancel_registry.request_run_cancel("run-1");
        })));
        let terminal = harness.run_with("run-1", &token, code_only_params());
        test_hooks::set_at_settle(None);
        assert_eq!(terminal.state, TerminalState::Cancelled);
        assert!(terminal.settled);
        assert_eq!(
            harness.port.settles(),
            vec!["settle_cancelled:work_finished=true".to_owned()],
            "work must be finished before the cancel is confirmed"
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn a_result_storage_failure_is_never_reported_as_a_user_cancellation() {
        let harness = Harness::new("storage");
        FakePort::inject(
            &harness.port.settle_completed_error,
            PortError::storage("blob write failed: disk full"),
        );
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(terminal.error_code, Some(codes::RESULT_STORAGE_FAILED));
        assert_ne!(terminal.error_code, Some(codes::CANCELLED));
        assert_eq!(
            harness.port.settle_names(),
            vec![
                "settle_completed".to_owned(),
                format!("settle_failed:{}", codes::RESULT_STORAGE_FAILED),
            ],
            "{:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn a_concurrent_winner_on_the_result_write_keeps_its_terminal_state() {
        let harness = Harness::new("winner");
        FakePort::inject(
            &harness.port.settle_completed_error,
            PortError::conflict("job already settled by a cancel"),
        );
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::Superseded);
        assert!(!terminal.settled, "no second terminal may be written");
        assert_eq!(
            harness.port.settle_names(),
            vec!["settle_completed".to_owned()],
            "the winner's terminal must not be overwritten: {:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn a_panic_inside_the_executor_is_settled_as_a_failure() {
        let harness = Harness::new("panic");
        harness.port.panic_in_report.store(true, Ordering::SeqCst);
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(terminal.error_code, Some(codes::EXECUTION_FAILED));
        assert_eq!(
            harness.port.settles(),
            vec![format!("settle_failed:{}", codes::EXECUTION_FAILED)],
            "{:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    /// A late report from an attempt that no longer owns the job stops the work
    /// and writes nothing, so it cannot clobber the newer attempt's outcome.
    #[test]
    fn a_fenced_attempt_stops_early_and_writes_no_terminal() {
        let harness = Harness::new("fence");
        harness.fixture.attach_csv("f1", 120_000);
        // Fail the second progress call: the first is the "measured" phase, so
        // chunk delivery has demonstrably begun.
        *harness.port.report_error.lock().unwrap() =
            Some((PortErrorKind::Conflict, "a newer attempt owns this job".into(), 2));
        let token = token_for("run-1");
        let params = json!({
            "processing": "chunked",
            "profile": "large",
            "attachmentIds": ["f1"],
            "code": "let n=0;\nfunction onChunk(c){ n+=c.rows.length; }\nfunction onFinish(){ return {rows:n}; }",
        });
        let terminal = harness.run_with("run-1", &token, params);
        assert_eq!(terminal.state, TerminalState::Superseded);
        assert!(!terminal.settled);
        assert!(
            harness.port.settles().is_empty(),
            "a fenced attempt writes nothing: {:?}",
            harness.port.events()
        );
        // 120k rows across ≥3 chunks: the fence at report #2 means later chunks
        // were never processed.
        assert_eq!(
            harness.port.report_calls.load(Ordering::SeqCst),
            2,
            "the work must stop at the fence: {:?}",
            harness.port.events()
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn an_unrecordable_cancellation_is_an_unsettled_failure_not_a_cancel() {
        let harness = Harness::new("settlefail");
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("run-1").unwrap();
        registry.request_run_cancel("run-1");
        let token = registry.run_token("run-1").unwrap();
        FakePort::inject(
            &harness.port.settle_cancelled_error,
            PortError::other("database is locked"),
        );
        let terminal = harness.run_with("run-1", &token, code_only_params());
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(terminal.error_code, Some(codes::SETTLE_FAILED));
        assert!(
            !terminal.settled,
            "a failed terminal write is not a confirmed cancellation"
        );
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    /// A real JavaScript exception raised by the user's own code is an execution
    /// failure, NOT a parameter error. Before this was fixed, the classifier ran
    /// `message.contains("code ")` against the interpreter's own location text
    /// ("near code line N"), so every executed exception was reported as
    /// `compute.invalid_params` — the message text of a user error decided the
    /// classification. The exception here is deliberately worded to look like a
    /// parameter diagnostic as well, so parameters can never be inferred from
    /// the text at all.
    #[test]
    fn a_real_javascript_exception_is_an_execution_failure_not_an_invalid_parameter() {
        let harness = Harness::new("js-exception");
        // A real attachment: the chunked code only runs once rows are delivered,
        // so this exercises the genuine QuickJS execution path.
        harness.fixture.attach_csv("f1", 8);
        let params = json!({
            "processing": "chunked",
            "attachmentIds": ["f1"],
            "code": "function onChunk(c){}\nfunction onFinish(){ throw new Error('processing profile 档位 code is invalid'); }",
        });
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, params);
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(
            terminal.error_code,
            Some(codes::EXECUTION_FAILED),
            "a real executed exception must be an execution failure; message: {:?}",
            terminal.error_message
        );
        assert_eq!(
            harness.port.settles(),
            vec![format!("settle_failed:{}", codes::EXECUTION_FAILED)],
            "{:?}",
            harness.port.events()
        );
        let message = terminal.error_message.unwrap_or_default();
        // The real interpreter diagnostics must still be present in the message:
        // this is the executed path, not a synthetic string.
        assert!(message.contains("JavaScript execution failed"), "{message}");
        assert!(message.contains("processing profile 档位 code is invalid"), "{message}");
        assert!(message.contains("code line"), "{message}");
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
    }

    #[test]
    fn invalid_params_and_deadline_produce_their_own_terminals() {        let harness = Harness::new("params");
        let token = token_for("run-1");
        let terminal = harness.run_with("run-1", &token, json!({"processing": "chunked"}));
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(terminal.error_code, Some(codes::INVALID_PARAMS));
        assert_eq!(
            harness.port.settles(),
            vec![format!("settle_failed:{}", codes::INVALID_PARAMS)]
        );

        let harness2 = Harness::new("deadline");
        let token2 = token_for("run-2");
        let context = ComputeJobContext {
            run_id: "run-2".to_owned(),
            conversation_id: harness2.fixture.conversation_id.clone(),
            deadline: Instant::now() - Duration::from_secs(1),
            stop: StopSignal::new(token2.clone()),
            progress: Arc::new(|_| {}),
            database: harness2.fixture.database.clone(),
            attachments_dir: harness2.fixture.root.clone(),
            sessions_dir: harness2.fixture.sessions.clone(),
        };
        let port: Arc<dyn JobLifecyclePort + Send + Sync> = harness2.port.clone();
        let terminal = run_on_lifecycle(&context, &code_only_params(), port);
        assert_eq!(terminal.state, TerminalState::Failed);
        assert_eq!(terminal.error_code, Some(codes::TIMED_OUT));
        let _ = std::fs::remove_dir_all(&harness.fixture.root);
        let _ = std::fs::remove_dir_all(&harness2.fixture.root);
    }

    /// The worker-thread entry point settles exactly once and reports the
    /// terminal to the caller; joining the handle is the (deterministic)
    /// completion signal.
    #[test]
    fn spawn_on_lifecycle_settles_once_on_its_worker_thread() {
        let fixture = Fixture::new("spawn");
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run("run-1").unwrap();
        let token = registry.run_token("run-1").unwrap();
        fn noop(_: crate::data_compute::chunked::ChunkProgress) {}
        let context = ComputeJobContext {
            run_id: "run-1".to_owned(),
            conversation_id: fixture.conversation_id.clone(),
            deadline: Instant::now() + Duration::from_secs(60),
            stop: StopSignal::new(token),
            progress: Arc::new(noop),
            database: fixture.database.clone(),
            attachments_dir: fixture.root.clone(),
            sessions_dir: fixture.sessions.clone(),
        };
        let port: Arc<dyn JobLifecyclePort + Send + Sync> = Arc::new(FakePort::new());
        let handle = spawn_on_lifecycle("job-1".to_owned(), context, code_only_params(), port);
        let terminal = handle.join().expect("the worker thread finishes");
        assert_eq!(terminal.state, TerminalState::Completed);
        assert!(terminal.settled);
        let _ = std::fs::remove_dir_all(&fixture.root);
    }
}

fn finish_failed(port:&dyn JobLifecyclePort,code:&'static str,message:String)->JobTerminal {
    match port.settle_failed(code,&message) {
        Ok(())=>JobTerminal::failed(code,message),
        Err(error) if error.kind==PortErrorKind::Conflict=>JobTerminal::superseded(error.message),
        Err(error)=>JobTerminal::unsettled(codes::SETTLE_FAILED,error.message)
    }
}
