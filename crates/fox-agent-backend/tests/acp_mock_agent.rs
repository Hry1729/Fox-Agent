//! Host-side ACP client against a mock ACP v1 agent (Node). Skips when Node is
//! not installed; the CI image has it.
#![cfg(feature = "acp")]

use fox_agent_backend::acp::{AcpConnection, BackendEvent};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::reverse::WriteOutcome;
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
