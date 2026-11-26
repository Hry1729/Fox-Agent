//! fox-fix-pi: the Fox tools for pi, end to end with the REAL programs.
//!
//! Unlike `fox_mcp_bridge.rs` (a mock ACP "pi" that imports the extension
//! with Node's type stripping and skips the approval gate), this runs what a
//! user's machine runs: real pi-acp 0.0.34 (with Fox's audited runtime
//! patches) -> the real `fixtures/pi/pi-gated.sh` -> real pi 1.4.0 (its
//! bundled CLI, jiti extension loading, RPC mode) with the real
//! `fox-approval-gate.ts` and the real `fox-mcp-tools.ts` the Host writes ->
//! the Host's real `McpBridge`, driven through `ExternalRuntime` like the
//! desktop. The model is the test-only scripted provider
//! (`fixtures/pi/scripted-provider.ts`, an OpenAI-compatible server inside
//! pi), so no credentials are needed.
//!
//! The runtime comes from `FOX_PI_RUNTIME_DIR`: a directory holding
//! `backends/pi/node_modules`, built from the pinned lockfile
//! (`scripts/windows/runtime-package/backends/pi`, `npm ci`) and patched with
//! `scripts/windows/runtime_patches.py`; the Linux ACP CI job builds it.
//! Without it the tests print SKIPPED (not a pass); with
//! `FOX_REQUIRE_REAL_PI=1` (CI) a missing runtime fails.
//!
//! Runs on Linux and on Windows (there through the real `pi-gated.cmd`,
//! `pi.cmd` / `pi-acp.cmd` npm shims and cmd.exe, which is where 13.32 failed).
#![cfg(feature = "acp")]

#[path = "support/pi_real.rs"]
mod pi_real;

use fox_agent_backend::port::AgentBackend;
use pi_real::*;
use serde_json::json;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread")]
async fn real_native_pi_keeps_file_tools_and_steers_before_settlement() {
    use fox_agent_backend::acp::BackendEvent;
    use fox_agent_backend::external::ExternalLauncher;
    use fox_agent_backend::external_launch::{HostLaunchConfig, HostLauncher};
    use fox_agent_backend::fox_mcp::{FoxToolHost, McpBridge, ToolCallContext, ToolError};
    use futures::future::BoxFuture;
    use std::sync::{Arc, Mutex};

    let Some(runtime_root) = runtime_dir("real_native_pi_keeps_file_tools_and_steers_before_settlement") else { return };
    struct Host(Arc<Mutex<Vec<String>>>);
    impl FoxToolHost for Host {
        fn call_tool(&self, _ctx: ToolCallContext, tool: String, _input: serde_json::Value)
            -> BoxFuture<'static, Result<serde_json::Value, ToolError>>
        {
            self.0.lock().unwrap().push(tool);
            Box::pin(async { Ok(json!({"recorded": true})) })
        }
    }
    let calls = Arc::new(Mutex::new(Vec::new()));
    let bridge = McpBridge::start(Arc::new(Host(calls.clone())), &tokio::runtime::Handle::current()).unwrap();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut config = HostLaunchConfig::from_env(data.path());
    config.runtime_root = runtime_root;
    config.state_root = data.path().join("agent-state");
    config.native_login_home = None;
    let request = turn("native", "native-first", "GATEREAD:proof", project.path()).launch;
    let home = config.private_dirs(&request).unwrap().private_home.join(".pi/agent");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("settings.json"), json!({
        "defaultProvider": "foxscripted",
        "defaultModel": "scripted",
        "compaction": {"enabled": false, "keepRecentTokens": 1},
        "extensions": [fixtures().join("scripted-provider.ts")]
    }).to_string()).unwrap();
    std::fs::write(home.join("models.json"), json!({"providers": {"foxscripted": {
        "baseUrl": "http://127.0.0.1:1/v1", "api": "openai-completions", "apiKey": "local-test",
        "models": [{"id": "scripted", "name": "scripted"}]
    }}}).to_string()).unwrap();
    std::fs::write(project.path().join("fox-write-proof.txt"), "original working data").unwrap();
    let launcher = HostLauncher::with_mcp(config, bridge);
    let mut launched = launcher.launch(request.clone()).await.unwrap();
    let mut events = launched.connection.take_events();
    let BackendEvent::Initialized { initialize_response, .. } = events.recv().await.unwrap() else { panic!("expected initialize") };
    assert!(fox_agent_backend::backends::pi::native_rpc(&initialize_response));
    let id = match launched.connection.new_session(project.path().to_path_buf()).await {
        Ok(id) => id,
        Err(error) => {
            let (_, stderr) = launched.connection.close_with_report_and_tail().await;
            panic!("{error}\n{stderr}");
        }
    };
    assert_eq!(launched.fox_tools.as_ref().unwrap().progress(), fox_agent_backend::fox_mcp::McpProgress::Listed);
    launched.connection.set_model(&id, "foxscripted/scripted").await.unwrap();
    launched.connection.prompt(&id, "GATEREAD:proof").unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            match events.recv().await.unwrap() {
                BackendEvent::PermissionRequested { native_request_id, request, .. } => {
                    let parsed = fox_agent_backend::backends::pi::parse_tool_approval(&request).unwrap();
                    assert_eq!(parsed.tool_name, "read");
                    break native_request_id;
                }
                BackendEvent::TurnFailed { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }).await.unwrap();
    {
        let steering = launched.connection.steer(id.clone(), "corrected request".into(), true);
        tokio::pin!(steering);
        tokio::select! {
            result = &mut steering => panic!("steering reported before tool approval: {result:?}"),
            _ = tokio::time::sleep(Duration::from_millis(150)) => {}
        }
        launched.connection.answer_permission(&approval, Some("yes")).unwrap();
        tokio::time::timeout(Duration::from_secs(15), steering).await.unwrap().unwrap();
    }
    wait_for_native_turn(&launched.connection, &mut events).await;
    launched.connection.close_with_report().await;
    launched.on_closed.take().unwrap()();

    let saved = home.join("fox-sessions").join(format!("{id}.json"));
    let entry: serde_json::Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    let legacy = home.parent().unwrap().join("pi-acp");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join("session-map.json"), json!({
        "version": 1, "sessions": { (id.clone()): entry }
    }).to_string()).unwrap();
    std::fs::remove_file(saved).unwrap();

    let mut reopened = launcher.launch(request).await.unwrap();
    let mut events = reopened.connection.take_events();
    reopened.connection.load_session(&id, project.path().to_path_buf()).await.unwrap();
    reopened.connection.prompt(&id, "FOXTOOL:present_files").unwrap();
    wait_for_native_turn(&reopened.connection, &mut events).await;
    reopened.connection.prompt(&id, "FOXTOOL:companion_list").unwrap();
    wait_for_native_turn(&reopened.connection, &mut events).await;
    reopened.connection.prompt(&id, "GATE:exit-zero").unwrap();
    assert_eq!(wait_for_native_turn(&reopened.connection, &mut events).await, [("completed".to_owned(), 0)]);
    reopened.connection.prompt(&id, "GATEEXIT:exit-two").unwrap();
    assert_eq!(wait_for_native_turn(&reopened.connection, &mut events).await, [("failed".to_owned(), 2)]);
    assert_eq!(&*calls.lock().unwrap(), &["present_files", "companion_list"]);
    reopened.connection.prompt(&id, "/compact").unwrap();
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut compacted = false;
        loop {
            match events.recv().await.unwrap() {
                BackendEvent::Update { update, .. }
                    if update.pointer("/_meta/fox/piCompaction").is_some() =>
                {
                    assert_eq!(update["_meta"]["fox"]["piCompaction"]["reason"], "manual");
                    compacted = true;
                }
                BackendEvent::TurnEnded { stop_reason, .. } => {
                    assert_eq!(stop_reason, "end_turn");
                    assert!(compacted, "native compaction completion must arrive before the turn ends");
                    break;
                }
                BackendEvent::TurnFailed { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }).await.unwrap();
    reopened.connection.close_with_report().await;
    reopened.on_closed.take().unwrap()();
}

async fn wait_for_native_turn(
    connection: &fox_agent_backend::acp::AcpConnection,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<fox_agent_backend::acp::BackendEvent>,
) -> Vec<(String, i64)> {
    use fox_agent_backend::acp::BackendEvent;
    tokio::time::timeout(Duration::from_secs(20), async {
        let mut command_results = Vec::new();
        loop {
            match events.recv().await.unwrap() {
                BackendEvent::PermissionRequested { native_request_id, .. } => {
                    connection.answer_permission(&native_request_id, Some("yes")).unwrap();
                }
                BackendEvent::TurnEnded { stop_reason, .. } => {
                    assert_eq!(stop_reason, "end_turn");
                    return command_results;
                }
                BackendEvent::Update { update, .. } => {
                    if let Some(code) = update.pointer("/_meta/terminal_exit/exit_code").and_then(serde_json::Value::as_i64) {
                        command_results.push((update["status"].as_str().unwrap().to_owned(), code));
                    }
                }
                BackendEvent::TurnFailed { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }).await.unwrap()
}

/// The 13.32 blocker, on Linux: a whole conversation through real pi-acp +
/// real gated pi 1.4.0 with the Fox tool extension: plain answer, a Fox tool
/// call reaching the Host bound to this conversation and turn, ask_user
/// ending the turn awaiting the user, token revoked on close.
#[tokio::test(flavor = "multi_thread")]
async fn real_pi_and_pi_acp_converse_and_use_the_fox_tools() {
    let Some(pi_runtime) = runtime_dir("real_pi_and_pi_acp_converse_and_use_the_fox_tools") else { return };
    let Setup { runtime, host, bridge, sink, project, _state } = setup(pi_runtime, Wiring::Bridge);
    let conversation = "conv-real-pi";

    // 1. A plain turn: the session opens with the tools listed, the scripted
    //    model answers (this is where 13.32 sat until the 120 s timeout).
    let started = Instant::now();
    runtime.start_turn(turn(conversation, "real-hello", "hello", project.path())).unwrap();
    let terminal = sink.terminal("real-hello", Duration::from_secs(150)).await;
    assert_eq!(terminal["type"], "run.completed", "{terminal} {:?}", sink.events("real-hello"));
    assert_eq!(sink.text("real-hello"), "scripted hello");
    eprintln!("first turn (launch + session + answer) took {:?}", started.elapsed());

    // 2. pi calls a Fox tool: the gate asks, the Host allows, the extension
    //    calls the bridge, the Host sees this conversation and turn.
    runtime.start_turn(turn(conversation, "real-present", "FOXTOOL:present_files", project.path())).unwrap();
    let terminal = sink.terminal("real-present", Duration::from_secs(60)).await;
    assert_eq!(terminal["type"], "run.completed", "{terminal}");
    let (context, tool, arguments) = host.calls.lock().unwrap().last().cloned().expect("the Host got the call");
    assert_eq!((context.conversation_id.as_str(), context.backend, tool.as_str()), (conversation, AgentBackend::Pi, "present_files"));
    assert_eq!(arguments, json!({ "files": [{ "path": "report.docx" }] }));
    let text = sink.text("real-present");
    assert!(text.starts_with("tool-result:") && text.contains("real-present"), "the model read the Host result: {text}");

    // 3. ask_user: question card, turn completes awaiting the user.
    runtime.start_turn(turn(conversation, "real-ask", "FOXTOOL:ask_user", project.path())).unwrap();
    let terminal = sink.terminal("real-ask", Duration::from_secs(60)).await;
    assert_eq!(terminal["completionReason"], "awaiting_user", "{terminal}");
    assert!(sink.events("real-ask").iter().any(|event| event["type"] == "user.question.requested"));

    // 4. Stop: the launch's token is revoked with the process.
    assert_eq!(bridge.live_tokens(), 1);
    runtime.close_conversation(conversation, "test done").await;
    assert_eq!(bridge.live_tokens(), 0);
    runtime.shutdown().await;
}

async fn assert_fails_fast_as_unavailable(pi_runtime: PathBuf, wiring: Wiring, label: &str) {
    let Setup { runtime, host, bridge, sink, project, _state } = setup(pi_runtime, wiring);
    let started = Instant::now();
    runtime.start_turn(turn("conv-fail", label, "hello", project.path())).unwrap();
    let terminal = sink.terminal(label, Duration::from_secs(90)).await;
    let elapsed = started.elapsed();
    assert_eq!(terminal["type"], "run.failed", "{terminal}");
    assert_eq!(terminal["code"], "agent_backend_fox_tools_unavailable", "{terminal}");
    assert_eq!(terminal["launch"]["detail"]["bridge"], "not_connected", "{terminal}");
    // Well before the 120 s response timeout the 13.32 run hit.
    assert!(elapsed < Duration::from_secs(60), "{label} took {elapsed:?}");
    assert!(host.calls.lock().unwrap().is_empty());
    assert_eq!(bridge.live_tokens(), 0, "the failed launch's token is revoked");
    eprintln!("{label}: {} after {elapsed:?}", terminal["code"]);
    runtime.shutdown().await;
}

/// A bridge that takes the connection and never answers: the extension's
/// init deadline throws, pi exits, the Host reports fox_tools_unavailable
/// (not a pi that waits forever, not a 120 s timeout).
#[tokio::test(flavor = "multi_thread")]
async fn real_pi_with_a_silent_bridge_fails_fast_as_fox_tools_unavailable() {
    let Some(pi_runtime) = runtime_dir("real_pi_with_a_silent_bridge_fails_fast_as_fox_tools_unavailable") else { return };
    let silent = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("127.0.0.1:{}", silent.local_addr().unwrap().port());
    assert_fails_fast_as_unavailable(pi_runtime, Wiring::Endpoint(endpoint), "silent-bridge").await;
    drop(silent);
}

/// A token the bridge refuses (never issued / revoked).
#[tokio::test(flavor = "multi_thread")]
async fn real_pi_with_a_refused_token_fails_fast_as_fox_tools_unavailable() {
    let Some(pi_runtime) = runtime_dir("real_pi_with_a_refused_token_fails_fast_as_fox_tools_unavailable") else { return };
    assert_fails_fast_as_unavailable(pi_runtime, Wiring::WrongToken, "refused-token").await;
}

/// Nothing listens at the endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn real_pi_with_no_bridge_fails_fast_as_fox_tools_unavailable() {
    let Some(pi_runtime) = runtime_dir("real_pi_with_no_bridge_fails_fast_as_fox_tools_unavailable") else { return };
    let closed = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!("127.0.0.1:{}", closed.local_addr().unwrap().port());
    drop(closed);
    assert_fails_fast_as_unavailable(pi_runtime, Wiring::Endpoint(endpoint), "no-bridge").await;
}
