//! Real Host/QuickJS terminal Job facts delivered by automatic WaitingJobs wake.
//! A child Job failure or cancellation is distinct from cancelling its parent Run.
use super::*;
use std::time::{Duration, Instant};
use tauri::Manager;

fn provider_notice_text(value: &Value) -> Option<&str> {
    match value {
        Value::String(text) if text.contains("FOX_HOST_JOB_NOTICE_V1\n") => Some(text),
        Value::Array(items) => items.iter().find_map(provider_notice_text),
        Value::Object(fields) => fields.values().find_map(provider_notice_text),
        _ => None,
    }
}

#[test]
fn failed_and_independently_cancelled_jobs_auto_resume_on_both_pi_transports() {
    let selected = std::env::var("FOX_TEST_JOB_TERMINAL_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["live-failed", "round-failed", "live-cancelled", "round-cancelled"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("failed_and_independently_cancelled_jobs_auto_resume_on_both_pi_transports")
                .arg("--test-threads=1")
                .env("FOX_TEST_JOB_TERMINAL_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} terminal Job chain exceeded 55 seconds");
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
    assert!(matches!(mode.as_str(), "live-failed" | "round-failed"
        | "live-cancelled" | "round-cancelled"));
    run_terminal_job_chain(mode.starts_with("live"), mode.ends_with("failed"));
}

fn run_terminal_job_chain(live: bool, fail: bool) {
    let code = if fail {
        "function onChunk(c){} function onFinish(){throw new Error('planned compute failure');}"
    } else {
        "function onChunk(c){} function onFinish(){return {answer:42};}"
    };
    let arguments = json!({"idempotencyKey":"one-terminal-compute",
        "params":{"processing":"chunked","code":code}}).to_string();
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The Job is still running."}),
        json!({"role":"assistant","content":"I received the terminal Job notice."}),
    ]);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools = vec![
        json!({"name":"compute_job_start","description":"Start a bounded background compute Job",
            "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
                "params":{"type":"object"}},"required":["idempotencyKey","params"]}}),
        json!({"name":"compute_job_cancel","description":"Cancel a Job in the same Run",
            "parameters":{"type":"object","properties":{"jobId":{"type":"string"}},
                "required":["jobId"]}}),
    ];
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, run) = fixture_with_retry_opt(&clock, &config.hash().unwrap(),
        Some(&config), true, true, (0, 1));
    super::host_job_notice_tests::enable_notices(&db, &run);
    db.freeze_kernel_host_scope(&run, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["compute_job_start".into(), "compute_job_cancel".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(), lifecycle_hooks: vec![],
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
    let parent_token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(20))
            .expect("real QuickJS Job was never released after parent park");
    })));
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("real QuickJS Job did not reach its terminal write");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].attempts, 1);
    assert_eq!(jobs[0].state.as_str(), "running");
    assert!(!parent_token.is_cancelled(), "the parent Run lost its Job token at park");
    let job_id = jobs[0].job_id.clone();
    if !fail {
        host.execute_job_operation(&binding.conversation_id, &run,
            "compute_job_cancel", &json!({"jobId":job_id})).unwrap();
        assert!(db.kernel_job_snapshot(&job_id).unwrap().cancel_requested_at.is_some(),
            "the scoped Host cancel did not reach the real Job executor");
        assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"),
            "a Job cancel may not cancel its parent Run");
    }
    release_tx.send(()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(25);
    while db.kernel_host_run_state(&run).unwrap().as_deref() != Some("completed") {
        assert!(Instant::now() < deadline,
            "terminal Job notice did not automatically finish the parent: {:?}",
            db.kernel_host_run_state(&run).unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    let job = db.kernel_job_snapshot(&job_id).unwrap();
    let notice = db.kernel_job_notice(&binding.conversation_id, &run, &job_id).unwrap().unwrap();
    let expected = if fail { "failed" } else { "cancelled" };
    assert_eq!(job.state.as_str(), expected);
    assert_eq!(notice.terminal_state.as_str(), expected);
    assert_eq!(job.attempts, 1);
    assert_eq!(notice.attempt, 1);
    assert_eq!(notice.error_code.as_deref(),
        Some(if fail { "compute.execution_failed" } else { "compute.cancelled" }));
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "a terminal Job has one automatic continuation");
    assert_eq!(requests[2]["messages"].to_string().matches("FOX_HOST_JOB_NOTICE_V1").count(), 1);
    let message = provider_notice_text(&requests[2]["messages"])
        .expect("the third Provider request omitted the typed Host notice");
    let wire: fox_engine_protocol::HostJobNotice = serde_json::from_str(
        message.rsplit('\n').next().unwrap()).expect("the model-visible terminal fact is malformed");
    assert_eq!(wire.run_id, run);
    assert_eq!(wire.job_id, job_id);
    assert_eq!(wire.attempt, notice.attempt);
    assert_eq!(wire.terminal_state, notice.terminal_state.as_str());
    assert_eq!(wire.error_code, notice.error_code);
    assert_eq!(wire.finished_at, notice.finished_at);
    assert_eq!(crate::runtime_host::kernel_host::waiting_wake_test_hooks::take_continuation_transports_for_test(&run),
        vec![if live { "live" } else { "per_round" }]);
    let (parks, wakes, replies, formal): (i64, i64, i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_response'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                    AND json_extract(payload_json,'$.toolCallId')='job-start')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap();
    assert_eq!((parks, wakes, replies, formal), (1, 1, 1, 1));
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("completed"));
    drop(host);
    drop(app);
}
