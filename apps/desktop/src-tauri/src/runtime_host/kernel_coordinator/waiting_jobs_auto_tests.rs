use super::*;
use std::time::{Duration, Instant};
use tauri::Manager;

/// The real Job is held immediately before its terminal DB transaction. The
/// original Host must park and release its Pi session before that transaction
/// commits; the test never calls a wake method.
#[test]
fn real_host_auto_wakes_after_job_settles_on_both_pi_transports() {
    let selected = std::env::var("FOX_TEST_AUTO_JOB_CHAIN_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("real_host_auto_wakes_after_job_settles_on_both_pi_transports")
                .arg("--test-threads=1")
                .env("FOX_TEST_AUTO_JOB_CHAIN_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} automatic Job wake exceeded 55 seconds");
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
    run_auto_park_then_settle(mode == "live");
}

fn run_auto_park_then_settle(live: bool) {
    let arguments = json!({"idempotencyKey":"one-auto-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The compute Job is still running."}),
        json!({"role":"assistant","content":"The Host result is ready."}),
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
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let parent_token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        assert!(release_rx.recv_timeout(Duration::from_secs(25)).is_ok(),
            "bounded QuickJS settlement release never arrived");
    })));

    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"),
        "the original Host must park while the Job is held before settlement");
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("real QuickJS Job did not reach the terminal write");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 1, "one formal job_start creates exactly one Job");
    let job_id = jobs[0].job_id.clone();
    assert_eq!(jobs[0].attempts, 1);
    assert_eq!(jobs[0].state.as_str(), "running");
    assert!(!parent_token.is_cancelled(), "park retired the child Job parent token");
    let formal_before: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
           AND json_extract(payload_json,'$.toolCallId')='job-start'",
        [&run], |row| row.get(0))).unwrap();
    assert_eq!(formal_before, 1);

    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let state = db.kernel_host_run_state(&run).unwrap();
        if state.as_deref() == Some("completed") { break; }
        assert!(Instant::now() < deadline, "automatic wake did not finish: state={state:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    let job = db.kernel_job_snapshot(&job_id).unwrap();
    assert_eq!(job.state.as_str(), "completed");
    assert_eq!(job.attempts, 1);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "tool, parked Final, and one automatic continuation");
    assert!(requests[1]["messages"].to_string().contains(&job_id));
    assert!(!requests[1]["messages"].to_string().contains("FOX_HOST_JOB_NOTICE_V1"));
    let continued = requests[2]["messages"].to_string();
    assert_eq!(continued.matches("FOX_HOST_JOB_NOTICE_V1").count(), 1);
    assert!(continued.contains(&job_id));
    let (parks, wakes, replies, formal): (i64, i64, i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_response'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                    AND json_extract(payload_json,'$.toolCallId')='job-start')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap();
    assert_eq!((parks, wakes, replies, formal), (1, 1, 1, 1));
    assert_eq!(db.kernel_build_full_snapshot(&run).unwrap().state, "completed");
    let retirement_deadline = Instant::now() + Duration::from_secs(5);
    while !parent_token.is_cancelled() {
        assert!(Instant::now() < retirement_deadline, "terminal Run retained the old child scope");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(parent_token.is_cancelled(), "terminal Run retained the old child scope");
    drop(host);
    drop(app);
}

// The lost-wake race needs a separate test-only seam after the park transaction
// commits but before the original KernelRunLock drops. Settling during a held
// second Provider response takes the already-covered direct-notice path (0
// parks), and cannot prove that an automatic parked wake survives lock contention.
