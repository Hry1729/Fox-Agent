//! Real Host recovery from a committed WaitingJobs notice, with an isolated
//! SQLite database reopened after the original Host shuts down.
use super::*;
use crate::database::{JobStartRequest, ModelNoticeInput};
use crate::kernel::WaitingJobFact;
use std::time::{Duration, Instant};
use tauri::Manager;

#[test]
fn committed_job_notice_recovers_once_across_reopened_hosts_and_pi_transports() {
    let selected = std::env::var("FOX_TEST_JOB_NOTICE_RECOVERY_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("committed_job_notice_recovers_once_across_reopened_hosts_and_pi_transports")
                .arg("--test-threads=1")
                .env("FOX_TEST_JOB_NOTICE_RECOVERY_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} committed-notice recovery exceeded 55 seconds");
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
    recover_committed_notice(mode == "live");
}

fn recover_committed_notice(live: bool) {
    let arguments = json!({"idempotencyKey":"one-recovered-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The Job is still running."}),
        json!({"role":"assistant","content":"The recovered Job result is ready."}),
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
    let old = crate::runtime_host::RuntimeHost::new(app.handle().clone(), db.clone(), root.clone(),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    // Only the old Host's process-local signal is disabled. The Job and its
    // durable terminal notice still pass through the real QuickJS/DB path.
    old.disable_auto_wake_for_test(&run);
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    {
        let state = old.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
    }
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(20))
            .expect("the old Host did not commit its park before Job settlement");
    })));
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        old.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        old.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("the real QuickJS Job never reached its terminal write");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 1);
    let job_id = jobs[0].job_id.clone();
    assert_eq!(jobs[0].state.as_str(), "running");
    release_tx.send(()).unwrap();
    let notice_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let job = db.kernel_job_snapshot(&job_id).unwrap();
        let notice = db.kernel_job_notice(&jobs[0].conversation_id, &run, &job_id).unwrap();
        if job.state.as_str() == "completed" && notice.is_some() { break; }
        assert!(Instant::now() < notice_deadline,
            "old Host Job result and scoped notice did not commit: state={}", job.state.as_str());
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    assert!(!old.state.lock().unwrap().waiting_wakes.contains_key(&run));
    let old_wakes: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'",
        [&run], |row| row.get(0))).unwrap();
    assert_eq!(old_wakes, 0, "the original Host unexpectedly delivered the notice");
    // Model requests one and two have already used the local Provider. The
    // third is reserved for the recovered continuation.
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    old.stop_kernel_runs().unwrap();
    drop(old);
    drop(db);

    let reopened_a = Database::open(root.join("facts.db")).unwrap();
    let reopened_b = Database::open(root.join("facts.db")).unwrap();
    assert_eq!(reopened_a.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    assert!(reopened_a.kernel_job_notice(&jobs[0].conversation_id, &run, &job_id)
        .unwrap().is_some(), "the committed notice disappeared during DB reopen");
    let new_a = crate::runtime_host::RuntimeHost::new(app.handle().clone(), reopened_a.clone(),
        root.clone(), root.join("attachments"), root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap());
    let new_b = crate::runtime_host::RuntimeHost::new(app.handle().clone(), reopened_b.clone(),
        root.clone(), root.join("attachments"), root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap());
    if !live {
        new_a.force_auto_wake_round_for_test(&run);
        new_b.force_auto_wake_round_for_test(&run);
    }
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let a = { let host = new_a.clone(); let barrier = barrier.clone();
        std::thread::spawn(move || { barrier.wait(); host.recover_kernel_runs_detached() }) };
    let b = { let host = new_b.clone(); let barrier = barrier.clone();
        std::thread::spawn(move || { barrier.wait(); host.recover_kernel_runs_detached() }) };
    barrier.wait();
    a.join().unwrap().unwrap();
    b.join().unwrap().unwrap();
    // A repeated recovery is harmless whether the first drive still owns the
    // OS lock or has already left the Run parked for its single wake.
    new_a.recover_kernel_runs_detached().unwrap();
    let completion_deadline = Instant::now() + Duration::from_secs(20);
    while reopened_a.kernel_host_run_state(&run).unwrap().as_deref() != Some("completed") {
        assert!(Instant::now() < completion_deadline,
            "reopened Hosts did not deliver the committed Job result exactly once: {:?}",
            reopened_a.kernel_host_run_state(&run).unwrap());
        std::thread::sleep(Duration::from_millis(20));
    }
    new_b.recover_kernel_runs_detached().unwrap();
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 3, "recovery replayed or lost a Provider request");
    let job = reopened_a.kernel_job_snapshot(&job_id).unwrap();
    assert_eq!(job.state.as_str(), "completed");
    assert_eq!(job.attempts, 1, "recovery restarted the settled Job");
    let (parks, wakes, continuations, formal, initials): (i64, i64, i64, i64, i64) =
        reopened_a.with_connection(|conn| conn.query_row(
            "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_requested'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                        AND json_extract(payload_json,'$.toolCallId')='job-start'),
                    (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_requested')",
            [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))).unwrap();
    assert_eq!((parks, wakes, continuations, formal, initials), (1, 1, 1, 1, 1));
    assert_eq!(crate::runtime_host::kernel_host::waiting_wake_test_hooks::take_continuation_transports_for_test(&run),
        if live { vec!["live"] } else { vec!["per_round"] });
    drop(new_a);
    drop(new_b);
    drop(app);
}

/// This fixture has no live executor: the persisted owner marker is changed
/// to a reused-PID identity after a valid park, then a new Host performs the
/// production orphan reconciliation and detached WaitingJobs recovery.
#[test]
fn orphaned_running_job_stays_paused_until_the_original_wait_deadline() {
    let selected = std::env::var("FOX_TEST_ORPHAN_WAIT_RECOVERY_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["limited", "unlimited"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("orphaned_running_job_stays_paused_until_the_original_wait_deadline")
                .arg("--test-threads=1")
                .env("FOX_TEST_ORPHAN_WAIT_RECOVERY_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} orphan recovery exceeded 55 seconds");
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
    assert!(mode == "limited" || mode == "unlimited");
    recover_orphan_without_resuming(mode == "limited");
}

fn prepare_orphan_wait(limited: bool) -> (Database, std::path::PathBuf, String, String, i64, u64) {
    let now = crate::database::now_ms();
    let clock = TestClock::new(now);
    let model = worker_configuration();
    let budgets = TimeBudgets { run_execution_ms: 9_000, run_execution_limited: limited,
        ..TimeBudgets::default() };
    let (db, root, run) = fixture_with_budgets_opt(&clock, &model.hash().unwrap(),
        Some(&model), true, true, (0, 0), true, budgets);
    freeze_host_scope(&db, &run);
    super::host_job_notice_tests::enable_notices(&db, &run);
    let cancellation = CancellationRegistry::default();
    drop(KernelCoordinator::start_prepared(&db, &clock, &run, &cancellation).unwrap());
    let job = db.kernel_job_start(&JobStartRequest {
        run_id: run.clone(), kind: "attachment_compute".into(),
        idempotency_key: "orphan-wait-test".into(), params: json!({"input":"fixture"}),
        deadline_ms: Some(now + 12_000), progress_total: None,
    }).unwrap().snapshot().job_id.clone();
    db.kernel_job_claim_attempt(&job, 1).unwrap();
    let mut controller = RunController::rehydrate(db.kernel_rehydrate(&run).unwrap().unwrap()).unwrap();
    let input = db.kernel_initial_input(&run).unwrap();
    let frame = fox_engine_protocol::KernelInitialModelFrame {
        schema_version: 1, input, idempotency_key: kernel::INITIAL_MODEL_IDEMPOTENCY_KEY.into(),
        continuation_key: None, continuation_lane: None,
        checkpoint_seq: controller.last_event_seq(), host_job_notices: Vec::new(),
    };
    frame.validate().unwrap();
    let frame_json = serde_json::to_value(&frame).unwrap();
    let history = frame.input.messages.clone();
    let historical_bytes = fox_engine_protocol::historical_host_job_notice_bytes(&history).unwrap();
    let bound = ModelNoticeInput {
        payload: &frame_json, delivered_history: &history, live_history: None,
        checkpoint_seq: frame.checkpoint_seq, history_start: history.len(), historical_bytes,
    };
    let dispatched = controller.begin_initial_model_request(now, now).unwrap();
    db.kernel_commit_initial_model(&run, now, &controller.persist_command(&dispatched),
        "orphan-fixture-owner", false, None, Some(&bound)).unwrap();
    let response = fox_engine_protocol::KernelInitialModelResponse {
        schema_version: 1, run_id: run.clone(), turn_id: frame.input.turn_id.clone(),
        checkpoint_seq: frame.checkpoint_seq,
        assistant_message: json!({"role":"assistant","stopReason":"stop",
            "content":[{"type":"text","text":"The child Job is still running."}]}),
    };
    response.validate().unwrap();
    let response_json = serde_json::to_string(&response).unwrap();
    let settled = controller.record_initial_model_response(&response_json).unwrap();
    let response_seq = controller.last_event_seq();
    let mut effects = vec![settled];
    effects.extend(controller.park_waiting_jobs(now, now, db.kernel_data_root_id(),
        response_seq, &response_json, &serde_json::to_string(&history).unwrap(),
        &[WaitingJobFact { job_id: job.clone(), attempt: 1, deadline_wall_ms: now + 12_000 }]).unwrap());
    db.kernel_commit_initial_model(&run, now, &controller.persist_command(&effects),
        "orphan-fixture-owner", true, None, None).unwrap();
    let park_seq = controller.last_event_seq();
    let park_json: String = db.with_connection(|conn| conn.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run], |row| row.get(0))).unwrap();
    let park: Value = serde_json::from_str(&park_json).unwrap();
    let original_deadline = park["waitDeadlineWallMs"].as_i64().unwrap();
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    assert!(original_deadline > now + 6_000);
    assert!(original_deadline <= now + if limited { 9_000 } else { 12_000 });
    // A reused marker for this same PID is deterministically dead on Windows
    // without probing an arbitrary process or leaving any executor running.
    db.with_connection(|conn| conn.execute(
        "UPDATE kernel_jobs SET owner_pid=?2, owner_started_at=1 WHERE job_id=?1",
        rusqlite::params![job, i64::from(std::process::id())])).unwrap();
    (db, root, run, job, original_deadline, park_seq)
}

fn recover_orphan_without_resuming(limited: bool) {
    let (db, root, run, job_id, original_deadline, park_seq) = prepare_orphan_wait(limited);
    assert_eq!(db.kernel_job_snapshot(&job_id).unwrap().state.as_str(), "running");
    drop(db);
    let reopened = Database::open(root.join("facts.db")).unwrap();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = crate::runtime_host::RuntimeHost::new(app.handle().clone(), reopened.clone(), root.clone(),
        root.join("attachments"), root.join("skills"), crate::yuxi::YuxiClient::new().unwrap());
    let paused = reopened.kernel_job_snapshot(&job_id).unwrap();
    assert_eq!(paused.state.as_str(), "paused", "new Host did not reconcile the dead owner");
    assert!(paused.resumable);
    assert_eq!(paused.attempts, 1);
    host.recover_kernel_runs_detached().unwrap();
    host.recover_kernel_runs_detached().unwrap();
    let settled_park = Instant::now() + Duration::from_secs(3);
    loop {
        let active = host.state.lock().unwrap().kernel_active_runs.contains(&run);
        if !active { break; }
        assert!(Instant::now() < settled_park, "detached recovery did not leave the Run parked");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(reopened.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    assert_eq!(reopened.kernel_job_snapshot(&job_id).unwrap().state.as_str(), "paused");
    assert_eq!(reopened.kernel_job_snapshot(&job_id).unwrap().attempts, 1);
    let (current_seq, park): (i64, String) = reopened.with_connection(|conn| conn.query_row(
        "SELECT seq,payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert_eq!(current_seq, park_seq as i64, "recovery created a new park instead of retaining the old one");
    assert_eq!(serde_json::from_str::<Value>(&park).unwrap()["waitDeadlineWallMs"].as_i64(),
        Some(original_deadline));
    let terminal_deadline = Instant::now() + Duration::from_millis(
        original_deadline.saturating_sub(crate::database::now_ms()).max(0) as u64)
        + Duration::from_secs(6);
    loop {
        let state = reopened.kernel_host_run_state(&run).unwrap();
        if matches!(state.as_deref(), Some("failed" | "budget_exhausted")) { break; }
        assert!(Instant::now() < terminal_deadline,
            "orphaned paused Job left Run waiting past its original deadline: {state:?}");
        assert_eq!(reopened.kernel_job_snapshot(&job_id).unwrap().attempts, 1,
            "recovery automatically resumed the uncertain Job");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (terminal_event, code): (String, String) = reopened.with_connection(|conn| conn.query_row(
        "SELECT event_type,json_extract(payload_json,'$.code') FROM kernel_events
           WHERE run_id=?1 AND event_type IN ('run.failed','run.budget_exhausted')
           ORDER BY seq DESC LIMIT 1",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    let expected = if limited {
        ("run.budget_exhausted", "runtime.duration_budget_exceeded")
    } else { ("run.failed", "kernel.job_wait_deadline_reached") };
    assert_eq!((terminal_event.as_str(), code.as_str()), expected);
    let intervals: Vec<(i64, i64)> = reopened.with_connection(|conn| {
        let mut statement = conn.prepare("SELECT json_extract(payload_json,'$.fromWallMs'),
            json_extract(payload_json,'$.throughWallMs') FROM kernel_events
            WHERE run_id=?1 AND event_type='run.jobs_wait_accounted' ORDER BY seq")?;
        let rows = statement.query_map([&run], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    }).unwrap();
    assert!(!intervals.is_empty());
    for pair in intervals.windows(2) {
        assert_eq!(pair[0].1, pair[1].0, "recovery double-charged or skipped a wait interval");
    }
    assert_eq!(intervals.last().unwrap().1, original_deadline,
        "the original absolute wait deadline was refreshed or bypassed");
    let final_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (watching, active, inflight, retired) = {
            let state = host.state.lock().unwrap();
            (state.waiting_wakes.contains_key(&run), state.kernel_active_runs.contains(&run),
                state.kernel_wake_inflight.contains(&run), state.cancellation.run_token(&run).is_err())
        };
        let unlocked = match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(lock) => { drop(lock); true }
            Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED => false,
            Err(error) => panic!("orphan terminal OS lock probe failed: {error}"),
        };
        if !watching && !active && !inflight && retired && unlocked { break; }
        assert!(Instant::now() < final_deadline,
            "orphan terminal cleanup retained ownership: watching={watching} active={active} inflight={inflight} retired={retired} unlocked={unlocked}");
        std::thread::sleep(Duration::from_millis(10));
    }
    let job = reopened.kernel_job_snapshot(&job_id).unwrap();
    assert_eq!(job.attempts, 1);
    assert_ne!(job.state.as_str(), "completed", "orphan was silently replayed");
    let (wakes, continuations): (i64, i64) = reopened.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_requested')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert_eq!((wakes, continuations), (0, 0));
    drop(host);
    drop(app);
}
