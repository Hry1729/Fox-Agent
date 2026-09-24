//! Real Host lifecycle and current-policy boundaries for automatic Job wakes.
use super::*;
use std::time::{Duration, Instant};
use tauri::Manager;

/// Keep the local Provider listening after its scripted replies. A third
/// request then remains observable even when the Run was meant to stop.
fn counted_local_provider(replies: Vec<Value>) -> (
    std::net::SocketAddr,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    std::thread::JoinHandle<()>,
) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let count = std::sync::Arc::new(AtomicUsize::new(0));
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let worker_count = count.clone();
    let worker_stop = stop.clone();
    let thread = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(50);
        while !worker_stop.load(Ordering::SeqCst) {
            assert!(Instant::now() < deadline, "local Provider fixture was not stopped");
            let mut stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("local Provider accept failed: {error}"),
            };
            stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            let (header_end, length) = loop {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0, "Provider connection ended before its request");
                bytes.extend_from_slice(&buffer[..read]);
                assert!(bytes.len() < 1_048_576);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                    let length: usize = headers.lines().find_map(|line|
                        line.strip_prefix("content-length:")).unwrap().trim().parse().unwrap();
                    assert!(length < 1_048_576);
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let read = stream.read(&mut buffer).unwrap();
                assert!(read > 0);
                bytes.extend_from_slice(&buffer[..read]);
            }
            let _: Value = serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            let index = worker_count.fetch_add(1, Ordering::SeqCst);
            if let Some(delta) = replies.get(index) {
                let finish = if delta["tool_calls"].is_array() { "tool_calls" } else { "stop" };
                let chunks = [
                    json!({"id":"boundary-local","object":"chat.completion.chunk","created":1,
                        "model":"kernel-http-test","choices":[{"index":0,"delta":delta,"finish_reason":null}]}),
                    json!({"id":"boundary-local","object":"chat.completion.chunk","created":1,
                        "model":"kernel-http-test","choices":[{"index":0,"delta":{},"finish_reason":finish}]}),
                ];
                let response = format!("data: {}\n\ndata: {}\n\ndata: [DONE]\n\n", chunks[0], chunks[1]);
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response).unwrap();
            } else {
                let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
        }
    });
    (address, count, stop, thread)
}

#[test]
fn shutdown_during_owned_auto_wake_cancels_without_continuation_on_both_pi_transports() {
    let selected = std::env::var("FOX_TEST_AUTO_WAKE_STOP_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("shutdown_during_owned_auto_wake_cancels_without_continuation_on_both_pi_transports")
                .arg("--test-threads=1")
                .env("FOX_TEST_AUTO_WAKE_STOP_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} owned-wake shutdown exceeded 55 seconds");
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success(), "{mode} stdout: {}\n{mode} stderr: {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    assert!(mode == "live" || mode == "round");
    shutdown_owned_wake(mode == "live");
}

fn shutdown_owned_wake(live: bool) {
    use std::sync::atomic::Ordering;
    let arguments = json!({"idempotencyKey":"shutdown-owned-wake",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address, requests, stop_provider, server) = counted_local_provider(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The Job remains in progress."}),
    ]);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools = vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, run) = fixture_with_retry_opt(&clock, &config.hash().unwrap(),
        Some(&config), true, true, (0, 1));
    super::host_job_notice_tests::enable_notices(&db, &run);
    db.freeze_kernel_host_scope(&run, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["compute_job_start".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    }).unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::create_dir_all(root.join("skills")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = crate::runtime_host::RuntimeHost::new(app.handle().clone(), db.clone(), root.clone(),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    if !live { host.force_auto_wake_round_for_test(&run); }
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (job_release_tx, job_release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        job_release_rx.recv_timeout(Duration::from_secs(20))
            .expect("the fixture never released the Job after park");
    })));
    let (wake_entered_tx, wake_entered_rx) = std::sync::mpsc::channel();
    let (wake_release_tx, wake_release_rx) = std::sync::mpsc::channel();
    let wake_release_rx = std::sync::Arc::new(std::sync::Mutex::new(wake_release_rx));
    crate::runtime_host::kernel_host::waiting_wake_test_hooks::set_before_wake_account(
        &run, std::sync::Arc::new(move || {
            wake_entered_tx.send(()).unwrap();
            wake_release_rx.lock().unwrap().recv_timeout(Duration::from_secs(20))
                .expect("the shutdown fixture did not release the owned wake");
        }));
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("real Job never reached its terminal transaction");
    let job_id = db.kernel_jobs_for_run(&run).unwrap()[0].job_id.clone();
    job_release_tx.send(()).unwrap();
    wake_entered_rx.recv_timeout(Duration::from_secs(15))
        .expect("automatic wake never held its admission gate");
    assert_eq!(db.kernel_job_snapshot(&job_id).unwrap().state.as_str(), "completed");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    {
        let state = host.state.lock().unwrap();
        assert!(state.kernel_wake_inflight.contains(&run));
    }
    assert!(matches!(crate::runtime_host::kernel_host::acquire(&root, &run),
        Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED));
    let stopping = { let host = host.clone(); std::thread::spawn(move || host.stop_kernel_runs()) };
    let signal_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let shutting_down = host.state.lock().unwrap().shutting_down;
        let cancel_queued = db.pending_kernel_host_commands(&run).unwrap().iter()
            .any(|command| command.kind == "cancel");
        if shutting_down && cancel_queued && token.is_cancelled() { break; }
        assert!(Instant::now() < signal_deadline,
            "Host stop did not publish shutdown/cancel while wake was held: shutdown={shutting_down} cancel={cancel_queued}");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    wake_release_tx.send(()).unwrap();
    stopping.join().unwrap().unwrap();
    let terminal_deadline = Instant::now() + Duration::from_secs(5);
    while db.kernel_host_run_state(&run).unwrap().as_deref() != Some("cancelled") {
        assert!(Instant::now() < terminal_deadline,
            "owned wake shutdown did not settle the parent cancellation: {:?}",
            db.kernel_host_run_state(&run).unwrap());
        std::thread::sleep(Duration::from_millis(10));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    std::thread::sleep(Duration::from_millis(300));
    stop_provider.store(true, Ordering::SeqCst);
    server.join().unwrap();
    assert_eq!(requests.load(Ordering::SeqCst), 2,
        "shutdown dispatched a continuation Provider request");
    let (parks, wakes, continuations): (i64, i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_requested')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))).unwrap();
    assert_eq!((parks, wakes, continuations), (1, 0, 0));
    let state = host.state.lock().unwrap();
    assert!(state.shutting_down);
    assert!(!state.waiting_wakes.contains_key(&run));
    assert!(!state.kernel_active_runs.contains(&run));
    assert!(!state.kernel_wake_inflight.contains(&run));
    assert!(state.cancellation.run_token(&run).is_err(), "terminal scope was not retired");
    drop(state);
    assert!(token.is_cancelled());
    drop(crate::runtime_host::kernel_host::acquire(&root, &run).unwrap());
    drop(host);
    drop(app);
}
