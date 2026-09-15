use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

// Real HTTP SSE provider scripted per model request. Each request is answered
// with either a plain stop ("stop") or a tool-call proposal ("tool").
fn delivery_provider(
    script: Vec<&'static str>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    delivery_provider_with_ids(script.into_iter().map(|reply| (reply, "read-once")).collect())
}

/// Same provider, with an explicit tool-call id per round. Durable tool
/// identity is per call, so a script that proposes tools in several rounds must
/// propose a different call each time.
fn delivery_provider_with_ids(
    script: Vec<(&'static str, &'static str)>,
) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for (reply, tool_call_id) in script {
            let deadline = Instant::now() + Duration::from_secs(25);
            let (mut stream, request) = loop {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing model request for {reply}");
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
                // Undici may open a replacement idle socket while the worker
                // advances; it is not another model request.
                if let Some(request) = request {
                    break (stream, request);
                }
                assert!(Instant::now() < deadline, "only abandoned sockets for {reply}");
            };
            requests.push(request);
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let send = |stream: &mut std::net::TcpStream, delta: Value, finish: Value| {
                let body = format!(
                    "data: {}\n\n",
                    json!({"id":"delivery-live","object":"chat.completion.chunk","created":1,"model":"delivery-live",
                        "choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
                );
                write!(stream, "{:x}\r\n{}\r\n", body.len(), body)
            };
            if reply == "tool" {
                send(
                    &mut stream,
                    json!({"role":"assistant","tool_calls":[{"index":0,"id":tool_call_id,"type":"function",
                        "function":{"name":"read","arguments":"{\"path\":\"proof.txt\"}"}}]}),
                    Value::Null,
                )
                .unwrap();
                send(&mut stream, json!({}), json!("tool_calls")).unwrap();
            } else {
                send(
                    &mut stream,
                    json!({"role":"assistant","content":"我这就给出最终答复。"}),
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

struct DeliveryLive {
    db: Database,
    root: PathBuf,
    run_id: String,
}

fn delivery_live_fixture(address: std::net::SocketAddr) -> DeliveryLive {
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
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
    // Fixtures bypass the runtime_host run start, so the authoritative lead's
    // delivery checklist is seeded the same way production start does it.
    let seeds =
        crate::runtime_host::delivery::expectations_from_task("请生成一份 Excel 成果");
    assert_eq!(seeds.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    DeliveryLive { db, root, run_id }
}

/// Same fixture, but the task also demands a chart per sheet: the checklist is
/// seeded through the production seeding step (task text → seeds → the
/// requirements those items must satisfy).
fn delivery_live_fixture_with_chart_demand(address: std::net::SocketAddr) -> DeliveryLive {
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"delivery-live",
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
    let task = "请生成 AGV汇总.xlsx，其中每个 Sheet 配一张图表。";
    let mut seeds = crate::runtime_host::delivery::expectations_from_task(task);
    assert_eq!(seeds.len(), 1);
    let requirements = crate::runtime_host::delivery::requirements_from_task(task);
    assert!(
        !requirements.is_empty(),
        "the chart demand must be parsed from the task"
    );
    for seed in seeds.iter_mut() {
        crate::runtime_host::delivery::attach_requirements(seed, &requirements);
    }
    assert_eq!(seeds[0].requirements.len(), 1);
    db.seed_delivery_checklist(&run_id, &seeds, crate::database::now_ms())
        .unwrap();
    let stored = db.delivery_requirements(&run_id).unwrap();
    assert_eq!(
        stored.len(),
        1,
        "the seeded chart demand must be readable back at the gate"
    );
    DeliveryLive { db, root, run_id }
}

fn real_workbook_bytes() -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/office-reading.xlsx"),
    )
    .unwrap()
}

#[test]
fn live_delivery_repair_loop_rides_continuation_without_consuming_review_budget_or_replaying_tools() {
    let (address, server) = delivery_provider(vec!["stop", "tool", "stop"]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        // The single tool call represents the write tool's real external side
        // effect: a parseable workbook lands inside the frozen project root.
        assert_eq!(executions.fetch_add(1, Ordering::SeqCst), 0);
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((
            true,
            json!({"content":[{"type":"text","text":"workbook written"}]}),
        ))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-live",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // The run completes and its promised file passed deterministic checks.
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, "passed");
    assert_eq!(items[0].target_path.as_deref(), Some("AGV汇总.xlsx"));
    assert!(items[0].checked_at.is_some());

    // Exactly one bounded business repair happened; the stop-review lane and
    // its independent counter stayed at zero.
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);

    // One real side effect, never replayed.
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3);
    // The repair directive reached the model as a user turn naming the failed
    // deliverable; it never masquerades as the stop-review prompt.
    let second_request = requests[1].to_string();
    assert!(second_request.contains("Fox 交付核验"));
    assert!(!second_request.contains("Fox 续答检查"));
}

#[test]
fn live_delivery_repair_budget_exhausts_with_completed_run_and_visibly_failed_item() {
    let (address, server) = delivery_provider(vec!["stop", "stop", "stop"]);
    let DeliveryLive { db, root: _root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    // The model never proposes a tool: no side effect may ever run.
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        executions.fetch_add(1, Ordering::SeqCst);
        Ok((true, json!({"content":[{"type":"text","text":"unexpected"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-exhausted",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // Exhaustion completes the run rather than looping forever, while the
    // business delivery stays visibly failed.
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items[0].status, "failed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 2);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 2);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 0);
    assert_eq!(server.join().unwrap().len(), 3);
}

/// R6: a deliverable that is parseable but does not contain what the task
/// demanded must reach the existing bounded delivery repair through the REAL
/// coordinator path — not only through a test-only checker.
#[test]
fn live_delivery_gate_repairs_a_parseable_artifact_that_misses_a_stated_demand() {
    // Round 1 writes a real but chart-less workbook and stops: the gate must
    // reject it and arm one bounded repair. Round 2 (the repair round) writes the
    // charted workbook and stops: the demand is now met.
    let (address, server) = delivery_provider_with_ids(vec![
        ("tool", "write-plain"),
        ("stop", "stop-after-plain"),
        ("tool", "write-charted"),
        ("stop", "stop-after-charted"),
    ]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture_with_chart_demand(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let executions = AtomicUsize::new(0);
    let plain = real_workbook_bytes();
    // The task demands a chart per worksheet, so the repaired deliverable must
    // carry one *worksheet-referenced* chart per sheet — a whole-file chart count
    // could not satisfy (or verify) a per-sheet demand, and an unreferenced chart
    // part is not something the user can see.
    let charted = crate::runtime_host::delivery::zip_attach_chart_per_sheet(&plain);
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        // Each tool round represents a real write of the promised artifact.
        let round = executions.fetch_add(1, Ordering::SeqCst);
        let bytes = if round == 0 { &plain } else { &charted };
        std::fs::write(root.join("AGV汇总.xlsx"), bytes).unwrap();
        Ok((
            true,
            json!({"content":[{"type":"text","text":"workbook written"}]}),
        ))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-demands",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();

    // The repair happened through the delivery lane, not the stop-review lane,
    // and the artifact was written twice (once per real tool round).
    let requests = server.join().unwrap();
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(
        items.first().map(|item| item.status.clone()),
        Some("passed".to_owned()),
        "delivery item: {:?}; requests: {}",
        items.first().map(|item| item.finding.clone()),
        requests.len()
    );
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_delivery_repairs(&run_id).unwrap(), 1);
    assert_eq!(db.kernel_count_continuations(&run_id).unwrap(), 0);
    assert_eq!(executions.load(Ordering::SeqCst), 2);

    // The final verdict shows the demand met, with the structural evidence.
    let finding: Value = serde_json::from_str(items[0].finding.as_deref().unwrap()).unwrap();
    let requirement = finding["requirements"]
        .as_array()
        .expect("requirement findings")
        .iter()
        .find(|entry| entry["check"] == json!("charts"))
        .expect("chart requirement reported");
    assert_eq!(requirement["state"], json!("passed"));
    assert_eq!(
        finding["checks"]["charts>=per-sheet:1"]["state"],
        json!("passed"),
        "the per-sheet demand is recorded as such and met: {finding}"
    );

    assert_eq!(requests.len(), 4);
    // The repair prompt names the missing chart requirement and rides the
    // delivery lane rather than masquerading as the stop-review prompt.
    let repair_request = requests[2].to_string();
    assert!(repair_request.contains("Fox 交付核验"));
    assert!(repair_request.contains("图表"), "repair must name the demand");
    assert!(!repair_request.contains("Fox 续答检查"));
}

/// A task that promises files but states no machine-decidable content demand
/// still gets the structural checks, and the run completes without inventing a
/// requirement or charging a repair round.
#[test]
fn live_delivery_gate_stays_quiet_without_a_structured_demand() {
    let (address, server) = delivery_provider(vec!["tool", "stop"]);
    let DeliveryLive { db, root, run_id } = delivery_live_fixture(address);
    let clock = TestClock::new(crate::database::now_ms());
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator =
        KernelCoordinator::start_prepared(&db, &clock, &run_id, &cancellation)
            .unwrap()
            .with_preview(&preview);
    let workbook_bytes = real_workbook_bytes();
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        std::fs::write(root.join("AGV汇总.xlsx"), &workbook_bytes).unwrap();
        Ok((true, json!({"content":[{"type":"text","text":"written"}]})))
    };
    coordinator
        .dispatch_initial_live(
            "delivery-plain",
            &Allow,
            &real_worker_command(),
            "local-test-only",
            &execute,
            &|_| Ok(()),
            &|_| Ok(()),
        )
        .unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state, "completed");
    assert_eq!(db.delivery_repair_round_count(&run_id).unwrap(), 0);
    let items = db.delivery_checklist(&run_id).unwrap();
    assert_eq!(items[0].status, "passed");
    let finding: Value = serde_json::from_str(items[0].finding.as_deref().unwrap()).unwrap();
    assert!(
        finding.get("requirements").is_none(),
        "no structured demand was stated, so none may be reported: {finding}"
    );
    assert_eq!(server.join().unwrap().len(), 2);
}
