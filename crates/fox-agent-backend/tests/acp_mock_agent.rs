//! Host-side ACP client against a mock ACP v1 agent (Node). Skips when Node is
//! not installed; the CI image has it.
#![cfg(feature = "acp")]

use fox_agent_backend::acp::{AcpConnection, BackendEvent, InitializeFailureCode};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::acp::WriteOutcome;
use std::time::Duration;

fn mock_spec() -> Option<LaunchSpec> {
    std::process::Command::new("node").arg("--version").output().ok()?;
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-agent.mjs");
    Some(LaunchSpec {
        backend: AgentBackend::Pi,
        program: "node".into(),
        args: vec![script.into()],
        version_pin: "mock",
        credential_env: &[],
        experimental: false,
        session_env: vec![],
    })
}

async fn next(connection: &mut AcpConnection) -> BackendEvent {
    tokio::time::timeout(Duration::from_secs(10), connection.events.recv())
        .await
        .expect("event within 10s")
        .expect("event channel open")
}

async fn until<F: Fn(&BackendEvent) -> bool>(connection: &mut AcpConnection, wanted: F) -> Vec<BackendEvent> {
    let mut seen = Vec::new();
    loop {
        let event = next(connection).await;
        let done = wanted(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn text_of(events: &[BackendEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            BackendEvent::Update { update, .. } => update.pointer("/content/text").and_then(|v| v.as_str()).map(str::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("|")
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_reads_capabilities_and_answer_is_written_not_acknowledged() {
    let Some(spec) = mock_spec() else { return };
    std::env::set_var("FOX_TEST_BACKEND_SECRET_TOKEN", "must-not-cross");
    let mut connection = AcpConnection::spawn(&spec, &std::env::temp_dir()).await.expect("spawn mock");
    std::env::remove_var("FOX_TEST_BACKEND_SECRET_TOKEN");
    match next(&mut connection).await {
        BackendEvent::Initialized { capabilities, .. } => {
            assert!(capabilities.session.load);
            assert!(capabilities.session.close);
            assert!(!capabilities.session.resume);
            assert!(!capabilities.approval_answer_acknowledged);
        }
        other => panic!("expected Initialized, got {other:?}"),
    }
    let session = connection.new_session(std::env::temp_dir()).await.expect("session");
    connection.prompt(&session, "hello").unwrap();
    let events = until(&mut connection, |event| matches!(event, BackendEvent::PermissionRequested { .. })).await;
    assert!(text_of(&events).contains("env:clean"), "secret-shaped variable reached the backend");
    let BackendEvent::PermissionRequested { native_request_id, option_ids, .. } = events.last().unwrap().clone() else { unreachable!() };
    assert_eq!(option_ids, ["allow", "reject"]);
    assert_eq!(connection.answer_permission(&native_request_id, Some("allow")).unwrap(), WriteOutcome::Written);
    assert!(connection.answer_permission(&native_request_id, Some("allow")).is_err(), "second answer must be refused");
    let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
    assert!(text_of(&events).contains("permission:allow"));
    connection.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_answers_pending_permissions_with_cancelled() {
    let Some(spec) = mock_spec() else { return };
    let mut connection = AcpConnection::spawn(&spec, &std::env::temp_dir()).await.expect("spawn mock");
    let _ = next(&mut connection).await;
    let session = connection.new_session(std::env::temp_dir()).await.expect("session");
    connection.prompt(&session, "hello").unwrap();
    until(&mut connection, |event| matches!(event, BackendEvent::PermissionRequested { .. })).await;
    assert_eq!(connection.pending_permissions(), 1);
    let settled = connection.cancel(&session);
    assert_eq!(settled.len(), 1);
    assert_eq!(connection.pending_permissions(), 0);
    let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
    assert!(text_of(&events).contains("permission:cancelled"));
    let stop = events.iter().find_map(|event| match event {
        BackendEvent::TurnEnded { stop_reason, .. } => Some(stop_reason.clone()),
        _ => None,
    });
    assert_eq!(stop, Some(serde_json::json!("cancelled")));
    connection.close().await;
}

/// Revocation stop on a live connection: the pending permission is answered
/// `cancelled`, in-flight work is `outcome_unknown`, the process is gone and
/// the session waits for a rebuild without full access, without replay.
#[tokio::test(flavor = "multi_thread")]
async fn revocation_stop_cancels_settles_and_kills_the_backend() {
    use fox_agent_backend::port::{RevokedSessionState, StopConfirmation};
    use fox_agent_backend::settlement::{PendingOutcome, PendingRegistry, RunTerminal, TerminalArbiter};
    let Some(spec) = mock_spec() else { return };
    let mut connection = AcpConnection::spawn(&spec, &std::env::temp_dir()).await.expect("spawn mock");
    let _ = next(&mut connection).await;
    let session = connection.new_session(std::env::temp_dir()).await.expect("session");
    connection.prompt(&session, "hello").unwrap();
    until(&mut connection, |event| matches!(event, BackendEvent::PermissionRequested { .. })).await;
    // Checked after the stop on Unix (`kill -0`).
    #[cfg_attr(not(unix), allow(unused_variables))]
    let pid = connection.process_id().expect("pid");
    let mut pending = PendingRegistry::default();
    pending.open("prompt-1", 0);
    pending.mark_in_flight("prompt-1");
    let mut arbiter = TerminalArbiter::default();
    let stop = connection.stop_for_revocation(&session, &mut pending, &mut arbiter, "full access withdrawn").await;
    assert_eq!(stop.cancelled_permissions.len(), 1);
    assert!(matches!(pending.outcome("prompt-1"), Some(PendingOutcome::OutcomeUnknown { .. })));
    assert_eq!(stop.terminal.terminal, RunTerminal::Cancelled);
    assert_eq!(stop.process_tree.confirmation, StopConfirmation::Confirmed, "{:?}", stop.process_tree);
    assert_eq!(stop.state, RevokedSessionState::NeedsRebuild);
    assert!(!stop.replay_last_operation);
    #[cfg(unix)]
    {
        let alive = std::process::Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false);
        assert!(!alive, "backend process {pid} still running after the revocation stop");
    }
}

/// A permission request that arrives after the stop began (here: sent by the
/// agent only after `session/cancel`) is answered `cancelled` by the Host on
/// arrival, never becomes answerable, and is reported only as an observation;
/// new work is refused while stopping.
#[tokio::test(flavor = "multi_thread")]
async fn late_permission_after_stopping_is_auto_cancelled_and_new_work_is_refused() {
    use fox_agent_backend::acp::STOPPING_REFUSAL;
    use fox_agent_backend::port::{RevokedSessionState, StopConfirmation};
    use fox_agent_backend::settlement::{PendingRegistry, TerminalArbiter};
    let Some(spec) = mock_spec() else { return };
    let mut connection = AcpConnection::spawn(&spec, &std::env::temp_dir()).await.expect("spawn mock");
    let _ = next(&mut connection).await;
    let session = connection.new_session(std::env::temp_dir()).await.expect("session");
    connection.prompt(&session, "late").unwrap();
    until(&mut connection, |event| matches!(event, BackendEvent::Update { .. })).await;
    assert_eq!(connection.pending_permissions(), 0, "nothing pending yet");

    assert!(connection.begin_stopping().is_empty(), "nothing was pending when the stop began");
    assert!(connection.is_stopping());
    connection.cancel(&session);
    let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
    assert!(
        !events.iter().any(|event| matches!(event, BackendEvent::PermissionRequested { .. })),
        "a late approval reached the Host as answerable: {events:?}"
    );
    let observed = events.iter().find_map(|event| match event {
        BackendEvent::PermissionAutoCancelled { request, reason, .. } => Some((request.clone(), reason.clone())),
        _ => None,
    });
    let (request, reason) = observed.expect("late request reported as auto-cancelled");
    assert_eq!(request.pointer("/toolCall/toolCallId").and_then(|v| v.as_str()), Some("t-late"));
    assert!(reason.contains("stopping"));
    assert!(text_of(&events).contains("late-permission:cancelled"), "agent got `cancelled`: {}", text_of(&events));
    assert_eq!(connection.pending_permissions(), 0, "no answerable approval on the Host side");

    assert_eq!(connection.prompt(&session, "hello").unwrap_err(), STOPPING_REFUSAL);
    assert_eq!(connection.new_session(std::env::temp_dir()).await.unwrap_err(), STOPPING_REFUSAL);

    let mut pending = PendingRegistry::default();
    let mut arbiter = TerminalArbiter::default();
    let stop = connection.stop_for_revocation(&session, &mut pending, &mut arbiter, "full access withdrawn").await;
    assert_eq!(stop.process_tree.confirmation, StopConfirmation::Confirmed, "{:?}", stop.process_tree);
    assert_eq!(stop.state, RevokedSessionState::NeedsRebuild);
}

// ---- initialization diagnostics (backend_exited_before_initialize) ----

fn node_spec(args: &[&str]) -> Option<LaunchSpec> {
    let mut spec = mock_spec()?;
    spec.args = args.iter().map(|arg| arg.to_string()).collect();
    Some(spec)
}

#[tokio::test(flavor = "multi_thread")]
async fn backend_exiting_0_before_initialize_is_not_initialized() {
    let Some(spec) = node_spec(&["-e", "process.exit(0)"]) else { return };
    let failure = AcpConnection::spawn_detailed(&spec, &std::env::temp_dir()).await.err().expect("exit 0 must not count as initialized");
    assert_eq!(failure.code, InitializeFailureCode::BackendExitedBeforeInitialize);
    assert_eq!(failure.code.as_str(), "backend_exited_before_initialize");
    assert_eq!(failure.exit_code, Some(0));
    assert_eq!(failure.stage.as_deref(), Some("initialize"));
}

#[tokio::test(flavor = "multi_thread")]
async fn backend_writing_stderr_then_exit_1_reports_code_and_redacted_tail() {
    let script = "console.error('starting'); console.error('boom: proxy http://user:s3cr3tpass@127.0.0.1:60080 OPENAI_API_KEY=sk-live'); process.exit(1)";
    let Some(spec) = node_spec(&["-e", script]) else { return };
    let failure = AcpConnection::spawn_detailed(&spec, &std::env::temp_dir()).await.err().expect("must fail");
    assert_eq!(failure.code, InitializeFailureCode::BackendExitedBeforeInitialize);
    assert_eq!(failure.exit_code, Some(1));
    assert!(failure.backend_stderr_tail.contains("boom"), "{failure:?}");
    assert!(failure.backend_stderr_tail.contains("starting"), "{failure:?}");
    let text = failure.to_string();
    assert!(text.starts_with("backend_exited_before_initialize:"), "{text}");
    assert!(!text.contains("s3cr3tpass") && !text.contains("sk-live"), "{text}");
    // The string API carries the same code.
    let error = AcpConnection::spawn(&spec, &std::env::temp_dir()).await.err().unwrap();
    assert!(error.starts_with("backend_exited_before_initialize:"), "{error}");
}

