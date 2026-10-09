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
    sync::mpsc::{self, Receiver},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

const MAX_FRAME: usize = 1_048_576;

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
        return Some("kind".into());
    }
    if response["requestId"] != request["id"] {
        return Some("requestId".into());
    }
    for key in ["runId", "conversationId", "runtimeSessionId"] {
        if response[key] != request[key] {
            return Some(key.into());
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
const RESPONSE_ADMISSION_PREFIX: &str = "kernel.response_admission_failed:";

/// Classification comes from a fixed owned transport branch. Worker facts stay
/// explicitly marked as observations and never grant retry or tool authority.
pub(super) fn response_failure_code(error: &str) -> Option<(&'static str, &'static str)> {
    match error.strip_prefix(RESPONSE_ADMISSION_PREFIX)? {
        "length" | "truncated_tool_proposal" | "incomplete_response" => Some(("kernel.model_response_incomplete", "模型响应未完整结束，本轮提案未接纳，未自动重放。")),
        "protocol_invalid" | "invalid_tool_arguments" => Some(("kernel.model_response_invalid", "模型响应或工具参数无效，本轮提案未接纳，未自动重放。")),
        "argument_validation_unavailable" => Some(("kernel.model_response_unverified", "工具参数完整性检查超出有界预算，本轮提案未接纳，未自动重放。")),
        "stream_interrupted" => Some(("kernel.model_response_interrupted", "模型响应流中断，结果未确认，未自动重放。")),
        "worker_exception" | "unknown" => Some(("kernel.model_worker_failed", "模型 worker 未返回可接纳结果，原因见有界诊断，未自动重放。")),
        _ => None,
    }
}

fn diagnostic_enum(value: &Value, key: &str, allowed: &[&str]) -> String {
    value[key].as_str().filter(|item| allowed.contains(item)).unwrap_or("unknown").into()
}

fn sanitized_response_diagnostic(source: &Value) -> Value {
    let number = |key: &str| source[key].as_u64().filter(|value| *value <= 67_108_864);
    let usage_request = source["usageRequestId"].as_str().filter(|value| !value.is_empty() && value.len() <= 128
        && value.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'_')));
    json!({"schemaVersion":1,"source":"worker_observation",
        "stage":diagnostic_enum(source,"stage",&["provider_stream","response_projection","tool_proposal","worker_result"]),
        "category":diagnostic_enum(source,"category",&["length","invalid_tool_arguments","truncated_tool_proposal",
            "protocol_invalid","stream_interrupted","worker_exception","cancelled","incomplete_response","argument_validation_unavailable"]),
        "providerFinishReason":diagnostic_enum(source,"providerFinishReason",&["stop","length","tool_calls","function_call","content_filter"]),
        "adapterStopReason":diagnostic_enum(source,"adapterStopReason",&["stop","length","toolUse","error","aborted"]),
        "maxOutputTokens":number("maxOutputTokens"),"outputTokens":number("outputTokens"),
        "usageRequestId":usage_request,"resultComplete":false})
}

fn model_payload_without_diagnostic(payload: &Value) -> Value {
    let mut value = payload.clone();
    if let Some(object) = value.as_object_mut() { object.remove("responseDiagnostic"); }
    value
}

/// The worker's own branch name, admitted only through this allow-list.
///
/// `incomplete_response` covers several distinct causes, so the branch has to survive
/// the category — but a worker string the Host does not recognise must never cross
/// verbatim, and neither may free-form worker text. Anything unlisted reads `unknown`.
/// `incomplete_tool_proposal` and `stream_interrupted` are listed because those exits
/// are already distinguishable elsewhere; naming them here keeps one vocabulary.
fn model_failure_reason(reason: Option<&str>) -> &'static str {
    match reason {
        Some("empty_final_answer") => "empty_final_answer",
        Some("missing_final_marker") => "missing_final_marker",
        Some("output_length_limit") => "output_length_limit",
        Some("incomplete_tool_proposal") => "incomplete_tool_proposal",
        Some("stream_interrupted") => "stream_interrupted",
        Some("transport_no_output") => "transport_no_output",
        _ => "unknown",
    }
}

/// A settled model failure's own observations, projected into bounded JSON for the
/// Run's durable response diagnostic.
///
/// Each field is admitted only when the worker actually reported it, so an unobserved
/// fact stays absent and a reader can tell "never seen" from "seen as zero". Only
/// enumerated values, bounded counters and internal dispatch identity cross: no
/// credential, prompt text, model input or tool argument.
fn sanitized_model_failure_diagnostic(failure: &fox_engine_protocol::KernelModelFailure) -> Value {
    let token = |value: &Option<String>, limit: usize| -> Option<String> {
        value.as_ref()
            .filter(|text| !text.is_empty() && text.len() <= limit
                && text.bytes().all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'_' | b'-' | b'.' | b':' | b'/')))
            .cloned()
    };
    let mut out = serde_json::Map::new();
    out.insert("reason".into(), json!(model_failure_reason(
        failure.diagnostic.as_ref().and_then(|item| item.reason.as_deref()))));
    if let Some(item) = &failure.diagnostic {
        if let Some(stop) = token(&item.stop_reason, 32) { out.insert("stopReason".into(), json!(stop)); }
        if let Some(value) = item.text_chars { out.insert("textChars".into(), json!(value)); }
        if let Some(value) = item.thinking_chars { out.insert("thinkingChars".into(), json!(value)); }
        if let Some(value) = item.tool_call_count { out.insert("toolCallCount".into(), json!(value)); }
        if let Some(value) = item.completion_required { out.insert("completionRequired".into(), json!(value)); }
        if let Some(value) = item.marker_seen { out.insert("markerSeen".into(), json!(value)); }
        if let Some(dispatch) = token(&item.dispatch_id, 200) { out.insert("dispatchId".into(), json!(dispatch)); }
        if let Some(effect) = token(&item.effect_key, 200) { out.insert("effectKey".into(), json!(effect)); }
    }
    Value::Object(out)
}
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

pub(super) fn bind_runtime_capabilities(config: &mut KernelModelConfig, binding: &RunControlBinding, manifest_hash: &str) {
    let facts=super::host_runtime_capabilities(manifest_hash,&binding.execution_profile_id);
    config.system_prompt.push_str(&format!("\n\nHost runtime availability for this frozen manifest/profile:\n{facts}\nTool schemas declare contracts; they do not prove environment availability. Do not call unavailable actions or repeat them to seek approval. Approval cannot provide a missing backend. Command status/output/cancel do not start a process. Use available in-process computation and report required capabilities that are unavailable. Host rechecks availability before approval and dispatch."));
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

fn wait_observed<T>(receiver: &Receiver<T>, token: &CancellationToken, deadline: Instant,
    database: Option<&crate::database::Database>, run_id: &str, request_id: &str,
    phase: &str, started: Instant, last_notice: &mut i64, identity: Option<(i64,i64)>) -> Result<T, String> {
    loop {
        token.check()?;
        let now = Instant::now();
        let remaining = deadline.checked_duration_since(now).ok_or("Kernel worker deadline exceeded; reconcile delivery")?;
        let elapsed = now.duration_since(started).as_millis().min(i64::MAX as u128) as i64;
        if elapsed >= *last_notice + 15_000 {
            *last_notice = elapsed;
            if let Some(database) = database { let _ = database.record_kernel_model_waiting(run_id, request_id,
                elapsed, remaining.as_millis().min(i64::MAX as u128) as i64, phase, identity); }
        }
        match receiver.recv_timeout(remaining.min(Duration::from_millis(20))) {
            Ok(value) => { token.check()?; return Ok(value); }
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("Kernel worker disconnected; reconcile delivery".into()),
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
    fn record_response_failure(&self, request: &Value, cursor: u64,
        identity: Option<(i64,i64)>, source: &Value, host_category: &str) -> Result<Value,String> {
        let turn = binding_turn(request["type"].as_str().unwrap_or_default(),request);
        let matches = source["schemaVersion"] == 1 && source["turnId"] == turn
            && source["checkpointSeq"].as_u64() == Some(cursor);
        let diagnostic = sanitized_response_diagnostic(if matches {source} else {&Value::Null});
        self.persist_response_diagnostic(request,turn,cursor,identity,&diagnostic,host_category);
        Ok(diagnostic)
    }
    /// Record a settled model failure together with the round facts its worker actually
    /// observed. The provider-neutral category cannot tell an empty answer from one that
    /// never closed its answer, so without this projection the Run keeps only
    /// `incomplete_response` and the branch behind it is lost. The projection is
    /// allow-listed and bounded: no worker string crosses verbatim.
    fn record_settled_model_failure(&self, request: &Value, cursor: u64,
        identity: Option<(i64,i64)>, source: &Value,
        failure: &fox_engine_protocol::KernelModelFailure) -> Result<(),String> {
        let turn = binding_turn(request["type"].as_str().unwrap_or_default(),request);
        let matches = source["schemaVersion"] == 1 && source["turnId"] == turn
            && source["checkpointSeq"].as_u64() == Some(cursor);
        let mut diagnostic = sanitized_response_diagnostic(if matches {source} else {&Value::Null});
        if let Some(object) = diagnostic.as_object_mut() {
            object.insert("modelFailure".into(), sanitized_model_failure_diagnostic(failure));
        }
        self.persist_response_diagnostic(request,turn,cursor,identity,&diagnostic,"settled_model_failure");
        Ok(())
    }
    fn persist_response_diagnostic(&self, request: &Value, turn: &str, cursor: u64,
        identity: Option<(i64,i64)>, diagnostic: &Value, host_category: &str) {
        if let Some(database) = &self.usage_database {
            if database.record_kernel_response_diagnostic(request["runId"].as_str().unwrap_or_default(),
                request["id"].as_str().unwrap_or_default(),turn,cursor,diagnostic,host_category,identity)
                .is_err() { eprintln!("[kernel-worker] response diagnostic persistence unavailable"); }
        }
    }
    fn observe_transport_error(&self, request:&Value,cursor:u64,identity:Option<(i64,i64)>,error:String)->String {
        let category=if is_reaped_transport_failure(&error) {"transport_outcome_unknown"} else {"protocol_invalid"};
        let _=self.record_response_failure(request,cursor,identity,&Value::Null,category);
        error
    }

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
        let waiting_started = Instant::now();
        let mut last_waiting_notice = 0;
        let waiting_identity = self.usage_database.as_ref().and_then(|db| db.kernel_model_wait_identity(run_id).ok().flatten());
        let cursor = request["payload"].get("initialModel").or_else(||request["payload"].get("batchResume"))
            .and_then(|frame|frame["checkpointSeq"].as_u64()).unwrap_or(0);
        loop {
            let response = if model_round || expected == "kernel.compaction_result" {
                wait_observed(&self.messages, token, deadline, self.usage_database.as_ref(), run_id,
                    attempt_id, if model_round {"model_response"} else {"context_compaction"}, waiting_started, &mut last_waiting_notice, waiting_identity)
                    .and_then(|result|result).map_err(|error|if model_round {
                        self.observe_transport_error(&request,cursor,waiting_identity,error)
                    } else {error})?
            } else { wait(&self.messages, token, deadline)?? };
            if let Some(reason) = response_identity_mismatch(&request, &response) {
                if model_round { self.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?; }
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
                    serde_json::from_value(model_payload_without_diagnostic(&response["payload"]))
                        .map_err(|_|self.observe_transport_error(&request,cursor,waiting_identity,"invalid settled Kernel model failure".into()))?;
                failure.validate().map_err(|error|self.observe_transport_error(&request,cursor,waiting_identity,error))?;
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
                    self.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
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
                self.record_settled_model_failure(&request,cursor,waiting_identity,&response["payload"]["responseDiagnostic"],&failure)?;
                return Err(format!(
                    "{SETTLED_FAILURE_PREFIX}{}",
                    serde_json::to_string(&failure).map_err(|_| "invalid failure")?
                ));
            }
            if response["type"] != expected {
                if model_round {
                    let diagnostic=self.record_response_failure(&request,cursor,waiting_identity,
                        &response["payload"]["responseDiagnostic"],if response["type"]=="request_failed" {"worker_rejected"} else {"protocol_invalid"})?;
                    let category=if response["type"]=="request_failed" {diagnostic["category"].as_str().unwrap_or("unknown")} else {"protocol_invalid"};
                    return Err(format!("{RESPONSE_ADMISSION_PREFIX}{category}"));
                }
                if expected == "kernel.compaction_result" && response["type"] == "kernel.compaction_failure" {
                    let source=&response["payload"];
                    if source["runId"] != request["runId"] || source["compactionId"] != request["payload"]["compaction"]["compactionId"] || source["schemaVersion"] != 1 {
                        return Err("Kernel compaction failure identity mismatch".into());
                    }
                    let mut diagnostic=source.clone();
                    if let Some(object)=diagnostic.as_object_mut(){object.retain(|key,_|matches!(key.as_str(),"schemaVersion"|"category"|"httpStatus"|"upstreamMessage"|"upstreamCode"|"outcomeKnown"|"retryable"));}
                    diagnostic["upstreamMessage"]=json!(super::redact_execution_diagnostic(source["upstreamMessage"].as_str().unwrap_or("No upstream message was supplied"),600));
                    return Err(format!("kernel.compaction_worker_failure:{diagnostic}"));
                }
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
    token.check()?;
    let deadline = Instant::now() + Duration::from_millis(requested_budget.try_into().map_err(|_| "invalid model budget")?);
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
    initialization["responseDiagnostics"] = json!(1);
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
                "host:worker_ready kind={kind} ready_ms={} budget_ms={requested_budget}",
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
    let request=request(kind, request_payload);
    let cursor=request["payload"].get("initialModel").or_else(||request["payload"].get("batchResume"))
        .and_then(|frame|frame["checkpointSeq"].as_u64()).unwrap_or(0);
    let identity=database.and_then(|db|db.kernel_model_wait_identity(&binding.run_id).ok().flatten());
    let payload=worker.exchange_with_preview(
        request.clone(),
        expected,
        token,
        deadline,
        preview,
        diagnostic_attempt,
    )?;
    if expected=="kernel.model_response" {
        let source=&payload["response"];
        let valid=if kind=="kernel.start_initial" {
            serde_json::from_value::<fox_engine_protocol::KernelInitialModelResponse>(source.clone())
                .map_err(|_|"invalid Kernel model result".into()).and_then(|value|value.validate())
        } else {
            serde_json::from_value::<KernelModelResponse>(source.clone())
                .map_err(|_|"invalid Kernel model result".into()).and_then(|value|value.validate())
        };
        if valid.is_err() {
            worker.record_response_failure(&request,cursor,identity,&Value::Null,"protocol_invalid")?;
            return Err(format!("{RESPONSE_ADMISSION_PREFIX}protocol_invalid"));
        }
    }
    Ok(payload)
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
        initialization["responseDiagnostics"] = json!(1);
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
        let mut waiting_started = Instant::now();
        let mut last_waiting_notice = 0;
        let mut waiting_identity = self.worker.usage_database.as_ref().and_then(|db| db.kernel_model_wait_identity(&binding.run_id).ok().flatten());
        let mut cursor = request["payload"].get("initialModel").or_else(||request["payload"].get("batchResume"))
            .and_then(|frame|frame["checkpointSeq"].as_u64()).unwrap_or(0);
        // Set once the Host commits Final, before writing its directive. The
        // terminal commit cancels the Run token. One fixed deadline bounds
        // both the Final write and its acknowledgement, with no further work.
        let mut final_deadline = None;
        loop {
            let response = if let Some(drain_deadline) = final_deadline {
                wait_final(&self.worker.messages, drain_deadline)??
            } else {
                wait_observed(&self.worker.messages, cancel, deadline_for()?, self.worker.usage_database.as_ref(),
                    &binding.run_id, request["id"].as_str().unwrap_or_default(), "model_response", waiting_started, &mut last_waiting_notice, waiting_identity)
                    .and_then(|result|result).map_err(|error|self.worker.observe_transport_error(&request,cursor,waiting_identity,error))?
            };
            if response["protocol"] != PROTOCOL_NAME
                || response["version"] != PROTOCOL_VERSION
                || ["runId", "conversationId", "runtimeSessionId"]
                    .iter()
                    .any(|key| response[key] != request[key])
            {
                self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
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
                    self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
                    return Err("unexpected Kernel worker request; reconcile delivery".into());
                }
                let envelope: fox_engine_protocol::RuntimeEnvelope =
                    serde_json::from_value(response.clone())
                        .map_err(|_| "invalid Kernel round output envelope")?;
                let frame: fox_engine_protocol::KernelRoundOutputFrame =
                    serde_json::from_value(response["payload"].clone())
                        .map_err(|_|self.worker.observe_transport_error(&request,cursor,waiting_identity,"invalid Kernel round output frame".into()))?;
                frame.validate().map_err(|error|self.worker.observe_transport_error(&request,cursor,waiting_identity,error))?;
                if frame.turn_id != binding_turn(request_type, &request) {
                    self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
                    return Err("Kernel round output belongs to another turn".into());
                }
                if response["runId"].as_str() != Some(binding.run_id.as_str()) {
                    return Err("Kernel round output belongs to another run".into());
                }
                let directive = on_round(frame)?;
                waiting_started = Instant::now();
                last_waiting_notice = 0;
                waiting_identity = self.worker.usage_database.as_ref().and_then(|db| db.kernel_model_wait_identity(&binding.run_id).ok().flatten());
                directive.validate()?;
                if let Some(next)=directive.checkpoint_seq.or(directive.preview_seq) {cursor=next;}
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
                self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
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
                    serde_json::from_value(model_payload_without_diagnostic(&response["payload"]))
                        .map_err(|_|self.worker.observe_transport_error(&request,cursor,waiting_identity,"invalid settled Kernel model failure".into()))?;
                failure.validate().map_err(|error|self.worker.observe_transport_error(&request,cursor,waiting_identity,error))?;
                if failure.run_id != binding.run_id || failure.turn_id != binding_turn(request_type,&request)
                    || failure.checkpoint_seq != cursor {
                    self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
                    return Err("Kernel failure belongs to another run".into());
                }
                self.worker.record_settled_model_failure(&request,cursor,waiting_identity,&response["payload"]["responseDiagnostic"],&failure)?;
                return Err(format!(
                    "{SETTLED_FAILURE_PREFIX}{}",
                    serde_json::to_string(&failure).map_err(|_| "invalid failure")?
                ));
            }
            if response["type"] != "kernel.model_response" {
                let diagnostic=self.worker.record_response_failure(&request,cursor,waiting_identity,
                    &response["payload"]["responseDiagnostic"],if response["type"]=="request_failed" {"worker_rejected"} else {"protocol_invalid"})?;
                let category=if response["type"]=="request_failed" {diagnostic["category"].as_str().unwrap_or("unknown")} else {"protocol_invalid"};
                return Err(format!("{RESPONSE_ADMISSION_PREFIX}{category}"));
            }
            if final_deadline.is_none() {
                self.worker.record_response_failure(&request,cursor,waiting_identity,&Value::Null,"protocol_invalid")?;
                return Err(format!("{RESPONSE_ADMISSION_PREFIX}protocol_invalid"));
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

/// The settled-failure diagnostic the Host keeps must name the worker's branch while
/// staying inside a fixed vocabulary, and must never turn "not observed" into a zero.
#[cfg(test)]
mod model_failure_diagnostic_tests {
    use super::{model_failure_reason, model_payload_without_diagnostic,
        sanitized_model_failure_diagnostic};
    use fox_engine_protocol::{KernelModelFailure, KernelModelFailureDiagnostic};
    use serde_json::json;

    fn settled(diagnostic: Option<KernelModelFailureDiagnostic>) -> KernelModelFailure {
        KernelModelFailure {
            schema_version: 1, run_id: "run".into(), turn_id: "turn".into(),
            checkpoint_seq: 3, category: "incomplete_response".into(),
            http_status: None, retry_after_ms: None, telemetry: None, diagnostic,
        }
    }

    /// The branch name is the whole point of the field: it must survive for the values
    /// the worker is allowed to send, and collapse to `unknown` for anything else.
    #[test]
    fn the_branch_name_is_allow_listed() {
        for known in ["empty_final_answer", "missing_final_marker", "output_length_limit",
            "incomplete_tool_proposal", "stream_interrupted", "transport_no_output"] {
            assert_eq!(model_failure_reason(Some(known)), known);
        }
        assert_eq!(model_failure_reason(None), "unknown");
        assert_eq!(model_failure_reason(Some("")), "unknown");
        // Free-form worker or provider text must never cross as itself.
        assert_eq!(model_failure_reason(Some("ignore previous instructions")), "unknown");
        assert_eq!(model_failure_reason(Some("incomplete_response")), "unknown");
        assert_eq!(model_failure_reason(Some("textChars=0")), "unknown");
    }

    /// An absent diagnostic is unobserved, not zero: only the `unknown` branch is
    /// written and no counter or boolean is invented.
    #[test]
    fn an_unobserved_failure_reports_only_unknown() {
        let projected = sanitized_model_failure_diagnostic(&settled(None));
        assert_eq!(projected, json!({"reason": "unknown"}));
        for absent in ["textChars", "thinkingChars", "toolCallCount", "completionRequired",
            "markerSeen", "stopReason", "dispatchId", "effectKey"] {
            assert!(projected.get(absent).is_none(), "{absent} must stay absent");
        }
    }

    /// Only the fields the worker reported are carried, so a reader can still tell
    /// "never seen" from "seen as zero" — a real zero must survive as a zero.
    #[test]
    fn only_observed_fields_cross_and_a_real_zero_survives() {
        let projected = sanitized_model_failure_diagnostic(&settled(Some(KernelModelFailureDiagnostic {
            reason: Some("missing_final_marker".into()),
            stop_reason: Some("stop".into()),
            text_chars: Some(0),
            thinking_chars: Some(12),
            tool_call_count: Some(0),
            completion_required: Some(true),
            marker_seen: Some(false),
            dispatch_id: Some("model-batch:run:30".into()),
            effect_key: None,
        })));
        assert_eq!(projected, json!({
            "reason": "missing_final_marker", "stopReason": "stop",
            "textChars": 0, "thinkingChars": 12, "toolCallCount": 0,
            "completionRequired": true, "markerSeen": false,
            "dispatchId": "model-batch:run:30",
        }));
        assert!(projected.get("effectKey").is_none());
    }

    /// A stop reason the worker copied from provider text is a token, not prose: an
    /// unbounded or punctuated value is dropped rather than recorded.
    #[test]
    fn a_non_token_stop_reason_is_dropped() {
        let projected = sanitized_model_failure_diagnostic(&settled(Some(KernelModelFailureDiagnostic {
            reason: Some("output_length_limit".into()),
            stop_reason: Some("stopped because\n the model said so".into()),
            ..Default::default()
        })));
        assert_eq!(projected, json!({"reason": "output_length_limit"}));
    }

    /// Compatibility direction: a payload an older sidecar sends — with no `diagnostic`
    /// at all — must still be accepted and validated by this Host.
    #[test]
    fn an_old_sidecar_payload_without_a_diagnostic_is_still_accepted() {
        let legacy: KernelModelFailure = serde_json::from_value(json!({
            "schemaVersion": 1, "runId": "r", "turnId": "t", "checkpointSeq": 2,
            "category": "incomplete_response", "httpStatus": null, "retryAfterMs": null,
        })).expect("the older wire shape must still deserialize");
        assert!(legacy.validate().is_ok(), "and it must still validate");
        assert!(legacy.diagnostic.is_none());
        assert_eq!(sanitized_model_failure_diagnostic(&legacy), json!({"reason": "unknown"}));
    }

    /// The new shape also has to survive the exact path the Host uses on the wire:
    /// strip the side-channel `responseDiagnostic`, then deserialize and validate.
    #[test]
    fn the_new_shape_survives_the_host_wire_path() {
        let payload = json!({
            "schemaVersion": 1, "runId": "r", "turnId": "t", "checkpointSeq": 2,
            "category": "incomplete_response", "httpStatus": null, "retryAfterMs": null,
            "diagnostic": {"reason": "transport_no_output", "textChars": 0,
                "completionRequired": false, "markerSeen": false},
            "responseDiagnostic": {"schemaVersion": 1, "stage": "provider_stream"},
        });
        let stripped = model_payload_without_diagnostic(&payload);
        assert!(stripped.get("responseDiagnostic").is_none(), "the side channel is stripped first");
        let failure: KernelModelFailure = serde_json::from_value(stripped)
            .expect("the settled failure must deserialize with deny_unknown_fields intact");
        assert!(failure.validate().is_ok());
        assert_eq!(sanitized_model_failure_diagnostic(&failure), json!({
            "reason": "transport_no_output", "textChars": 0,
            "completionRequired": false, "markerSeen": false,
        }));
    }

    /// A worker that sends a field this Host does not know is a protocol error, not a
    /// silently accepted payload: `deny_unknown_fields` must still hold inside the
    /// nested diagnostic, or a future field could disable diagnosis unnoticed.
    #[test]
    fn an_unknown_diagnostic_field_is_rejected() {
        let rejected = serde_json::from_value::<KernelModelFailure>(json!({
            "schemaVersion": 1, "runId": "r", "turnId": "t", "checkpointSeq": 2,
            "category": "incomplete_response", "httpStatus": null, "retryAfterMs": null,
            "diagnostic": {"reason": "empty_final_answer", "futureField": 1},
        }));
        assert!(rejected.is_err(), "an unknown nested field must fail closed");
    }
}
