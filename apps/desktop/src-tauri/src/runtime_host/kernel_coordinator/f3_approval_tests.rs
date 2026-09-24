//! F3: exercise an approval policy change inside the real live Pi round loop.
//! The HTTP Provider is fixed and local; the Node worker and Host are real.

use super::*;

#[test]
fn f3_live_ask_to_allow_reevaluates_pending_write() {
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start(version.clone());
    let run = run_fixture_with_mode("f3-live-ask-allow", SMALL, model(provider.address), "ask");
    let writes = AtomicUsize::new(0);
    let waiting_visits = AtomicUsize::new(0);
    let switched = AtomicBool::new(false);
    let cancellation = CancellationRegistry::default();
    let worker = real_worker_command();
    let gateway = policy(&run);
    let previews = Arc::new(AtomicUsize::new(0));
    let received_previews = previews.clone();
    let preview = move |_: &fox_engine_protocol::KernelModelPreview| {
        received_previews.fetch_add(1, Ordering::SeqCst);
    };
    let coordinator = KernelCoordinator::start_prepared(&run.db, &run.clock, &run.id, &cancellation)
        .unwrap()
        .with_preview(&preview);
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        execute_real(&run, &version, &writes, binding, effect, token)
    };
    let after_commit = |_: &str| Ok(());
    let settle = |_: bool| {
        let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
        let write = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f1-write");
        if write.is_some_and(|call| call.state == "completed") {
            return Err(super::super::super::live::LIVE_DETACHED.into());
        }
        if write.is_some_and(|call| call.state == "waiting_approval") {
            if !switched.swap(true, Ordering::SeqCst) {
                let old = run.db.execution_policy(&run.conversation)?.version;
                run.db.change_execution_policy(&run.conversation, "f3-live-ask-allow", old, "allow")?;
            }
            // An internal bound also makes the old implementation fail promptly:
            // it cannot leave this test in an approval wait until the run budget.
            if waiting_visits.fetch_add(1, Ordering::SeqCst) >= 5 {
                return Err(super::super::super::live::LIVE_DETACHED.into());
            }
        }
        Ok(())
    };
    let outcome = coordinator.dispatch_initial_live(
        "f3-live-owner", &gateway, &worker, "local-test-only", &execute, &after_commit, &settle,
    );
    assert_eq!(outcome.unwrap_err(), super::super::super::live::LIVE_DETACHED);
    let requests = provider.finish();
    assert!(switched.load(Ordering::SeqCst), "the live barrier was not entered for the pending write");
    assert_eq!(requests.len(), 2, "the real live Node worker requested read and write rounds");
    assert!(strings(&requests[1]["messages"]).contains(SMALL), "actual Provider input includes the read result");
    // Text previews are optional for tool-only streaming rounds. The receiver
    // was wired through with_preview; the two Provider messages prove the
    // live Node worker reached both model rounds.
    let _preview_events = previews.load(Ordering::SeqCst);
    let snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
    let write = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f1-write").unwrap();
    assert_eq!(write.state, "completed", "live loop must re-evaluate the expired ask intent");
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(std::fs::read_to_string(run.root.join("target.txt")).unwrap(), CANDIDATE);
    assert_eq!(version_count(&run), 1);
    assert!(run.db.pending_kernel_host_commands(&run.id).unwrap().is_empty());
}
