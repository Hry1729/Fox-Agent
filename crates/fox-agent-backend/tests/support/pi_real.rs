//! Shared by the real pi + pi-acp tests (`pi_real_fox_tools.rs`,
//! `pi_real_no_console_window.rs`): the production-shaped launcher, the sink,
//! the recording Fox tool host. See `pi_real_fox_tools.rs` for the chain.
#![allow(dead_code)]

use fox_agent_backend::acp::AcpConnection;
use fox_agent_backend::capability_view::PiLaunch;
use fox_agent_backend::external::{ExternalLauncher, ExternalRuntime, ExternalSink, HostPermissionAnswer, LaunchFailure, LaunchRequest, Launched, TurnRequest};
use fox_agent_backend::external_home::{LaunchDeclaration, PermissionMode};
use fox_agent_backend::fox_mcp::{self, FoxToolHost, McpBridge, McpInjection, ToolCallContext, ToolError};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::{AutoPermission, PermissionCard, PermissionOption};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub fn runtime_dir(test: &str) -> Option<PathBuf> {
    let dir = std::env::var_os("FOX_PI_RUNTIME_DIR").map(PathBuf::from);
    let usable = dir.as_ref().filter(|dir| dir.join(shim("backends/pi/node_modules/.bin/pi-acp")).is_file() && dir.join(shim("backends/pi/node_modules/.bin/pi")).is_file());
    match usable {
        Some(dir) => Some(dir.clone()),
        None if std::env::var("FOX_REQUIRE_REAL_PI").as_deref() == Ok("1") => {
            panic!("{test}: FOX_REQUIRE_REAL_PI=1 but FOX_PI_RUNTIME_DIR ({dir:?}) has no backends/pi/node_modules/.bin/{{pi-acp,pi}}")
        }
        None => {
            eprintln!("SKIPPED {test}: set FOX_PI_RUNTIME_DIR to a runtime with backends/pi (npm ci of the pinned lockfile) to run real pi + pi-acp (skip is not a pass)");
            None
        }
    }
}

/// npm's command shim: `<name>.cmd` on Windows.
pub fn shim(path: &str) -> String {
    if cfg!(windows) { format!("{path}.cmd") } else { path.to_owned() }
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pi")
}

#[derive(Clone, Default)]
pub struct Sink(Arc<Mutex<Vec<(String, Value)>>>);

impl ExternalSink for Sink {
    fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
        self.0.lock().unwrap().push((run_id.to_owned(), event));
        Ok(())
    }
    fn request_approval(&self, _run_id: &str, _conversation_id: &str, _key: &str, _card: &PermissionCard) -> Result<String, String> {
        Err("every gate request is answered by host_permission_answer in this test".into())
    }
    // The Host's approval engine stands in as "allow": the gate's request
    // for each tool call (pi has no permission system of its own).
    fn host_permission_answer(&self, _run_id: &str, _conversation_id: &str, _backend: AgentBackend, _native_mode: &str, _request: &Value, options: &[PermissionOption]) -> Option<HostPermissionAnswer> {
        let option_id = options.iter().find(|option| option.kind == "allow_once").map(|option| option.option_id.clone())?;
        Some(HostPermissionAnswer { decision: AutoPermission::Allow { option_id }, policy: json!({ "test": "allow" }) })
    }
}

impl Sink {
    pub fn events(&self, run: &str) -> Vec<Value> {
        self.0.lock().unwrap().iter().filter(|(id, _)| id == run).map(|(_, event)| event.clone()).collect()
    }
    pub fn text(&self, run: &str) -> String {
        self.events(run).iter().filter(|event| event["type"] == "message.delta").filter_map(|event| event["delta"].as_str().map(str::to_owned)).collect()
    }
    pub async fn terminal(&self, run: &str, within: Duration) -> Value {
        let deadline = Instant::now() + within;
        while Instant::now() < deadline {
            if let Some(event) = self.events(run).into_iter().find(|event| event["type"].as_str().is_some_and(|kind| matches!(kind, "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted"))) {
                return event;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("no terminal for {run} within {within:?}: {:?}", self.events(run));
    }
}

pub type Runtime = ExternalRuntime<Sink, RealPiLauncher>;

#[derive(Clone, Default)]
pub struct RecordingHost {
    pub calls: Arc<Mutex<Vec<(ToolCallContext, String, Value)>>>,
    pub runtime: Arc<OnceLock<Runtime>>,
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
                _ => Err(ToolError::new("local_knowledge.binding_invalid", "not enabled")),
            }
        })
    }
}

/// What the extension is pointed at.
#[derive(Clone)]
pub enum Wiring {
    /// The launch's own endpoint and token (production).
    Bridge,
    /// A loopback listener that accepts and never answers (a stuck Host).
    Endpoint(String),
    /// The right endpoint with a token the bridge never issued.
    WrongToken,
}

/// The production shape (fresh token, extension env from
/// `fox_mcp::injection_for`, revoked on close, probe handed to the runtime),
/// launching real pi-acp with the real gate and real pi.
#[derive(Clone)]
pub struct RealPiLauncher {
    pub bridge: McpBridge,
    pub state_root: PathBuf,
    pub runtime: PathBuf,
    pub wiring: Wiring,
}

pub fn node() -> PathBuf {
    let output = fox_agent_backend::console::HideConsole::hide_console(&mut std::process::Command::new("node")).args(["-e", "process.stdout.write(process.execPath)"]).output().expect("node");
    PathBuf::from(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl ExternalLauncher for RealPiLauncher {
    fn desired_mode(&self, _request: &LaunchRequest) -> PermissionMode {
        PermissionMode::Standard
    }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let launcher = self.clone();
        Box::pin(async move {
            let assets = fox_mcp::write_assets(&launcher.state_root).map_err(|error| LaunchFailure::new("agent_backend_fox_tools_unavailable", error))?;
            let grant = launcher.bridge.issue(&request.conversation_id, request.backend).map_err(|error| LaunchFailure::new("agent_backend_fox_tools_unavailable", error))?;
            let node = node();
            let Some(McpInjection::PiExtension { env }) = fox_mcp::injection_for(AgentBackend::Pi, &grant, &node, &assets) else {
                panic!("pi gets the extension")
            };
            let mut env: Vec<(String, String)> = env;
            match &launcher.wiring {
                Wiring::Bridge => {}
                Wiring::Endpoint(endpoint) => env.iter_mut().filter(|(key, _)| key == fox_mcp::ENDPOINT_ENV).for_each(|(_, value)| *value = endpoint.clone()),
                Wiring::WrongToken => env.iter_mut().filter(|(key, _)| key == fox_mcp::TOKEN_ENV).for_each(|(_, value)| *value = "0".repeat(64)),
            }
            // The gate and what it loads, as the runtime package lays them
            // out (pi-gate/). Unix: the checkout stores pi-gated.sh without its
            // executable bit, so pi-acp spawns a private executable copy.
            let gate = launcher.state_root.join("pi-gate");
            std::fs::create_dir_all(&gate).unwrap();
            let gate_script = if cfg!(windows) { "pi-gated.cmd" } else { "pi-gated.sh" };
            for file in [gate_script, "fox-approval-gate.ts"] {
                std::fs::copy(fixtures().join(file), gate.join(file)).unwrap();
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(gate.join(gate_script), std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let home = launcher.state_root.join(format!("home-{}", request.session_key));
            std::fs::create_dir_all(&home).unwrap();
            let bin = launcher.runtime.join("backends/pi/node_modules/.bin");
            let separator = if cfg!(windows) { ";" } else { ":" };
            let path = [bin.display().to_string(), node.parent().unwrap().display().to_string(), std::env::var("PATH").unwrap_or_default()].join(separator);
            let scripted = fixtures().join("scripted-provider.ts").display().to_string();
            env.extend([
                ("PI_ACP_PI_COMMAND".to_owned(), gate.join(gate_script).display().to_string()),
                ("FOX_PI_BIN".to_owned(), bin.join(shim("pi")).display().to_string()),
                ("HOME".to_owned(), home.display().to_string()),
                ("USERPROFILE".to_owned(), home.display().to_string()),
                ("PATH".to_owned(), path),
                // The scripted model listens on 127.0.0.1 inside pi.
                ("NO_PROXY".to_owned(), "127.0.0.1,localhost".to_owned()),
                ("no_proxy".to_owned(), "127.0.0.1,localhost".to_owned()),
                // Failure cases should not wait the production 20 s.
                ("FOX_MCP_INIT_TIMEOUT_MS".to_owned(), "4000".to_owned()),
            ]);
            // The scripted model: pi-gated.cmd quotes one test extension path
            // (FOX_PI_TEST_EXTENSION); FOX_PI_EXTRA_ARGS is word-split there.
            if cfg!(windows) {
                env.push(("FOX_PI_TEST_EXTENSION".to_owned(), scripted));
                env.push(("FOX_PI_EXTRA_ARGS".to_owned(), "--provider foxscripted --model scripted".to_owned()));
            } else {
                env.push(("FOX_PI_EXTRA_ARGS".to_owned(), format!("-e {scripted} --provider foxscripted --model scripted")));
            }
            let spec = LaunchSpec {
                backend: AgentBackend::Pi,
                program: bin.join(shim("pi-acp")).display().to_string(),
                args: vec![],
                version_pin: fox_agent_backend::launch::PI_ACP_VERSION,
                credential_env: &[],
                experimental: false,
                session_env: env,
            };
            let connection = AcpConnection::spawn_detailed(&spec, &request.project_root).await.map_err(LaunchFailure::from)?;
            let bridge = launcher.bridge.clone();
            let token = grant.token.clone();
            Ok(Launched {
                connection,
                mode: PermissionMode::Standard,
                declaration: LaunchDeclaration::declare(PermissionMode::Standard),
                pi_launch: PiLaunch::Gated,
                fox_tools: Some(launcher.bridge.probe(&grant.token)),
                on_closed: Some(Box::new(move || bridge.revoke(&token))),
            })
        })
    }
}

pub fn turn(conversation: &str, run: &str, prompt: &str, project_root: &Path) -> TurnRequest {
    TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: conversation.into(),
            backend: AgentBackend::Pi,
            project_uuid: None,
            project_root: project_root.to_path_buf(),
            session_key: format!("k-{run}"),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        },
    }
}

pub struct Setup {
    pub runtime: Runtime,
    pub host: RecordingHost,
    pub bridge: McpBridge,
    pub sink: Sink,
    pub project: tempfile::TempDir,
    pub _state: tempfile::TempDir,
}

pub fn setup(pi_runtime: PathBuf, wiring: Wiring) -> Setup {
    let host = RecordingHost::default();
    let bridge = McpBridge::start(Arc::new(host.clone()), &tokio::runtime::Handle::current()).unwrap();
    let state = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let launcher = RealPiLauncher { bridge: bridge.clone(), state_root: state.path().to_path_buf(), runtime: pi_runtime, wiring };
    let sink = Sink::default();
    let runtime = ExternalRuntime::new(sink.clone(), launcher, tokio::runtime::Handle::current());
    let _ = host.runtime.set(runtime.clone());
    Setup { runtime, host, bridge, sink, project, _state: state }
}

