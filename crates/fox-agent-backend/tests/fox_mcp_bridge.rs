//! Plan C3: the Fox tool MCP bridge injected into an external Agent, end to
//! end on Linux. A mock ACP agent (`tests/fixtures/mock-acp-mcp-agent.mjs`)
//! does what Codex / dsh do with ACP `session/new` `mcpServers`: it starts the
//! stdio server it was given (the real `fox-mcp-relay.mjs`, run by Node),
//! speaks MCP through it to the Host's `McpBridge`, and calls tools. Checks:
//! the injected server (name, relay, token env), tools/list, a tool call that
//! reaches the Host bound to the launch's conversation, `ask_user` completing
//! the turn as `awaiting_user`, token revocation when the process stops, and
//! that a stale token is refused. Skips when Node is not installed.
#![cfg(feature = "acp")]

use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{ExternalLauncher, ExternalRuntime, ExternalSink, LaunchFailure, LaunchRequest, Launched, TurnRequest};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use fox_agent_backend::fox_mcp::{self, FoxToolHost, McpBridge, McpInjection, ToolCallContext, ToolError};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::PermissionCard;
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<(String, Value)>>>);

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0.lock().unwrap().push((run_id.to_owned(), event));
        Ok(())
    }
    fn request_approval(&self, _run_id: &str, _conversation_id: &str, _key: &str, _card: &PermissionCard) -> Result<String, String> {
        Err("no approvals in this test".into())
    }
}

impl Sink {
    fn events(&self, run: &str) -> Vec<Value> {
        self.0.lock().unwrap().iter().filter(|(id, _)| id == run).map(|(_, event)| event.clone()).collect()
    }
    fn text(&self, run: &str) -> String {
        self.events(run).iter().filter(|event| event["type"] == "message.delta").map(|event| event["delta"].as_str().unwrap().to_owned()).collect()
    }
    async fn terminal(&self, run: &str) -> Value {
        for _ in 0..300 {
            if let Some(event) = self.events(run).into_iter().find(|event| event["type"].as_str().is_some_and(|kind| matches!(kind, "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted"))) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no terminal for {run}: {:?}", self.events(run));
    }
}

type Runtime = ExternalRuntime<Sink, BridgeLauncher>;

/// The Host side: records calls; `ask_user` marks the turn like the desktop.
#[derive(Clone, Default)]
struct RecordingHost {
    calls: Arc<Mutex<Vec<(ToolCallContext, String, Value)>>>,
    runtime: Arc<OnceLock<Runtime>>,
}

impl FoxToolHost for RecordingHost {
    fn call_tool(&self, context: ToolCallContext, tool: String, arguments: Value) -> BoxFuture<'static, Result<Value, ToolError>> {
        self.calls.lock().unwrap().push((context.clone(), tool.clone(), arguments.clone()));
        let runtime = self.runtime.get().cloned();
        Box::pin(async move {
            let runtime = runtime.ok_or_else(|| ToolError::new("fox_mcp.host_unavailable", "no runtime"))?;
            let run = runtime.active_run_of(&context.conversation_id).ok_or_else(|| ToolError::new("fox_mcp.no_active_run", "no turn"))?;
            match tool.as_str() {
                "ask_user" => {
                    runtime.sink().emit(&run, &context.conversation_id, json!({ "type": "user.question.requested", "questions": arguments["questions"] })).unwrap();
                    assert!(runtime.mark_awaiting_user(&context.conversation_id, &run));
                    Ok(Value::String("question shown; end the turn".into()))
                }
                "present_files" => Ok(json!({ "declared": 1, "recorded": true, "run": run })),
                "search_knowledge" => Ok(Value::String("x".repeat(100_000))),
                _ => Err(ToolError::new("local_knowledge.binding_invalid", "not enabled")),
            }
        })
    }
}

/// Launches the MCP mock agent with the bridge injected the way the real
/// launcher does for Codex / dsh (fresh token, stdio relay, revoked on close).
#[derive(Clone)]
struct BridgeLauncher {
    bridge: McpBridge,
    state_root: std::path::PathBuf,
    tokens: Arc<Mutex<Vec<String>>>,
}

fn node() -> Option<std::path::PathBuf> {
    let output = std::process::Command::new("node").args(["-e", "process.stdout.write(process.execPath)"]).output().ok()?;
    output.status.success().then(|| std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).into_owned()))
}

impl ExternalLauncher for BridgeLauncher {
    fn desired_mode(&self, _request: &LaunchRequest) -> PermissionMode {
        PermissionMode::Standard
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let bridge = self.bridge.clone();
        let state_root = self.state_root.clone();
        let tokens = Arc::clone(&self.tokens);
        Box::pin(async move {
            let assets = fox_mcp::write_assets(&state_root).map_err(|error| LaunchFailure::new("agent_backend_fox_tools_unavailable", error))?;
            let grant = bridge.issue(&request.conversation_id, request.backend).map_err(|error| LaunchFailure::new("agent_backend_fox_tools_unavailable", error))?;
            tokens.lock().unwrap().push(grant.token.clone());
            let node = node().expect("node");
            let connection = match fox_mcp::injection_for(request.backend, &grant, &node, &assets) {
                Some(McpInjection::AcpStdio(server)) => {
                    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-mcp-agent.mjs");
                    let spec = LaunchSpec { backend: request.backend, program: "node".into(), args: vec![script.into()], version_pin: "mock", credential_env: &[], experimental: false, session_env: vec![] };
                    let mut connection = AcpConnection::spawn_detailed(&spec, &request.project_root).await.map_err(LaunchFailure::from)?;
                    connection.set_mcp_servers(vec![server]);
                    connection
                }
                // pi (plan C3): the gate loads the extension; the launch is the
                // real fixtures/pi/pi-gated.sh with FOX_PI_BIN = the mock pi.
                Some(McpInjection::PiExtension { env }) => {
                    let wrapper = state_root.join("fake-pi.sh");
                    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mock-acp-pi-agent.mjs");
                    std::fs::write(&wrapper, format!("#!/bin/sh\nexec '{}' '{fixture}' \"$@\"\n", node.display())).unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
                    }
                    let mut session_env = env;
                    session_env.push(("FOX_PI_BIN".into(), wrapper.to_string_lossy().into_owned()));
                    let gate = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pi/pi-gated.sh");
                    let spec = LaunchSpec { backend: request.backend, program: "sh".into(), args: vec![gate.into()], version_pin: "mock", credential_env: &[], experimental: false, session_env };
                    AcpConnection::spawn_detailed(&spec, &request.project_root).await.map_err(LaunchFailure::from)?
                }
                _ => return Err(LaunchFailure::new("agent_backend_invalid", "no Fox tool injection")),
            };
            let token = grant.token.clone();
            Ok(Launched {
                connection,
                mode: PermissionMode::Standard,
                declaration: LaunchDeclaration::declare(PermissionMode::Standard),
                pi_launch: PiLaunch::Stock,
                on_closed: Some(Box::new(move || bridge.revoke(&token))),
            })
        })
    }
}

fn turn(conversation: &str, run: &str, prompt: &str, backend: AgentBackend) -> TurnRequest {
    TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend,
            project_uuid: None,
            project_root: std::env::temp_dir(),
            session_key: format!("k-{run}"),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bridge_is_injected_used_and_revoked_for_codex_and_dsh() {
    if node().is_none() {
        eprintln!("SKIPPED: node not installed");
        return;
    }
    let host = RecordingHost::default();
    let bridge = McpBridge::start(Arc::new(host.clone()), &tokio::runtime::Handle::current()).unwrap();
    let state = tempfile::tempdir().unwrap();
    let launcher = BridgeLauncher { bridge: bridge.clone(), state_root: state.path().to_path_buf(), tokens: Arc::default() };
    let sink = Sink::default();
    let runtime = ExternalRuntime::new(sink.clone(), launcher.clone(), tokio::runtime::Handle::current());
    let _ = host.runtime.set(runtime.clone());

    for backend in [AgentBackend::Codex, AgentBackend::Deepseek] {
        let conversation = format!("conv-{}", backend.as_str());
        let run = |name: &str| format!("{}-{name}", backend.as_str());

        // 1. What the Agent received in session/new.
        runtime.start_turn(turn(&conversation, &run("servers"), "servers", backend)).unwrap();
        assert_eq!(sink.terminal(&run("servers")).await["type"], "run.completed");
        let servers: Value = serde_json::from_str(&sink.text(&run("servers"))).unwrap();
        assert_eq!(servers[0]["name"], fox_mcp::SERVER_NAME);
        assert!(servers[0]["args"][0].as_str().unwrap().ends_with(fox_mcp::RELAY_FILE));
        assert_eq!(servers[0]["envNames"], json!([fox_mcp::ENDPOINT_ENV, fox_mcp::TOKEN_ENV]));

        // 2. tools/list through the relay.
        runtime.start_turn(turn(&conversation, &run("tools"), "tools", backend)).unwrap();
        assert_eq!(sink.terminal(&run("tools")).await["type"], "run.completed");
        let tools: Vec<String> = serde_json::from_str(&sink.text(&run("tools"))).unwrap();
        assert_eq!(tools, fox_mcp::TOOL_NAMES);

        // 3. A tool call reaches the Host bound to this conversation and turn;
        //    arguments cannot name another conversation.
        let call = r#"call present_files {"files":[{"path":"report.docx"}],"conversationId":"someone-else"}"#;
        runtime.start_turn(turn(&conversation, &run("present"), call, backend)).unwrap();
        assert_eq!(sink.terminal(&run("present")).await["type"], "run.completed");
        let result: Value = serde_json::from_str(&sink.text(&run("present"))).unwrap();
        assert_eq!(result["isError"], false);
        let payload: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(payload["run"], run("present"), "the Host saw the active turn");
        let (context, tool, _) = host.calls.lock().unwrap().last().cloned().unwrap();
        assert_eq!((context.conversation_id.as_str(), context.backend, tool.as_str()), (conversation.as_str(), backend, "present_files"));

        // 4. The 64 KiB ceiling and a Host refusal as a tool error.
        runtime.start_turn(turn(&conversation, &run("big"), "call search_knowledge {\"query\":\"q\"}", backend)).unwrap();
        assert_eq!(sink.terminal(&run("big")).await["type"], "run.completed");
        let result: Value = serde_json::from_str(&sink.text(&run("big"))).unwrap();
        assert!(result["content"][0]["text"].as_str().unwrap().len() <= fox_mcp::MAX_TOOL_RESULT_BYTES + 512);
        runtime.start_turn(turn(&conversation, &run("refused"), "call read_knowledge_document {\"documentId\":\"d\"}", backend)).unwrap();
        assert_eq!(sink.terminal(&run("refused")).await["type"], "run.completed");
        let result: Value = serde_json::from_str(&sink.text(&run("refused"))).unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().starts_with("[local_knowledge.binding_invalid]"));

        // 5. ask_user: the question card, and the turn completes awaiting
        //    the user even though the Agent wrote no text.
        runtime.start_turn(turn(&conversation, &run("ask"), "ask", backend)).unwrap();
        let terminal = sink.terminal(&run("ask")).await;
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert_eq!(terminal["completionReason"], "awaiting_user");
        assert!(sink.events(&run("ask")).iter().any(|event| event["type"] == "user.question.requested"));

        // 6. Stop: the launch's token is revoked; a stale token is refused.
        assert_eq!(bridge.live_tokens(), 1, "one live launch");
        runtime.close_conversation(&conversation, "test done").await;
        assert_eq!(bridge.live_tokens(), 0, "revoked with the process");
    }
    let stale = launcher.tokens.lock().unwrap()[0].clone();
    let stream = tokio::net::TcpStream::connect(bridge.endpoint()).await.unwrap();
    let (read, mut write) = stream.into_split();
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    write.write_all(format!("{}\n", json!({ "foxMcpToken": stale })).as_bytes()).await.unwrap();
    let mut lines = tokio::io::BufReader::new(read).lines();
    let refusal: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
    assert_eq!(refusal["error"]["code"], fox_mcp::rpc_code::TOKEN_REFUSED);
    runtime.shutdown().await;
}

/// Plan C5 (Linux): pi reaches the Fox tools through the C3 gate, which loads
/// the real `fox-mcp-tools.ts` (`FOX_PI_MCP_EXTENSION`) after the approval
/// gate; the extension connects to the same bridge with the launch's token.
/// Unix only: the launch is `sh fixtures/pi/pi-gated.sh` (Windows runs the
/// `.cmd` twin, covered by the 13.32 Windows checks).
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn pi_loads_the_fox_tool_extension_through_the_gate_and_uses_the_bridge() {
    let Some(node) = node() else {
        eprintln!("SKIPPED: node not installed");
        return;
    };
    // fox-mcp-tools.ts is TypeScript: Node runs it with type stripping (22.18+ default).
    let strips = std::process::Command::new(&node).args(["-e", "process.exit(process.features.typescript ? 0 : 1)"]).status().map(|status| status.success()).unwrap_or(false);
    if !strips {
        eprintln!("SKIPPED: this Node cannot strip TypeScript types");
        return;
    }
    let host = RecordingHost::default();
    let bridge = McpBridge::start(Arc::new(host.clone()), &tokio::runtime::Handle::current()).unwrap();
    let state = tempfile::tempdir().unwrap();
    let launcher = BridgeLauncher { bridge: bridge.clone(), state_root: state.path().to_path_buf(), tokens: Arc::default() };
    let sink = Sink::default();
    let runtime = ExternalRuntime::new(sink.clone(), launcher.clone(), tokio::runtime::Handle::current());
    let _ = host.runtime.set(runtime.clone());
    let conversation = "conv-pi";

    // 1. The gate gave pi the approval gate and then the Fox tool extension.
    runtime.start_turn(turn(conversation, "pi-args", "args", AgentBackend::Pi)).unwrap();
    assert_eq!(sink.terminal("pi-args").await["type"], "run.completed");
    let args: Value = serde_json::from_str(&sink.text("pi-args")).unwrap();
    assert_eq!(args["extensions"], json!(["fox-approval-gate.ts", fox_mcp::PI_EXTENSION_FILE]), "{args}");

    // 2. The extension registered exactly the bridge's tools with pi.
    runtime.start_turn(turn(conversation, "pi-tools", "tools", AgentBackend::Pi)).unwrap();
    assert_eq!(sink.terminal("pi-tools").await["type"], "run.completed");
    let tools: Vec<String> = serde_json::from_str(&sink.text("pi-tools")).unwrap();
    assert_eq!(tools, fox_mcp::TOOL_NAMES);

    // 3. A pi tool call reaches the Host bound to this conversation and turn.
    let call = r#"call present_files {"files":[{"path":"report.docx"}],"conversationId":"someone-else"}"#;
    runtime.start_turn(turn(conversation, "pi-present", call, AgentBackend::Pi)).unwrap();
    assert_eq!(sink.terminal("pi-present").await["type"], "run.completed");
    let result: Value = serde_json::from_str(&sink.text("pi-present")).unwrap();
    let payload: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["run"], "pi-present", "the Host saw the active turn");
    let (context, tool, _) = host.calls.lock().unwrap().last().cloned().unwrap();
    assert_eq!((context.conversation_id.as_str(), context.backend, tool.as_str()), (conversation, AgentBackend::Pi, "present_files"));

    // 4. A Host refusal is thrown from execute (pi reports it as a tool error).
    runtime.start_turn(turn(conversation, "pi-refused", "call read_knowledge_document {\"documentId\":\"d\"}", AgentBackend::Pi)).unwrap();
    assert_eq!(sink.terminal("pi-refused").await["type"], "run.completed");
    assert!(sink.text("pi-refused").starts_with("tool-error:") && sink.text("pi-refused").contains("local_knowledge.binding_invalid"), "{}", sink.text("pi-refused"));

    // 5. ask_user ends the turn awaiting the user.
    runtime.start_turn(turn(conversation, "pi-ask", "ask", AgentBackend::Pi)).unwrap();
    let terminal = sink.terminal("pi-ask").await;
    assert_eq!(terminal["completionReason"], "awaiting_user", "{terminal}");
    assert!(sink.events("pi-ask").iter().any(|event| event["type"] == "user.question.requested"));

    // 6. Stop: the token is revoked with the process.
    assert_eq!(bridge.live_tokens(), 1);
    runtime.close_conversation(conversation, "test done").await;
    assert_eq!(bridge.live_tokens(), 0);
    runtime.shutdown().await;
}
