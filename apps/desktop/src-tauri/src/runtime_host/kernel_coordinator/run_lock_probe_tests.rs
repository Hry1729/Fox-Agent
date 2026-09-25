//! Test-only probes and a regression test for the `kernel.run_already_owned`
//! conflict on the real Host start/recovery lock race.
//!
//! Evidence gathered here (see the card's evidence log):
//!
//! * Repeating the real test body 40 times in-process fails ~15% of the time at
//!   the same place the suite fails: the immediate reacquire after the park.
//! * The captured lock trace names the holder every time: the Host's own
//!   WaitingJobs watch task (`tokio-rt-worker`), which takes the same advisory
//!   Run lease for a legitimate bounded wake check right after the start returns.
//! * With that watch task disabled for the Run, 40 repeats conflict zero times.
//!
//! So the failure is a test timing assumption, not a production lock leak: the
//! start path has released the lease (the trace shows the release before the
//! reacquire), and a *different, legitimate* Host path holds it.
//!
//! Everything here is observation or a deterministic contract test. No lock,
//! budget, token or error semantics are changed. The heavy probes are gated
//! behind `FOX_RUN_LOCK_TRACE` so the ordinary suite only pays an env lookup.
#![cfg(test)]

use super::*;
use super::waiting_jobs_storage_tests::parked_job_fixture;
use crate::runtime_host::RuntimeHost;

/// Bounded repeats inside ONE test, so the conflict is caught without the
/// runner being re-invoked in a loop.
const PROBE_REPEATS_DEFAULT: usize = 40;

fn probe_repeats() -> usize {
    std::env::var("FOX_RUN_LOCK_REPEATS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(PROBE_REPEATS_DEFAULT)
}

fn probe_enabled() -> bool {
    std::env::var_os("FOX_RUN_LOCK_TRACE").is_some()
}

/// The failing scenario, repeated. Each failure prints the captured lock trace
/// (who acquired, who conflicted) instead of just the bare error string.
#[test]
fn probe_real_lock_race_repeats() {
    if !probe_enabled() {
        return;
    }
    let repeats = probe_repeats();
    let mut failures = 0usize;
    for attempt in 0..repeats {
        if let Err(report) = super::waiting_jobs_storage_tests::real_host_lock_race_probe() {
            failures += 1;
            eprintln!("[run-lock-repeat] attempt {attempt} FAILED\n{report}\n---");
        }
    }
    eprintln!("[run-lock-repeat] repeats={repeats} failures={failures}");
}

/// CONTROL: the same scenario with the Host's WaitingJobs watch task disabled
/// for the Run. Zero conflicts here attributes the conflict to that watch task
/// rather than to the start or recovery ownership paths.
#[test]
fn probe_real_lock_race_with_auto_wake_disabled() {
    if !probe_enabled() {
        return;
    }
    let repeats = probe_repeats();
    let mut failures = 0usize;
    for attempt in 0..repeats {
        if let Err(report) =
            super::waiting_jobs_storage_tests::real_host_lock_race_probe_auto_wake_disabled()
        {
            failures += 1;
            eprintln!("[run-lock-control] attempt {attempt} FAILED\n{report}\n---");
        }
    }
    eprintln!("[run-lock-control] repeats={repeats} failures={failures}");
    assert_eq!(
        failures, 0,
        "with auto-wake disabled the immediate reacquire must always succeed"
    );
}

/// Regression test for the actual contract, written so it cannot race.
///
/// `start_kernel_run` may legitimately hand the lease straight to the WaitingJobs
/// watch task, so "the very next `acquire` must succeed" is not a property of
/// the Host. What the Host does guarantee is bounded and worth pinning:
///
/// * a conflict is reported as `kernel.run_already_owned`, never as a
///   cancellation or a different error;
/// * the issued Job parent token survives the conflict window;
/// * the Run stays durably parked and no shadow owner is left in
///   `kernel_active_runs`;
/// * the lease is released within a bounded number of attempts, after which the
///   Run can be owned again.
///
/// The retry budget below is small and fixed: it observes a legitimate bounded
/// handoff, it is not a pass-by-retry loop.
#[test]
fn run_lease_handoff_to_watch_task_is_bounded_and_preserves_the_parent_token() {
    use tauri::Manager;
    const ATTEMPTS: usize = 100;
    let (db, root, run, _conversation, _job, _controller, _park_seq, _now) = parked_job_fixture();
    let mut context = tauri::generate_context!();
    for window in &mut context.config_mut().app.windows {
        window.create = false;
    }
    let app = tauri::Builder::default().any_thread().build(context).unwrap();
    let host = RuntimeHost::new(
        app.handle().clone(),
        db.clone(),
        root.clone(),
        root.join("attachments"),
        root.join("skills"),
        crate::yuxi::YuxiClient::new().unwrap(),
    );
    let binding = db.run_control_binding(&run).unwrap().unwrap();
    let token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };
    let ownership = crate::runtime_host::kernel_host::acquire(&root, &run).unwrap();
    host.recover_kernel_runs_detached().unwrap();
    host.start_kernel_run(
        ownership,
        &binding,
        serde_json::Value::Null,
        serde_json::Value::Null,
    )
    .unwrap();

    // Durable state and marker hygiene must hold no matter who wins the lease.
    assert_eq!(
        db.kernel_host_run_state(&run).unwrap().as_deref(),
        Some("waiting_jobs")
    );
    assert!(host.state.lock().unwrap().kernel_active_runs.is_empty());
    assert!(
        !token.is_cancelled(),
        "a lease conflict must never cancel the issued Job parent token"
    );

    // Force the conflict deterministically instead of hoping to lose a race.
    // First wait (bounded) until the lease is free again: `start_kernel_run` may
    // hand it straight to the WaitingJobs watch task, which is the very handoff
    // being characterised. Only then take it from a competing holder.
    let mut free = false;
    for _ in 0..ATTEMPTS {
        match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(lease) => {
                drop(lease);
                free = true;
                break;
            }
            Err(error) => {
                assert_eq!(
                    error,
                    crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED,
                    "only a real ownership conflict may block the reacquire, got: {error}"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    assert!(
        free,
        "the lease must become obtainable again within {ATTEMPTS} bounded attempts"
    );

    // Now hold it from another thread and prove the conflict contract while it
    // is genuinely held. This is the exact condition the suite hit when the
    // WaitingJobs watch task won the handoff.
    let (held_tx, held_rx) = std::sync::mpsc::channel::<()>();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let root_for_holder = root.clone();
    let run_for_holder = run.clone();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let held =
                crate::runtime_host::kernel_host::acquire(&root_for_holder, &run_for_holder)
                    .expect("the competing holder must be able to take the lease");
            held_tx.send(()).unwrap();
            let _ = release_rx.recv();
            drop(held);
        });
        held_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the competing holder must acquire within the bound");

        // While it is genuinely held, the conflict must be exactly one error.
        let conflict = match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(_lease) => panic!("the lease is held by another owner; this acquire must conflict"),
            Err(error) => error,
        };
        assert_eq!(
            conflict,
            crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED,
            "an ownership conflict must stay kernel.run_already_owned, got: {conflict}"
        );

        // The conflict window must not cancel the issued Job parent token, must
        // not invent an owner marker, and must not move the Run out of park.
        assert!(
            !token.is_cancelled(),
            "a lease conflict must never cancel the issued Job parent token"
        );
        assert!(
            host.state.lock().unwrap().kernel_active_runs.is_empty(),
            "a conflicting acquire must not register a shadow owner"
        );
        assert_eq!(
            db.kernel_host_run_state(&run).unwrap().as_deref(),
            Some("waiting_jobs"),
            "a lease conflict must not change the durable Run state"
        );

        release_tx.send(()).unwrap();
    });

    // After the competing holder releases, the lease must be obtainable again:
    // the conflict was bounded, not a leak.
    let mut owned = false;
    for _ in 0..ATTEMPTS {
        match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(lease) => {
                drop(lease);
                owned = true;
                break;
            }
            Err(error) => {
                assert_eq!(
                    error,
                    crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED,
                    "only a real ownership conflict may block the reacquire, got: {error}"
                );
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
    assert!(
        owned,
        "the lease must be obtainable again after the competing holder \
         released it within {ATTEMPTS} bounded attempts"
    );
    // The contracts still hold after the handoff window.
    assert!(
        !token.is_cancelled(),
        "the parent token must survive the whole handoff"
    );
    assert_eq!(
        db.kernel_host_run_state(&run).unwrap().as_deref(),
        Some("waiting_jobs"),
        "the Run must still be durably parked after the handoff"
    );
    assert!(host.state.lock().unwrap().kernel_active_runs.is_empty());
    drop(host);
    drop(app);
}
