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
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let worker = real_worker_command();
    let gateway = policy(&run);
    let previews = Arc::new(AtomicUsize::new(0));
    let received_previews = previews.clone();
    let preview = move |_: &fox_engine_protocol::KernelModelPreview| {
        received_previews.fetch_add(1, Ordering::SeqCst);
    };
    let coordinator = KernelCoordinator::start_prepared(&run.db, &clock, &run.id, &cancellation)
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

#[test]
fn f3_fixed_clock_expired_card_is_rejected_after_real_node_proposal() {
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start(version.clone());
    let run = run_fixture_with_mode("f3-fixed-clock-expired", SMALL, model(provider.address), "ask");
    // This one test intentionally mixes a stale deterministic Kernel clock
    // with the Host repository's real wall clock. The real transport tests
    // below use ReconcilerClock for both sides of the approval deadline.
    run.clock.advance(-20_000);
    let writes = AtomicUsize::new(0);
    let observed_expiry = AtomicBool::new(false);
    let visits = AtomicUsize::new(0);
    let cancellation = CancellationRegistry::default();
    let worker = real_worker_command();
    let gateway = policy(&run);
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        execute_real(&run, &version, &writes, binding, effect, token)
    };
    let settle = |_: bool| {
        let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
        if visits.fetch_add(1, Ordering::SeqCst) > 30 {
            return Err("real Node did not reach the fixed-clock approval".into());
        }
        if snapshot.tool_calls.iter().any(|call| call.tool_call_id == "f1-write"
            && call.state == "waiting_approval") {
            let deadline = snapshot.approval_deadline_wall_ms.expect("pending approval deadline");
            assert!(deadline < crate::database::now_ms(), "the test card is deliberately expired by real wall time");
            let card = run.db.kernel_approval_identity(&run.id, "f1-write")?;
            let refusal = run.db.queue_kernel_host_approval(&card, "allow_once")
                .expect_err("expired card must be refused by the real Host repository");
            assert!(refusal.contains("Kernel approval is no longer actionable"), "{refusal}");
            observed_expiry.store(true, Ordering::SeqCst);
            return Err(super::super::super::live::LIVE_DETACHED.into());
        }
        Ok(())
    };
    let after_commit = |_: &str| Ok(());
    let result = KernelCoordinator::start_prepared(&run.db, &run.clock, &run.id, &cancellation)
        .unwrap().with_preview(&preview).dispatch_initial_live(
            "f3-fixed-clock-owner", &gateway, &worker, "local-test-only",
            &execute, &after_commit, &settle);
    assert_eq!(result.unwrap_err(), super::super::super::live::LIVE_DETACHED);
    assert_eq!(provider.finish().len(), 2, "real Node produced read then pending write");
    assert!(observed_expiry.load(Ordering::SeqCst));
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_to_string(run.root.join("target.txt")).unwrap(), SMALL);
    assert_eq!(version_count(&run), 0);
    assert!(run.db.pending_kernel_host_commands(&run.id).unwrap().is_empty());
}

#[derive(Clone, Copy, Debug)]
enum F3Case {
    AskAllow,
    AskReadOnly,
    QueuedThenReadOnly,
    ExpiredCardReplay,
    ConversationGrantReuse,
    ReadOnlyAsk,
}

impl F3Case {
    fn initial_mode(self) -> &'static str {
        if matches!(self, Self::ReadOnlyAsk) { "read_only" } else { "ask" }
    }

    fn repeats(self) -> bool {
        matches!(self, Self::ConversationGrantReuse | Self::ReadOnlyAsk)
    }

    fn expected_writes(self) -> usize {
        match self {
            Self::AskReadOnly | Self::QueuedThenReadOnly => 0,
            Self::ConversationGrantReuse => 2,
            _ => 1,
        }
    }
}

// Both entries use the real Pi Node worker and fixed local HTTP Provider. The
// per-round path enters the owning Host drive through its test-only transport
// selector; live enters dispatch_initial_live directly so it cannot fall back.
fn f3_matrix_case(case: F3Case, live: bool) {
    let label = format!("f3-{:?}-{}", case, if live { "live" } else { "round" });
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start_with_repeated_write(version.clone(), case.repeats());
    let run = run_fixture_with_mode(&label, SMALL, model(provider.address), case.initial_mode());
    let writes = AtomicUsize::new(0);
    let visits = AtomicUsize::new(0);
    let acted = AtomicBool::new(false);
    let approved_reissue = AtomicBool::new(false);
    let original_ticket = Mutex::new(None::<String>);
    let cancellation = CancellationRegistry::default();
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let worker = real_worker_command();
    let gateway = policy(&run);
    let preview_count = Arc::new(AtomicUsize::new(0));
    let received_previews = preview_count.clone();
    let preview = move |_: &fox_engine_protocol::KernelModelPreview| {
        received_previews.fetch_add(1, Ordering::SeqCst);
    };
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        execute_real(&run, &version, &writes, binding, effect, token)
    };
    let after_commit = |_: &str| Ok(());
    let settle = |_: bool| {
        let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
        if visits.fetch_add(1, Ordering::SeqCst) > 80 {
            return Err(format!("F3 {:?} {:?} did not reach barrier: state={} calls={:?}",
                case, live, snapshot.state, snapshot.tool_calls));
        }
        let first = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f1-write");
        let second = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f3-write-2");
        let first_waiting = first.is_some_and(|call| call.state == "waiting_approval"
            && call.approval_state.as_deref() == Some("pending"));
        let second_waiting = second.is_some_and(|call| call.state == "waiting_approval"
            && call.approval_state.as_deref() == Some("pending"));
        let first_settled = first.is_some_and(|call| matches!(call.state.as_str(), "completed" | "failed"));
        let second_settled = second.is_some_and(|call| matches!(call.state.as_str(), "completed" | "failed"));
        if matches!(case, F3Case::ReadOnlyAsk) && first_settled && !acted.swap(true, Ordering::SeqCst) {
            let old = run.db.execution_policy(&run.conversation)?.version;
            run.db.change_execution_policy(&run.conversation, &format!("{label}-ask"), old, "ask")?;
        }
        if first_waiting && !acted.load(Ordering::SeqCst) {
            acted.store(true, Ordering::SeqCst);
            let old = run.db.execution_policy(&run.conversation)?.version;
            match case {
                F3Case::AskAllow => {
                    run.db.change_execution_policy(&run.conversation, &label, old, "allow")?;
                }
                F3Case::AskReadOnly => {
                    run.db.change_execution_policy(&run.conversation, &label, old, "read_only")?;
                }
                F3Case::QueuedThenReadOnly => {
                    let ticket = run.db.kernel_approval_identity(&run.id, "f1-write")?;
                    assert!(run.db.queue_kernel_host_approval(&ticket, "allow_once")?);
                    run.db.change_execution_policy(&run.conversation, &label, old, "read_only")?;
                }
                F3Case::ExpiredCardReplay => {
                    let ticket = run.db.kernel_approval_identity(&run.id, "f1-write")?;
                    *original_ticket.lock().unwrap() = Some(ticket);
                    let middle = run.db.change_execution_policy(&run.conversation, &format!("{label}-ro"), old, "read_only")?;
                    run.db.change_execution_policy(&run.conversation, &format!("{label}-ask"), middle.version, "ask")?;
                }
                F3Case::ConversationGrantReuse => {
                    let ticket = run.db.kernel_approval_identity(&run.id, "f1-write")?;
                    assert!(run.db.queue_kernel_host_approval(&ticket, "allow_conversation")?);
                }
                F3Case::ReadOnlyAsk => unreachable!("first write was refused under read_only"),
            }
        } else if first_waiting && matches!(case, F3Case::ExpiredCardReplay)
            && acted.load(Ordering::SeqCst) && !approved_reissue.swap(true, Ordering::SeqCst) {
            let old = original_ticket.lock().unwrap().clone().expect("original card");
            assert!(run.db.queue_kernel_host_approval(&old, "allow_once").is_err(),
                "an expired card cannot be replayed");
            let new = run.db.kernel_approval_identity(&run.id, "f1-write")?;
            assert_ne!(old, new, "same tool call needs a new policy generation");
            assert!(run.db.queue_kernel_host_approval(&new, "allow_once")?);
        }
        if second_waiting && matches!(case, F3Case::ReadOnlyAsk)
            && !approved_reissue.swap(true, Ordering::SeqCst) {
            let ticket = run.db.kernel_approval_identity(&run.id, "f3-write-2")?;
            assert!(run.db.queue_kernel_host_approval(&ticket, "allow_once")?);
        }
        let done = match case {
            F3Case::AskReadOnly | F3Case::QueuedThenReadOnly => first_settled,
            F3Case::ConversationGrantReuse | F3Case::ReadOnlyAsk => second_settled,
            F3Case::AskAllow | F3Case::ExpiredCardReplay => first_settled,
        };
        if done {
            return Err(super::super::super::live::LIVE_DETACHED.into());
        }
        Ok(())
    };
    let outcome = if live {
        KernelCoordinator::start_prepared(&run.db, &clock, &run.id, &cancellation)
            .unwrap().with_preview(&preview).dispatch_initial_live(
                &label, &gateway, &worker, "local-test-only", &execute, &after_commit, &settle)
    } else {
        let ownership = super::super::super::super::kernel_host::acquire(&run.root, &run.id).unwrap();
        super::super::super::super::kernel_host::drive_with_actions_per_round(
            &ownership, &run.db, &clock, &cancellation, &run.id, &worker,
            "local-test-only", &gateway, &execute, &after_commit, &settle, &preview)
    };
    match outcome {
        Ok(()) => {
            let state = run.db.kernel_build_full_snapshot(&run.id).unwrap().state;
            assert!(matches!(state.as_str(), "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired"),
                "{:?} {} returned before a durable terminal state: {state}", case,
                if live { "live" } else { "round" });
        }
        Err(error) => assert_eq!(error, super::super::super::live::LIVE_DETACHED,
            "{:?} {} failed; Provider={:?}", case,
            if live { "live" } else { "round" }, provider.event_log()),
    }
    let requests = provider.finish();
    let minimum_requests = if case.repeats() { 4 } else { 2 };
    assert!((minimum_requests..=minimum_requests + 3).contains(&requests.len()),
        "{:?} {} real Provider rounds: got {}, expected at least {}",
        case, if live { "live" } else { "round" }, requests.len(), minimum_requests);
    assert!(strings(&requests[1]["messages"]).contains(SMALL), "actual second request carries the real read");
    if case.repeats() {
        assert!(strings(&requests[3]["messages"]).contains("f3-read-2"),
            "{:?} {} fourth request carries the second real read", case,
            if live { "live" } else { "round" });
    }
    // Tool-only Provider rounds may emit no text preview. The sink is wired;
    // request and durable Host evidence establish the transport.
    let _previews = preview_count.load(Ordering::SeqCst);
    assert!(acted.load(Ordering::SeqCst), "the policy/approval transition was not exercised");
    let expected_body = match case {
        F3Case::AskReadOnly | F3Case::QueuedThenReadOnly => SMALL,
        F3Case::ConversationGrantReuse | F3Case::ReadOnlyAsk => SECOND_CANDIDATE,
        _ => CANDIDATE,
    };
    assert_eq!(writes.load(Ordering::SeqCst), case.expected_writes());
    assert_eq!(std::fs::read_to_string(run.root.join("target.txt")).unwrap(), expected_body);
    assert_eq!(version_count(&run), case.expected_writes() as i64);
    assert!(run.db.pending_kernel_host_commands(&run.id).unwrap().is_empty(), "Host commands acknowledged");
    if matches!(case, F3Case::QueuedThenReadOnly) {
        let acknowledged: i64 = run.db.with_connection(|connection| connection.query_row(
            "SELECT COUNT(*) FROM kernel_host_commands WHERE run_id=?1 AND kind='approval' AND status='completed'",
            [&run.id], |row| row.get(0),
        )).unwrap();
        assert_eq!(acknowledged, 1, "stale queued approval is consumed without granting");
    }
    if matches!(case, F3Case::ConversationGrantReuse) {
        let grants = run.db.kernel_effective_authorization_grants(&run.id, crate::database::now_ms()).unwrap();
        assert_eq!(grants.len(), 1, "one current scope grant covers both writes");
        let snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
        let second = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f3-write-2").unwrap();
        assert_ne!(second.approval_state.as_deref(), Some("pending"));
    }
    let final_snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
    let active_grants = run.db.kernel_effective_authorization_grants(
        &run.id, crate::database::now_ms()).unwrap().len();
    eprintln!("[F3] case={case:?} transport={} provider_requests={} state={} writes={} active_grants={}",
        if live { "live" } else { "per_round_host" }, requests.len(),
        final_snapshot.state, writes.load(Ordering::SeqCst), active_grants);
}

macro_rules! f3_matrix_tests {
    ($($name:ident: $case:ident, $live:expr;)*) => {
        $(#[test] fn $name() { f3_matrix_case(F3Case::$case, $live); })*
    };
}

f3_matrix_tests! {
    f3_live_ask_allow: AskAllow, true;
    f3_live_ask_read_only: AskReadOnly, true;
    f3_live_queued_then_read_only: QueuedThenReadOnly, true;
    f3_live_expired_card_replay: ExpiredCardReplay, true;
    f3_live_conversation_grant_reuse: ConversationGrantReuse, true;
    f3_live_read_only_ask: ReadOnlyAsk, true;
    f3_round_ask_allow: AskAllow, false;
    f3_round_ask_read_only: AskReadOnly, false;
    f3_round_queued_then_read_only: QueuedThenReadOnly, false;
    f3_round_expired_card_replay: ExpiredCardReplay, false;
    f3_round_conversation_grant_reuse: ConversationGrantReuse, false;
    f3_round_read_only_ask: ReadOnlyAsk, false;
}

#[test]
fn f3_live_cancel_wins_over_approval_in_same_host_batch() {
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start(version.clone());
    let run = run_fixture_with_mode("f3-live-cancel-priority", SMALL, model(provider.address), "ask");
    let writes = AtomicUsize::new(0);
    let queued = AtomicBool::new(false);
    let visits = AtomicUsize::new(0);
    let cancellation = CancellationRegistry::default();
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let worker = real_worker_command();
    let gateway = policy(&run);
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        execute_real(&run, &version, &writes, binding, effect, token)
    };
    let settle = |_: bool| {
        let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
        if visits.fetch_add(1, Ordering::SeqCst) > 30 {
            return Err("cancel was not observed at the live barrier".into());
        }
        if snapshot.tool_calls.iter().any(|call| call.tool_call_id == "f1-write"
            && call.state == "waiting_approval") && !queued.swap(true, Ordering::SeqCst) {
            let card = run.db.kernel_approval_identity(&run.id, "f1-write")?;
            assert!(run.db.queue_kernel_host_approval(&card, "allow_once")?);
            assert!(run.db.queue_kernel_host_command(&run.id, None)?);
        }
        Ok(())
    };
    let result = KernelCoordinator::start_prepared(&run.db, &clock, &run.id, &cancellation)
        .unwrap().with_preview(&preview).dispatch_initial_live(
            "f3-cancel-owner", &gateway, &worker, "local-test-only", &execute, &|_| Ok(()), &settle);
    assert_eq!(result.unwrap_err(), super::super::super::live::LIVE_CANCEL,
        "the live loop must hand cancellation to its owner");
    let requests = provider.finish();
    assert_eq!(requests.len(), 2, "real Node requested the read and write rounds");
    assert!(queued.load(Ordering::SeqCst));
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_to_string(run.root.join("target.txt")).unwrap(), SMALL);
    assert_eq!(version_count(&run), 0);
    let commands = run.db.pending_kernel_host_commands(&run.id).unwrap();
    assert_eq!(commands.len(), 2, "neither decision was consumed before the cancel owner runs");
    assert_eq!(commands[0].kind, "cancel");
    let snapshot = run.db.kernel_build_full_snapshot(&run.id).unwrap();
    let approval = snapshot.tool_calls.iter().find(|call| call.tool_call_id == "f1-write").unwrap();
    assert_eq!(approval.approval_state.as_deref(), Some("pending"));
}

#[test]
fn f3_outer_host_cancel_acks_queued_approval_without_consuming_it() {
    let version = Arc::new(Mutex::new(None));
    let provider = LocalProvider::start(version.clone());
    let run = run_fixture_with_mode("f3-outer-cancel-priority", SMALL, model(provider.address), "ask");
    let writes = AtomicUsize::new(0);
    let queued = AtomicBool::new(false);
    let visits = AtomicUsize::new(0);
    let cancellation = CancellationRegistry::default();
    let clock = crate::runtime_host::shadow_reconcile::ReconcilerClock;
    let worker = real_worker_command();
    let gateway = policy(&run);
    let preview = |_: &fox_engine_protocol::KernelModelPreview| {};
    let execute = |binding: &RunControlBinding,
                   effect: &kernel::OutboxEffect,
                   token: &kernel::CancellationToken| {
        execute_real(&run, &version, &writes, binding, effect, token)
    };
    let settle = |_: bool| {
        let snapshot = run.db.kernel_build_full_snapshot(&run.id)?;
        assert_ne!(snapshot.state, "cancelled", "child settlement cannot run again after parent terminal");
        if visits.fetch_add(1, Ordering::SeqCst) > 30 {
            return Err("outer Host did not reach the approval barrier".into());
        }
        if snapshot.tool_calls.iter().any(|call| call.tool_call_id == "f1-write"
            && call.state == "waiting_approval") && !queued.swap(true, Ordering::SeqCst) {
            let card = run.db.kernel_approval_identity(&run.id, "f1-write")?;
            assert!(run.db.queue_kernel_host_approval(&card, "allow_once")?);
            assert!(run.db.queue_kernel_host_command(&run.id, None)?);
        }
        Ok(())
    };
    let ownership = super::super::super::super::kernel_host::acquire(&run.root, &run.id).unwrap();
    let after_commit = |_: &str| Ok(());
    super::super::super::super::kernel_host::drive_with_actions_per_round(
        &ownership, &run.db, &clock, &cancellation, &run.id, &worker,
        "local-test-only", &gateway, &execute, &after_commit, &settle, &preview,
    ).unwrap();
    let requests = provider.finish();
    assert_eq!(requests.len(), 2, "real Node proposed read then write before cancellation");
    assert!(queued.load(Ordering::SeqCst));
    assert_eq!(run.db.kernel_build_full_snapshot(&run.id).unwrap().state, "cancelled");
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read_to_string(run.root.join("target.txt")).unwrap(), SMALL);
    assert_eq!(version_count(&run), 0);
    assert!(run.db.pending_kernel_host_commands(&run.id).unwrap().is_empty());
    let approval_status: (String, String) = run.db.with_connection(|connection| connection.query_row(
        "SELECT c.status,a.state FROM kernel_host_commands c JOIN kernel_approvals a
         ON a.run_id=c.run_id AND a.tool_call_id=c.tool_call_id
         WHERE c.run_id=?1 AND c.kind='approval'", [&run.id],
        |row| Ok((row.get(0)?,row.get(1)?)),
    )).unwrap();
    assert_eq!(approval_status.0, "completed", "terminal gate acknowledges the stale queued command");
    assert_ne!(approval_status.1, "allow_once", "queued decision was never applied");
    assert!(run.db.kernel_effective_authorization_grants(&run.id, crate::database::now_ms()).unwrap().is_empty());
}
