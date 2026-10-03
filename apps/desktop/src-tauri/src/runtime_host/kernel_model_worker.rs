//! Isolated transport for a claimed Kernel batch; never shares Legacy workers.
use super::RuntimeCommand;
use crate::kernel::CancellationToken;
pub(super) use crate::kernel_model_config::KernelModelConfig;
use fox_engine_protocol::{
    KernelBatchResumeFrame, KernelModelResponse, RunControlBinding, PROTOCOL_NAME, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{mpsc::{self, Receiver}, Arc, OnceLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 1_048_576;

fn model_request_gate() -> &'static Arc<super::run_admission::Gate> {
    static GATE: OnceLock<Arc<super::run_admission::Gate>> = OnceLock::new();
    GATE.get_or_init(|| super::run_admission::Gate::new(
        super::run_admission::configured_limit("FOX_KERNEL_MODEL_REQUEST_CAPACITY", 2)))
}

/// A bounded, single-line excerpt of a worker payload for stderr diagnostics.
///
/// Capped so a large payload cannot flood a log, and never returned to the
/// caller: durable diagnostics must not absorb child/provider text.
fn bounded_diagnostic(payload: &Value) -> String {
    let text = payload
        .get("error")
        .map(|error| error.to_string())
        .or_else(|| payload.get("message").map(|message| message.to_string()))
        .unwrap_or_else(|| payload.to_string());
    text.chars().take(600).collect::<String>().replace('\n', " ")
}

/// Which identity field of a worker response disagrees with its request.
///
/// The transport comparison is unchanged; this only replaces a bare "mismatch"
/// with the field name, because without it a failure here is unattributable —
/// exactly the class of problem the 2026-09-19 review flagged.
fn response_identity_mismatch(request: &Value, response: &Value) -> Option<String> {
    if response["protocol"] != PROTOCOL_NAME {
        return Some("protocol".into());
    }
    if response["version"] != PROTOCOL_VERSION {
        return Some("version".into());
    }
    if response["kind"] != "response" {
        return Some(format!("kind={}", response["kind"]));
    }
    if response["requestId"] != request["id"] {
        return Some("requestId".into());
    }
    for key in ["runId", "conversationId", "runtimeSessionId"] {
        if response[key] != request[key] {
            return Some(format!(
                "{key}: request={} response={}",
                request[key], response[key]
            ));
        }
    }
    None
}/// Transport-only time for the worker to report its already expired model round.
/// Late model output is rejected by Host; this does not extend generation budget.
pub(super) const MODEL_SETTLE_GRACE_MS: i64 = 1_000;
/// A finite worker handshake window, independent of a model request that has
/// not yet been durably dispatched. The same limit applies to describe.
pub(super) const WORKER_READY_TIMEOUT_MS: i64 = 30_000;
pub(super) const MODEL_WINDOW_EXPIRED: &str = "Kernel model window expired before response commit";

#[cfg(windows)]
#[path = "kernel_model_worker_job.rs"]
mod worker_job;

const SETTLED_FAILURE_PREFIX: &str = "kernel.settled_model_failure:";
/// These errors originate in the owned transport. Only retry after its Worker
/// has been dropped (including process reaping); protocol/identity errors stay fatal.
pub(super) fn is_reaped_transport_failure(error: &str) -> bool {
    matches!(error, MODEL_WINDOW_EXPIRED | "Kernel worker deadline exceeded; reconcile delivery"
        | "Kernel worker disconnected; reconcile delivery" | "Kernel worker write failed")
}

pub(super) fn settled_failure(error: &str) -> Option<fox_engine_protocol::KernelModelFailure> {
    let evidence: fox_engine_protocol::KernelModelFailure = serde_json::from_str(error.strip_prefix(SETTLED_FAILURE_PREFIX)?).ok()?;
    evidence.validate().ok()?;
    Some(evidence)
}

pub(super) fn describe(
    runtime: &RuntimeCommand,
    binding: &RunControlBinding,
    model_service: Value,
    prompt: Value,
    supported_tools: Vec<&str>,
    token: &CancellationToken,
) -> Result<KernelModelConfig, String> {
    token.check()?;
    let request = json!({
        "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request", "type": "kernel.describe",
        "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(),
        "runId": binding.run_id, "conversationId": binding.conversation_id,
        "runtimeSessionId": format!("kernel-description-{}", uuid::Uuid::new_v4()),
        "payload": {"executionProfileId":binding.execution_profile_id,"modelService":model_service,"prompt":prompt,"supportedTools":supported_tools},
    });
    let mut worker = Worker::spawn(runtime)?;
    let result = worker.exchange(
        request,
        "kernel.description",
        token,
        Instant::now() + Duration::from_millis(WORKER_READY_TIMEOUT_MS as u64),
    )?;
    if result["schemaVersion"] != 1 || result["executionProfileId"] != binding.execution_profile_id
    {
        return Err("Kernel description identity mismatch".into());
    }
    let config = KernelModelConfig {
        engine_id: binding.engine_id.clone(),
        native_adapter: crate::kernel_model_config::capture_native_adapter(&binding.engine_id)?,
        execution_profile_id: binding.execution_profile_id.clone(),
        model_service,
        system_prompt: result["systemPrompt"]
            .as_str()
            .ok_or("missing Kernel system prompt")?
            .into(),
        proposal_tools: result["proposalTools"]
            .as_array()
            .ok_or("missing Kernel tool descriptions")?
            .clone(),
    };
    config.hash()?;
    if config.proposal_tools.iter().any(|tool| !supported_tools.contains(&tool["name"].as_str().unwrap_or_default())) {
        return Err("Kernel description added unsupported resource tools".into());
    }
    Ok(config)
}

// Drop always terminates/reaps only the child created here, then joins bounded
// readers/writers. A timeout never leaves a pipe writer or model process alive.
struct Worker {
    usage_database: Option<crate::database::Database>,
    child: Child,
    #[cfg(windows)]
    job: Option<worker_job::WorkerJob>,
    stdin: Option<ChildStdin>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    messages: Receiver<Result<Value, String>>,
}

pub(super) type PreviewSink = dyn Fn(&fox_engine_protocol::KernelModelPreview) + Sync;

/// Test-only, opt-in monotonic trace of the formal worker path.
///
/// The F1/F3 fixtures measure a model round from the *Provider's* side, which
/// cannot see how long the Host spent before the worker even opened its socket.
/// This registry lets the Host and the fixture correlate per-Run, so a stalled
/// round can be attributed to worker startup, the model exchange, or the
/// durable round decision instead of being guessed from request counts.
///
/// Recording is a no-op unless `FOX_F1_TIMELINE` is set: the ordinary suite
/// pays one relaxed atomic load and nothing else. The store is process-global
/// and keyed by Run id, so concurrently executing tests cannot mix timelines.
#[cfg(test)]
pub(crate) mod host_trace {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    /// Cap on retained entries. A full-suite parallel run has hundreds of Runs;
    /// without a cap the ring itself becomes a resource the diagnosis creates.
    const MAX_ENTRIES: usize = 4096;

    /// Process-wide monotonic origin. Every recorded entry carries its own
    /// absolute offset from this origin, so entries from different threads (the
    /// Host thread and a fixture's Provider thread) compare directly instead of
    /// each keeping a private zero.
    fn origin() -> &'static Instant {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        ORIGIN.get_or_init(Instant::now)
    }

    /// Milliseconds since the process-wide origin, on the same scale as every
    /// recorded entry. A fixture records this once when it starts its Provider
    /// thread, which is the explicit anchor that lets Provider-relative offsets
    /// be compared with Host offsets instead of assumed to share a zero.
    pub(crate) fn origin_elapsed_ms() -> u128 {
        origin().elapsed().as_millis()
    }

    /// Frozen on first use: a diagnostic switch must not change mid-run.
    fn enabled_cache() -> &'static AtomicBool {
        static ENABLED: OnceLock<AtomicBool> = OnceLock::new();
        ENABLED.get_or_init(|| AtomicBool::new(std::env::var_os("FOX_F1_TIMELINE").is_some()))
    }

    pub(crate) fn enabled() -> bool {
        enabled_cache().load(Ordering::Relaxed)
    }

    fn store() -> &'static Mutex<Vec<(String, u128, String)>> {
        static STORE: OnceLock<Mutex<Vec<(String, u128, String)>>> = OnceLock::new();
        STORE.get_or_init(|| Mutex::new(Vec::new()))
    }

    fn counters() -> &'static Mutex<HashMap<(String, &'static str), u64>> {
        static COUNTERS: OnceLock<Mutex<HashMap<(String, &'static str), u64>>> = OnceLock::new();
        COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// The store is only ever locked when `enabled()`; a poisoned lock is
    /// recovered rather than propagated, so diagnostics can never turn a
    /// failure into a second, different failure.
    fn with_store<T>(f: impl FnOnce(&mut Vec<(String, u128, String)>) -> T) -> Option<T> {
        match store().lock() {
            Ok(mut entries) => Some(f(&mut entries)),
            Err(poisoned) => Some(f(&mut poisoned.into_inner())),
        }
    }

    /// Record one event. Callers guard on [`enabled`] when building a message,
    /// so the formatted string is not constructed on the ordinary path.
    pub(crate) fn record(run_id: &str, event: impl Into<String>) {
        if !enabled() {
            return;
        }
        let at = origin().elapsed().as_millis();
        with_store(|entries| {
            if entries.len() >= MAX_ENTRIES {
                // Drop the oldest half: the newest entries belong to the run
                // whose failure is being diagnosed right now.
                entries.drain(..MAX_ENTRIES / 2);
            }
            entries.push((run_id.to_owned(), at, event.into()));
        });
    }

    /// Bounded per-Run counter, also recording nothing while the switch is off.
    pub(crate) fn count(run_id: &str, key: &'static str) -> u64 {
        if !enabled() {
            return 0;
        }
        let mut counters = match counters().lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        let entry = counters.entry((run_id.to_owned(), key)).or_insert(0);
        *entry += 1;
        *entry
    }

    /// Take one Run's entries and drop its counters. Called by the fixture on
    /// every exit path (success or panic), so finished Runs leave nothing behind.
    pub(crate) fn take(run_id: &str) -> Vec<(u128, String)> {
        let mut mine = with_store(|entries| {
            let mut mine = Vec::new();
            entries.retain(|(id, at, event)| {
                if id == run_id {
                    mine.push((*at, event.clone()));
                    false
                } else {
                    true
                }
            });
            mine
        })
        .unwrap_or_default();
        mine.sort_by_key(|(at, _)| *at);
        let mut counters = match counters().lock() {
            Ok(counters) => counters,
            Err(poisoned) => poisoned.into_inner(),
        };
        counters.retain(|(id, _), _| id != run_id);
        mine
    }

    /// Discard a Run's entries without printing them, so a reused or retried Run
    /// id cannot inherit a previous attempt's timeline.
    pub(crate) fn clear(run_id: &str) {
        let _ = take(run_id);
    }

    /// Every Run still holding entries, so a diagnostic run can attribute stray
    /// entries instead of silently dropping them.
    pub(crate) fn runs() -> Vec<String> {
        let mut ids = with_store(|entries| {
            entries.iter().map(|(id, _, _)| id.clone()).collect::<Vec<_>>()
        })
        .unwrap_or_default();
        ids.sort();
        ids.dedup();
        ids
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.job.take();
        let _ = self.child.kill();
        self.stdin.take();
        let _ = self.child.wait();
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        if let Some(reader) = self.reader.take() { let _ = reader.join(); }
    }
}

fn wait<T>(
    receiver: &Receiver<T>,
    token: &CancellationToken,
    deadline: Instant,
) -> Result<T, String> {
    wait_opts(receiver, Some(token), deadline)
}

/// Like [`wait`] but can ignore the cooperative cancel token while still
/// honouring the deadline. Used to drain the worker's final `model_response`
/// after the Host has already committed a terminal Run state inside an
/// `on_round` callback: committing completion cancels the run scope, but the
/// in-process worker still has to return the final frame it was asked for.
fn wait_final<T>(
    receiver: &Receiver<T>,
    deadline: Instant,
) -> Result<T, String> {
    wait_opts(receiver, None, deadline)
}

fn wait_opts<T>(
    receiver: &Receiver<T>,
    token: Option<&CancellationToken>,
    deadline: Instant,
) -> Result<T, String> {
    loop {
        if let Some(token) = token {
            token.check()?;
        }
        let remaining = deadline.checked_duration_since(Instant::now())
            .ok_or("Kernel worker deadline exceeded; reconcile delivery")?;
        match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(value) => {
                if let Some(token) = token {
                    token.check()?;
                }
                return Ok(value);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Kernel worker disconnected; reconcile delivery".into())
            }
        }
    }
}

/// How the worker's stderr is wired.
///
/// The worker's stderr is discarded by default: it may carry model input
/// fragments, and the Host already reports categorized failures. Opting in
/// (`FOX_KERNEL_WORKER_STDERR=inherit`) is a diagnosis switch for the real chain
/// — without it a worker that fails before its first model request leaves no
/// text anywhere, which is exactly how a startup failure looked like a
/// mysterious stall.
///
/// There is deliberately **no** `piped` mode. A piped stderr with no reader
/// deadlocks the worker: it blocks in its own write once the OS pipe buffer
/// fills, and the Host then waits for a protocol frame that is never sent.
/// Capturing the bytes would need a draining reader of bounded capacity, and
/// nothing consumes them today.
#[derive(Debug, PartialEq, Eq)]
enum StderrMode {
    Discard,
    Inherit,
}

fn stderr_mode(requested: Option<&str>) -> StderrMode {
    match requested {
        Some("inherit") => StderrMode::Inherit,
        _ => StderrMode::Discard,
    }
}

impl Worker {
    fn spawn(runtime: &RuntimeCommand) -> Result<Self, String> {
        let mut command = Command::new(&runtime.program);
        if let Some(script) = &runtime.script { command.arg(script); }
        let stderr = match stderr_mode(std::env::var("FOX_KERNEL_WORKER_STDERR").ok().as_deref()) {
            StderrMode::Inherit => Stdio::inherit(),
            StderrMode::Discard => Stdio::null(),
        };
        command.arg("--kernel-worker").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(stderr);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(|_| "could not start isolated Kernel worker")?;
        #[cfg(windows)]
        let job = match worker_job::WorkerJob::attach(&child) {
            Ok(job) => Some(job),
            Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
        };
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped worker stdout");
        let (sender, messages) = mpsc::sync_channel(16);
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut bytes = Vec::new();
                let result = reader.by_ref().take((MAX_FRAME + 1) as u64).read_until(b'\n', &mut bytes);
                if matches!(result, Ok(0)) { break; }
                let message = if result.is_err() || bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
                    Err("invalid or oversized Kernel worker response".into())
                } else {
                    serde_json::from_slice(&bytes).map_err(|_| "invalid Kernel worker JSON".into())
                };
                let failed = message.is_err();
                // Unsolicited flooding fails closed, never blocks Drop's join.
                if sender.try_send(message).is_err() || failed { break; }
            }
        });
        Ok(Self {
            usage_database: None,
            child,
            #[cfg(windows)]
            job,
            stdin,
            reader: Some(reader),
            writer: None,
            messages,
        })
    }

    fn exchange(
        &mut self,
        request: Value,
        expected: &str,
        token: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, String> {
        self.exchange_with_preview(request, expected, token, deadline, None, None)
    }

    fn exchange_with_preview(
        &mut self,
        request: Value,
        expected: &str,
        token: &CancellationToken,
        deadline: Instant,
        preview: Option<&PreviewSink>,
        diagnostic_attempt: Option<&str>,
    ) -> Result<Value, String> {
        let mut bytes = serde_json::to_vec(&request).map_err(|_| "invalid Kernel request")?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME { return Err("Kernel request exceeds frame limit".into()); }
        let mut stdin = self.stdin.take().ok_or("Kernel worker input unavailable")?;
        let (sender, written) = mpsc::sync_channel(1);
        self.writer = Some(thread::spawn(move || {
            let result = stdin.write_all(&bytes).and_then(|_| stdin.flush()).map_err(|_| "Kernel worker write failed");
            let _ = sender.send((stdin, result));
        }));
        let (stdin, result) = wait(&written, token, deadline)?;
        self.stdin = Some(stdin);
        if let Some(writer) = self.writer.take() { let _ = writer.join(); }
        result?;
        let model_round = expected == "kernel.model_response";
        let run_id = request["runId"].as_str().unwrap_or_default();
        let attempt_id = diagnostic_attempt.unwrap_or_else(|| request["id"].as_str().unwrap_or_default());
        if model_round {
            let observed_at = crate::database::now_ms();
            if let Some(database) = &self.usage_database {
                let _ = database.record_host_stage_point(
                    run_id, attempt_id, "worker_handoff", observed_at,
                );
            }
        }
        let mut first_model_event_seen = false;
        let mut revision = 0;
        loop {
            let response = wait(&self.messages, token, deadline)??;
            if let Some(reason) = response_identity_mismatch(&request, &response) {
                // No child message/provider error is copied into durable
                // diagnostics, but the *field* that disagreed is a Host fact and
                // is what makes this failure attributable.
                return Err(format!(
                    "Kernel worker response identity/type mismatch ({reason}); reconcile delivery"
                ));
            }
            if response["type"] == "kernel.usage_record" {
                let run=request["runId"].as_str().ok_or("usage request has no Run")?;
                if response["payload"]["runId"]!=run {return Err("usage Run identity mismatch".into());}
                if let Some(db)=&self.usage_database {db.record_model_usage(run,&response["payload"])?;}
                continue;
            }
            if response["type"] == "kernel.model_preview" {
                let Some(sink) = preview.filter(|_| expected == "kernel.model_response") else {
                    return Err("unexpected Kernel model preview".into());
                };
                let notice: fox_engine_protocol::KernelModelPreview =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid Kernel model preview")?;
                notice.validate()?;
                let frame = request["payload"]
                    .get("initialModel")
                    .or_else(|| request["payload"].get("batchResume"))
                    .ok_or("missing model preview cursor")?;
                let turn = frame
                    .get("turnId")
                    .or_else(|| frame.get("input").and_then(|input| input.get("turnId")));
                if notice.run_id != request["runId"]
                    || notice.conversation_id != request["conversationId"]
                    || Some(&Value::String(notice.turn_id.clone())) != turn
                    || notice.checkpoint_seq != frame["checkpointSeq"]
                    || notice.revision <= revision
                {
                    return Err("Kernel model preview identity/order mismatch".into());
                }
                revision = notice.revision;
                token.check()?;
                if model_round && !first_model_event_seen {
                    first_model_event_seen = true;
                    let observed_at = crate::database::now_ms();
                    if let Some(database) = &self.usage_database {
                        let _ = database.record_host_stage_point(
                            run_id, attempt_id, "first_valid_worker_event", observed_at,
                        );
                    }
                }
                sink(&notice);
                continue;
            }
            if response["type"] == "kernel.model_failure" && expected == "kernel.model_response" {
                let failure: fox_engine_protocol::KernelModelFailure =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid settled Kernel model failure")?;
                failure.validate()?;
                let frame = request["payload"]
                    .get("initialModel")
                    .or_else(|| request["payload"].get("batchResume"))
                    .ok_or("missing failure request identity")?;
                let turn = frame
                    .get("turnId")
                    .or_else(|| frame.get("input").and_then(|input| input.get("turnId")));
                if failure.run_id != request["runId"]
                    || Some(&Value::String(failure.turn_id.clone())) != turn
                    || failure.checkpoint_seq != frame["checkpointSeq"]
                {
                    return Err("Kernel failure belongs to another model request".into());
                }
                if model_round && !first_model_event_seen {
                    let observed_at = crate::database::now_ms();
                    if let Some(database) = &self.usage_database {
                        let _ = database.record_host_stage_point(
                            run_id, attempt_id, "first_valid_worker_event", observed_at,
                        );
                    }
                }
                return Err(format!(
                    "{SETTLED_FAILURE_PREFIX}{}",
                    serde_json::to_string(&failure).map_err(|_| "invalid failure")?
                ));
            }
            if response["type"] != expected {
                // The worker rejected the frame. Its message is deliberately NOT
                // copied into the returned error (which can become a durable
                // diagnostic), but without any trace at all this failure is
                // unattributable, so a bounded excerpt goes to stderr only.
                eprintln!(
                    "[kernel-worker] unexpected response type {} while expecting {expected}: {}",
                    response["type"],
                    bounded_diagnostic(&response["payload"]),
                );
                return Err(format!(
                    "Kernel worker response identity/type mismatch (expected {expected}, got {}); reconcile delivery",
                    response["type"]
                ));
            }
            if model_round && !first_model_event_seen {
                let observed_at = crate::database::now_ms();
                if let Some(database) = &self.usage_database {
                    let _ = database.record_host_stage_point(
                        run_id, attempt_id, "first_valid_worker_event", observed_at,
                    );
                }
            }
            return Ok(response["payload"].clone());
        }
    }
}

pub(crate) fn deliver(
    runtime: &RuntimeCommand, config: &KernelModelConfig, api_key: &str,
    binding: &RunControlBinding, frame: &KernelBatchResumeFrame, token: &CancellationToken,
    remaining_budget_ms: i64,
) -> Result<KernelModelResponse, String> {
    deliver_with_preview(
        runtime,
        config,
        api_key,
        binding,
        frame,
        token,
        remaining_budget_ms,
        None,
        None,
        None,
    )
}

pub(super) fn deliver_with_preview(
    runtime: &RuntimeCommand, config: &KernelModelConfig, api_key: &str,
    binding: &RunControlBinding, frame: &KernelBatchResumeFrame, token: &CancellationToken,
    remaining_budget_ms: i64, preview: Option<&PreviewSink>, database: Option<&crate::database::Database>,
    diagnostic_attempt: Option<&str>,
) -> Result<KernelModelResponse, String> {
    token.check()?;
    frame.validate()?;
    let payload = call_model(
        runtime,
        config,
        api_key,
        binding,
        "kernel.resume_batch",
        json!({"controlBinding":binding,"batchResume":frame}),
        token,
        remaining_budget_ms,
        preview,
        database,
        diagnostic_attempt,
    )?;
    if payload["idempotencyKey"] != frame.idempotency_key
        || payload["checkpointSeq"] != frame.checkpoint_seq
    {
        return Err("Kernel worker returned a different delivery cursor".into());
    }
    let response: KernelModelResponse = serde_json::from_value(payload["response"].clone()).map_err(|_| "invalid Kernel model response")?;
    response.validate()?;
    if response.run_id != binding.run_id || response.turn_id != frame.turn_id || response.batch_id != frame.batch_id
        || response.checkpoint_seq != frame.checkpoint_seq { return Err("Kernel model response identity mismatch".into()); }
    token.check()?;
    Ok(response)
}

pub(crate) fn deliver_initial(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    frame: &fox_engine_protocol::KernelInitialModelFrame,
    token: &CancellationToken,
    remaining_budget_ms: i64,
) -> Result<fox_engine_protocol::KernelInitialModelResponse, String> {
    deliver_initial_with_preview(
        runtime,
        config,
        api_key,
        binding,
        frame,
        token,
        remaining_budget_ms,
        None,
        None,
        None,
    )
}

pub(super) fn deliver_initial_with_preview(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    frame: &fox_engine_protocol::KernelInitialModelFrame,
    token: &CancellationToken,
    remaining_budget_ms: i64,
    preview: Option<&PreviewSink>,
    database: Option<&crate::database::Database>,
    diagnostic_attempt: Option<&str>,
) -> Result<fox_engine_protocol::KernelInitialModelResponse, String> {
    token.check()?;
    frame.validate()?;
    if frame.input.run_id != binding.run_id || frame.input.prompt_config_hash != config.hash()? {
        return Err("initial input configuration mismatch".into());
    }
    let payload = call_model(
        runtime,
        config,
        api_key,
        binding,
        "kernel.start_initial",
        json!({"controlBinding":binding,"initialModel":frame}),
        token,
        remaining_budget_ms,
        preview,
        database,
        diagnostic_attempt,
    )?;
    if payload["idempotencyKey"] != frame.idempotency_key
        || payload["checkpointSeq"] != frame.checkpoint_seq
    {
        return Err("initial delivery cursor mismatch".into());
    }
    let response: fox_engine_protocol::KernelInitialModelResponse =
        serde_json::from_value(payload["response"].clone())
            .map_err(|_| "invalid initial model response")?;
    response.validate()?;
    if response.run_id != binding.run_id || response.turn_id != frame.input.turn_id || response.checkpoint_seq != frame.checkpoint_seq {
        return Err("initial model response identity mismatch".into());
    }
    token.check()?;
    Ok(response)
}

fn call_model(
    runtime: &RuntimeCommand,
    config: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    kind: &str,
    mut request_payload: Value,
    token: &CancellationToken,
    remaining_budget_ms: i64,
    preview: Option<&PreviewSink>,
    database: Option<&crate::database::Database>,
    diagnostic_attempt: Option<&str>,
) -> Result<Value, String> {
    config.hash()?;
    binding.validate()?;
    if binding.engine_id != config.engine_id || binding.authority != fox_engine_protocol::ExecutionAuthority::Authoritative
        || config.execution_profile_id != binding.execution_profile_id {
        return Err("isolated Kernel model identity mismatch".into());
    }
    let requested_budget = binding.budgets.limit_operation_ms(binding.budgets.model_request_ms, 0).min(remaining_budget_ms);
    if requested_budget <= 0 { return Err("Kernel worker has no remaining execution budget".into()); }
    // This quota is independent of the run quota. Its FIFO wait is bounded by
    // the run's original request budget and responds to cancellation.
    let wait_started = Instant::now();
    let request_id = format!("{}:{}", binding.run_id, uuid::Uuid::new_v4());
    let permit = model_request_gate().acquire(&request_id, ||
        token.check().is_err() || wait_started.elapsed().as_millis() >= requested_budget as u128,
        "Kernel model request capacity wait exceeded its budget");
    if permit.is_err() { token.check()?; }
    let _request_permit = permit?;
    token.check()?;
    let budget = requested_budget.saturating_sub(wait_started.elapsed().as_millis() as i64);
    if budget <= 0 { return Err("Kernel worker has no remaining execution budget".into()); }
    let deadline = Instant::now() + Duration::from_millis(budget.try_into().map_err(|_| "invalid model budget")?);
    let session_id = format!("kernel-once-{}", uuid::Uuid::new_v4());
    let request = |kind: &str, payload: Value| {
        json!({
            "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request",
            "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(), "type": kind,
            "runId": binding.run_id, "conversationId": binding.conversation_id, "runtimeSessionId": session_id, "payload": payload,
        })
    };
    let mut initialization =
        serde_json::to_value(config).map_err(|_| "invalid Kernel configuration")?;
    initialization["modelService"]["apiKey"] = Value::String(api_key.into());
    // The durable per-round transport requests one model delivery per process;
    // the worker must not open the live round loop on this path.
    initialization["execution"] = json!("once");
    initialization["usageRecords"] = json!(database.is_some());
    // Diagnostic-only: the timeline measures spawn→ready cost. In a production
    // build there is no timeline, so the instant itself must not exist.
    #[cfg(test)]
    let spawn_started = Instant::now();
    let mut worker = Worker::spawn(runtime)?;
    #[cfg(test)]
    if host_trace::enabled() {
        host_trace::record(
            &binding.run_id,
            format!("host:worker_spawned kind={kind} spawn_ms={}", spawn_started.elapsed().as_millis()),
        );
    }
    worker.usage_database=database.cloned();
    let ready = worker.exchange(
        request("kernel.initialize", initialization),
        "kernel.ready",
        token,
        deadline,
    )?;
    if ready["singleUse"] != true
        || ready["resourceExecution"] != false
        || ready["automaticReplay"] != false
        || ready["roundLoop"] != false
        || ready["adapterVersion"] != config.adapter_version()?
    {
        return Err("Kernel worker lacks isolated single-use capability".into());
    }
    #[cfg(test)]
    if host_trace::enabled() {
        host_trace::record(
            &binding.run_id,
            format!(
                "host:worker_ready kind={kind} ready_ms={} budget_ms={budget}",
                spawn_started.elapsed().as_millis()
            ),
        );
    }
    request_payload["streamPreview"] = json!(preview.is_some());
    let expected = if kind == "kernel.compact_context" {
        if ready["hostCompaction"] != true {
            return Err("Kernel worker lacks Host compaction capability".into());
        }
        "kernel.compaction_result"
    } else {
        "kernel.model_response"
    };
    worker.exchange_with_preview(
        request(kind, request_payload),
        expected,
        token,
        deadline,
        preview,
        diagnostic_attempt,
    )
}

pub(super) fn compact_context(
    runtime: &RuntimeCommand,
    frozen: &KernelModelConfig,
    api_key: &str,
    binding: &RunControlBinding,
    input: &fox_engine_protocol::KernelCompactionRequest,
    token: &CancellationToken,
    remaining_ms: i64,
    database: Option<&crate::database::Database>,
) -> Result<fox_engine_protocol::KernelCompactionResponse, String> {
    input.validate()?;
    if input.run_id != binding.run_id {
        return Err("compaction Run identity mismatch".into());
    }
    // The caller verifies the original frozen hash. This derived configuration
    // can only remove capabilities; credentials still arrive separately.
    let mut summarizer = frozen.clone();
    summarizer.system_prompt = crate::kernel_compaction::SUMMARY_PROMPT.into();
    summarizer.proposal_tools.clear();
    let output = frozen.model_service["maxOutputTokens"]
        .as_u64()
        .unwrap_or(8192)
        .min(2048);
    summarizer.model_service["maxOutputTokens"] = json!(output);
    let payload = call_model(
        runtime,
        &summarizer,
        api_key,
        binding,
        "kernel.compact_context",
        json!({"controlBinding":binding,"compaction":input}),
        token,
        remaining_ms,
        None,
        database,
        None,
    )?;
    let response: fox_engine_protocol::KernelCompactionResponse =
        serde_json::from_value(payload).map_err(|_| "invalid compaction response")?;
    response.validate_for(input)?;
    token.check()?;
    Ok(response)
}

/// One long-lived engine session for a single authoritative Run. The worker
/// process drives the engine's own loop; every engine round output is sent to
/// the Host as a `kernel.round_output` request and the Host answers with a
/// durable directive. No engine tool ever executes locally.
pub(super) struct LiveKernelSession {
    worker: Worker,
    session_id: String,
}

impl LiveKernelSession {
    pub(super) fn observe_usage(&mut self,database:&crate::database::Database) {
        self.worker.usage_database=Some(database.clone());
    }
    fn envelope(
        request_type: &str,
        payload: Value,
        binding: &RunControlBinding,
        session_id: &str,
    ) -> Value {
        json!({
            "protocol": PROTOCOL_NAME, "version": PROTOCOL_VERSION, "kind": "request",
            "id": uuid::Uuid::new_v4().to_string(), "timestamp": fox_engine_protocol::timestamp(), "type": request_type,
            "runId": binding.run_id, "conversationId": binding.conversation_id, "runtimeSessionId": session_id, "payload": payload,
        })
    }

    /// Spawn and initialize a loop-capable worker. Older runtimes that do not
    /// advertise `roundLoop` are rejected so the caller can fall back to the
    /// per-round transport.
    pub(super) fn spawn(
        runtime: &RuntimeCommand,
        config: &KernelModelConfig,
        api_key: &str,
        binding: &RunControlBinding,
        token: &CancellationToken,
        deadline: Instant,
    ) -> Result<Self, String> {
        config.hash()?;
        binding.validate()?;
        if binding.engine_id != config.engine_id
            || binding.authority != fox_engine_protocol::ExecutionAuthority::Authoritative
            || config.execution_profile_id != binding.execution_profile_id
        {
            return Err("isolated Kernel model identity mismatch".into());
        }
        let session_id = format!("kernel-loop-{}", uuid::Uuid::new_v4());
        let mut worker = Worker::spawn(runtime)?;
        let mut initialization =
            serde_json::to_value(config).map_err(|_| "invalid Kernel configuration")?;
        initialization["modelService"]["apiKey"] = Value::String(api_key.into());
        // The whole authoritative Run is driven through one live loop session.
        initialization["execution"] = json!("loop");
        initialization["usageRecords"] = json!(true);
        let ready = worker.exchange(
            Self::envelope("kernel.initialize", initialization, binding, &session_id),
            "kernel.ready",
            token,
            deadline,
        )?;
        if ready["singleUse"] != true
            || ready["resourceExecution"] != false
            || ready["automaticReplay"] != false
            || ready["roundLoop"] != true
            || ready["adapterVersion"] != config.adapter_version()?
        {
            return Err("Kernel worker lacks the durable round-loop capability".into());
        }
        Ok(Self { worker, session_id })
    }

    /// Deliver one engine seed (initial or batch resume) and service round
    /// outputs until the engine reports a final answer. `on_round` performs the
    /// durable commit and batch execution for each output and returns the
    /// directive payload. The returned payload is the final model response.
    pub(super) fn run(
        &mut self,
        request_type: &str,
        request_payload: Value,
        binding: &RunControlBinding,
        cancel: &CancellationToken,
        // The remaining execution deadline is recomputed before every blocking
        // wait from durable Host facts. Unlike a fixed Instant, this excludes
        // approval/execution waits (the controller's running clock is suspended
        // then) so a long approval cannot expire the transport deadline.
        deadline_for: &mut dyn FnMut() -> Result<Instant, String>,
        preview: Option<&PreviewSink>,
        on_round: &mut dyn FnMut(
            fox_engine_protocol::KernelRoundOutputFrame,
        ) -> Result<fox_engine_protocol::KernelRoundDirective, String>,
    ) -> Result<Value, String> {
        cancel.check()?;
        let request = Self::envelope(request_type, request_payload, binding, &self.session_id);
        let mut bytes = serde_json::to_vec(&request).map_err(|_| "invalid Kernel request")?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME {
            return Err("Kernel request exceeds frame limit".into());
        }
        let mut stdin = self.worker.stdin.take().ok_or("Kernel worker input unavailable")?;
        let (sender, written) = mpsc::sync_channel(1);
        self.worker.writer = Some(thread::spawn(move || {
            let result = stdin
                .write_all(&bytes)
                .and_then(|_| stdin.flush())
                .map_err(|_| "Kernel worker write failed");
            let _ = sender.send((stdin, result));
        }));
        let (stdin, result) = wait(&written, cancel, deadline_for()?)?;
        self.worker.stdin = Some(stdin);
        if let Some(writer) = self.worker.writer.take() {
            let _ = writer.join();
        }
        result?;
        let mut revision = 0u64;
        // Set once the Host commits Final, before writing its directive. The
        // terminal commit cancels the Run token. One fixed deadline bounds
        // both the Final write and its acknowledgement, with no further work.
        let mut final_deadline = None;
        loop {
            let response = if let Some(drain_deadline) = final_deadline {
                wait_final(&self.worker.messages, drain_deadline)??
            } else {
                wait(&self.worker.messages, cancel, deadline_for()?)??
            };
            if response["protocol"] != PROTOCOL_NAME
                || response["version"] != PROTOCOL_VERSION
                || ["runId", "conversationId", "runtimeSessionId"]
                    .iter()
                    .any(|key| response[key] != request[key])
            {
                return Err(
                    "Kernel worker response identity/type mismatch; reconcile delivery".into(),
                );
            }
            if final_deadline.is_some()
                && (response["kind"] != "response"
                    || response["type"] != "kernel.model_response"
                    || response["requestId"] != request["id"])
            {
                return Err("unexpected Kernel message after Final; no further work admitted".into());
            }
            if response["kind"] == "request" {
                if response["type"] != "kernel.round_output" {
                    return Err("unexpected Kernel worker request; reconcile delivery".into());
                }
                let envelope: fox_engine_protocol::RuntimeEnvelope =
                    serde_json::from_value(response.clone())
                        .map_err(|_| "invalid Kernel round output envelope")?;
                let frame: fox_engine_protocol::KernelRoundOutputFrame =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid Kernel round output frame")?;
                frame.validate()?;
                if frame.turn_id != binding_turn(request_type, &request) {
                    return Err("Kernel round output belongs to another turn".into());
                }
                if response["runId"].as_str() != Some(binding.run_id.as_str()) {
                    return Err("Kernel round output belongs to another run".into());
                }
                let directive = on_round(frame)?;
                directive.validate()?;
                let is_final =
                    directive.kind == fox_engine_protocol::KernelRoundDirectiveKind::Final;
                if is_final {
                    final_deadline = Some(Instant::now() + Duration::from_secs(10));
                }
                let reply = fox_engine_protocol::HostResponse::for_request(
                    &envelope,
                    "kernel.round_directive",
                    serde_json::to_value(&directive).map_err(|_| "invalid directive")?,
                );
                let mut reply_bytes =
                    serde_json::to_vec(&reply).map_err(|_| "invalid directive")?;
                reply_bytes.push(b'\n');
                if reply_bytes.len() > MAX_FRAME {
                    return Err("Kernel directive exceeds frame limit".into());
                }
                let mut stdin =
                    self.worker.stdin.take().ok_or("Kernel worker input unavailable")?;
                let (sender, written) = mpsc::sync_channel(1);
                self.worker.writer = Some(thread::spawn(move || {
                    let result = stdin
                        .write_all(&reply_bytes)
                        .and_then(|_| stdin.flush())
                        .map_err(|_| "Kernel worker write failed");
                    let _ = sender.send((stdin, result));
                }));
                let (stdin, write_result) = if let Some(drain_deadline) = final_deadline {
                    wait_final(&written, drain_deadline)?
                } else {
                    wait(&written, cancel, deadline_for()?)?
                };
                self.worker.stdin = Some(stdin);
                if let Some(writer) = self.worker.writer.take() {
                    let _ = writer.join();
                }
                write_result?;
                continue;
            }
            if response["kind"] != "response" || response["requestId"] != request["id"] {
                return Err(
                    "Kernel worker response identity/type mismatch; reconcile delivery".into(),
                );
            }
            if response["type"] == "kernel.usage_record" {
                if response["payload"]["runId"]!=binding.run_id {return Err("usage Run identity mismatch".into());}
                if let Some(db)=&self.worker.usage_database {db.record_model_usage(&binding.run_id,&response["payload"])?;}
                continue;
            }
            if response["type"] == "kernel.model_preview" {
                let Some(sink) = preview else {
                    return Err("unexpected Kernel model preview".into());
                };
                let notice: fox_engine_protocol::KernelModelPreview =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_| "invalid Kernel model preview")?;
                notice.validate()?;
                if notice.run_id != binding.run_id || notice.revision <= revision {
                    return Err("Kernel model preview order mismatch".into());
                }
                revision = notice.revision;
                cancel.check()?;
                sink(&notice);
                continue;
            }
            if response["type"] == "kernel.model_failure" {
                let failure: fox_engine_protocol::KernelModelFailure =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|error| format!("invalid settled Kernel model failure: {error}"))?;
                failure.validate()?;
                if failure.run_id != binding.run_id {
                    return Err("Kernel failure belongs to another run".into());
                }
                return Err(format!(
                    "{SETTLED_FAILURE_PREFIX}{}",
                    serde_json::to_string(&failure).map_err(|_| "invalid failure")?
                ));
            }
            if response["type"] != "kernel.model_response" {
                return Err(
                    "Kernel worker response identity/type mismatch; reconcile delivery".into(),
                );
            }
            return Ok(response["payload"].clone());
        }
    }
}

fn binding_turn<'a>(request_type: &str, request: &'a Value) -> &'a str {
    if request_type == "kernel.start_initial" {
        request["payload"]["initialModel"]["input"]["turnId"]
            .as_str()
            .unwrap_or_default()
    } else {
        request["payload"]["batchResume"]["turnId"]
            .as_str()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::{CancellationPort, CancellationRegistry};

    fn script(source: &str) -> (std::path::PathBuf, RuntimeCommand) {
        let root = std::env::temp_dir().join(format!("fox-kernel-pipe-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let script = root.join("worker.mjs");
        std::fs::write(&script, source).unwrap();
        (
            root,
            RuntimeCommand {
                program: "node".into(),
                script: Some(script),
            },
        )
    }

    #[test]
    fn blocked_pipe_write_is_bounded_and_the_owned_child_is_reaped() {
        let (root, runtime) = script("setInterval(() => {}, 1000)");
        let registry = CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.run_token("r").unwrap();
        let start = Instant::now();
        {
            let mut worker = Worker::spawn(&runtime).unwrap();
            let result = worker.exchange(
                json!({"id":"r","payload":"x".repeat(900_000)}),
                "unused",
                &token,
                Instant::now() + Duration::from_millis(200),
            );
            assert!(result.unwrap_err().contains("deadline"));
        }
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_requested_stderr_pipe_without_a_reader_is_never_selected() {
        assert_eq!(stderr_mode(None), StderrMode::Discard);
        assert_eq!(stderr_mode(Some("")), StderrMode::Discard);
        assert_eq!(stderr_mode(Some("inherit")), StderrMode::Inherit);
        // `piped` used to create a stderr pipe nobody drained. An unknown or
        // withdrawn request must not resurrect that deadlock.
        assert_eq!(stderr_mode(Some("piped")), StderrMode::Discard);
    }

    #[test]
    fn a_worker_flooding_stderr_before_its_first_read_still_answers_the_protocol() {
        // 4 MiB is far past any OS pipe buffer, and it is written *before* the
        // script reads stdin: if the Host's stderr wiring applied backpressure,
        // this round would never return.
        let (root, runtime) = script(
            "import { createInterface } from 'node:readline';\n\
             process.stderr.write('d'.repeat(4 * 1024 * 1024));\n\
             createInterface({ input: process.stdin }).on('line', line => {\n\
               const request = JSON.parse(line);\n\
               process.stdout.write(JSON.stringify({ protocol: 'fox-runtime-jsonl', version: 1,\n\
                 kind: 'response', type: 'kernel.ready', requestId: request.id, payload: {} }) + '\\n');\n\
             });\n\
             setInterval(() => {}, 1000)",
        );
        let registry = CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.run_token("r").unwrap();
        let start = Instant::now();
        {
            let mut worker = Worker::spawn(&runtime).unwrap();
            worker
                .exchange(
                    json!({ "id": "r" }),
                    "kernel.ready",
                    &token,
                    Instant::now() + Duration::from_secs(20),
                )
                .expect("a stderr flood must not block the protocol frame");
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "the round took {:?}",
            start.elapsed()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_stdout_and_foreign_response_are_rejected_without_echoing_content() {
        for source in [
            "process.stdout.write('s'.repeat(1048580)+'\\n'); setInterval(() => {}, 1000)",
            "process.stdin.once('data', () => { process.stdout.write(JSON.stringify({kind:'response',type:'kernel.ready',requestId:'foreign',payload:'secret-provider-data'})+'\\n') }); setInterval(() => {}, 1000)",
        ] {
            let (root, runtime) = script(source);
            let registry = CancellationRegistry::default();
            registry.register_run("r").unwrap();
            let token = registry.run_token("r").unwrap();
            {
                let mut worker = Worker::spawn(&runtime).unwrap();
                let error = worker.exchange(json!({"id":"r"}), "kernel.ready", &token,
                    Instant::now() + Duration::from_secs(10)).unwrap_err();
                assert!(error.contains("oversized") || error.contains("mismatch"), "{error}");
                assert!(!error.contains("secret-provider-data"));
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancellation_during_pipe_wait_cleans_up_without_waiting_for_the_deadline() {
        let (root, runtime) = script("setInterval(() => {}, 1000)");
        let registry = CancellationRegistry::default();
        registry.register_run("r").unwrap();
        let token = registry.run_token("r").unwrap();
        let start = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                registry.request_run_cancel("r");
            });
            let mut worker = Worker::spawn(&runtime).unwrap();
            assert!(worker.exchange(json!({"id":"r"}), "kernel.ready", &token,
                Instant::now() + Duration::from_secs(30)).unwrap_err().contains("cancelled"));
        });
        assert!(start.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn live_final_exchange_drains_only_the_acknowledgement_after_terminal_cancel() {
        for late_round in [false, true] {
            let source = r#"
import {createInterface} from 'node:readline';
let seed;
const send = message => process.stdout.write(JSON.stringify(message)+'\n');
const output = () => ({...seed,kind:'request',type:'kernel.round_output',id:'round',
  payload:{schemaVersion:1,turnId:'t',assistantMessage:{role:'assistant',stopReason:'stop',content:[{type:'text',text:'done'}]}}});
createInterface({input:process.stdin}).on('line',line=>{
  const message=JSON.parse(line);
  if (!seed) {seed=message;send(output());return;}
  if (message.type!=='kernel.round_directive' || message.conversationId!==seed.conversationId
      || message.runId!==seed.runId || message.runtimeSessionId!==seed.runtimeSessionId
      || message.requestId!=='round' || !message.id || !message.timestamp) {process.exit(2);}
  setTimeout(()=>send(LATE_ROUND ? output() : {...seed,kind:'response',type:'kernel.model_response',
    requestId:seed.id,payload:{acknowledged:true}}),30);
});
"#.replace("LATE_ROUND", if late_round { "true" } else { "false" });
            let (root, runtime) = script(&source);
            let binding: RunControlBinding = serde_json::from_value(json!({
                "schemaVersion":1,"runId":"r","conversationId":"c","engineId":"pi",
                "executionProfileId":"legacy","authority":"authoritative","readOnlyExecutor":"rust",
                "permissionSnapshotId":"frozen-test","permission":{"mode":"read_only","projectRoot":null,"grants":[]},
                "budgets":{"modelRequestMs":2000,"toolExecutionMs":2000,"runExecutionMs":10000,"approvalWaitMs":2000}
            })).unwrap();
            let registry = CancellationRegistry::default();
            registry.register_run("r").unwrap();
            let token = registry.run_token("r").unwrap();
            let mut rounds = 0;
            let mut on_round = |_: fox_engine_protocol::KernelRoundOutputFrame| {
                rounds += 1;
                registry.request_run_cancel("r"); // Same signal as a durable terminal commit.
                Ok(fox_engine_protocol::KernelRoundDirective {
                    schema_version:1,kind:fox_engine_protocol::KernelRoundDirectiveKind::Final,
                    batch_id:None,checkpoint_seq:None,preview_seq:None,tools:Vec::new(),prompt:None,
                    steering:Vec::new(),host_job_notices:Vec::new(),
                })
            };
            let mut deadline_for = || {
                assert!(!token.is_cancelled(), "terminal draining must not renew the Run deadline");
                Ok(Instant::now()+Duration::from_secs(3))
            };
            let result = {
                let mut session = LiveKernelSession { worker:Worker::spawn(&runtime).unwrap(),session_id:"s".into() };
                session.run("kernel.start_initial",json!({"initialModel":{"input":{"turnId":"t"}}}),
                    &binding,&token,&mut deadline_for,None,&mut on_round)
            };
            assert_eq!(rounds,1,"no callbacks may execute after Final");
            if late_round { assert!(result.unwrap_err().contains("after Final")); }
            else { assert_eq!(result.unwrap()["acknowledged"],true); }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn final_drain_wait_still_obeys_its_fixed_deadline() {
        let (_sender, receiver) = mpsc::channel::<()>();
        let deadline = Instant::now()+Duration::from_millis(30);
        assert!(wait_final(&receiver,deadline).unwrap_err().contains("deadline"));
        // Reusing an elapsed deadline must fail immediately, not renew it.
        assert!(wait_final(&receiver,deadline).unwrap_err().contains("deadline"));
    }
}
