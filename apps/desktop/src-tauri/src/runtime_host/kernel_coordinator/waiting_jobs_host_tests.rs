//! Real detached RuntimeHost recovery entry tests for parked and failed Runs.
//! The fixture uses isolated SQLite and a windowless local Tauri AppHandle.
use super::*;
use std::time::{Duration, Instant};
use tauri::Manager;

fn wait_until(label: &str, mut complete: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !complete() {
        assert!(Instant::now() < deadline, "timed out waiting for {label}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn waiting_jobs_recovery_detached_accounts_and_releases_host_without_retiring_job_token() {
    let (db, root, run, _conversation, _job, _controller, _park_seq, _now) =
        super::waiting_jobs_storage_tests::parked_job_fixture();
    let before = db.kernel_rehydrate(&run).unwrap().unwrap();
    let previous_accounted = before.wait_accounted_until_wall_ms.unwrap();
    let previous_account_events: i64 = db.with_connection(|conn| conn.query_row(
        "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_wait_accounted'",
        [&run], |row| row.get(0),
    )).unwrap();

    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = super::super::super::RuntimeHost::new(
        app.handle().clone(), db.clone(), root.clone(), root.join("attachments"),
        root.join("skills"), crate::yuxi::YuxiClient::new().unwrap(),
    );
    let job_parent_token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    // The accounting event below is the completion witness. Merely seeing an
    // empty active set and an available lock could pass before the task starts.
    std::thread::sleep(Duration::from_millis(50));
    host.recover_kernel_runs_detached().unwrap();
    wait_until("detached parked-Run accounting and cleanup", || {
        let accounted = db.kernel_rehydrate(&run).unwrap().unwrap()
            .wait_accounted_until_wall_ms.unwrap_or_default();
        let events: i64 = db.with_connection(|conn| conn.query_row(
            "SELECT COUNT(*) FROM kernel_events WHERE run_id=?1 AND event_type='run.jobs_wait_accounted'",
            [&run], |row| row.get(0),
        )).unwrap();
        accounted > previous_accounted && events > previous_account_events
            && host.state.lock().unwrap().kernel_active_runs.is_empty()
    });

    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("waiting_jobs"));
    assert!(!job_parent_token.is_cancelled(), "parked Job parent token was retired");
    let ownership = super::super::super::kernel_host::acquire(&root, &run)
        .expect("detached parked Run must release its OS lock");
    drop(ownership);
    drop(host);
    drop(app);
}

#[test]
fn created_recovery_detached_records_preparation_failure_and_retires_scope() {
    if std::env::var_os("FOX_TEST_DETACHED_PREP_FAILURE_CHILD").is_none() {
        // runtime_command reads a process-wide override; isolate this test so
        // other concurrent Runs never see the deliberately missing executable.
        let missing = std::env::temp_dir().join(format!(
            "fox-missing-runtime-{}", uuid::Uuid::new_v4(),
        ));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("created_recovery_detached_records_preparation_failure_and_retires_scope")
            .arg("--test-threads=1")
            .env("FOX_TEST_DETACHED_PREP_FAILURE_CHILD", "1")
            .env("FOX_RUNTIME_EXECUTABLE", &missing)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("isolated detached preparation failure exceeded 20 seconds");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "child stdout: {}\nchild stderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr));
        return;
    }

    let now = crate::database::now_ms();
    let clock = TestClock::new(now);
    let model = worker_configuration();
    let (db, root, run) = fixture_with_start_opt(
        &clock, &model.hash().unwrap(), Some(&model), true, true,
    );
    assert_eq!(db.kernel_host_run_state(&run).unwrap().as_deref(), Some("created"));
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows { window.create = false; }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = super::super::super::RuntimeHost::new(
        app.handle().clone(), db.clone(), root.clone(), root.join("attachments"),
        root.join("skills"), crate::yuxi::YuxiClient::new().unwrap(),
    );
    let issued_token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.run_token(&run).unwrap()
    };
    host.recover_kernel_runs_detached().unwrap();
    wait_until("detached preparation failure callback", || {
        db.kernel_host_run_state(&run).unwrap().as_deref() == Some("failed")
            && host.state.lock().unwrap().kernel_active_runs.is_empty()
    });
    let legacy_status: String = db.with_connection(|conn| conn.query_row(
        "SELECT status FROM runs WHERE id=?1", [&run], |row| row.get(0),
    )).unwrap();
    assert_eq!(legacy_status, "failed");
    assert!(issued_token.is_cancelled(), "failed Run scope was not retired");
    let ownership = super::super::super::kernel_host::acquire(&root, &run)
        .expect("failed detached Run must release its OS lock");
    drop(ownership);
    drop(host);
    drop(app);
}
