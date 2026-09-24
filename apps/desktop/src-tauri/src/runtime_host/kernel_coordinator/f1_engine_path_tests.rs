//! A-F1-R1: capture the actual HTTP Provider input from the authoritative Pi
//! worker. The local Provider is scripted; the Node worker and Host read,
//! projection, admission and file execution paths are real. No UI is involved.

use super::*;
use crate::database::Database;
use crate::runtime_host::{kernel_gateway, managed_files};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

const MIDDLE: &str = "F1-REAL-PROVIDER-MIDDLE-9f82c0";
const SMALL: &str = "alpha\nbeta\ngamma\n";
const CANDIDATE: &str = "ALPHA\nBETA\nGAMMA\n";
const SECOND_CANDIDATE: &str = "Alpha\nBeta\nGamma\n";

fn big_body() -> String {
    format!("{}{}{}", "H".repeat(15_000), MIDDLE, "T".repeat(15_000))
}

fn model(address: SocketAddr) -> crate::kernel_model_config::KernelModelConfig {
    crate::kernel_model_config::KernelModelConfig {
        execution_profile_id: "legacy".into(),
        model_service: json!({"apiType":"openai-completions","modelId":"f1-local-http",
            "baseUrl":format!("http://{address}/v1")}),
        system_prompt: "Use only the received tool result before proposing a write.".into(),
        proposal_tools: ["read", "write_file"]
            .into_iter()
            .map(|name| {
                json!({
                    "name":name,"description":"Host owned file operation",
                    "parameters":{"type":"object","properties":{
                        "path":{"type":"string"},"content":{"type":"string"},
                        "expectedVersion":{"type":"string"}},"required":["path"]}
                })
            })
            .collect(),
        engine_id: "pi".into(),
        native_adapter: None,
    }
}

struct Run {
    db: Database,
    root: PathBuf,
    conversation: String,
    id: String,
    scope: crate::database::KernelHostScope,
    clock: TestClock,
    config: crate::kernel_model_config::KernelModelConfig,
}

fn run_fixture(
    label: &str,
    body: &str,
    config: crate::kernel_model_config::KernelModelConfig,
) -> Run {
    run_fixture_with_mode(label, body, config, "allow")
}

fn run_fixture_with_mode(
    label: &str,
    body: &str,
    config: crate::kernel_model_config::KernelModelConfig,
    mode: &str,
) -> Run {
    let root = std::env::temp_dir().join(format!(
        "fox-f1-engine-{label}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("target.txt"), body).unwrap();
    let db = Database::open(root.join("facts.db")).unwrap();
    let conversation = db
        .create_conversation(
            db.default_agent_id(),
            None,
            Some(root.to_str().unwrap()),
            Some(mode),
        )
        .unwrap();
    let id = db
        .create_run(&conversation.id, "F1 local Provider", None)
        .unwrap()
        .run
        .id;
    let budgets = fox_engine_protocol::TimeBudgets {
        model_request_ms: 15_000,
        model_first_response_ms: 5_000,
        model_idle_ms: 5_000,
        tool_execution_ms: 10_000,
        run_execution_ms: 25_000,
        run_execution_limited: true,
        approval_wait_ms: 10_000,
    };
    let binding = db
        .freeze_kernel_run_control(&id, "legacy", budgets)
        .unwrap();
    let hash = config.hash().unwrap();
    let frozen = kernel::RunFrozenConfig {
        engine_id: "pi".into(),
        kernel_mode: "authoritative".into(),
        capability_manifest_version: 2,
        capability_manifest_hash: "f1-engine-test".into(),
        permission_snapshot_id: binding.permission_snapshot_id.clone(),
        execution_profile_id: binding.execution_profile_id.clone(),
        prompt_config_hash: hash.clone(),
        model_request_timeout_ms: binding.budgets.model_request_ms,
        model_first_response_ms: binding.budgets.model_first_response_ms,
        model_idle_ms: binding.budgets.model_idle_ms,
        tool_execution_timeout_ms: binding.budgets.tool_execution_ms,
        run_execution_budget_ms: binding.budgets.run_execution_ms,
        run_execution_limited: binding.budgets.run_execution_limited,
        approval_wait_timeout_ms: binding.budgets.approval_wait_ms,
        provider_max_retries: 0,
        turn_max_retries: 0,
        experimental_compute_job_notice: false,
    };
    db.kernel_create_run(
        &id,
        "pi",
        "authoritative",
        2,
        &binding.permission_snapshot_id,
        &binding.execution_profile_id,
        &hash,
        &serde_json::to_string(&frozen).unwrap(),
    )
    .unwrap();
    db.freeze_kernel_model_config(&id, &config).unwrap();
    db.freeze_kernel_initial_input(&fox_engine_protocol::KernelInitialModelInput {
        schema_version: 1, run_id: id.clone(), turn_id: "turn-1".into(),
        prompt_config_hash: hash,
        messages: vec![json!({"role":"user","content":[{"type":"text","text":"Read target.txt, then propose a replacement."}]})],
    }).unwrap();
    let scope = kernel_gateway::freeze_scope(&db, &binding, &json!({}), &config).unwrap();
    db.freeze_kernel_host_scope(&id, &scope).unwrap();
    db.apply_runtime_event(&id, 1, &json!({"type":"run.started"}))
        .unwrap();
    Run {
        db,
        root,
        conversation: conversation.id,
        id,
        scope,
        clock: TestClock::new(crate::database::now_ms()),
        config,
    }
}

fn policy(run: &Run) -> kernel_gateway::GatewayPolicy {
    kernel_gateway::GatewayPolicy {
        binding: run.db.run_control_binding(&run.id).unwrap().unwrap(),
        scope: run.scope.clone(),
        database: Some(run.db.clone()),
        sessions_dir: None,
        artifacts_dir: None,
    }
}

fn execute_real(
    run: &Run,
    version: &Mutex<Option<String>>,
    writes: &AtomicUsize,
    binding: &RunControlBinding,
    effect: &kernel::OutboxEffect,
    token: &kernel::CancellationToken,
) -> Result<(bool, Value), String> {
    let payload: Value = serde_json::from_str(&effect.payload_json).map_err(|e| e.to_string())?;
    let tool = payload["tool"].as_str().ok_or("tool missing")?;
    let id = effect.tool_call_id.as_deref().ok_or("tool id missing")?;
    let reader_policy = policy(run);
    let result = crate::runtime_host::kernel_host::execute_claimed_dispatch(
        &run.db,
        binding,
        effect,
        tool,
        &payload,
        |durable_input, claim| {
            let input: Value = serde_json::from_str(durable_input).unwrap();
            let (result, evidence) = if tool == "read" {
                (
                    reader_policy.execute_reader(&run.db, tool, &input, id, token),
                    fox_engine_protocol::ExecutionEvidence::NotStarted,
                )
            } else {
                writes.fetch_add(1, Ordering::SeqCst);
                managed_files::execute_admitted_file(
                    managed_files::ManagedExecutionContext {
                        database: &run.db,
                        backups_dir: &run.root.join("managed-files"),
                        conversation_id: &binding.conversation_id,
                        run_id: &binding.run_id,
                        project_root: binding.permission.project_root.as_deref(),
                        permission_mode: binding.permission.mode.as_str(),
                        scope: &run.scope,
                        sessions_dir: None,
                    },
                    tool,
                    &input,
                    id,
                    claim.credential(),
                    Some(token),
                    &mut || Ok(()),
                )
            };
            let outcome = crate::runtime_host::kernel_host::call_outcome(&result);
            (result, evidence, outcome)
        },
    )?;
    if tool == "read" {
        let value = result["details"]["readVersion"]
            .as_str()
            .ok_or("real read did not produce readVersion")?;
        *version.lock().unwrap() = Some(value.to_owned());
    }
    let success = result["isError"].as_bool() != Some(true);
    Ok((success, result))
}

struct LocalProvider {
    address: SocketAddr,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Vec<Value>>>,
    events: Arc<Mutex<Vec<String>>>,
    started: Instant,
    /// Absolute offset of this Provider's own zero on the shared diagnostic
    /// clock, so Provider-relative offsets have an explicit relation to Host
    /// offsets instead of an assumed common origin.
    anchor_ms: u128,
    printed: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<Vec<Value>>>,
}

/// Monotonic timestamp relative to one Provider's own start.
fn since_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}

/// A mutex whose holder panicked is still readable: the diagnostic must survive
/// the very panic it exists to explain, never turn it into a second failure.
fn lock_recover<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl LocalProvider {
    fn start(version: Arc<Mutex<Option<String>>>) -> Self {
        Self::start_with_repeated_write(version, false)
    }

    fn start_with_repeated_write(version: Arc<Mutex<Option<String>>>, repeat_write: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let started = Instant::now();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = stop.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_worker = seen.clone();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_worker = events.clone();
        let worker = std::thread::spawn(move || {
            struct Client {
                stream: std::net::TcpStream,
                bytes: Vec<u8>,
                accepted: Instant,
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            let mut clients: Vec<Client> = Vec::new();
            while !stop_worker.load(Ordering::SeqCst) {
                // Original failure text, unchanged: the fixture's own deadline,
                // with no lock held and nothing else formatted into it.
                assert!(
                    Instant::now() < deadline,
                    "local Provider exceeded its 20 s deadline"
                );
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(true).unwrap();
                            lock_recover(&events_worker)
                                .push(format!("accepted@{}ms", since_ms(started)));
                            clients.push(Client {
                                stream,
                                bytes: Vec::new(),
                                accepted: Instant::now(),
                            });
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => panic!("local Provider accept: {error}"),
                    }
                }
                let mut ready = None;
                for index in (0..clients.len()).rev() {
                    let client = &mut clients[index];
                    let mut closed = false;
                    loop {
                        let mut buffer = [0u8; 8192];
                        match client.stream.read(&mut buffer) {
                            Ok(0) => {
                                closed = true;
                                break;
                            }
                            Ok(count) => {
                                client.bytes.extend_from_slice(&buffer[..count]);
                                assert!(
                                    client.bytes.len() < 1_048_576,
                                    "Provider request is unexpectedly large"
                                );
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(error) => {
                                lock_recover(&events_worker).push(format!(
                                    "read error={:?} bytes={}",
                                    error.kind(),
                                    client.bytes.len()
                                ));
                                closed = true;
                                break;
                            }
                        }
                    }
                    if closed {
                        lock_recover(&events_worker)
                            .push(format!("eof bytes={}", client.bytes.len()));
                        clients.swap_remove(index);
                        continue;
                    }
                    let Some(end) = client.bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        if client.accepted.elapsed() > Duration::from_secs(12) {
                            lock_recover(&events_worker)
                                .push(format!("idle deadline bytes={}", client.bytes.len()));
                            clients.swap_remove(index);
                        }
                        continue;
                    };
                    let header = String::from_utf8_lossy(&client.bytes[..end]).to_ascii_lowercase();
                    let length: usize = header
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .expect("Content-Length")
                        .trim()
                        .parse()
                        .unwrap();
                    assert!(length < 1_048_576);
                    if client.bytes.len() < end + 4 + length {
                        continue;
                    }
                    let client = clients.swap_remove(index);
                    let request =
                        serde_json::from_slice::<Value>(&client.bytes[end + 4..end + 4 + length])
                            .unwrap();
                    ready = Some((client.stream, client.bytes.len(), request));
                    break;
                }
                let Some((mut stream, request_bytes, request)) = ready else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let index = requests.len();
                lock_recover(&seen_worker).push(request.clone());
                lock_recover(&events_worker).push(format!(
                    "request {index} body_bytes={request_bytes}@{}ms",
                    since_ms(started)
                ));
                requests.push(request);
                let (delta, finish) = match index {
                    0 => (
                        json!({"role":"assistant","tool_calls":[{"index":0,"id":"f1-read",
                        "type":"function","function":{"name":"read",
                        "arguments":"{\"path\":\"target.txt\"}"}}]}),
                        "tool_calls",
                    ),
                    1 => {
                        let expected = version
                            .lock()
                            .unwrap()
                            .clone()
                            .expect("read settled before write proposal");
                        let arguments = json!({"path":"target.txt","content":CANDIDATE,
                            "expectedVersion":expected})
                        .to_string();
                        (
                            json!({"role":"assistant","tool_calls":[{"index":0,"id":"f1-write",
                            "type":"function","function":{"name":"write_file",
                            "arguments":arguments}}]}),
                            "tool_calls",
                        )
                    }
                    2 if repeat_write => (
                        json!({"role":"assistant","tool_calls":[{"index":0,"id":"f3-read-2",
                        "type":"function","function":{"name":"read",
                        "arguments":"{\"path\":\"target.txt\"}"}}]}),
                        "tool_calls",
                    ),
                    3 if repeat_write => {
                        let expected = version.lock().unwrap().clone().expect("second read settled");
                        let arguments = json!({"path":"target.txt","content":SECOND_CANDIDATE,
                            "expectedVersion":expected}).to_string();
                        (
                            json!({"role":"assistant","tool_calls":[{"index":0,"id":"f3-write-2",
                            "type":"function","function":{"name":"write_file",
                            "arguments":arguments}}]}),
                            "tool_calls",
                        )
                    }
                    _ => (json!({"role":"assistant","content":"Finished."}), "stop"),
                };
                let chunks = [
                    json!({"id":"f1-http","object":"chat.completion.chunk","created":1,
                        "model":"f1-local-http","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                    json!({"id":"f1-http","object":"chat.completion.chunk","created":1,
                        "model":"f1-local-http","choices":[{"index":0,"delta":{},"finish_reason":finish}]}),
                ];
                let body = format!(
                    "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
                    chunks[0], chunks[1]
                );
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
                lock_recover(&events_worker).push(format!(
                    "response {index} body_bytes={}@{}ms",
                    body.len(),
                    since_ms(started)
                ));
            }
            requests
        });
        Self {
            address,
            stop,
            seen,
            events,
            started,
            // Recorded unconditionally so the anchor exists whenever a timeline
            // is produced; it costs one monotonic read per Provider.
            anchor_ms: crate::runtime_host::kernel_model_worker::host_trace::origin_elapsed_ms(),
            printed: Arc::new(AtomicBool::new(false)),
            worker: Some(worker),
        }
    }

    fn request_count(&self) -> usize {
        lock_recover(&self.seen).len()
    }

    fn event_log(&self) -> Vec<String> {
        lock_recover(&self.events).clone()
    }

    /// This Provider's own zero on the shared diagnostic clock.
    fn anchor_ms(&self) -> u128 {
        self.anchor_ms
    }

    fn finish(mut self) -> Vec<Value> {
        self.stop.store(true, Ordering::SeqCst);
        // Join first: `expect` must raise the worker's own panic message
        // untouched. Printing before it would risk masking the original error
        // with a second failure, which is exactly what this fixture must not do.
        let outcome = self.worker.take().unwrap().join();
        self.print_diagnostics();
        outcome.expect("local Provider thread")
    }

    /// One line per test naming how many rounds actually reached the Provider,
    /// plus per-event monotonic offsets. `printed` is set only when a line is
    /// actually written, so a suppressed call can never retire a later one.
    fn print_diagnostics(&mut self) {
        let requested = std::env::var_os("FOX_F1_TIMELINE").is_some();
        if !requested && !std::thread::panicking() {
            return;
        }
        if self.printed.swap(true, Ordering::SeqCst) {
            return;
        }
        // The snapshot is taken under one short lock and printed after it is
        // released: a stuck or panicking worker can never make the diagnostic
        // itself block or poison.
        let requests = self.request_count();
        let events = self.event_log();
        eprintln!(
            "[f1-provider] requests={requests} anchor_ms={} events={events:?}",
            self.anchor_ms
        );
    }
}

impl Drop for LocalProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // `finish()` takes the handle before joining. If it is still here, the
        // test unwound before finishing, and this timeline is the only record of
        // where the round's time went.
        let unfinished = self.worker.is_some();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if unfinished || std::thread::panicking() {
            self.print_diagnostics();
        }
    }
}

/// Dump and clear the opt-in Host timeline for one Run, from every exit path.
/// `anchor_ms` is this Provider's own start on the shared diagnostic clock, so
/// Provider-relative offsets (`absolute_ms - anchor_ms`) line up with these Host
/// offsets explicitly rather than by an assumed common zero.
fn dump_host_timeline(label: &str, run_id: &str, anchor_ms: u128) {
    let entries = crate::runtime_host::kernel_model_worker::host_trace::take(run_id);
    if entries.is_empty() && !crate::runtime_host::kernel_model_worker::host_trace::enabled() {
        return;
    }
    eprintln!(
        "[f1-host-timeline] {label} run={run_id} entries={} anchor_ms={anchor_ms}",
        entries.len()
    );
    for (at, event) in entries {
        eprintln!("[f1-host-timeline] {label} @{at}ms {event}");
    }
}

/// Dump and clear the opt-in Host timeline for one Run, from every exit path.
struct F1Timeline {
    label: String,
    run_id: String,
    anchor_ms: u128,
}

impl Drop for F1Timeline {
    fn drop(&mut self) {
        dump_host_timeline(&self.label, &self.run_id, self.anchor_ms);
    }
}

fn version_count(run: &Run) -> i64 {
    let path = run
        .root
        .join("target.txt")
        .canonicalize()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    run.db
        .with_connection(|c| {
            c.query_row(
        "SELECT COUNT(*) FROM managed_file_versions WHERE conversation_id=?1 AND storage_path=?2",
        rusqlite::params![run.conversation, path], |row| row.get(0))
        })
        .unwrap()
}

fn strings(value: &Value) -> String {
    fn append(value: &Value, output: &mut String) {
        match value {
            Value::String(text) => {
                output.push_str(text);
                output.push('\n');
            }
            Value::Array(items) => {
                for item in items {
                    append(item, output);
                }
            }
            Value::Object(fields) => {
                for value in fields.values() {
                    append(value, output);
                }
            }
            _ => {}
        }
    }
    let mut output = String::new();
    append(value, &mut output);
    output
}

fn assert_provider_read_delivery(run: &Run, requests: &[Value], small: bool) {
    assert_eq!(
        requests.len(),
        2,
        "exactly read then write Provider requests"
    );
    let second = strings(&requests[1]["messages"]);
    assert!(
        second.contains("f1-read"),
        "second Provider request includes the actual read result"
    );
    assert!(
        !second.contains(MIDDLE),
        "large-file middle did not reach actual Provider messages"
    );
    if small {
        assert!(second.contains(SMALL), "small file reached Provider intact");
    }
    let raw: String = run
        .db
        .with_connection(|c| {
            c.query_row(
        "SELECT result_json FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='f1-read'",
        [&run.id], |row| row.get(0))
        })
        .unwrap();
    let durable = strings(&serde_json::from_str::<Value>(&raw).unwrap());
    if small {
        assert!(
            durable.contains(SMALL),
            "durable Host read preserves the small body"
        );
    } else {
        assert!(
            durable.contains(MIDDLE),
            "durable Host read preserves the omitted middle"
        );
    }
}

fn assert_pending_and_unchanged(run: &Run, body: &str, writes: &AtomicUsize) {
    let snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
    assert_eq!(
        snapshot.state, "waiting_approval",
        "write needs a dedicated replacement confirmation"
    );
    let state: String = run
        .db
        .with_connection(|c| {
            c.query_row(
                "SELECT state FROM kernel_whole_file_replace_grants WHERE run_id=?1",
                [&run.id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(state, "pending");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(run.root.join("target.txt")).unwrap(),
        body.as_bytes()
    );
    assert_eq!(version_count(run), 0);
}

#[derive(Clone, Copy)]
enum PathKind {
    Live,
    PerRound,
}

fn engine_case(path: PathKind, recovery: bool, small: bool) {
    let body = if small { SMALL.to_owned() } else { big_body() };
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start(version.clone());
    let label = match (path, recovery, small) {
        (PathKind::Live, true, _) => "live-recovery-big",
        (PathKind::Live, false, true) => "live-small",
        (PathKind::Live, false, false) => "live-big",
        (PathKind::PerRound, true, _) => "round-recovery-big",
        (PathKind::PerRound, false, true) => "round-small",
        (PathKind::PerRound, false, false) => "round-big",
    };
    let mut run = run_fixture(label, &body, model(provider.address));
    crate::runtime_host::kernel_model_worker::host_trace::clear(&run.id);
    let _timeline = F1Timeline {
        label: label.to_owned(),
        run_id: run.id.clone(),
        anchor_ms: provider.anchor_ms(),
    };
    let writes = AtomicUsize::new(0);
    let cancellation = CancellationRegistry::default();
    let worker = real_worker_command();
    let gateway = policy(&run);
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&run.db, &run.clock, &run.id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    {
        let execute = |binding: &RunControlBinding,
                       effect: &kernel::OutboxEffect,
                       token: &kernel::CancellationToken| {
            execute_real(&run, &version, &writes, binding, effect, token)
        };
        match path {
            PathKind::Live => {
                let after_commit = |_: &str| Ok(());
                let settle = |_: bool| {
                    let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
                    let (read_done, write_done): (bool, bool) = run.db.with_connection(|c| {
                            Ok((
                                c.query_row("SELECT EXISTS(SELECT 1 FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='f1-read' AND state='completed')", [&run.id], |row| row.get(0))?,
                                c.query_row("SELECT EXISTS(SELECT 1 FROM kernel_tool_calls WHERE run_id=?1 AND tool_call_id='f1-write' AND state='completed')", [&run.id], |row| row.get(0))?,
                            ))
                        })?;
                    if (recovery && read_done)
                        || (small && write_done)
                        || (!small && !recovery && snapshot.state == "waiting_approval")
                    {
                        Err(super::super::live::LIVE_DETACHED.into())
                    } else {
                        Ok(())
                    }
                };
                let outcome = coordinator.dispatch_initial_live(
                    "f1-live",
                    &gateway,
                    &worker,
                    "local-test-only",
                    &execute,
                    &after_commit,
                    &settle,
                );
                if let Err(error) = outcome {
                    assert_eq!(
                        error,
                        super::super::live::LIVE_DETACHED,
                        "direct live entry may only return normally or detach at its safe point; Provider={:?}",
                        provider.event_log()
                    );
                }
            }
            PathKind::PerRound => {
                coordinator
                    .dispatch_initial_with_worker(
                        "f1-round-read",
                        &gateway,
                        &worker,
                        "local-test-only",
                    )
                    .unwrap();
                let snapshot = coordinator.snapshot().unwrap();
                assert!(
                    snapshot
                        .tool_calls
                        .iter()
                        .any(|tool| tool.tool_call_id == "f1-read"),
                    "initial worker produced no read: state={} last_event={:?} Provider={:?}",
                    snapshot.state,
                    run.db.kernel_last_event_type(&run.id).unwrap(),
                    provider.event_log()
                );
                assert!(coordinator
                    .dispatch_tool("f1-read", "f1-round-reader", &execute)
                    .unwrap());
            }
        }
    }
    drop(coordinator);
    if recovery {
        // Close the database after read settlement, before *any* write proposal.
        // A detached live session resumes through the production stored-batch
        // transport, which is per-round; it does not silently restart live.
        let snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
        assert_eq!(
            snapshot.tool_calls.len(),
            1,
            "only the real read may exist before reopen"
        );
        assert_eq!(snapshot.tool_calls[0].tool_call_id, "f1-read");
        assert_eq!(snapshot.tool_calls[0].state, "completed");
        assert!(
            snapshot.pending_effects.iter().any(|effect| effect.kind
                == kernel::OutboxEffectKind::DeliverToolBatch
                && effect.status == kernel::OutboxStatus::Pending),
            "settled read leaves an undelivered durable batch"
        );
        assert_eq!(
            provider.request_count(),
            1,
            "the live/per-round first phase must stop before the write proposal; events={:?}",
            provider.event_log()
        );
        assert_eq!(version_count(&run), 0);
        let root = run.root.clone();
        let id = run.id.clone();
        let conversation = run.conversation.clone();
        let scope = run.scope.clone();
        let config = run.config.clone();
        drop(gateway);
        drop(run);
        run = Run {
            db: Database::open(root.join("facts.db")).unwrap(),
            root,
            conversation,
            id,
            scope,
            clock: TestClock::new(crate::database::now_ms()),
            config,
        };
    }
    if recovery || matches!(path, PathKind::PerRound) {
        let gateway = policy(&run);
        let cancellation = CancellationRegistry::default();
        let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
        let coordinator = KernelCoordinator::reopen(&run.db, &run.clock, &run.id, &cancellation)
            .unwrap()
            .with_preview(&preview);
        let batch = coordinator
            .snapshot()
            .unwrap()
            .pending_effects
            .into_iter()
            .find(|effect| {
                effect.kind == kernel::OutboxEffectKind::DeliverToolBatch
                    && effect.status == kernel::OutboxStatus::Pending
            })
            .and_then(|effect| effect.batch_id)
            .expect("settled read has a pending batch delivery");
        // The owning Host uses this stored-batch entry after a settled read,
        // including when an earlier live session detached before the next
        // proposal. It validates the frozen context and starts a real worker.
        coordinator
            .dispatch_stored_batch_with_worker(
                &batch,
                "f1-round-write",
                &gateway,
                &worker,
                "local-test-only",
            )
            .unwrap();
        let state = coordinator.snapshot().unwrap().state;
        assert_ne!(
            state,
            "failed",
            "stored batch returned after model failure; last event: {:?}; Provider={:?}",
            run.db.kernel_last_event_type(&run.id).unwrap(),
            provider.event_log()
        );
        if small {
            let execute = |binding: &RunControlBinding,
                           effect: &kernel::OutboxEffect,
                           token: &kernel::CancellationToken| {
                execute_real(&run, &version, &writes, binding, effect, token)
            };
            assert!(coordinator
                .dispatch_tool("f1-write", "f1-round-writer", &execute)
                .unwrap());
        }
    }
    let requests = provider.finish();
    assert_provider_read_delivery(&run, &requests, small);
    if small {
        assert_eq!(writes.load(Ordering::SeqCst), 1);
        assert_eq!(
            std::fs::read_to_string(run.root.join("target.txt")).unwrap(),
            CANDIDATE
        );
        assert_eq!(version_count(&run), 1);
    } else {
        assert_pending_and_unchanged(&run, &body, &writes);
    }
}

#[test]
fn f1_live_worker_provider_big_read_requires_replacement_confirmation() {
    engine_case(PathKind::Live, false, false);
}

#[test]
fn f1_live_worker_provider_small_read_allows_replacement() {
    engine_case(PathKind::Live, false, true);
}

#[test]
fn f1_live_read_then_reopened_stored_batch_rejects_first_write() {
    engine_case(PathKind::Live, true, false);
}

#[test]
fn f1_per_round_worker_provider_big_read_requires_replacement_confirmation() {
    engine_case(PathKind::PerRound, false, false);
}

#[test]
fn f1_per_round_worker_provider_small_read_allows_replacement() {
    engine_case(PathKind::PerRound, false, true);
}

#[test]
fn f1_per_round_reopen_after_read_rejects_first_write() {
    engine_case(PathKind::PerRound, true, false);
}

#[path = "f3_approval_tests.rs"]
mod f3_approval_tests;

/// Negative case for the diagnostics themselves: when the Provider reaches its
/// deadline the thread still raises the ORIGINAL failure text, no lock is left
/// poisoned behind it, and the timeline survives so the failure stays
/// explicable. A diagnostic that replaced the real error with a PoisonError (or
/// lost the timeline) would defeat the whole point of the instrumentation.
#[test]
fn local_provider_deadline_preserves_the_original_error_and_its_timeline() {
    let version: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut provider = LocalProvider::start(version);
    // Never connect: nothing is accepted, so the fixture's own absolute deadline
    // is the only thing that can end the worker thread. This is bounded by that
    // same 20 s deadline, which this test deliberately does NOT change.
    let join = provider.worker.take().unwrap().join();
    let payload = join.expect_err("the Provider must fail at its own deadline");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .unwrap_or_else(|| panic!("unexpected Provider panic payload"));
    assert_eq!(
        message, "local Provider exceeded its 20 s deadline",
        "the original failure text must be preserved verbatim"
    );

    // The worker panicked while holding the events lock in the unimplemented
    // case; reading it back must recover rather than raise a second failure.
    let events = provider.event_log();
    assert!(
        events.is_empty(),
        "no request ever reached the Provider, so its timeline must be empty: {events:?}"
    );
    assert_eq!(
        provider.request_count(),
        0,
        "the request count must stay readable after the timeout, not poison"
    );
    assert!(
        !crate::runtime_host::kernel_model_worker::host_trace::runs()
            .contains(&"local-provider-deadline-negative".to_owned()),
        "the negative case must not leave a stray Run timeline behind"
    );
}

/// The suppression contract, exercised on the real method rather than a copy of
/// its logic. It must hold in BOTH switch states, because the ordinary suite runs
/// with the timeline off while a diagnostic run turns it on: at most one line
/// per Provider, and the flag tracks whether a line was actually written.
#[test]
fn provider_diagnostics_emit_at_most_one_line_per_provider() {
    let timeline_on = std::env::var_os("FOX_F1_TIMELINE").is_some();
    let version: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut provider = LocalProvider::start(version);
    let printed = provider.printed.clone();
    assert!(
        !printed.load(Ordering::SeqCst),
        "a fresh Provider must not be marked as already printed"
    );
    provider.print_diagnostics();
    let after_first = printed.load(Ordering::SeqCst);
    provider.print_diagnostics();
    let after_second = printed.load(Ordering::SeqCst);
    assert_eq!(
        after_first, after_second,
        "a repeated call must not change whether anything was printed"
    );
    assert_eq!(
        after_first, timeline_on,
        "the diagnostics print only when the timeline was requested: \
         switch_on={timeline_on}"
    );
    assert_eq!(provider.request_count(), 0);
}

/// A poisoned events lock must not turn into a second failure. The earlier
/// revision formatted the log while holding that lock inside its deadline
/// assert, so the panic unwound through a held guard and every later reader
/// raised `PoisonError` instead of the original timeout.
#[test]
fn provider_diagnostics_recover_from_a_poisoned_event_log() {
    let version: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let mut provider = LocalProvider::start(version);
    lock_recover(&provider.events).push("recorded".into());
    assert_eq!(provider.event_log(), vec!["recorded".to_owned()]);

    // Poison the log exactly the way a panicking worker would.
    let events = provider.events.clone();
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = events.lock().unwrap();
        panic!("simulated worker panic while holding the event log");
    }));
    std::panic::set_hook(hook);
    assert!(poisoned.is_err(), "the simulated worker panic must happen");
    assert!(
        events.is_poisoned(),
        "the event log must really be poisoned for this to prove anything"
    );

    // Reading, counting and printing must all still work: recovering is the
    // difference between an explicable timeout and a confusing PoisonError.
    assert_eq!(provider.request_count(), 0);
    assert_eq!(
        provider.event_log(),
        vec!["recorded".to_owned()],
        "the recovered log must still be readable"
    );
    provider.print_diagnostics();
}

/// The timeline switch is read once and, when off, `record`/`count` must be
/// no-ops. Asserted against the switch itself rather than against global store
/// contents, so an instrumented parallel run with the flag on cannot make it
/// flaky.
#[test]
fn host_timeline_is_a_no_op_while_the_switch_is_off() {
    let switched_on = std::env::var_os("FOX_F1_TIMELINE").is_some();
    assert_eq!(
        crate::runtime_host::kernel_model_worker::host_trace::enabled(),
        switched_on,
        "the timeline must follow the diagnostic switch exactly"
    );
    if !switched_on {
        // A distinct Run id that no other test uses; it must stay invisible.
        assert_eq!(
            crate::runtime_host::kernel_model_worker::host_trace::count(
                "f1-switch-off-probe",
                "probe"
            ),
            0,
            "the counter must not be maintained while the switch is off"
        );
        crate::runtime_host::kernel_model_worker::host_trace::record(
            "f1-switch-off-probe",
            "must not be stored",
        );
        assert!(
            !crate::runtime_host::kernel_model_worker::host_trace::runs()
                .contains(&"f1-switch-off-probe".to_owned()),
            "nothing may be stored while the switch is off"
        );
    }
}
