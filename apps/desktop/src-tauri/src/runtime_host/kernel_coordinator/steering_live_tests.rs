//! Live-loop tests for additional user input accepted mid-run ("运行中补充要求").
//!
//! The durable state machine itself is covered by `steering_tests.rs`; these
//! tests drive the REAL live loop over a scripted HTTP/SSE provider so the
//! end-of-turn decision is exercised: input the user was told Fox had accepted
//! must be answered by a model round, never silently discarded by a normal
//! completion.

use super::steering_tests::{steering_model_fixture, stop_response};
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Scripted provider: a tool proposal round or a plain stop per request. The
/// text of a stop reply is scripted too, so a test can tell the rounds apart.
fn steering_provider(
    script: Vec<(&'static str, &'static str)>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    steering_provider_shared(script).0
}

/// Same provider, but it also returns the live view of served requests so a test
/// can assert on a round the Run dispatched *after* the scripted stop, without
/// needing an extra script entry that would never be served.
fn steering_provider_shared(
    script: Vec<(&'static str, &'static str)>,
) -> (
    (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>),
    Arc<Mutex<Vec<Value>>>,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let publisher = Arc::clone(&seen);
    let (address, server) = steering_provider_inner(script, publisher);
    ((address, server), seen)
}

fn steering_provider_inner(
    script: Vec<(&'static str, &'static str)>,
    publisher: Arc<Mutex<Vec<Value>>>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (reply, call_id) in script {
            let deadline = Instant::now() + Duration::from_secs(25);
            let (mut stream, request) = loop {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing model request");
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_millis(300))).unwrap();
                stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut bytes = Vec::new();
                let request = loop {
                    let mut buffer = [0u8; 8192];
                    let n = match stream.read(&mut buffer) { Ok(n) if n > 0 => n, _ => break None };
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() < 1_048_576);
                    if let Some(end) = bytes.windows(4).position(|p| p == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length:"))
                            .unwrap()
                            .trim()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break Some(
                                serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                                    .unwrap(),
                            );
                        }
                    }
                };
                if let Some(request) = request {
                    break (stream, request);
                }
                assert!(Instant::now() < deadline, "only abandoned sockets");
            };
            requests.push(request.clone());
            publisher.lock().unwrap().push(request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let send = |stream: &mut std::net::TcpStream, delta: Value, finish: Value| {
                let body = format!(
                    "data: {}\n\n",
                    json!({"id":"steering-live","object":"chat.completion.chunk","created":1,"model":"steering-live",
                        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
                );
                write!(stream, "{:x}\r\n{}\r\n", body.len(), body)
            };
            if reply == "tool" {
                send(
                    &mut stream,
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":call_id,"type":"function",
                        "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]}),
                    Value::Null,
                )
                .unwrap();
                send(&mut stream, json!({}), json!("tool_calls")).unwrap();
            } else {
                // Same two-chunk shape the other live-loop tests use: the
                // assistant delta carries the text, the terminal chunk carries
                // the finish reason.
                send(
                    &mut stream,
                    json!({"role":"assistant","content":reply}),
                    Value::Null,
                )
                .unwrap();
                send(&mut stream, json!({}), json!("stop")).unwrap();
            }
            let done = "data: [DONE]\n\n";
            write!(stream, "{:x}\r\n{}\r\n0\r\n\r\n", done.len(), done).unwrap();
        }
        requests
    });
    (address, server)
}

/// Wait (bounded) until the provider has served at least `count` requests.
fn await_requests(seen: &Arc<Mutex<Vec<Value>>>, count: usize, label: &str) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let served = seen.lock().unwrap().clone();
        if served.len() >= count {
            return served;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: only {} model request(s) arrived",
            served.len()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct SteeringLive {
    db: Database,
    root: PathBuf,
    run_id: String,
}

fn steering_live_fixture(address: std::net::SocketAddr) -> SteeringLive {
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"steering-live",
        "baseUrl":format!("http://{address}/v1")});
    let clock = TestClock::new(crate::database::now_ms());
    let (db, root, run_id) = fixture_with_start_opt(
        &clock,
        &config.hash().unwrap(),
        Some(&config),
        true,
        true,
    );
    freeze_host_scope(&db, &run_id);
    SteeringLive { db, root, run_id }
}

/// Same fixture, but the Run's task explicitly promises a file, which is what
/// makes the Host review the first stop (so the model gets a second round in
/// which the additional request can arrive).
fn steering_live_fixture_with_deliverable(address: std::net::SocketAddr) -> SteeringLive {
    let live = steering_live_fixture(address);
    let seeds =
        crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1, "the sample task promises exactly one file");
    live.db
        .seed_delivery_checklist(&live.run_id, &seeds, crate::database::now_ms())
        .unwrap();
    live
}

/// The bytes of the promised deliverable: a real, parseable workbook that
/// satisfies every structural delivery check.
fn deliverable_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/office-reading.xlsx"),
    )
    .unwrap()
}

#[test]
fn live_loop_answers_input_accepted_during_the_last_round_instead_of_completing() {
    // The user's additional request is accepted WHILE the first model round is
    // in flight — the dispatch that carries it was already frozen, so the row is
    // still `received` when that round stops. The stop must not complete the Run:
    // the accepted request gets its own model round first. The second round then
    // stops again and completes, with the request applied exactly once.
    // This case supplies its model response through dispatch_initial's local
    // callback. Keep an HTTP guard to detect unexpected requests, but do not
    // leave a two-reply provider waiting after the test has already finished.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut requests = 0;
        loop {
            match done_rx.try_recv() {
                Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) =>
                    return (requests, false),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            }
            if Instant::now() >= deadline { return (requests, true); }
            match listener.accept() {
                Ok((mut stream, _)) => {
                    requests += 1;
                    stream.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
                    let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock =>
                    std::thread::sleep(Duration::from_millis(10)),
                Err(error) => panic!("{error}"),
            }
        }
    });
    let SteeringLive { db, root: _root, run_id } = steering_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let db_for_accept = db.clone();
    let run_for_accept = run_id.clone();
    let seq_before_dispatch = coordinator.last_event_seq_for_test();
    coordinator
        .dispatch_initial("initial-owner", &Allow, move |binding, frame, _token| {
            // The request arrives while this round is in flight and the Host
            // confirms acceptance (a durable `received` row).
            db_for_accept
                .enqueue_run_steering(
                    &run_for_accept,
                    "steer-last-round",
                    "另外检查 Sheet2 的耗时分布",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
            Ok(stop_response(frame, binding))
        })
        .unwrap();

    // The accepted request was NOT discarded: the Host refused to complete the
    // round and committed a `steering` lane round carrying it, whose durable
    // request already contains the user's text and the steering prompt — and
    // never the stop-review prompt. (Driving that round to its own response is
    // the live session's job and is covered by the live tests; what is asserted
    // here is exactly the durable decision this transport committed.)
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].status, "received",
        "the deferred round must leave the request open for its own lane: {rows:?}"
    );
    assert_eq!(
        coordinator.snapshot().unwrap().state,
        "running",
        "an accepted request must keep the Run out of a completed state"
    );
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    // The steering round's frozen input is stored with its decision: it is what
    // the next transport resumes, and it carries the user's own text.
    let staged_seq = coordinator.last_event_seq_for_test();
    assert!(staged_seq > seq_before_dispatch);
    let staged = db
        .kernel_continuation_input(&run_id, &format!("continuation:{staged_seq}"))
        .expect("the steering round input is frozen with its decision");
    let staged_text = serde_json::to_string(&staged).unwrap();
    assert!(
        staged_text.contains("Fox 补充要求处理"),
        "the steering round must carry the steering prompt: {staged_text}"
    );
    assert!(
        !staged_text.contains("Fox 续答检查"),
        "the accepted input must not ride the stop-review prompt: {staged_text}"
    );
    assert!(
        staged_text.contains("已完成"),
        "the refused stop stays in the durable history the next round answers from"
    );
    // The user's own text is spliced from the durable queue at the dispatch
    // boundary (single source), so it is not duplicated into the frozen input.
    assert!(
        !staged_text.contains("这条要求必须被回答"),
        "the frozen input must not duplicate the queued text: {staged_text}"
    );
    done_tx.send(()).unwrap();
    let (requests, expired) = server.join().unwrap();
    assert!(!expired, "the no-request HTTP guard must finish within its bound");
    assert_eq!(requests, 0,
        "this synthetic transport commits the steering round without HTTP model IO");
}

/// F1: a request accepted **inside the live decision window** — after the last
/// queue read, before the write-set — must be answered by its own lane round and
/// end as `applied` with the Run completed. The old behaviour detached, which the
/// outer loop then classified as an uncertain leased dispatch and failed the Run.
#[test]
fn a_request_accepted_inside_the_live_decision_window_is_answered_and_applied() {
    let (address, server) =
        steering_provider(vec![("first stop", "stop-1"), ("final stop", "stop-2")]);
    let SteeringLive { db, root: _root, run_id } = steering_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    // Deterministic barrier: enqueue exactly between the last queue read and the
    // decision write-set of the first round.
    let db_barrier = db.clone();
    let run_barrier = run_id.clone();
    super::live::test_barrier::install(
        &run_id,
        Box::new(move || {
            db_barrier
                .enqueue_run_steering(
                    &run_barrier,
                    "live-window-row",
                    "窗口内追加的现场要求",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
        }),
    );
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        Ok((true, json!({"content":[{"type":"text","text":"no tool expected"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "live-window",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .expect("the raced live round must be re-planned, not failed");

    // The Run completed normally, with the accepted request applied exactly once.
    assert_eq!(
        coordinator.snapshot().unwrap().state,
        "completed",
        "the raced Run must finish as a normal completion, not an uncertain execution"
    );
    assert_eq!(
        db.kernel_host_run_state(&run_id).unwrap().as_deref(),
        Some("completed")
    );
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].status, "applied",
        "the request accepted in the window must be answered by its own round: {rows:?}"
    );
    assert!(rows[0].applied_at.is_some());
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);

    // Two model rounds: the stop that raced the arrival, then the round that
    // answers it. The second request carries the user's text and the steering
    // prompt, never the stop-review prompt.
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "the accepted request needs its own round");
    let steering_request = requests[1].to_string();
    assert!(steering_request.contains("窗口内追加的现场要求"));
    assert!(steering_request.contains("Fox 补充要求处理"));
    assert!(!steering_request.contains("Fox 续答检查"));
    // The reply of the raced round is preserved exactly once in that request.
    assert_eq!(
        steering_request.matches("first stop").count(),
        1,
        "the raced round's reply must appear exactly once"
    );
}

/// F1 at a **tool** boundary: the request arrives inside the decision window of
/// the round that proposed a tool, so the retried decision arms a batch directive
/// carrying it. The tool must execute exactly once (the raced round's proposal is
/// settled, not replayed) and the request must still end `applied`.
#[test]
fn a_request_accepted_inside_a_tool_batch_window_executes_the_tool_once() {
    let (address, server) = steering_provider(vec![
        ("tool", "read-once"),
        ("I will analyze the tool result.", "stop-1"),
        ("I will finish the review.", "stop-2"),
        ("final stop", "stop-3"),
    ]);
    let SteeringLive { db, root, run_id } = steering_live_fixture(address);
    std::fs::write(root.join("proof.txt"), "proof").unwrap();
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let db_barrier = db.clone();
    let run_barrier = run_id.clone();
    super::live::test_barrier::install(
        &run_id,
        Box::new(move || {
            db_barrier
                .enqueue_run_steering(
                    &run_barrier,
                    "batch-window-row",
                    "同时在报告里补一句风险提示",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
        }),
    );
    let executions = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen_executions = Arc::clone(&executions);
    let execute = move |_: &RunControlBinding,
                        effect: &kernel::OutboxEffect,
                        _: &kernel::CancellationToken| {
        seen_executions
            .lock()
            .unwrap()
            .push(effect.tool_call_id.clone().unwrap_or_default());
        Ok((
            true,
            json!({"content":[{"type":"text","text":"proof.txt: proof"}]}),
        ))
    };
    coordinator
        .dispatch_initial_live(
            "batch-window",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .expect("the raced batch round must be re-planned, not failed");

    if coordinator.snapshot().unwrap().state != "completed" {
        // Diagnostic: on failure the durable events say which decision gave up.
        if let Ok(connection) = rusqlite::Connection::open(root.join("facts.db")) {
            let mut statement = connection
                .prepare("SELECT seq, event_type, substr(payload_json,1,300) FROM kernel_events WHERE run_id=?1 ORDER BY seq")
                .unwrap();
            for row in statement
                .query_map([&run_id], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .unwrap()
                .flatten()
            {
                eprintln!("[batch-window] {} {} {}", row.0, row.1, row.2);
            }
        }
    }
    assert_eq!(
        coordinator.snapshot().unwrap().state,
        "completed",
        "the raced tool round must finish normally"
    );
    // No replay: the model proposed one tool call and it ran exactly once.
    let executed = executions.lock().unwrap().clone();
    assert_eq!(
        executed,
        vec!["read-once".to_owned()],
        "a completed tool must never be re-executed by the recovery path"
    );
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "applied", "{rows:?}");
    // The request was adopted into the tool-batch round the raced decision armed,
    // so the model saw it in that round and no extra lane round was needed.
    assert_eq!(
        db.kernel_count_steering_followups(&run_id).unwrap(),
        0,
        "a request adopted by the batch directive must not also spawn a lane round"
    );
    // No uncertain execution was recorded for the raced dispatch.
    let uncertain = db
        .query_count_raw(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type=?2",
            &[&run_id, &"kernel.uncertain_execution"],
        )
        .unwrap();
    assert_eq!(uncertain, 0);
    let requests = server.join().unwrap();
    assert_eq!(
        requests.len(),
        4,
        "tool round, batch round, review round, final round"
    );
    // The accepted request reached the model in the **tool-batch round** the raced
    // decision armed: that request already carries the notice wrapper it is
    // delivered under, and it is the first request in which it appears — the
    // request did not wait for a separate lane round.
    let bodies = requests
        .iter()
        .map(|request| request.to_string())
        .collect::<Vec<_>>();
    let first_with_request = bodies
        .iter()
        .position(|body| body.contains("补一句风险提示"))
        .expect("the accepted request must reach a model round");
    assert_eq!(
        first_with_request, 1,
        "the request must ride the tool-batch round, not a later round"
    );
    assert!(
        bodies[first_with_request].contains("补充要求"),
        "the request must arrive in its user-facing notice wrapper"
    );
    assert!(
        !bodies[first_with_request].contains("Fox 续答检查"),
        "the request must not ride the stop-review prompt"
    );
}

/// Without any mid-run input the live loop behaves exactly as before: one
/// request, no steering lane round, a clean completion.
#[test]
fn live_loop_completes_without_a_steering_round_when_nothing_was_accepted() {
    let (address, server) = steering_provider(vec![("plain stop", "round-1")]);
    let SteeringLive { db, root: _root, run_id } = steering_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        Ok((true, json!({"content":[{"type":"text","text":"checked"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "steering-quiet",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 0);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert!(db.run_steering_messages(&run_id).unwrap().is_empty());
    assert_eq!(server.join().unwrap().len(), 1);
}

/// A successful completion may not abandon input that was received but never
/// handed to a model. The guarantee is enforced twice, and this test pins both
/// layers of the same fact:
///  * the transport defers the stop into its own `steering` lane round, and
///  * the terminal decision itself refuses to commit `completed` while such a
///    row is still open — so no path (live loop, replacement transport or
///    recovery) can complete around it.
#[test]
fn a_completed_decision_is_refused_while_accepted_input_was_never_delivered() {
    let clock = TestClock::new(1000);
    let (db, _root, run_id) = steering_model_fixture(&clock);
    let cancellation = CancellationRegistry::default();
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation).unwrap();
    // Layer 1: the request is accepted while this model round is in flight (its
    // dispatch was already frozen), so the round's stop is answered by a
    // `steering` lane instead of completing the Run.
    let db_for_deliver = db.clone();
    let run_for_deliver = run_id.clone();
    let accepted = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&accepted);
    coordinator
        .dispatch_initial("initial-owner", &Allow, move |binding, frame, _token| {
            let seq = db_for_deliver
                .enqueue_run_steering(
                    &run_for_deliver,
                    "steer-open",
                    "这条要求必须被回答",
                    crate::database::now_ms(),
                )
                .expect("the Host accepts a request while the Run is active");
            sink.lock().unwrap().push(seq);
            Ok(stop_response(frame, binding))
        })
        .unwrap();
    assert_eq!(accepted.lock().unwrap().len(), 1);
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(
        rows[0].status, "received",
        "the row stays open until its own lane round answers it: {rows:?}"
    );
    assert_eq!(db.kernel_count_steering_followups(&run_id).unwrap(), 1);
    assert_eq!(coordinator.snapshot().unwrap().state, "running");

    // Layer 2: even if a caller tried to complete now, the durable terminal
    // decision refuses, because the row was never given to a model. The refusal
    // is a *scheduling* signal with a stable marker (never a generic engine
    // error), so each transport can re-plan instead of failing the Run.
    let error = coordinator
        .complete_bypassing_the_steering_lane_for_test()
        .expect_err("a completed decision must be refused while input is unanswered");
    assert!(
        error.starts_with(kernel::STEERING_COMPETITION),
        "the refusal must carry the competition marker: {error}"
    );
    assert_eq!(
        super::replan_or_error(error),
        super::STEERING_REPLAN,
        "the marker must translate into the re-plan signal"
    );
    assert_eq!(
        db.run_steering_messages(&run_id).unwrap()[0].status,
        "received",
        "the refused completion must not cancel the accepted request"
    );
    assert_ne!(coordinator.snapshot().unwrap().state, "completed");

    // An explicit cancellation is a different, explainable end state: the queued
    // text never reached a model and is recorded as such.
    coordinator
        .fail("kernel.test_cancel", "user cancelled the run")
        .unwrap();
    let rows = db.run_steering_messages(&run_id).unwrap();
    assert_eq!(rows[0].status, "cancelled");
}
