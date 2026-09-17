use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

struct AdvancingClock { base: i64, started: Instant }
impl Clock for AdvancingClock {
    fn now_monotonic_ms(&self) -> i64 { self.base + self.started.elapsed().as_millis() as i64 }
    fn now_wall_ms(&self) -> i64 { self.now_monotonic_ms() }
}

// Real HTTP SSE; the partial tool arguments continue arriving until the worker
// closes its connection. No executable tool proposal is ever completed by this stream.
fn live_provider(continuation: bool) -> (std::net::SocketAddr, std::thread::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        let replies = if continuation { vec!["tool", "preface", "stream", "final"] }
            else { vec!["stream", "final"] };
        for reply in replies {
            let deadline = Instant::now() + Duration::from_secs(25);
            let (mut stream, request) = loop {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < deadline, "missing model request for {reply}");
                        std::thread::sleep(Duration::from_millis(10)); continue;
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
                        let length = headers.lines().find_map(|l|l.strip_prefix("content-length:")).unwrap().trim().parse::<usize>().unwrap();
                        if bytes.len() >= end + 4 + length {
                            break Some(serde_json::from_slice::<Value>(&bytes[end+4..end+4+length]).unwrap());
                        }
                    }
                };
                // Undici may open a replacement idle socket while the aborted
                // worker is exiting. It is not another model request.
                if let Some(request) = request { break (stream, request); }
                assert!(Instant::now() < deadline, "only abandoned sockets for {reply}");
            };
            requests.push(request);
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").unwrap();
            let send = |stream: &mut std::net::TcpStream, delta: Value, finish: Value| {
                let body = format!("data: {}\n\n", json!({"id":"live-limit","object":"chat.completion.chunk","created":1,"model":"live-limit",
                    "choices":[{"index":0,"delta":delta,"finish_reason":finish}]}));
                write!(stream,"{:x}\r\n{}\r\n",body.len(),body)
            };
            let tool = |arguments: &str| json!({"role":"assistant","tool_calls":[{"index":0,"id":"read-once","type":"function",
                "function":{"name":"read","arguments":arguments}}]});
            if reply == "stream" {
                send(&mut stream,tool("{\"path\":\""),Value::Null).unwrap();
                let deadline = Instant::now() + Duration::from_secs(8);
                while Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(80));
                    if send(&mut stream,json!({"tool_calls":[{"index":0,"function":{"arguments":"x"}}]}),Value::Null).is_err() { break; }
                }
            } else {
                let delta = match reply {
                    "tool" => tool("{\"path\":\"proof.txt\"}"),
                    "preface" => json!({"role":"assistant","content":"I will now summarize the stored result."}),
                    _ => json!({"role":"assistant","content":"Verified final answer from durable facts."}),
                };
                send(&mut stream,delta,Value::Null).unwrap();
                send(&mut stream,json!({}),json!(if reply == "tool" { "tool_calls" } else { "stop" })).unwrap();
                let done = "data: [DONE]\n\n";
                write!(stream,"{:x}\r\n{}\r\n0\r\n\r\n",done.len(),done).unwrap();
            }
        }
        requests
    });
    (address,server)
}

#[test]
fn live_total_timeout_with_continuous_arguments_retries_initial_without_execution() {
    assert_live_total_retry(false);
}

#[test]
fn live_continuation_total_timeout_reopens_its_lease_without_replaying_completed_tools() {
    assert_live_total_retry(true);
}

fn assert_live_total_retry(continuation: bool) {
    let (address, server) = live_provider(continuation);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"live-limit","baseUrl":format!("http://{address}/v1")});
    let setup_clock = TestClock::new(crate::database::now_ms());
    let budgets = TimeBudgets { model_request_ms: 3_500, model_first_response_ms: 2_500, model_idle_ms: 2_500, ..TimeBudgets::default() };
    let (db,root,run_id) = fixture_with_budgets_opt(&setup_clock,&config.hash().unwrap(),Some(&config),true,true,(0,1),true,budgets);
    freeze_host_scope(&db,&run_id);
    let clock = AdvancingClock { base: setup_clock.get(), started: Instant::now() };
    let cancellation = CancellationRegistry::default();
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap().with_preview(&preview);
    let binding = coordinator.binding.clone();
    let executions = AtomicUsize::new(0);
    let execute = |_: &RunControlBinding, _: &kernel::OutboxEffect, _: &kernel::CancellationToken| {
        assert!(continuation); executions.fetch_add(1,Ordering::SeqCst);
        Ok((true,json!({"content":[{"type":"text","text":"durable read proof"}]})))
    };
    coordinator.dispatch_initial_live("first-live",&Allow,&real_worker_command(),"local-test-only",
        &execute,&|_|Ok(()),&|_|Ok(())).unwrap();
    let snapshot = coordinator.snapshot().unwrap();
    assert_eq!(snapshot.state,"retry_scheduled");
    let tools = snapshot.tool_calls.clone();
    assert_eq!(executions.load(Ordering::SeqCst),usize::from(continuation));
    let kind = if continuation { kernel::OutboxEffectKind::ContinuationModel } else { kernel::OutboxEffectKind::InitialModel };
    let retry = snapshot.pending_effects.iter().find(|e|e.kind == kind && e.status == kernel::OutboxStatus::Pending).unwrap();
    let key = retry.effect_key.clone();
    let payload = retry.payload_json.clone();
    let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
    let rejection: String = conn.query_row("SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='engine.model_rejected'",[&run_id],|row|row.get(0)).unwrap();
    let rejection: Value = serde_json::from_str(&rejection).unwrap();
    assert_eq!(rejection["failure"]["category"],"model_timeout");
    assert!(rejection["failure"]["telemetry"]["toolParamBytes"].as_u64().unwrap() > 0);
    assert!(db.kernel_rehydrate(&run_id).unwrap().unwrap().model_request_since_wall_ms.is_none());
    drop(conn); drop(coordinator); drop(db);
    std::thread::sleep(Duration::from_millis(1_100));
    let db = Database::open(root.join("facts.db")).unwrap();
    let coordinator = KernelCoordinator::reopen(&db,&clock,&run_id,&cancellation).unwrap().with_preview(&preview);
    coordinator.tick().unwrap();
    assert_eq!(coordinator.binding,binding);
    assert!(coordinator.snapshot().unwrap().pending_effects.iter().any(|e|e.effect_key == key && e.payload_json == payload));
    if continuation {
        coordinator.dispatch_continuation_live(&key,"resumed-live",&Allow,&real_worker_command(),"local-test-only",
            &execute,&|_|Ok(()),&|_|Ok(())).unwrap();
    } else {
        coordinator.dispatch_initial_live("resumed-live",&Allow,&real_worker_command(),"local-test-only",
            &execute,&|_|Ok(()),&|_|Ok(())).unwrap();
    }
    assert_eq!(coordinator.snapshot().unwrap().state,"completed");
    assert_eq!(coordinator.snapshot().unwrap().tool_calls,tools);
    assert_eq!(executions.load(Ordering::SeqCst),usize::from(continuation));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(),if continuation {4}else{2});
    if continuation { assert!(requests[3]["messages"].to_string().contains("durable read proof")); }
    println!("live_total_retry continuation={continuation} requests={} executions={} final=completed",requests.len(),executions.load(Ordering::SeqCst));
}

fn broken_worker(root: &std::path::Path, mode: &str) -> super::super::super::RuntimeCommand {
    let script = root.join("broken-live-worker.mjs");
    std::fs::write(&script,include_str!("../../../../../../services/agent-runtime/test/fixtures/lost-live-frame.mjs")
        .replace("__MODE__",mode).replace("__ADAPTER__",crate::kernel_model_config::KERNEL_MODEL_ADAPTER)).unwrap();
    super::super::super::RuntimeCommand { program:"node".into(),script:Some(script) }
}

#[test]
fn live_host_deadline_and_lost_worker_frame_are_settled_before_retry() {
    for mode in ["stall","exit"] {
        let config = worker_configuration();
        let setup_clock = TestClock::new(crate::database::now_ms());
        let budgets = TimeBudgets { model_request_ms: 400, model_first_response_ms: 400, model_idle_ms: 400, ..TimeBudgets::default() };
        let (db,root,run_id) = fixture_with_budgets_opt(&setup_clock,&config.hash().unwrap(),Some(&config),true,true,(0,1),true,budgets);
        let clock = AdvancingClock {base:setup_clock.get(),started:Instant::now()};
        let cancellation = CancellationRegistry::default();
        let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
        let before = Instant::now();
        coordinator.dispatch_initial_live("broken-live",&Allow,&broken_worker(&root,mode),"local-test-only",
            &|_,_,_|panic!("no tools"),&|_|Ok(()),&|_|Ok(())).unwrap();
        assert_eq!(coordinator.snapshot().unwrap().state,"retry_scheduled","{mode}");
        assert!(before.elapsed() < Duration::from_secs(5));
        assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().retry.turn_attempts,1);
        assert!(coordinator.snapshot().unwrap().tool_calls.is_empty());
        let conn = rusqlite::Connection::open(root.join("facts.db")).unwrap();
        let category: String = conn.query_row("SELECT json_extract(payload_json,'$.failure.category') FROM kernel_events WHERE run_id=?1 AND event_type='engine.model_rejected'",[&run_id],|row|row.get(0)).unwrap();
        assert_eq!(category, if mode == "exit" { "model_transport_failure" } else { "model_timeout" });
    }
}

#[test]
fn live_cancel_wins_while_worker_is_waiting_at_model_deadline() {
    let config = worker_configuration();
    let setup_clock = TestClock::new(crate::database::now_ms());
    let budgets = TimeBudgets {model_request_ms:400,model_first_response_ms:400,model_idle_ms:400,..TimeBudgets::default()};
    let (db,root,run_id) = fixture_with_budgets_opt(&setup_clock,&config.hash().unwrap(),Some(&config),true,true,(0,1),true,budgets);
    let clock = AdvancingClock {base:setup_clock.get(),started:Instant::now()};
    let cancellation = CancellationRegistry::default();
    let coordinator = KernelCoordinator::start_prepared(&db,&clock,&run_id,&cancellation).unwrap();
    let runtime = broken_worker(&root,"stall");
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let deadline = Instant::now()+Duration::from_secs(5);
            while !root.join("worker-started").exists() {
                assert!(Instant::now()<deadline); std::thread::sleep(Duration::from_millis(10));
            }
            std::thread::sleep(Duration::from_millis(410));
            coordinator.cancel().unwrap();
        });
        assert!(coordinator.dispatch_initial_live("cancel-live",&Allow,&runtime,"local-test-only",
            &|_,_,_|panic!("no tools"),&|_|Ok(()),&|_|Ok(())).is_err());
    });
    coordinator.settle_cancellation().unwrap();
    assert_eq!(coordinator.snapshot().unwrap().state,"cancelled");
    assert_eq!(db.kernel_rehydrate(&run_id).unwrap().unwrap().retry.turn_attempts,0);
}
