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
            Some("allow"),
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
    worker: Option<std::thread::JoinHandle<Vec<Value>>>,
}

impl LocalProvider {
    fn start(version: Arc<Mutex<Option<String>>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_worker = stop.clone();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_worker = seen.clone();
        let events = Arc::new(Mutex::new(Vec::new()));
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
                assert!(
                    Instant::now() < deadline,
                    "local Provider exceeded its 20 s deadline"
                );
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            stream.set_nonblocking(true).unwrap();
                            events_worker.lock().unwrap().push("accepted".into());
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
                                events_worker.lock().unwrap().push(format!(
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
                        events_worker
                            .lock()
                            .unwrap()
                            .push(format!("eof bytes={}", client.bytes.len()));
                        clients.swap_remove(index);
                        continue;
                    }
                    let Some(end) = client.bytes.windows(4).position(|part| part == b"\r\n\r\n")
                    else {
                        if client.accepted.elapsed() > Duration::from_secs(12) {
                            events_worker
                                .lock()
                                .unwrap()
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
                seen_worker.lock().unwrap().push(request.clone());
                events_worker
                    .lock()
                    .unwrap()
                    .push(format!("request {index} body_bytes={request_bytes}"));
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
                events_worker
                    .lock()
                    .unwrap()
                    .push(format!("response {index} body_bytes={}", body.len()));
            }
            requests
        });
        Self {
            address,
            stop,
            seen,
            events,
            worker: Some(worker),
        }
    }

    fn request_count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn event_log(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }

    fn finish(mut self) -> Vec<Value> {
        self.stop.store(true, Ordering::SeqCst);
        self.worker
            .take()
            .unwrap()
            .join()
            .expect("local Provider thread")
    }
}

impl Drop for LocalProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if std::thread::panicking() {
            eprintln!(
                "[f1-provider] requests={} events={:?}",
                self.request_count(),
                self.event_log()
            );
        }
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
