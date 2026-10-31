//! Host external backend runtime end to end against a mock ACP v1 agent
//! (Node, `tests/fixtures/mock-acp-session-agent.mjs`): start -> streamed
//! updates -> permission -> allow / reject -> completion; cancel (confirmed
//! and unconfirmed); backend crash and relaunch with `session/load`; session
//! reuse; launch failure; stop reasons; DeepSeek auto-reject; close.
//! Skips (prints SKIPPED) when Node is not installed.
#![cfg(feature = "acp")]

use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{
    AnswerOutcome, CancelOutcome, ExternalLauncher, ExternalRuntime, ExternalSink, HostPermissionAnswer, LaunchFailure, LaunchRequest, Launched, NativeModeOutcome,
    RevokeOutcome, StartError, TurnRequest,
};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::{AgentBackend, StopConfirmation};
use fox_agent_backend::projection::{AutoPermission, PermissionCard, PermissionOption};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Recorded {
    events: Vec<(String, Value)>,
    approvals: Vec<(String, String, PermissionCard)>,
    audits: Vec<(String, String, Value)>,
    /// Plan C2: what the Host approval engine answers pi (None = no engine).
    host_policy: Option<fn(&str) -> AutoPermission>,
    /// (backend, native mode) of every request put to the engine.
    policy_calls: Vec<(String, String)>,
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Recorded>>);

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0.lock().unwrap().events.push((run_id.to_owned(), event));
        Ok(())
    }

    fn request_approval(&self, run_id: &str, _conversation_id: &str, key: &str, card: &PermissionCard) -> Result<String, String> {
        let mut recorded = self.0.lock().unwrap();
        let id = format!("appr-{}", recorded.approvals.len() + 1);
        assert!(key.starts_with("acp-permission:"), "{key}");
        recorded.approvals.push((run_id.to_owned(), id.clone(), card.clone()));
        Ok(id)
    }

    fn audit(&self, conversation_id: &str, _backend: AgentBackend, event: &str, details: &Value) {
        self.0.lock().unwrap().audits.push((conversation_id.to_owned(), event.to_owned(), details.clone()));
    }

    fn host_permission_answer(&self, _run_id: &str, _conversation_id: &str, backend: AgentBackend, native_mode: &str, _request: &Value, options: &[PermissionOption]) -> Option<HostPermissionAnswer> {
        let mut recorded = self.0.lock().unwrap();
        let policy = recorded.host_policy?;
        recorded.policy_calls.push((backend.as_str().to_owned(), native_mode.to_owned()));
        let decision = match policy(native_mode) {
            AutoPermission::Allow { .. } => AutoPermission::Allow { option_id: options.iter().find(|option| option.kind == "allow_once").unwrap().option_id.clone() },
            AutoPermission::Reject { .. } => AutoPermission::Reject { option_id: options.iter().find(|option| option.kind == "reject_once").map(|option| option.option_id.clone()) },
            AutoPermission::Ask => AutoPermission::Ask,
        };
        Some(HostPermissionAnswer { decision, policy: serde_json::json!({ "mode": native_mode, "class": "command" }) })
    }
}

impl Sink {
    fn events(&self, run_id: &str) -> Vec<Value> {
        self.0.lock().unwrap().events.iter().filter(|(run, _)| run == run_id).map(|(_, event)| event.clone()).collect()
    }

    fn types(&self, run_id: &str) -> Vec<String> {
        self.events(run_id).iter().map(|event| event["type"].as_str().unwrap().to_owned()).collect()
    }

    fn text(&self, run_id: &str) -> String {
        self.events(run_id).iter().filter(|event| event["type"] == "message.delta").map(|event| event["delta"].as_str().unwrap().to_owned()).collect()
    }

    fn approvals(&self) -> Vec<(String, String, PermissionCard)> {
        self.0.lock().unwrap().approvals.clone()
    }

    async fn wait_for(&self, run_id: &str, wanted: &str) -> Value {
        for _ in 0..200 {
            if let Some(event) = self.events(run_id).into_iter().find(|event| event["type"] == wanted) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no {wanted} for {run_id}; got {:?}", self.types(run_id));
    }

    async fn terminal(&self, run_id: &str) -> Value {
        for _ in 0..200 {
            if let Some(event) = self.events(run_id).into_iter().find(|event| event["type"].as_str().is_some_and(|kind| {
                matches!(kind, "run.completed" | "run.cancelled" | "run.failed" | "run.interrupted")
            })) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no terminal for {run_id}; got {:?}", self.types(run_id));
    }

    async fn wait_approval(&self, count: usize) -> (String, String, PermissionCard) {
        for _ in 0..200 {
            let approvals = self.approvals();
            if approvals.len() >= count {
                return approvals[count - 1].clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no approval #{count}");
    }
}

#[derive(Clone, Default)]
struct MockLauncher {
    launches: Arc<AtomicUsize>,
    fail: bool,
    /// Full access withdrawn: desired / launched mode is `Standard`.
    revoked: Arc<AtomicBool>,
    modes: Arc<Mutex<Vec<PermissionMode>>>,
    config_revision: Arc<AtomicUsize>,
    selected_model: Arc<Mutex<Option<String>>>,
    key_reads: Arc<AtomicUsize>,
    fail_key_after_launch: bool,
    closed: Arc<AtomicUsize>,
    /// L-1 / L-2: how long `launch` takes before the process starts.
    launch_delay_ms: Arc<std::sync::atomic::AtomicU64>,
}

impl ExternalLauncher for MockLauncher {
    fn connection_key(&self, _request: &LaunchRequest) -> Result<Option<String>, LaunchFailure> {
        if self.key_reads.fetch_add(1, Ordering::SeqCst) > 0 && self.fail_key_after_launch {
            return Err(LaunchFailure::new("agent_backend_model_endpoint_invalid", "configuration became unreadable"));
        }
        Ok(Some(self.config_revision.load(Ordering::SeqCst).to_string()))
    }
    fn selected_model(&self, _request: &LaunchRequest) -> Option<String> { self.selected_model.lock().unwrap().clone() }
    fn desired_mode(&self, _request: &LaunchRequest) -> PermissionMode {
        if self.revoked.load(Ordering::SeqCst) { PermissionMode::Standard } else { PermissionMode::FullAccess }
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let launches = Arc::clone(&self.launches);
        let fail = self.fail;
        let mode = self.desired_mode(&request);
        let closed = Arc::clone(&self.closed);
        let delay = self.launch_delay_ms.load(Ordering::SeqCst);
        self.modes.lock().unwrap().push(mode);
        Box::pin(async move {
            launches.fetch_add(1, Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if fail {
                return Err(LaunchFailure::new("agent_backend_not_installed", "backend runtime is not installed").remedy("reinstall"));
            }
            let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-session-agent.mjs");
            let spec = LaunchSpec {
                backend: request.backend,
                program: "node".into(),
                args: vec![script.into()],
                version_pin: "mock",
                credential_env: &[],
                experimental: false,
                session_env: vec![],
            };
            let connection = AcpConnection::spawn_detailed(&spec, &request.project_root).await.map_err(LaunchFailure::from)?;
            Ok(Launched {
                connection,
                mode,
                declaration: LaunchDeclaration::declare(mode),
                pi_launch: PiLaunch::Stock,
                on_closed: Some(Box::new(move || { closed.fetch_add(1, Ordering::SeqCst); })),
                fox_tools: None,
            })
        })
    }
}

fn node_available() -> bool {
    if std::process::Command::new("node").arg("--version").output().is_ok() {
        return true;
    }
    eprintln!("SKIPPED: node not installed");
    false
}

fn new_runtime(launcher: MockLauncher) -> (ExternalRuntime<Sink, MockLauncher>, Sink) {
    let sink = Sink::default();
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), launcher, tokio::runtime::Handle::current(), Duration::from_millis(800));
    (runtime, sink)
}

fn turn(conversation: &str, run: &str, prompt: &str, backend: AgentBackend, stored: Option<&str>) -> TurnRequest {
    TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: stored.map(str::to_owned),
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend,
            project_uuid: Some("8f14e45f-ceea-467a-9a36-dedd4bea2543".into()),
            project_root: std::env::temp_dir(),
            session_key: format!("k-{run}"),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn response_timeout_fails_once_stops_process_and_allows_retry() {
    assert!(node_available(), "this regression requires Node");
    let sink = Sink::default();
    let launcher = MockLauncher::default();
    let runtime = ExternalRuntime::with_idle_timeout(sink.clone(), launcher.clone(), tokio::runtime::Handle::current(), Duration::from_millis(200), Duration::from_secs(2), Duration::from_millis(250));
    runtime.start_turn(turn("idle", "hung", "hang", AgentBackend::Pi, None)).unwrap();
    let end = sink.terminal("hung").await;
    assert_eq!(end["type"], "run.failed");
    assert_eq!(end["code"], "agent_backend_response_timeout");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(sink.types("hung").iter().filter(|kind| kind.starts_with("run.") && kind.as_str() != "run.started").count(), 1);
    runtime.start_turn(turn("idle", "retry", "simple", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("retry").await["type"], "run.completed");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn response_timeout_pauses_while_waiting_for_user_approval() {
    assert!(node_available(), "this regression requires Node");
    let sink = Sink::default();
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let runtime = ExternalRuntime::with_idle_timeout(sink.clone(), launcher, tokio::runtime::Handle::current(), Duration::from_millis(200), Duration::from_secs(2), Duration::from_millis(250));
    runtime.start_turn(turn("approval-idle", "approval", "hello", AgentBackend::Pi, None)).unwrap();
    let (_, id, _) = sink.wait_approval(1).await;
    tokio::time::sleep(Duration::from_millis(650)).await;
    assert!(!sink.types("approval").contains(&"run.failed".into()));
    runtime.answer(&id, true);
    assert_eq!(sink.terminal("approval").await["type"], "run.completed");
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn model_settlement_requires_delivery_and_preserves_partial_failure() {
    assert!(node_available(), "this regression requires Node");
    let (runtime, sink) = new_runtime(MockLauncher::default());
    for (prompt, kind, code) in [
        ("empty", "run.failed", "agent_backend_empty_output"),
        ("status-only", "run.failed", "agent_backend_empty_output"),
        ("reasoning-only", "run.failed", "agent_backend_empty_output"),
        ("tools-only", "run.completed", ""),
        ("partial-model-failure", "run.failed", "agent_backend_model_network_allowlist"),
    ] {
        runtime.start_turn(turn("settlement", prompt, prompt, AgentBackend::Pi, None)).unwrap();
        let terminal = sink.terminal(prompt).await;
        assert_eq!(terminal["type"], kind, "{prompt}: {terminal}");
        if !code.is_empty() { assert_eq!(terminal["code"], code); }
        assert_eq!(sink.events(prompt).iter().filter(|event| matches!(event["type"].as_str(), Some("run.completed" | "run.failed"))).count(), 1);
    }
    assert!(sink.text("status-only").is_empty());
    assert_eq!(sink.text("partial-model-failure"), "partial answer");
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn model_source_changes_reopen_history_and_apply_current_selection() {
    assert!(node_available(), "this regression requires Node");
    let launcher = MockLauncher::default();
    *launcher.selected_model.lock().unwrap() = Some("magpie/first".into());
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("models", "first", "model-state", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("first").await["type"], "run.completed");
    assert_eq!(sink.text("first"), "magpie/first");
    let original_session = runtime.session_info("models").unwrap().native_session_id;
    *launcher.selected_model.lock().unwrap() = Some("remote/next".into());
    launcher.config_revision.fetch_add(1, Ordering::SeqCst);
    runtime.start_turn(turn("models", "second", "model-state", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("second").await["type"], "run.completed");
    assert_eq!(sink.text("second"), "remote/next");
    assert_eq!(runtime.session_info("models").unwrap().native_session_id, original_session, "history session survives a model change");
    launcher.config_revision.fetch_add(1, Ordering::SeqCst); // same target, rotated credentials
    runtime.start_turn(turn("models", "rotated", "model-state", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("rotated").await["type"], "run.completed");
    assert_eq!(sink.text("rotated"), "remote/next");
    assert_eq!(runtime.session_info("models").unwrap().native_session_id, original_session);
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 3);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unreadable_source_after_spawn_closes_the_unpublished_connection() {
    assert!(node_available(), "this regression requires Node");
    let launcher = MockLauncher { fail_key_after_launch: true, ..Default::default() };
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("source-failed", "source-failed", "simple", AgentBackend::Pi, None)).unwrap();
    let terminal = sink.terminal("source-failed").await;
    assert_eq!(terminal["type"], "run.failed");
    assert_eq!(terminal["code"], "agent_backend_model_endpoint_invalid");
    assert_eq!(launcher.closed.load(Ordering::SeqCst), 1, "the Windows/private cleanup hook must run");
    assert!(!runtime.session_info("source-failed").unwrap().live);
    assert!(!sink.types("source-failed").contains(&"agent_backend.session".to_owned()));
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn model_selection_timeout_closes_the_unpublished_connection() {
    assert!(node_available(), "this regression requires Node");
    let launcher = MockLauncher::default();
    *launcher.selected_model.lock().unwrap() = Some("fixture/hang-model".into());
    let sink = Sink::default();
    let runtime = ExternalRuntime::with_timeouts(sink.clone(), launcher.clone(), tokio::runtime::Handle::current(), Duration::from_millis(800), Duration::from_millis(200));
    runtime.start_turn(turn("selection-timeout", "selection-timeout", "simple", AgentBackend::Pi, None)).unwrap();
    let terminal = sink.terminal("selection-timeout").await;
    assert_eq!(terminal["type"], "run.failed");
    assert_eq!(terminal["code"], "agent_backend_model_selection_failed");
    assert!(terminal["message"].as_str().unwrap().contains("超时"));
    assert_eq!(launcher.closed.load(Ordering::SeqCst), 1);
    assert!(!runtime.session_info("selection-timeout").unwrap().live);
    assert!(!sink.types("selection-timeout").contains(&"agent_backend.session".to_owned()));
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn start_streams_asks_permission_and_completes_on_allow() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c1", "r1", "hello", AgentBackend::Codex, None)).unwrap();
    let (run, approval, card) = sink.wait_approval(1).await;
    assert_eq!(run, "r1");
    assert_eq!(card.title, "npm test");
    assert_eq!(card.available_decisions, ["allow_once", "deny"], "allow_always is never offered");
    assert!(runtime.approval_pending(&approval));
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::Written);
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending, "one answer per request");
    let terminal = sink.terminal("r1").await;
    assert_eq!(terminal["type"], "run.completed");
    assert_eq!(terminal["stopReason"], "end_turn");
    assert_eq!(terminal["agentBackend"], "codex");
    let types = sink.types("r1");
    for expected in [
        "run.started",
        "agent_backend.launching",
        "agent_backend.session",
        "reasoning.delta",
        "message.started",
        "tool.started",
        "plan.updated",
        "tool.completed",
        "message.completed",
        "run.completed",
    ] {
        assert!(types.iter().any(|kind| kind == expected), "missing {expected}: {types:?}");
    }
    assert_eq!(types.first().unwrap(), "run.started");
    assert_eq!(types.last().unwrap(), "run.completed");
    assert_eq!(types.iter().filter(|kind| *kind == "message.started").count(), 1);
    assert_eq!(sink.text("r1"), "Running tests permission:allow", "allow_once was written, not allow_always");
    let completed = sink.events("r1").into_iter().find(|event| event["type"] == "tool.completed").unwrap();
    assert_eq!(completed["isError"], false);
    assert_eq!(completed["result"]["text"], "tests passed");
    let info = runtime.session_info("c1").unwrap();
    assert!(info.live && info.active_run.is_none());
    assert_eq!(info.mode, Some(PermissionMode::FullAccess));
    let report = runtime.close_conversation("c1", "test done").await.expect("live process");
    assert_eq!(report.confirmation, StopConfirmation::Confirmed);
}

#[tokio::test(flavor = "multi_thread")]
async fn reject_writes_the_native_reject_option() {
    if !node_available() {
        return;
    }
    // pi without full access in 询问 (its default): the card. (Full access would be 自动.)
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher);
    runtime.start_turn(turn("c2", "r2", "hello", AgentBackend::Pi, None)).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    assert_eq!(runtime.answer(&approval, false), AnswerOutcome::Written);
    let terminal = sink.terminal("r2").await;
    assert_eq!(terminal["type"], "run.completed");
    assert!(sink.text("r2").ends_with("permission:reject"));
    let completed = sink.events("r2").into_iter().find(|event| event["type"] == "tool.completed").unwrap();
    assert_eq!(completed["isError"], true);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_answers_pending_permissions_and_ends_cancelled() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c3", "r3", "hello", AgentBackend::Codex, None)).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    match runtime.cancel("r3") {
        CancelOutcome::Requested { answered_permissions } => assert_eq!(answered_permissions.len(), 1),
        other => panic!("{other:?}"),
    }
    let terminal = sink.terminal("r3").await;
    assert_eq!(terminal["type"], "run.cancelled");
    assert!(terminal.get("outcome").is_none(), "confirmed by the backend: {terminal}");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending, "a late approval is never written");
    assert_eq!(runtime.cancel("r3"), CancelOutcome::NotActive);
    // The connection survives a confirmed cancel and takes the next turn.
    runtime.start_turn(turn("c3", "r3b", "simple", AgentBackend::Codex, None)).unwrap();
    assert_eq!(sink.terminal("r3b").await["type"], "run.completed");
    runtime.shutdown().await;
}

/// Plan C5 (Linux): cancel at an open approval card ends the turn as
/// cancelled for each of the three Agents (pi 询问, Codex workspace-write, dsh
/// workspace-write ask), the late answer is never written, and the same
/// session takes the next turn.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_at_an_open_card_ends_the_turn_for_pi_codex_and_dsh() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher.clone());
    for (index, (backend, mode)) in [(AgentBackend::Pi, "ask"), (AgentBackend::Codex, "workspace-write"), (AgentBackend::Deepseek, "workspace-write")].into_iter().enumerate() {
        let conversation = format!("c-c5-cancel-{}", backend.as_str());
        let run = format!("r-c5-cancel-{}", backend.as_str());
        let mut request = turn(&conversation, &run, "hello", backend, None);
        request.launch.native_permission = Some(mode.into());
        runtime.start_turn(request).unwrap();
        let (card_run, approval, _) = sink.wait_approval(index + 1).await;
        assert_eq!(card_run, run, "{backend:?}: the card belongs to this turn");
        match runtime.cancel(&run) {
            CancelOutcome::Requested { answered_permissions } => assert_eq!(answered_permissions.len(), 1, "{backend:?}"),
            other => panic!("{backend:?}: {other:?}"),
        }
        let terminal = sink.terminal(&run).await;
        assert_eq!(terminal["type"], "run.cancelled", "{backend:?}: {terminal}");
        assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending, "{backend:?}: a late approval is never written");
        let mut next = turn(&conversation, &format!("{run}-next"), "simple", backend, None);
        next.launch.native_permission = Some(mode.into());
        runtime.start_turn(next).unwrap();
        assert_eq!(sink.terminal(&format!("{run}-next")).await["type"], "run.completed", "{backend:?}: the session survives the cancel");
    }
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unconfirmed_cancel_ends_with_outcome_unknown_and_stops_the_process() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("c4", "r4", "hang", AgentBackend::Pi, None)).unwrap();
    sink.wait_for("r4", "message.delta").await;
    assert!(matches!(runtime.cancel("r4"), CancelOutcome::Requested { .. }));
    let terminal = sink.terminal("r4").await;
    assert_eq!(terminal["type"], "run.cancelled");
    assert_eq!(terminal["outcome"], "unknown");
    assert_eq!(sink.types("r4").iter().filter(|kind| kind.starts_with("run.") && *kind != "run.started").count(), 1, "one terminal");
    for _ in 0..100 {
        if !runtime.session_info("c4").unwrap().live {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!runtime.session_info("c4").unwrap().live, "process stopped after an unconfirmed cancel");
    // Next turn relaunches and reopens the same native session.
    let first_native = runtime.session_info("c4").unwrap().native_session_id.unwrap();
    runtime.start_turn(turn("c4", "r4b", "simple", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("r4b").await["type"], "run.completed");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    let session = sink.events("r4b").into_iter().find(|event| event["type"] == "agent_backend.session").unwrap();
    assert_eq!(session["reopened"], "load");
    assert_eq!(session["nativeSessionId"], first_native.as_str());
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_fails_the_run_and_the_next_turn_reloads_without_replaying_history() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("c5", "r5", "crash", AgentBackend::Pi, None)).unwrap();
    let terminal = sink.terminal("r5").await;
    assert_eq!(terminal["type"], "run.failed");
    assert_eq!(terminal["code"], "agent_backend_exited");
    assert_eq!(terminal["outcome"], "unknown");
    assert_eq!(sink.types("r5").iter().filter(|kind| *kind == "message.completed").count(), 1, "partial text is closed, not left streaming");
    runtime.start_turn(turn("c5", "r5b", "simple", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("r5b").await["type"], "run.completed");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 2);
    let session = sink.events("r5b").into_iter().find(|event| event["type"] == "agent_backend.session").unwrap();
    assert_eq!(session["reopened"], "load");
    assert!(!sink.text("r5b").contains("replayed-old"), "history replay is never projected: {}", sink.text("r5b"));
    assert!(!sink.text("r5b").contains("old question"));
    runtime.shutdown().await;
}

/// Reopening a conversation: the backend replays its whole history between
/// `session/load` and its reply (and here also emits more old output right
/// after the reply, before any prompt). None of it may appear in the new
/// Run: only what the backend sends for the new prompt is projected.
#[tokio::test(flavor = "multi_thread")]
async fn reopening_with_session_load_projects_only_events_after_the_load() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("c-replay", "r-replay", "after-load", AgentBackend::Pi, Some("sess-longreplay-1"))).unwrap();
    let terminal = sink.terminal("r-replay").await;
    assert_eq!(terminal["type"], "run.completed", "{:?}", sink.types("r-replay"));
    let session = sink.events("r-replay").into_iter().find(|event| event["type"] == "agent_backend.session").unwrap();
    assert_eq!(session["reopened"], "load");
    assert_eq!(session["nativeSessionId"], "sess-longreplay-1");
    assert_eq!(sink.text("r-replay"), "fresh-after-load", "only the new turn's text");
    let rendered = serde_json::to_string(&sink.events("r-replay")).unwrap();
    for old in ["replayed-old", "old question", "old thought", "old-tool-", "late old tool", "replayed-after-reply"] {
        assert!(!rendered.contains(old), "history leaked into the new Run ({old}): {rendered}");
    }
    assert!(rendered.contains("new-tool"), "the new turn's tool call is projected: {rendered}");
    // A second turn on the same connection sees only its own output too.
    runtime.start_turn(turn("c-replay", "r-replay-2", "simple", AgentBackend::Pi, Some("sess-longreplay-1"))).unwrap();
    assert_eq!(sink.terminal("r-replay-2").await["type"], "run.completed");
    assert!(sink.text("r-replay-2").starts_with("pid:"), "{}", sink.text("r-replay-2"));
    assert!(!sink.text("r-replay-2").contains("fresh-after-load"));
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn turns_reuse_one_process_and_session_and_busy_is_refused() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("c6", "r6", "slow", AgentBackend::Codex, None)).unwrap();
    assert_eq!(runtime.start_turn(turn("c6", "r6x", "simple", AgentBackend::Codex, None)), Err(StartError::TurnBusy));
    assert_eq!(StartError::TurnBusy.code(), "agent_backend_turn_busy");
    sink.wait_for("r6", "message.delta").await;
    runtime.cancel("r6");
    assert_eq!(sink.terminal("r6").await["type"], "run.cancelled");
    runtime.start_turn(turn("c6", "r7", "simple", AgentBackend::Codex, None)).unwrap();
    sink.terminal("r7").await;
    runtime.start_turn(turn("c6", "r8", "simple", AgentBackend::Codex, None)).unwrap();
    sink.terminal("r8").await;
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "one process per conversation");
    assert_eq!(sink.text("r7"), sink.text("r8"), "same pid and native session");
    assert!(sink.events("r7").iter().all(|event| event["type"] != "agent_backend.session"), "no reopen on a live session");
    // A stored id from an earlier Fox process is loaded on first launch.
    runtime.start_turn(turn("c6b", "r9", "simple", AgentBackend::Codex, Some("sess-old-1"))).unwrap();
    sink.terminal("r9").await;
    assert!(sink.text("r9").ends_with("session:sess-old-1"), "{}", sink.text("r9"));
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn launch_failures_and_stop_reasons_settle_the_run() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher { fail: true, ..Default::default() });
    runtime.start_turn(turn("c7", "r10", "simple", AgentBackend::Pi, None)).unwrap();
    let terminal = sink.terminal("r10").await;
    assert_eq!(terminal["type"], "run.failed");
    assert_eq!(terminal["code"], "agent_backend_not_installed");
    assert!(terminal["message"].as_str().unwrap().contains("reinstall"));
    assert!(runtime.session_info("c7").unwrap().active_run.is_none(), "the conversation is free again");

    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c8", "r11", "maxtokens", AgentBackend::Pi, None)).unwrap();
    let terminal = sink.terminal("r11").await;
    assert_eq!(terminal["type"], "run.interrupted");
    assert_eq!(terminal["code"], "agent_backend_stop_max_tokens");
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_permission_requests_are_rejected_by_the_host_never_shown() {
    if !node_available() {
        return;
    }
    // Without full access: the default preset (approval never). In full access dsh's
    // danger-full-access would show the card instead.
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher);
    runtime.start_turn(turn("c9", "r12", "hello", AgentBackend::Deepseek, None)).unwrap();
    let terminal = sink.terminal("r12").await;
    assert!(sink.approvals().is_empty(), "no approval card for DeepSeek in its default preset (approval never)");
    assert!(sink.types("r12").iter().any(|kind| kind == "agent_backend.permission_auto_answered"));
    // Fail-closed: the native one-shot reject option was written.
    assert!(sink.text("r12").ends_with("permission:reject"));
    assert_eq!(terminal["type"], "run.completed");
    runtime.shutdown().await;
}

/// 13.19 D.5 (unit-level replacement of the Windows scenario): in a
/// full-access session (Codex `agent-full-access`, dsh `danger-full-access`)
/// a permission request that still arrives is shown as the approval card and
/// stays pending until the user answers; the Host never auto-allows (nor
/// auto-answers) it on Codex's or dsh's behalf. Contrast: pi's 完全访问 is
/// answered by Fox's own gate (automatic allow_once).
#[tokio::test(flavor = "multi_thread")]
async fn full_access_codex_and_dsh_requests_still_show_the_card_never_auto_allowed() {
    if !node_available() {
        return;
    }
    // MockLauncher default: the Host wants (and launches) full access.
    let (runtime, sink) = new_runtime(MockLauncher::default());
    for (index, (backend, full_mode, deny)) in [(AgentBackend::Codex, "agent-full-access", false), (AgentBackend::Deepseek, "danger-full-access", true)].into_iter().enumerate() {
        let conversation = format!("c-d5-{}", backend.as_str());
        let run = format!("r-d5-{}", backend.as_str());
        // The Agent's own full mode is the stored choice; Fox's mode only
        // says that it adds no outer sandbox of its own.
        let mut request = turn(&conversation, &run, "hello", backend, None);
        request.launch.native_permission = Some(full_mode.into());
        runtime.start_turn(request).unwrap();
        let session = sink.wait_for(&run, "agent_backend.session").await;
        assert_eq!(session["mode"], "full_access", "{session}");
        assert_eq!(session["permissionMode"], full_mode, "{session}");
        let (card_run, approval, card) = sink.wait_approval(index + 1).await;
        assert_eq!(card_run, run);
        assert_eq!(card.title, "npm test");
        assert_eq!(card.available_decisions, ["allow_once", "deny"]);
        // Nothing answers it behind the user's back.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(runtime.approval_pending(&approval), "{backend:?}: the card is still waiting for the user");
        let types = sink.types(&run);
        assert!(!types.iter().any(|kind| kind == "agent_backend.permission_auto_answered"), "{backend:?}: auto-answered in full access: {types:?}");
        assert!(!types.iter().any(|kind| matches!(kind.as_str(), "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted")), "{backend:?}: the turn did not move on without an answer: {types:?}");
        assert!(!sink.text(&run).contains("permission:"), "{backend:?}: no native answer was written");
        // The user's answer is what reaches the Agent.
        assert_eq!(runtime.answer(&approval, !deny), AnswerOutcome::Written);
        assert_eq!(sink.terminal(&run).await["type"], "run.completed");
        assert!(sink.text(&run).ends_with(if deny { "permission:reject" } else { "permission:allow" }), "{backend:?}: {}", sink.text(&run));
    }
    assert!(sink.0.lock().unwrap().audits.iter().all(|(_, event, _)| event != "agent_backend.permission_auto_answered"));

    // pi 完全访问: the stored native full mode makes Fox's gate answer (自动), no card.
    let mut pi = turn("c-d5-pi", "r-d5-pi", "hello", AgentBackend::Pi, None);
    pi.launch.native_permission = Some("full_access".into());
    runtime.start_turn(pi).unwrap();
    assert_eq!(sink.terminal("r-d5-pi").await["type"], "run.completed");
    assert_eq!(sink.approvals().len(), 2, "no card for pi in full access");
    let answered = sink.events("r-d5-pi").into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("pi gate answered");
    assert_eq!(answered["permissionMode"], "full_access");
    assert_eq!(answered["decision"], "allow_once");
    runtime.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_a_conversation_interrupts_its_turn_and_confirms_the_stop() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c10", "r13", "slow", AgentBackend::Pi, None)).unwrap();
    sink.wait_for("r13", "message.delta").await;
    let report = runtime.close_conversation("c10", "会话已删除").await.expect("process");
    assert_eq!(report.confirmation, StopConfirmation::Confirmed);
    let terminal = sink.terminal("r13").await;
    assert_eq!(terminal["type"], "run.interrupted");
    assert_eq!(terminal["code"], "agent_backend_session_closed");
    assert!(runtime.session_info("c10").is_none());
    assert!(runtime.close_conversation("c10", "again").await.is_none());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(sink.types("r13").iter().filter(|kind| kind.starts_with("run.") && *kind != "run.started").count(), 1, "no second terminal after close");
}

/// Approvals have no deadline (Claude Code / Codex CLI / ACP semantics): a
/// permission request left unanswered keeps waiting, and the turn keeps
/// running, until the user acts. Cancelling the turn then answers the native
/// request with `outcome: cancelled`.
#[tokio::test(flavor = "multi_thread")]
async fn an_unanswered_permission_waits_without_a_deadline_until_the_turn_is_cancelled() {
    if !node_available() {
        return;
    }
    // pi without full access in 询问 (its default): the card. (Full access would be 自动.)
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher);
    runtime.start_turn(turn("c20", "r20", "hello", AgentBackend::Pi, None)).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(runtime.approval_pending(&approval), "still waiting for the user");
    assert_eq!(runtime.active_runs(), vec!["r20".to_owned()]);
    let terminals = ["run.completed", "run.cancelled", "run.failed", "run.interrupted"];
    assert!(!sink.types("r20").iter().any(|kind| terminals.contains(&kind.as_str())), "no terminal while waiting");
    match runtime.cancel("r20") {
        CancelOutcome::Requested { answered_permissions } => assert_eq!(answered_permissions.len(), 1, "answered `cancelled`"),
        other => panic!("{other:?}"),
    }
    assert!(!runtime.approval_pending(&approval));
    assert_eq!(sink.terminal("r20").await["type"], "run.cancelled");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending);
    runtime.shutdown().await;
}

/// Closing the conversation ends a pending permission with the turn.
#[tokio::test(flavor = "multi_thread")]
async fn closing_a_conversation_ends_its_pending_permission() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c21", "r21", "hello", AgentBackend::Codex, None)).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    let report = runtime.close_conversation("c21", "会话已归档").await.expect("process");
    assert_eq!(report.confirmation, StopConfirmation::Confirmed);
    let terminal = sink.terminal("r21").await;
    assert_eq!(terminal["type"], "run.interrupted");
    assert_eq!(terminal["code"], "agent_backend_session_closed");
    assert!(!runtime.approval_pending(&approval));
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending);
}

/// Revoking full access mid-session (UX batch ②): `recheck_permission_mode`
/// -> RestartWithoutFullAccess. The running turn ends `interrupted`
/// (`agent_backend_full_access_revoked`), its pending approval is settled (a
/// late answer is never written), the process is stopped, and the next turn
/// launches without full access, without replaying the cancelled one.
/// dsh's full access lives in its process: leaving it stops dsh
/// (RestartWithoutFullAccess) and the next turn relaunches in the new preset.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_dsh_full_access_restarts_without_it_and_without_replay() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let revoked = Arc::clone(&launcher.revoked);
    let modes = Arc::clone(&launcher.modes);
    let launches = Arc::clone(&launcher.launches);
    let (runtime, sink) = new_runtime(launcher);
    let dsh = |run: &str, mode: &str| {
        let mut request = turn("c-rev", run, "hello", AgentBackend::Deepseek, None);
        request.launch.native_permission = Some(mode.into());
        request
    };
    assert_eq!(runtime.revoke_full_access(&dsh("r-x", "workspace-write").launch).await, RevokeOutcome::NotRunning);
    runtime.start_turn(dsh("r-rev-1", "danger-full-access")).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    // The Host stores the restricted mode first, then asks the runtime to restart.
    revoked.store(true, Ordering::SeqCst);
    assert_eq!(runtime.revoke_full_access(&dsh("r-x", "workspace-write").launch).await, RevokeOutcome::RestartWithoutFullAccess { confirmed: true });
    // 13.19 fix ②: the stopped full-access process no longer reports its
    // declaration / handshake (the capability chip falls back to the planned
    // view, without full access); leaving full access still reads "next turn".
    let info = runtime.session_info("c-rev").expect("session kept");
    assert!(!info.live);
    assert!(info.declaration.is_none(), "stale full-access declaration: {:?}", info.declaration);
    assert!(info.initialize_response.is_none() && info.capabilities.is_none());
    assert_eq!(runtime.apply_native_mode("c-rev", "workspace-write").await, NativeModeOutcome::NotRunning, "stopped: the next launch reads the new preset");
    let terminal = sink.terminal("r-rev-1").await;
    assert_eq!(terminal["type"], "run.interrupted", "{terminal}");
    assert_eq!(terminal["code"], "agent_backend_full_access_revoked", "{terminal}");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::NotPending, "a pending approval is settled, never written late");
    // Next turn: relaunched without full access; only the new prompt runs.
    let mut next = dsh("r-rev-2", "workspace-write");
    next.prompt = "simple".into();
    runtime.start_turn(next).unwrap();
    assert_eq!(sink.terminal("r-rev-2").await["type"], "run.completed");
    let launching = sink.wait_for("r-rev-2", "agent_backend.launching").await;
    assert_eq!(launching["mode"], "standard", "{launching}");
    assert_eq!(*modes.lock().unwrap(), [PermissionMode::FullAccess, PermissionMode::Standard]);
    assert_eq!(launches.load(Ordering::SeqCst), 2);
    assert!(!sink.text("r-rev-2").contains("permission:"), "the cancelled turn is not replayed");
    // The relaunch reports its own declaration again.
    let relaunched = runtime.session_info("c-rev").expect("session").declaration.expect("declaration after relaunch");
    assert_eq!(relaunched.mode, PermissionMode::Standard);
    // Already without full access: nothing to restart.
    assert_eq!(runtime.revoke_full_access(&dsh("r-x", "workspace-write").launch).await, RevokeOutcome::Unchanged);
    runtime.shutdown().await;
}

/// Codex leaves its own full access live (`session/set_mode`), no restart,
/// the running turn keeps going.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_codex_full_access_switches_the_live_session() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    let launches = Arc::clone(&launcher.launches);
    let (runtime, sink) = new_runtime(launcher);
    let mut first = turn("c-rev-codex", "r-rc-1", "hello", AgentBackend::Codex, None);
    first.launch.native_permission = Some("agent-full-access".into());
    runtime.start_turn(first).unwrap();
    let (_, approval, _) = sink.wait_approval(1).await;
    let mut restricted = turn("c-rev-codex", "r-x", "x", AgentBackend::Codex, None);
    restricted.launch.native_permission = Some("workspace-write".into());
    assert_eq!(runtime.revoke_full_access(&restricted.launch).await, RevokeOutcome::SwitchedLive);
    assert!(runtime.approval_pending(&approval), "the running turn was not cancelled");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::Written);
    assert_eq!(sink.terminal("r-rc-1").await["type"], "run.completed");
    assert_eq!(launches.load(Ordering::SeqCst), 1, "no restart");
    let switched = sink.events("r-rc-1").into_iter().find(|event| event["type"] == "agent_backend.permission_mode_switched").expect("switched");
    assert_eq!(switched["to"], "workspace-write");
    assert_eq!(runtime.revoke_full_access(&restricted.launch).await, RevokeOutcome::Unchanged);
    runtime.shutdown().await;
}

/// Plan C2: pi's gate requests are answered by the Host approval engine
/// (the sink's), with the conversation's tier; Codex and dsh never are.
#[tokio::test(flavor = "multi_thread")]
async fn pi_requests_go_to_the_host_approval_engine_and_codex_dsh_never_do() {
    if !node_available() {
        return;
    }
    let (runtime, sink) = new_runtime(MockLauncher::default());
    fn tier_policy(mode: &str) -> AutoPermission {
        match mode {
            "allow" | "full_access" => AutoPermission::Allow { option_id: String::new() },
            "read_only" => AutoPermission::Reject { option_id: None },
            _ => AutoPermission::Ask,
        }
    }
    sink.0.lock().unwrap().host_policy = Some(tier_policy);
    // 只读: the engine rejects; recorded as answered by the Host policy.
    let mut read_only = turn("c-hp-ro", "r-hp-ro", "hello", AgentBackend::Pi, None);
    read_only.launch.native_permission = Some("read_only".into());
    runtime.start_turn(read_only).unwrap();
    assert_eq!(sink.terminal("r-hp-ro").await["type"], "run.completed");
    assert!(sink.text("r-hp-ro").ends_with("permission:reject"));
    let answered = sink.events("r-hp-ro").into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("answered");
    assert_eq!(answered["answeredBy"], "host_approval_policy");
    assert_eq!(answered["policy"]["mode"], "read_only");
    // 询问: the engine leaves it to the user: a card.
    runtime.start_turn(turn("c-hp-ask", "r-hp-ask", "hello", AgentBackend::Pi, None)).unwrap();
    let (run, approval, _) = sink.wait_approval(1).await;
    assert_eq!(run, "r-hp-ask");
    assert_eq!(runtime.answer(&approval, false), AnswerOutcome::Written);
    assert!(sink.terminal("r-hp-ask").await["type"] == "run.completed");
    // 自动 switched mid-session: the next request is answered by the new tier.
    assert_eq!(runtime.apply_native_mode("c-hp-ask", "allow").await, NativeModeOutcome::Applied);
    let mut next = turn("c-hp-ask", "r-hp-ask-2", "hello", AgentBackend::Pi, None);
    next.launch.native_permission = Some("allow".into());
    runtime.start_turn(next).unwrap();
    assert_eq!(sink.terminal("r-hp-ask-2").await["type"], "run.completed");
    assert!(sink.text("r-hp-ask-2").ends_with("permission:allow"));
    // Codex / dsh: their own approval decides; the engine is never asked.
    runtime.start_turn(turn("c-hp-codex", "r-hp-codex", "hello", AgentBackend::Codex, None)).unwrap();
    let (run, approval, _) = sink.wait_approval(2).await;
    assert_eq!(run, "r-hp-codex");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::Written);
    sink.terminal("r-hp-codex").await;
    let mut dsh = turn("c-hp-dsh", "r-hp-dsh", "hello", AgentBackend::Deepseek, None);
    dsh.launch.native_permission = Some("workspace-write".into());
    runtime.start_turn(dsh).unwrap();
    let (run, approval, _) = sink.wait_approval(3).await;
    assert_eq!(run, "r-hp-dsh");
    assert_eq!(runtime.answer(&approval, true), AnswerOutcome::Written);
    sink.terminal("r-hp-dsh").await;
    let calls = sink.0.lock().unwrap().policy_calls.clone();
    assert_eq!(calls, [("pi".to_owned(), "read_only".to_owned()), ("pi".to_owned(), "ask".to_owned()), ("pi".to_owned(), "allow".to_owned())]);
    runtime.shutdown().await;
}

/// A JSON-RPC error with `data` (979313c closeout): `run.failed.message`
/// keeps only `code` + `message`; the `data` is redacted once into a
/// `source: "rpc_error_data"` diagnostic (≤ 4 KiB), next to the stderr tail
/// when there is one; no secret from `data` appears anywhere in the event.
#[tokio::test(flavor = "multi_thread")]
async fn a_json_rpc_error_with_data_is_split_out_and_redacted() {
    if !node_available() {
        return;
    }
    let secrets = ["mock-data-bearer-0001", "mock-data-header-key", "sk-mockdata-abcdef0123456789", "mock-data-apikey-value"];
    let check_common = |terminal: &Value| -> Value {
        assert_eq!(terminal["type"], "run.failed", "{terminal}");
        assert_eq!(terminal["code"], "agent_backend_prompt_failed", "{terminal}");
        let message = terminal["message"].as_str().expect("message");
        assert_eq!(message, "prompt failed: Authentication required (JSON-RPC -32000)", "{terminal}");
        assert!(!message.contains("upstream rejected") && !message.contains("api.example.test"), "data leaked into message: {message}");
        let whole = terminal.to_string();
        for secret in secrets {
            assert!(!whole.contains(secret), "{secret} leaked: {whole}");
        }
        let diagnostics = terminal["diagnostics"].as_array().expect("diagnostics array").clone();
        let data = diagnostics.iter().find(|item| item["source"] == "rpc_error_data").expect("rpc_error_data diagnostic").clone();
        assert_eq!(data["kind"], "rpc_error_data");
        assert_eq!(data["redacted"], true);
        assert_eq!(data["maxBytes"], 4096);
        let text = data["text"].as_str().expect("text");
        assert!(text.len() <= 4096);
        assert!(text.contains("upstream rejected the credential"), "{text}");
        assert!(text.contains("https://api.example.test/v1/messages"), "{text}");
        assert!(text.contains("Bearer <redacted>"), "{text}");
        assert!(text.contains("OPENAI_API_KEY=<redacted>"), "{text}");
        Value::Array(diagnostics)
    };

    let (runtime, sink) = new_runtime(MockLauncher::default());
    // data only, nothing on stderr: the single legacy `diagnostic` is the data.
    runtime.start_turn(turn("c-data", "r-data-1", "data-fail", AgentBackend::Codex, None)).unwrap();
    let terminal = sink.terminal("r-data-1").await;
    let diagnostics = check_common(&terminal);
    assert_eq!(diagnostics.as_array().unwrap().len(), 1, "{terminal}");
    assert_eq!(terminal["diagnostic"]["source"], "rpc_error_data", "{terminal}");

    // stderr tail AND data: both kept, stderr first (legacy `diagnostic`).
    runtime.start_turn(turn("c-data", "r-data-2", "data-stderr-fail", AgentBackend::Codex, None)).unwrap();
    let terminal = sink.terminal("r-data-2").await;
    let diagnostics = check_common(&terminal);
    let diagnostics = diagnostics.as_array().unwrap();
    assert_eq!(diagnostics.len(), 2, "{terminal}");
    assert_eq!(diagnostics[0]["kind"], "stderr_tail");
    assert_eq!(diagnostics[0]["source"], "prompt_failed");
    assert!(diagnostics[0]["stderrTail"].as_str().unwrap().contains("upstream said 401"));
    assert_eq!(diagnostics[1]["source"], "rpc_error_data");
    assert_eq!(terminal["diagnostic"]["kind"], "stderr_tail", "{terminal}");
    runtime.shutdown().await;
}

/// A failed prompt (JSON-RPC error, process alive) and a backend that exits
/// mid-turn both persist the redacted stderr tail in `run.failed.diagnostic`.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_prompt_or_exit_persists_the_redacted_stderr_tail() {
    if !node_available() {
        return;
    }
    let secrets = ["sk-mock-0123456789abcdef", "mock-bearer-value", "mock-header-key", "mock-plain-bearer", "mock-token-value"];
    let check = |terminal: &Value, code: &str, source: &str| {
        assert_eq!(terminal["type"], "run.failed", "{terminal}");
        assert_eq!(terminal["code"], code, "{terminal}");
        let diagnostic = &terminal["diagnostic"];
        assert_eq!(diagnostic["kind"], "stderr_tail", "{terminal}");
        assert_eq!(diagnostic["source"], source);
        assert_eq!(diagnostic["redacted"], true);
        let tail = diagnostic["stderrTail"].as_str().expect("stderrTail");
        assert!(tail.len() <= 4096, "{}", tail.len());
        assert!(tail.contains("Connection error (last line)"), "{tail}");
        assert!(tail.contains("DEEPSEEK_API_KEY=<redacted>\n"), "{tail}");
        // Redacted once: no leftover brackets, JSON braces and quotes intact.
        assert!(tail.contains(r#"request headers {"authorization":"Bearer <redacted>","x-api-key":"<redacted>"}"#), "{tail}");
        assert!(tail.contains("Authorization: Bearer <redacted> token: <redacted>"), "{tail}");
        assert!(!tail.contains("]]") && !tail.contains("[redacted"), "{tail}");
        // Long and CJK lines arrive whole.
        let long = tail.lines().find(|line| line.starts_with("mock: long /segment-0000")).expect("long line");
        assert!(long.len() > 1900 && long.ends_with("/segment-0149 end-of-long-line"), "{}", long.len());
        assert!(tail.contains("mock: 错误：无法连接本机网关（127.0.0.1:3425），请检查 Magpie 是否在运行；路径 C:\\Users\\张三\\.dsh"), "{tail}");
        let whole = terminal.to_string();
        for secret in secrets {
            assert!(!whole.contains(secret), "{secret} leaked: {whole}");
        }
    };

    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c-diag", "r-diag-1", "stderr-fail", AgentBackend::Deepseek, None)).unwrap();
    let terminal = sink.terminal("r-diag-1").await;
    check(&terminal, "agent_backend_prompt_failed", "prompt_failed");

    runtime.start_turn(turn("c-diag", "r-diag-2", "stderr-crash", AgentBackend::Deepseek, None)).unwrap();
    let terminal = sink.terminal("r-diag-2").await;
    check(&terminal, "agent_backend_exited", "backend_exited");
    runtime.shutdown().await;
}

/// Per-agent native permission list: the Host answers a permission request
/// itself only as pi's Fox gate (自动 writes the native `allow_once`, 只读
/// rejects the mock's `execute`), or fail-closed for a dsh preset whose
/// approval is `never` (rejected, no card). Codex / dsh are never
/// auto-allowed; 询问-type modes are covered by the tests above (card).
#[tokio::test(flavor = "multi_thread")]
async fn native_modes_answer_without_a_card_only_where_the_mode_says_so_and_are_audited() {
    if !node_available() {
        return;
    }
    // Sessions without full access: the stored choice is the mode (full access would
    // override it with the Agent's full-access option).
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher);
    let mut allow = turn("c-exec-allow", "r-exec-allow", "hello", AgentBackend::Pi, None);
    allow.launch.native_permission = Some("allow".into());
    runtime.start_turn(allow).unwrap();
    let terminal = sink.terminal("r-exec-allow").await;
    assert_eq!(terminal["type"], "run.completed");
    assert!(sink.approvals().is_empty(), "自动: no approval card");
    assert_eq!(sink.text("r-exec-allow"), "Running tests permission:allow", "allow_once was written, not allow_always");
    let answered = sink.events("r-exec-allow").into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("recorded");
    assert_eq!(answered["permissionMode"], "allow");
    assert_eq!(answered["decision"], "allow_once");
    assert_eq!(answered["toolKind"], "execute");
    assert_eq!(answered["nativeAnswer"], "response_written");

    let mut read_only = turn("c-exec-ro", "r-exec-ro", "hello", AgentBackend::Pi, None);
    read_only.launch.native_permission = Some("read_only".into());
    runtime.start_turn(read_only).unwrap();
    let terminal = sink.terminal("r-exec-ro").await;
    assert_eq!(terminal["type"], "run.completed");
    assert!(sink.approvals().is_empty(), "只读: no approval card");
    assert!(sink.text("r-exec-ro").ends_with("permission:reject"), "the execute request was rejected");
    let answered = sink.events("r-exec-ro").into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("recorded");
    assert_eq!(answered["permissionMode"], "read_only");
    assert_eq!(answered["decision"], "reject");

    let mut never = turn("c-exec-dsh", "r-exec-dsh", "hello", AgentBackend::Deepseek, None);
    never.launch.native_permission = Some("workspace-write-never".into());
    runtime.start_turn(never).unwrap();
    let terminal = sink.terminal("r-exec-dsh").await;
    assert_eq!(terminal["type"], "run.completed");
    assert!(sink.approvals().is_empty(), "approval never: fail-closed reject, no card");
    assert!(sink.text("r-exec-dsh").ends_with("permission:reject"));
    let answered = sink.events("r-exec-dsh").into_iter().find(|event| event["type"] == "agent_backend.permission_auto_answered").expect("recorded");
    assert_eq!(answered["permissionMode"], "workspace-write-never");
    assert_eq!(answered["decision"], "reject");

    let audits = sink.0.lock().unwrap().audits.clone();
    assert_eq!(audits.len(), 3);
    assert!(audits.iter().all(|(_, event, details)| event == "agent_backend.permission_auto_answered" && details.get("input").is_none()), "names and kinds only, no tool input");
    assert_eq!(audits[0].0, "c-exec-allow");
    assert_eq!(audits[1].0, "c-exec-ro");
    assert_eq!(audits[2].0, "c-exec-dsh");
    runtime.shutdown().await;
}

/// Per-agent permission list: a Codex mode change reaches the live session
/// at once (`session/set_mode`), a dsh preset change restarts dsh at the
/// next turn, and the turn's later permission requests use the new mode.
#[tokio::test(flavor = "multi_thread")]
async fn native_mode_changes_switch_codex_live_and_restart_dsh_next_turn() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("c-mode", "r-mode-1", "simple", AgentBackend::Codex, None)).unwrap();
    sink.terminal("r-mode-1").await;
    assert_eq!(runtime.apply_native_mode("c-mode", "read-only").await, fox_agent_backend::external::NativeModeOutcome::Applied);
    let mut next = turn("c-mode", "r-mode-2", "simple", AgentBackend::Codex, None);
    next.launch.native_permission = Some("read-only".into());
    runtime.start_turn(next).unwrap();
    sink.terminal("r-mode-2").await;
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "Codex: no restart for a mode change");
    let audits = sink.0.lock().unwrap().audits.clone();
    assert!(audits.iter().any(|(conversation, event, details)| conversation == "c-mode" && event == "agent_backend.permission_mode_switched" && details["to"] == "read-only" && details["method"] == "session/set_mode"));
    // Codex's own full access is switched live too (plan C2), no restart.
    assert_eq!(runtime.apply_native_mode("c-mode", "agent-full-access").await, fox_agent_backend::external::NativeModeOutcome::Applied);
    assert_eq!(runtime.apply_native_mode("c-mode", "workspace-write").await, fox_agent_backend::external::NativeModeOutcome::Applied);
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "Codex: entering / leaving full access never restarts");

    runtime.start_turn(turn("c-dsh-mode", "r-dsh-1", "simple", AgentBackend::Deepseek, None)).unwrap();
    sink.terminal("r-dsh-1").await;
    assert_eq!(runtime.apply_native_mode("c-dsh-mode", "read-only").await, fox_agent_backend::external::NativeModeOutcome::NextTurn);
    let mut next = turn("c-dsh-mode", "r-dsh-2", "simple", AgentBackend::Deepseek, None);
    next.launch.native_permission = Some("read-only".into());
    runtime.start_turn(next).unwrap();
    sink.terminal("r-dsh-2").await;
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 3, "dsh restarted for the new preset");
    assert!(sink.types("r-dsh-2").iter().any(|kind| kind == "agent_backend.permission_mode_restart"));
    runtime.shutdown().await;
}


async fn wait_count(counter: &AtomicUsize, wanted: usize) {
    for _ in 0..200 {
        if counter.load(Ordering::SeqCst) >= wanted {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("counter stayed at {}, wanted {wanted}", counter.load(Ordering::SeqCst));
}

/// L-1: revoking full access while the process is still starting. The
/// revocation answers `LaunchAborted`; the process that came up in full
/// access is stopped (its cleanup runs) before any prompt reaches it, and
/// the next turn starts without full access.
#[tokio::test(flavor = "multi_thread")]
async fn l1_revoke_during_launch_never_prompts_a_full_access_process() {
    if !node_available() {
        return;
    }
    for backend in [AgentBackend::Deepseek, AgentBackend::Codex] {
        let launcher = MockLauncher::default();
        launcher.launch_delay_ms.store(400, Ordering::SeqCst);
        let (runtime, sink) = new_runtime(launcher.clone());
        let conversation = format!("l1-{}", backend.as_str());
        let run = format!("{conversation}-run");
        let request = turn(&conversation, &run, "simple", backend, None);
        let launch = request.launch.clone();
        runtime.start_turn(request).unwrap();
        sink.wait_for(&run, "agent_backend.launching").await;
        launcher.revoked.store(true, Ordering::SeqCst);
        assert_eq!(runtime.revoke_full_access(&launch).await, RevokeOutcome::LaunchAborted, "{backend:?}");
        let terminal = sink.terminal(&run).await;
        assert_eq!(terminal["type"], "run.failed", "{backend:?}: {terminal}");
        assert_eq!(terminal["code"], "agent_backend_full_access_revoked");
        assert!(sink.text(&run).is_empty(), "{backend:?}: a prompt reached the full-access process");
        assert!(!sink.types(&run).contains(&"agent_backend.session".to_owned()), "never installed");
        wait_count(&launcher.closed, 1).await;
        assert!(!runtime.session_info(&conversation).unwrap().live);
        launcher.launch_delay_ms.store(0, Ordering::SeqCst);
        let next = format!("{conversation}-next");
        runtime.start_turn(turn(&conversation, &next, "simple", backend, None)).unwrap();
        assert_eq!(sink.terminal(&next).await["type"], "run.completed");
        assert_eq!(*launcher.modes.lock().unwrap().last().unwrap(), PermissionMode::Standard, "the next launch is restricted");
        runtime.shutdown().await;
    }
}

/// L-2 (a): the conversation is closed while its process is starting. The
/// new process is stopped with its own cleanup, nothing stays behind the
/// removed session, and the turn ends interrupted.
#[tokio::test(flavor = "multi_thread")]
async fn l2_close_during_launch_stops_the_new_process() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    launcher.launch_delay_ms.store(400, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("l2", "l2-run", "simple", AgentBackend::Codex, None)).unwrap();
    sink.wait_for("l2-run", "agent_backend.launching").await;
    assert!(runtime.close_conversation("l2", "deleted").await.is_none(), "nothing was running yet");
    let terminal = sink.terminal("l2-run").await;
    assert_eq!(terminal["type"], "run.interrupted", "{terminal}");
    wait_count(&launcher.closed, 1).await;
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1);
    assert!(runtime.session_info("l2").is_none(), "the closed session is not resurrected");
    assert!(runtime.active_runs().is_empty());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(launcher.closed.load(Ordering::SeqCst), 1, "exactly one cleanup");
    assert!(sink.text("l2-run").is_empty());
    runtime.shutdown().await;
}

/// L-2 (b): the cancel timer ends a turn whose launch is still pending and a
/// new turn starts at once. One launch per session at a time: the new turn
/// waits for the pending launch and reuses its process instead of starting a
/// second one that would overwrite (orphan) the first.
#[tokio::test(flavor = "multi_thread")]
async fn l2_a_new_turn_waits_for_the_pending_launch_instead_of_racing_it() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    launcher.launch_delay_ms.store(1500, Ordering::SeqCst);
    let sink = Sink::default();
    let runtime = ExternalRuntime::with_cancel_timeout(sink.clone(), launcher.clone(), tokio::runtime::Handle::current(), Duration::from_millis(200));
    runtime.start_turn(turn("l2b", "first", "simple", AgentBackend::Pi, None)).unwrap();
    sink.wait_for("first", "agent_backend.launching").await;
    assert!(matches!(runtime.cancel("first"), CancelOutcome::Requested { .. }));
    assert_eq!(sink.terminal("first").await["type"], "run.cancelled");
    runtime.start_turn(turn("l2b", "second", "simple", AgentBackend::Pi, None)).unwrap();
    assert_eq!(sink.terminal("second").await["type"], "run.completed");
    assert_eq!(launcher.launches.load(Ordering::SeqCst), 1, "no second, racing launch");
    assert_eq!(launcher.closed.load(Ordering::SeqCst), 0, "the only process is the live one");
    assert!(runtime.session_info("l2b").unwrap().live);
    runtime.shutdown().await;
    wait_count(&launcher.closed, 1).await;
}

/// L-3: stopping a backend never parks a runtime worker. On a single-thread
/// runtime a ticker keeps running while a process that ignores the end of
/// its stdin is stopped (grace period, kill, tree confirmation).
#[tokio::test(flavor = "current_thread")]
async fn l3_stop_does_not_block_the_runtime() {
    if !node_available() {
        return;
    }
    let launcher = MockLauncher::default();
    launcher.revoked.store(true, Ordering::SeqCst);
    let (runtime, sink) = new_runtime(launcher.clone());
    runtime.start_turn(turn("l3", "l3-run", "hang", AgentBackend::Pi, None)).unwrap();
    sink.wait_for("l3-run", "message.delta").await;
    let ticks = Arc::new(AtomicUsize::new(0));
    let ticker = {
        let ticks = Arc::clone(&ticks);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(20)).await;
                ticks.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let started = std::time::Instant::now();
    let report = runtime.close_conversation("l3", "test stop").await.expect("a live process");
    let elapsed = started.elapsed();
    ticker.abort();
    assert!(report.confirmation.is_confirmed(), "{:?}", report.confirmation);
    let expected = (elapsed.as_millis() / 20) as usize;
    assert!(ticks.load(Ordering::SeqCst) * 3 >= expected, "runtime starved during stop: {} ticks in {elapsed:?}", ticks.load(Ordering::SeqCst));
}
