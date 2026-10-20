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
    AnswerOutcome, CancelOutcome, ExternalLauncher, ExternalRuntime, ExternalSink, LaunchFailure, LaunchRequest, Launched, StartError, TurnRequest,
};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::{AgentBackend, StopConfirmation};
use fox_agent_backend::projection::PermissionCard;
use fox_agent_backend::sandbox::{HostOs, PermissionMode, SandboxDeclaration};
use futures::future::BoxFuture;
use serde_json::Value;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Recorded {
    events: Vec<(String, Value)>,
    approvals: Vec<(String, String, PermissionCard)>,
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
}

impl ExternalLauncher for MockLauncher {
    fn desired_mode(&self, _request: &LaunchRequest) -> PermissionMode {
        PermissionMode::FullAccess
    }

    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let launches = Arc::clone(&self.launches);
        let fail = self.fail;
        Box::pin(async move {
            launches.fetch_add(1, Ordering::SeqCst);
            if fail {
                return Err(LaunchFailure::new("sandbox_dependency_missing", "sandbox runtime is not installed").remedy("reinstall"));
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
            let connection = AcpConnection::spawn_detailed(&spec, &request.project_root, None).await.map_err(LaunchFailure::from)?;
            Ok(Launched {
                connection,
                mode: PermissionMode::FullAccess,
                declaration: SandboxDeclaration::declare(PermissionMode::FullAccess, None, HostOs::current()),
                pi_launch: PiLaunch::Stock,
                on_closed: None,
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
        },
    }
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
    let (runtime, sink) = new_runtime(MockLauncher::default());
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
    assert_eq!(terminal["code"], "sandbox_dependency_missing");
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
    let (runtime, sink) = new_runtime(MockLauncher::default());
    runtime.start_turn(turn("c9", "r12", "hello", AgentBackend::Deepseek, None)).unwrap();
    let terminal = sink.terminal("r12").await;
    assert!(sink.approvals().is_empty(), "no approval card for DeepSeek");
    assert!(sink.types("r12").iter().any(|kind| kind == "agent_backend.permission_auto_rejected"));
    // The mock offers no `reject-once`, so the Host answered `cancelled`.
    assert_eq!(terminal["type"], "run.cancelled");
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
    let (runtime, sink) = new_runtime(MockLauncher::default());
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
