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
    run_auto_park_then_settle(mode == "live", false);
}

/// A committed park while the old Host still owns the OS lock forces the
/// automatic checker through its lock-contention retry path. This is the
/// lost-wake window; settling during the earlier Provider reply instead would
/// take the direct-notice path and produce no park or wake.
#[test]
fn real_host_auto_wake_survives_park_lock_contention() {
    let selected = std::env::var("FOX_TEST_AUTO_JOB_LOCK_CHILD").ok();
    let Some(mode) = selected else {
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("real_host_auto_wake_survives_park_lock_contention")
                .arg("--test-threads=1")
                .env("FOX_TEST_AUTO_JOB_LOCK_CHILD", mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} automatic lock-contention wake exceeded 55 seconds");
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
    run_auto_park_then_settle(mode == "live", true);
}

fn run_auto_park_then_settle(live: bool, hold_park_lock: bool) {
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
        assert!(release_rx.recv_timeout(Duration::from_secs(25)).is_ok(),
            "bounded QuickJS settlement release never arrived");
    })));

    let mut at_settle_rx = Some(at_settle_rx);
    let mut release_tx = Some(release_tx);
    let race_gate = hold_park_lock.then(|| {
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (unlock_tx, unlock_rx) = std::sync::mpsc::channel();
        let unlock_rx = std::sync::Arc::new(std::sync::Mutex::new(unlock_rx));
        crate::runtime_host::kernel_host::waiting_wake_test_hooks::set_before_park_unlock(
            &run, std::sync::Arc::new(move || {
                entered_tx.send(()).unwrap();
                unlock_rx.lock().unwrap().recv_timeout(Duration::from_secs(20))
                    .expect("the committed park lock was never released by the fixture");
            }));
        (entered_rx, unlock_tx)
    });
    let race_settlement = race_gate.map(|(entered_rx, unlock_tx)| {
        let db = db.clone();
        let run = run.clone();
        let host = host.clone();
        let parent_token = parent_token.clone();
        let at_settle_rx = at_settle_rx.take().unwrap();
        let release_tx = release_tx.take().unwrap();
        std::thread::spawn(move || {
            entered_rx.recv_timeout(Duration::from_secs(20))
                .expect("Host never reached the committed park with its OS lock held");
            assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
            at_settle_rx.recv_timeout(Duration::from_secs(20))
                .expect("QuickJS Job never reached its terminal write");
            let jobs = db.kernel_jobs_for_run(&run).unwrap();
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].state.as_str(), "running");
            assert_eq!(jobs[0].attempts, 1);
            assert!(!parent_token.is_cancelled(), "park retired the child Job parent token");
            let job_id = jobs[0].job_id.clone();
            release_tx.send(()).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let settled = db.kernel_job_snapshot(&job_id).unwrap().state.is_terminal();
                let notice = db.kernel_job_notice(&jobs[0].conversation_id, &run, &job_id)
                    .unwrap().is_some();
                if settled && notice { break; }
                assert!(Instant::now() < deadline,
                    "Job terminal state and scoped notice did not commit before unlock");
                std::thread::sleep(Duration::from_millis(10));
            }
            while host.auto_wake_lock_conflicts_for_test(&run) == 0 {
                assert!(Instant::now() < deadline,
                    "automatic checker never observed the held Run OS lock");
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
            unlock_tx.send(()).unwrap();
            job_id
        })
    });

    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    if live {
        host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    } else {
        host.start_kernel_run_forced_round_for_test(ownership, &binding, Value::Null, Value::Null)
            .unwrap();
    }
    let job_id = if let Some(settlement) = race_settlement {
        settlement.join().unwrap()
    } else {
        assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"),
            "the original Host must park while the Job is held before settlement");
        at_settle_rx.take().unwrap().recv_timeout(Duration::from_secs(20))
            .expect("real QuickJS Job did not reach the terminal write");
        let jobs = db.kernel_jobs_for_run(&run).unwrap();
        assert_eq!(jobs.len(), 1, "one formal job_start creates exactly one Job");
        assert_eq!(jobs[0].attempts, 1);
        assert_eq!(jobs[0].state.as_str(), "running");
        assert!(!parent_token.is_cancelled(), "park retired the child Job parent token");
        release_tx.take().unwrap().send(()).unwrap();
        jobs[0].job_id.clone()
    };
    let formal_before: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
           AND json_extract(payload_json,'$.toolCallId')='job-start'",
        [&run], |row| row.get(0))).unwrap();
    assert_eq!(formal_before, 1);

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
    assert_eq!(
        crate::runtime_host::kernel_host::waiting_wake_test_hooks::take_continuation_transports_for_test(&run),
        vec![if live { "live" } else { "per_round" }],
        "the automatic continuation must use the requested real Pi transport",
    );
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
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (active, watching) = {
            let state = host.state.lock().unwrap();
            (state.kernel_active_runs.contains(&run), state.waiting_wakes.contains_key(&run))
        };
        let lock_released = match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(ownership) => { drop(ownership); true }
            Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED => false,
            Err(error) => panic!("terminal Run lock acquisition failed: {error}"),
        };
        if !active && !watching && lock_released { break; }
        assert!(Instant::now() < cleanup_deadline,
            "completed Run retained Host ownership: active={active} watching={watching} lock_released={lock_released}");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(host);
    drop(app);
}

#[test]
fn real_host_parked_user_cancel_stops_without_job_completion_or_model_resume() {
    isolated_stop_modes(
        "real_host_parked_user_cancel_stops_without_job_completion_or_model_resume",
        "FOX_TEST_PARKED_CANCEL_CHILD", false,
    );
}

#[test]
fn real_host_original_wait_deadline_stops_without_model_resume() {
    isolated_stop_modes(
        "real_host_original_wait_deadline_stops_without_model_resume",
        "FOX_TEST_PARKED_DEADLINE_CHILD", true,
    );
}

fn isolated_stop_modes(test_name: &str, child_env: &str, expire: bool) {
    let selected = std::env::var(child_env).ok();
    let Some(mode) = selected else {
        for mode in ["live", "round"] {
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .arg(test_name).arg("--test-threads=1").env(child_env, mode)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn().unwrap();
            let deadline = Instant::now() + Duration::from_secs(55);
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("{mode} parked {} exceeded 55 seconds",
                        if expire { "deadline" } else { "cancel" });
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
    run_parked_stop(mode == "live", expire);
}

/// The real QuickJS executor has reached its settlement boundary but cannot
/// write a terminal Job row until released below. This makes it possible to
/// prove that user cancel or the original timer signals cleanup first; letting
/// the Job finish naturally would take the ordinary notice wake path.
fn run_parked_stop(live: bool, expire: bool) {
    let arguments = json!({"idempotencyKey":"one-stopped-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (address, server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The compute Job is still running."}),
    ]);
    let mut config = worker_configuration();
    config.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{address}/v1")});
    config.proposal_tools = vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let budgets = if expire {
        TimeBudgets { run_execution_ms: 15_000, run_execution_limited: true,
            ..TimeBudgets::default() }
    } else { TimeBudgets::default() };
    let (db, root, run) = fixture_with_budgets_opt(&clock, &config.hash().unwrap(),
        Some(&config), true, true, (0, 1), true, budgets);
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
    let parent_token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(30))
            .expect("the stopped Job fixture was never released");
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
        .expect("QuickJS Job did not reach its terminal write");
    let jobs = db.kernel_jobs_for_run(&run).unwrap();
    assert_eq!(jobs.len(), 1);
    let job_id = jobs[0].job_id.clone();
    assert_eq!(jobs[0].attempts, 1);
    assert_eq!(jobs[0].state.as_str(), "running");
    assert!(!parent_token.is_cancelled());
    let park_json: String = db.with_connection(|conn| conn.query_row(
        "SELECT payload_json FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'",
        [&run], |row| row.get(0))).unwrap();
    let park: Value = serde_json::from_str(&park_json).unwrap();
    let original_deadline = park["waitDeadlineWallMs"].as_i64().unwrap();
    assert!(original_deadline > crate::database::now_ms());
    if expire {
        assert!(original_deadline <= park["parkedAtWallMs"].as_i64().unwrap() + 15_000);
    } else {
        let started = Instant::now();
        assert!(host.cancel_run(&run).unwrap(), "real user cancel was not queued");
        assert!(started.elapsed() < Duration::from_secs(3), "parked cancel entry blocked");
    }
    let signal_deadline = if expire {
        Instant::now() + Duration::from_millis(
            original_deadline.saturating_sub(crate::database::now_ms()).max(0) as u64)
            + Duration::from_secs(6)
    } else { Instant::now() + Duration::from_secs(6) };
    loop {
        let job = db.kernel_job_snapshot(&job_id).unwrap();
        // Both durable cancellation and the live executor signal must exist
        // before the held Job is allowed to acknowledge its terminal write.
        // A cancelled parent token alone was already supplied by the old
        // cancel_run path and cannot prove parked-child reconciliation.
        let stopped = job.cancel_requested_at.is_some() && parent_token.is_cancelled();
        let run_state = db.kernel_host_run_state(&run).unwrap();
        let terminal = if expire {
            matches!(run_state.as_deref(), Some("failed" | "budget_exhausted"))
        } else { run_state.as_deref() == Some("cancelled") };
        if terminal && stopped && (!expire || crate::database::now_ms() >= original_deadline) {
            break;
        }
        assert_eq!(job.state.as_str(), "running", "Job settled naturally before stop reconciliation");
        assert!(Instant::now() < signal_deadline,
            "parked {} did not terminalize while the Job was still running: run={run_state:?}",
            if expire { "deadline" } else { "cancel" });
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(db.kernel_job_snapshot(&job_id).unwrap().state.as_str(), "running");
    if expire {
        let (event_type, code): (String, String) = db.with_connection(|conn| conn.query_row(
            "SELECT event_type, json_extract(payload_json,'$.code') FROM kernel_events
               WHERE run_id=?1 AND event_type IN ('run.failed','run.budget_exhausted')
               ORDER BY seq DESC LIMIT 1",
            [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
        assert!(matches!((event_type.as_str(), code.as_str()),
            ("run.budget_exhausted", "runtime.duration_budget_exceeded")
                | ("run.failed", "kernel.job_wait_deadline_reached")),
            "parked Run stopped for an unrelated cause: {event_type}/{code}");
    }
    let before_release: (i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_requested')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert_eq!(before_release, (0, 0), "terminal stop incorrectly dispatched a model before Job ack");
    release_tx.send(()).unwrap();
    let terminal_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let run_state = db.kernel_host_run_state(&run).unwrap();
        let job = db.kernel_job_snapshot(&job_id).unwrap();
        let terminal = if expire {
            matches!(run_state.as_deref(), Some("failed" | "budget_exhausted"))
        } else { run_state.as_deref() == Some("cancelled") };
        if terminal && job.state.is_terminal() { break; }
        assert!(Instant::now() < terminal_deadline,
            "parked stop did not settle: run={run_state:?} job={}", job.state.as_str());
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    let job = db.kernel_job_snapshot(&job_id).unwrap();
    assert_eq!(job.state.as_str(), "cancelled", "Job must not publish its natural result");
    assert_eq!(job.attempts, 1);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2, "stopped Run must not ask the Provider again");
    let (parks, wakes, continuations, formal): (i64, i64, i64, i64) = db.with_connection(|conn| conn.query_row(
        "SELECT (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.waiting_jobs'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.continuation_requested'),
                (SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='tool.completed'
                    AND json_extract(payload_json,'$.toolCallId')='job-start')",
        [&run], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap();
    assert_eq!((parks, wakes, continuations, formal), (1, 0, 0, 1));
    if expire {
        let accounted: i64 = db.with_connection(|conn| conn.query_row(
            "SELECT MAX(json_extract(payload_json,'$.throughWallMs')) FROM kernel_events
               WHERE run_id=?1 AND event_type='run.jobs_wait_accounted'",
            [&run], |row| row.get(0))).unwrap();
        assert_eq!(accounted, original_deadline, "the original wait budget was reset or skipped");
    }
    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (active, watching, inflight, scope_retired) = {
            let state = host.state.lock().unwrap();
            (state.kernel_active_runs.contains(&run), state.waiting_wakes.contains_key(&run),
                state.kernel_wake_inflight.contains(&run),
                state.cancellation.run_token(&run).is_err())
        };
        let lock_released = match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(ownership) => { drop(ownership); true }
            Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED => false,
            Err(error) => panic!("terminal Run lock acquisition failed: {error}"),
        };
        if !active && !watching && !inflight && scope_retired && lock_released
            && parent_token.is_cancelled() { break; }
        assert!(Instant::now() < cleanup_deadline,
            "stopped Run retained Host ownership: active={active} watching={watching} inflight={inflight} scope_retired={scope_retired} lock_released={lock_released}");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(host);
    drop(app);
}

/// A second real Run is submitted through the public detached queue while the
/// first Run's automatic wake owns both its OS lock and wake-inflight marker.
/// It must actually reach its separate local Provider after that owner exits.
#[test]
fn real_host_queued_run_dispatches_after_auto_wake_releases_ownership() {
    if std::env::var_os("FOX_TEST_AUTO_WAKE_QUEUE_CHILD").is_none() {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("real_host_queued_run_dispatches_after_auto_wake_releases_ownership")
            .arg("--test-threads=1")
            .env("FOX_TEST_AUTO_WAKE_QUEUE_CHILD", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(55);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("automatic wake queue test exceeded 55 seconds");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "child stdout: {}\nchild stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        return;
    }

    let arguments = json!({"idempotencyKey":"one-queued-compute",
        "params":{"processing":"chunked",
            "code":"function onChunk(c){} function onFinish(){return {answer:42};}"}}).to_string();
    let (first_address, first_server) = start_http_model_fixture(vec![
        json!({"role":"assistant","tool_calls":[{"index":0,"id":"job-start","type":"function",
            "function":{"name":"compute_job_start","arguments":arguments}}]}),
        json!({"role":"assistant","content":"The first Job is still running."}),
        json!({"role":"assistant","content":"The first Run has finished."}),
    ]);
    let (second_request_tx, second_request_rx) = std::sync::mpsc::channel();
    let (second_address, second_server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"The queued Run has finished."})],
        Some((0, Box::new(move || { second_request_tx.send(()).unwrap(); }))));
    let mut first_model = worker_configuration();
    first_model.model_service = json!({"apiType":"openai-completions","modelId":"kernel-http-test",
        "baseUrl":format!("http://{first_address}/v1")});
    first_model.proposal_tools = vec![json!({"name":"compute_job_start",
        "description":"Start a bounded background compute Job",
        "parameters":{"type":"object","properties":{"idempotencyKey":{"type":"string"},
            "params":{"type":"object"}},"required":["idempotencyKey","params"]}})];
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let (db, root, first) = fixture_with_retry_opt(&clock, &first_model.hash().unwrap(),
        Some(&first_model), true, true, (0, 1));
    super::host_job_notice_tests::enable_notices(&db, &first);
    db.freeze_kernel_host_scope(&first, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: ["compute_job_start".into()].into_iter().collect(),
        mcp_server_hashes: Default::default(),
        knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(),
        office_tools: Default::default(),
        lifecycle_hooks: vec![],
    }).unwrap();

    // A distinct conversation satisfies the real create_run active-run guard.
    let conversation = db.create_conversation(db.default_agent_id(), None,
        Some(root.to_str().unwrap()), Some("read_only")).unwrap();
    let started = db.create_run(&conversation.id, "Queued Run local Provider fixture", None).unwrap();
    let second = started.run.id.clone();
    let mut second_binding = db.run_control_binding(&first).unwrap().unwrap();
    second_binding.run_id = second.clone();
    second_binding.conversation_id = conversation.id;
    db.freeze_run_control(&second_binding).unwrap();
    let mut second_model = first_model.clone();
    second_model.model_service["baseUrl"] = json!(format!("http://{second_address}/v1"));
    second_model.proposal_tools.clear();
    let second_hash = second_model.hash().unwrap();
    // The first Run is intentionally still `created` here; rehydration needs
    // start-time retry events. Read the already frozen config row instead.
    let first_frozen_json: String = db.with_connection(|conn| conn.query_row(
        "SELECT frozen_config_json FROM kernel_runs WHERE run_id=?1",
        [&first], |row| row.get(0))).unwrap();
    let mut second_frozen: kernel::RunFrozenConfig =
        serde_json::from_str(&first_frozen_json).unwrap();
    second_frozen.prompt_config_hash = second_hash.clone();
    second_frozen.experimental_compute_job_notice = false;
    db.kernel_create_run(&second, "pi", "authoritative", 2,
        &second_binding.permission_snapshot_id, &second_binding.execution_profile_id,
        &second_hash, &serde_json::to_string(&second_frozen).unwrap()).unwrap();
    db.freeze_kernel_model_config(&second, &second_model).unwrap();
    db.freeze_kernel_initial_input(&initial_input(&second, &second_hash)).unwrap();
    db.freeze_kernel_host_scope(&second, &crate::database::KernelHostScope {
        schema_version: 1,
        tool_names: Default::default(),
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
    let binding = db.run_control_binding(&first).unwrap().unwrap();
    {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&first).unwrap();
    }
    let (at_settle_tx, at_settle_rx) = std::sync::mpsc::channel();
    let (job_release_tx, job_release_rx) = std::sync::mpsc::channel();
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(Some(Box::new(move || {
        at_settle_tx.send(()).unwrap();
        job_release_rx.recv_timeout(Duration::from_secs(20))
            .expect("the first Job was not released after park");
    })));
    let (wake_entered_tx, wake_entered_rx) = std::sync::mpsc::channel();
    let (wake_release_tx, wake_release_rx) = std::sync::mpsc::channel();
    let wake_release_rx = std::sync::Arc::new(std::sync::Mutex::new(wake_release_rx));
    crate::runtime_host::kernel_host::waiting_wake_test_hooks::set_before_wake_account(
        &first, std::sync::Arc::new(move || {
            wake_entered_tx.send(()).unwrap();
            wake_release_rx.lock().unwrap().recv_timeout(Duration::from_secs(20))
                .expect("the automatic wake lock was not released by the fixture");
        }));
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &first).unwrap();
    host.start_kernel_run(ownership, &binding, Value::Null, Value::Null).unwrap();
    assert_eq!(db.kernel_host_run_state(&first).unwrap().as_deref(), Some("waiting_jobs"));
    at_settle_rx.recv_timeout(Duration::from_secs(20))
        .expect("the first QuickJS Job did not reach terminal write");
    job_release_tx.send(()).unwrap();
    wake_entered_rx.recv_timeout(Duration::from_secs(15))
        .expect("automatic wake did not acquire its admission gate");
    {
        let state = host.state.lock().unwrap();
        assert!(state.kernel_wake_inflight.contains(&first));
    }
    assert!(matches!(crate::runtime_host::kernel_host::acquire(&root, &first),
        Err(error) if error == crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED));
    host.start_run_detached(started, "Queued Run local Provider fixture".into(), vec![]);
    std::thread::sleep(Duration::from_millis(500));
    assert!(matches!(second_request_rx.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
        "the second Provider was called while the first wake owned its admission gate");
    assert_eq!(db.kernel_host_run_state(&second).unwrap().as_deref(), Some("created"));
    assert!(host.state.lock().unwrap().queued_runs.iter().any(|queued| queued.started.run.id == second));
    wake_release_tx.send(()).unwrap();

    second_request_rx.recv_timeout(Duration::from_secs(20))
        .expect("queued Run never reached its local Provider after wake ownership released");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let first_state = db.kernel_host_run_state(&first).unwrap();
        let second_state = db.kernel_host_run_state(&second).unwrap();
        if first_state.as_deref() == Some("completed")
            && second_state.as_deref() == Some("completed") { break; }
        assert!(Instant::now() < deadline,
            "queued Run did not finish after wake: first={first_state:?} second={second_state:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    crate::runtime_host::attachment_compute::jobs::test_hooks::set_at_settle(None);
    assert_eq!(first_server.join().unwrap().len(), 3);
    assert_eq!(second_server.join().unwrap().len(), 1);
    let first_wakes: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_woken'",
        [&first], |row| row.get(0))).unwrap();
    let second_initials: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='engine.initial_requested'",
        [&second], |row| row.get(0))).unwrap();
    assert_eq!((first_wakes, second_initials), (1, 1));
    drop(host);
    drop(app);
}
