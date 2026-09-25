//! Component test for the Run lease under a CONTROLLED competitor.
//!
//! This is deliberately NOT a proof about the Host's WaitingJobs watch task.
//! It hand-holds the same OS lease from a second thread and pins what the lease
//! primitive guarantees, so a change to the lock's error contract or its
//! release semantics fails here quickly and deterministically.
//!
//! The real Host-side handoff — where `start_kernel_run` can legitimately pass
//! the lease to the auto-wake watch task right after a park — is covered by
//! `waiting_jobs_storage_tests::waiting_jobs_real_host_start_and_recovery_lock_race_preserve_parent_token`,
//! which waits for that handoff under one bounded monotonic deadline.
#![cfg(test)]

use super::*;
use super::waiting_jobs_storage_tests::parked_job_fixture;
use crate::runtime_host::RuntimeHost;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Upper bound for every wait in this test. Both the competing holder and the
/// main thread are bounded, so a failure in either cannot hang the suite.
const COMPETITOR_TIMEOUT: Duration = Duration::from_secs(5);

/// The lease primitive under a controlled competitor.
///
/// The lease is acquired ONCE by the main thread and then MOVED into the holder
/// thread. There is deliberately no "main releases, then another thread tries to
/// take it" window: the holder provably owns the lease for the whole conflict
/// observation, so the conflict can only come from real ownership.
#[test]
fn run_lease_component_reports_owned_conflict_and_is_reusable_after_release() {
    use tauri::Manager;
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
    // A real parked Run with a live parent token, so the assertions below are
    // about a state the Host actually produces rather than a synthetic fixture.
    let token = {
        let state = host.state.lock().unwrap();
        state.cancellation.register_run(&run).unwrap();
        state.cancellation.tool_token(&run, "job-start").unwrap()
    };

    // Acquire once, here, then hand this very lease to the competing holder.
    let lease = crate::runtime_host::kernel_host::acquire(&root, &run)
        .expect("the Run lease must be obtainable before the competitor takes it");

    let (acquired_tx, acquired_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let root_for_holder = root.clone();
    let run_for_holder = run.clone();
    let mut holder_panicked = false;
    std::thread::scope(|scope| {
        let holder = scope.spawn(move || {
            // The lease is already owned here; nothing re-acquires it, so the
            // conflict the main thread sees cannot be a self-conflict or a
            // release-window artefact.
            let held = lease;
            if acquired_tx.send(()).is_err() {
                return;
            }
            // Bounded wait: if the main thread fails first, its drop of
            // `release_tx` ends this recv instead of parking the thread forever.
            let _ = release_rx.recv_timeout(COMPETITOR_TIMEOUT);
            drop(held);
        });

        acquired_rx
            .recv_timeout(COMPETITOR_TIMEOUT)
            .expect("the competing holder must signal ownership within the bound");

        // While the lease is genuinely held by the other thread, the error must
        // be exactly the ownership conflict and nothing else.
        let conflict = match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(_lease) => panic!("the lease is held by another owner; this acquire must conflict"),
            Err(error) => error,
        };
        assert_eq!(
            conflict,
            crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED,
            "an ownership conflict must stay kernel.run_already_owned, got: {conflict}"
        );

        // The conflict window must not disturb the Run's parked contracts.
        assert!(
            !token.is_cancelled(),
            "a lease conflict must never cancel the issued Job parent token"
        );
        assert!(
            host.state.lock().unwrap().kernel_active_runs.is_empty(),
            "a conflicting acquire must not register a shadow active owner"
        );
        assert_eq!(
            db.kernel_host_run_state(&run).unwrap().as_deref(),
            Some("waiting_jobs"),
            "a lease conflict must not change the durable Run state"
        );

        // Release the holder and collect it. Sending may fail only if the holder
        // already timed out, which is not a test failure by itself.
        let _ = release_tx.send(());
        holder_panicked = holder.join().is_err();
    });
    assert!(!holder_panicked, "the competing holder thread must not panic");

    // After the holder released, the same lease must be obtainable again: the
    // conflict was bounded ownership, not a leak. One bounded loop, never an
    // open-ended retry.
    let deadline = Instant::now() + COMPETITOR_TIMEOUT;
    let mut reused = false;
    while Instant::now() < deadline {
        match crate::runtime_host::kernel_host::acquire(&root, &run) {
            Ok(lease) => {
                drop(lease);
                reused = true;
                break;
            }
            Err(error) => {
                assert_eq!(
                    error,
                    crate::runtime_host::kernel_run_lock::KERNEL_RUN_ALREADY_OWNED,
                    "only a real ownership conflict may block the reacquire, got: {error}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }
    assert!(
        reused,
        "the lease must be obtainable again within {COMPETITOR_TIMEOUT:?} of the holder releasing it"
    );
    assert!(
        !token.is_cancelled(),
        "the parent token must survive the whole component scenario"
    );
    assert_eq!(
        db.kernel_host_run_state(&run).unwrap().as_deref(),
        Some("waiting_jobs"),
        "the Run must still be durably parked at the end"
    );
    drop(host);
    drop(app);
}
