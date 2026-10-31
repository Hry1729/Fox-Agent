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
