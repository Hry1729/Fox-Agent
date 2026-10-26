#![cfg(all(feature = "acp", windows))]

use fox_agent_backend::{
    acp::AcpConnection, capability_view::PiLaunch, external::*, launch::LaunchSpec, network::*,
    port::AgentBackend, projection::PermissionCard, sandbox::*,
};
use futures::future::BoxFuture;
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
struct Launcher {
    runtime: PathBuf,
}
impl ExternalLauncher for Launcher {
    fn desired_mode(&self, _: &LaunchRequest) -> PermissionMode {
        PermissionMode::Sandboxed
    }
    fn launch(&self, _: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        Box::pin(async { panic!("network callback must be wired") })
    }
    fn launch_with_network(
        &self,
        request: LaunchRequest,
        callback: NetworkCallback,
    ) -> BoxFuture<'static, Result<Launched, LaunchFailure>> {
        let this = self.clone();
        Box::pin(async move {
            use fox_agent_backend::{
                winacl::RealWindowsAcl,
                winancestor::{srt_acl_lock_path, AncestorAceCoordinator},
                winruntime::*,
            };
            let state = std::env::var_os("FOX_TEST_OUTPUT_ROOT").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA")))
                .join("FoxSandboxFixtures")
                .join(format!("network-preview-{}", std::process::id()))
                .join("state");
            std::fs::create_dir_all(&state).unwrap();
            let registry = read_srt_registry(&RealProbe)?;
            let known = record_srt_sid(&state, &registry)?;
            let layout = WindowsPrivateLayout::for_current_user(&RealWindowsAcl)?;
            prepare_windows_private_root(&layout, &RealWindowsAcl, &RealWindowsAcl, &known)?;
            let acl = Arc::new(RealWindowsAcl);
            let coordinator = Arc::new(
                AncestorAceCoordinator::new(state.clone(), acl.clone(), acl)
                    .with_srt_acl_lock(srt_acl_lock_path()),
            );
            let install = SrtInstall::under_prefix(
                this.runtime.join("node/node.exe"),
                &this.runtime.join("srt"),
            );
            let ready = preflight(&install, &RealProbe)?;
            let launcher = SrtLauncher::new(ready, state)
                .with_managed_runtime_root(this.runtime.clone())
                .with_tmp_root(layout.tmp_root())
                .with_srt_sandbox_sid(registry.sandbox_user_sid.clone());
            let home = PrivateHomes::new(layout.homes_root())
                .home_for(request.project_uuid.as_deref().unwrap(), AgentBackend::Pi)?;
            let policy = SandboxPolicy::new(
                vec![AuthorizedDir {
                    path: request.project_root.clone(),
                    writable: true,
                }],
                home,
                vec![],
                vec!["unused.fixture.invalid".into()],
                HostOs::Windows,
            );
            let spec = LaunchSpec {
                backend: AgentBackend::Pi,
                program: this.runtime.join("node/node.exe").display().to_string(),
                args: vec![request.project_root.join("agent.mjs").display().to_string()],
                version_pin: "fixture",
                credential_env: &[],
                experimental: false,
                session_env: vec![(
                    "FOX_TEST_NPM_CLI".into(),
                    request
                        .project_root
                        .join(".tools/npm/bin/npm-cli.js")
                        .display()
                        .to_string(),
                )],
            };
            let mut prepared = prepare_launch(
                PermissionMode::Sandboxed,
                spec,
                &request.session_key,
                SandboxSetup::Ready {
                    launcher: &launcher,
                    policy: &policy,
                },
                HostOs::Windows,
            )?;
            let broker = NetworkBroker::attach(&mut prepared, callback)
                .map_err(|error| LaunchFailure::new("broker", error))?;
            let declaration = prepared.declaration.clone();
            let handle = tokio::runtime::Handle::current();
            let coordinator_spawn = coordinator.clone();
            let session_key = request.session_key.clone();
            let connection = tokio::task::spawn_blocking(move || {
                let mut transition = coordinator_spawn.transition()?;
                transition.begin_session(
                    &session_key,
                    &registry.sandbox_user_sid,
                    &prepared.windows_ancestor_aces,
                )?;
                let result = handle
                    .block_on(AcpConnection::spawn_prepared_detailed(
                        prepared,
                        &request.project_root,
                    ))
                    .map_err(LaunchFailure::from);
                if result.is_err() {
                    let _ = transition.end_session(&session_key);
                }
                transition.finish();
                result
            })
            .await
            .unwrap()?;
            Ok(Launched {
                connection,
                mode: PermissionMode::Sandboxed,
                declaration,
                pi_launch: PiLaunch::Stock,
                on_closed: Some(Box::new(move || {
                    drop(broker);
                    if let Ok(mut transition) = coordinator.transition() {
                        let _ = transition.end_session(&request.session_key);
                        transition.finish();
                    }
                })),
            })
        })
    }
}

#[derive(Clone, Default)]
struct Sink {
    events: Arc<Mutex<Vec<(String, Value)>>>,
    approvals: Arc<Mutex<Vec<String>>>,
    network_rules: Arc<Mutex<Vec<NetworkTarget>>>,
}
impl ExternalSink for Sink {
    fn emit(&self, run: &str, _: &str, event: Value) -> Result<(), String> {
        self.events.lock().unwrap().push((run.into(), event));
        Ok(())
    }
    fn request_approval(
        &self,
        _: &str,
        _: &str,
        key: &str,
        card: &PermissionCard,
    ) -> Result<String, String> {
        println!("approval {key}: {}", card.input);
        self.approvals.lock().unwrap().push(key.into());
        Ok(key.into())
    }
    fn network_granted(&self, _: &str, target: &NetworkTarget) -> bool {
        self.network_rules.lock().unwrap().contains(target)
    }
}
impl Sink {
    async fn terminal(&self, run: &str) -> String {
        for _ in 0..600 {
            {
                let events = self.events.lock().unwrap();
                if let Some((_, event)) = events.iter().find(|(id, event)| {
                    id == run
                        && matches!(
                            event["type"].as_str(),
                            Some("run.completed" | "run.failed" | "run.interrupted")
                        )
                }) {
                    assert_eq!(
                        event["type"],
                        "run.completed",
                        "run={run} {event}; approvals={:?}",
                        self.approvals.lock().unwrap()
                    );
                    let text: String = events
                        .iter()
                        .filter(|(id, event)| id == run && event["type"] == "message.delta")
                        .filter_map(|(_, event)| event["delta"].as_str())
                        .collect();
                    println!("completed {run}: {text}");
                    return text;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no terminal: {:?}", self.events.lock().unwrap());
    }
    async fn approval(&self, count: usize) -> String {
        for _ in 0..300 {
            if let Some(id) = self.approvals.lock().unwrap().get(count - 1) {
                return id.clone();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "missing approval {count}; events={:?}",
            self.events.lock().unwrap()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires installed Windows srt; FOX_TEST_REAL_SRT=1 --include-ignored"]
async fn real_windows_network_approval_and_preview_lifecycle() {
    assert_eq!(std::env::var("FOX_TEST_REAL_SRT").as_deref(), Ok("1"));
    let base = std::env::var_os("FOX_TEST_OUTPUT_ROOT").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.test-target"))
        .join(format!("network-preview-{}", std::process::id()));
    let root = base.join("项目 with spaces");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("agent.mjs"),
        include_bytes!("fixtures/network-preview-agent.mjs"),
    )
    .unwrap();
    std::fs::write(base.join("outside-canary.txt"), "HOST_CANARY").unwrap();
    let previous = std::env::var_os("FOX_TEST_PREVIOUS_PROJECT").map(PathBuf::from);
    if let Some(previous) = &previous {
        assert_eq!(std::fs::read_to_string(previous.join("inside-write.txt")).unwrap(), "PROJECT_WRITE");
        std::fs::write(root.join("previous-project.json"), serde_json::to_vec(&json!({ "target": previous.join("inside-write.txt") })).unwrap()).unwrap();
    }
    // Reproduce ambient Users-group write permission on this fixture only.
    let acl = std::process::Command::new("icacls.exe")
        .arg(&base).args(["/grant", "*S-1-5-32-545:(OI)(CI)M"]).output().unwrap();
    assert!(acl.status.success(), "{}", String::from_utf8_lossy(&acl.stderr));
    if std::env::var("FOX_TEST_NPM_NETWORK").as_deref() == Ok("1") {
        fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
            std::fs::create_dir_all(destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                assert!(!kind.is_symlink(), "npm test source must not contain links");
                if kind.is_dir() {
                    copy_tree(&entry.path(), &destination.join(entry.file_name()));
                } else {
                    std::fs::copy(entry.path(), destination.join(entry.file_name())).unwrap();
                }
            }
        }
        let source = PathBuf::from(
            std::env::var_os("FOX_TEST_NPM_SOURCE")
                .expect("FOX_TEST_NPM_SOURCE must name the npm directory staged for the tested runtime"),
        );
        copy_tree(&source, &root.join(".tools/npm"));
    }
    let runtime_root = std::env::var_os("FOX_AGENT_RUNTIME_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("C:/Program Files/Fox/sandbox-runtime/{}", fox_agent_backend::launch::RUNTIME_PACKAGE_VERSION)));
    let launcher = Launcher {
        runtime: runtime_root,
    };
    let sink = Sink::default();
    let runtime = ExternalRuntime::new(sink.clone(), launcher, tokio::runtime::Handle::current());
    let uuid = format!("fb7312ca-8621-4d17-b309-{:012x}", std::process::id());
    let request = |run: &str, prompt: &str| TurnRequest {
        run_id: run.into(),
        prompt: prompt.into(),
        stored_native_session: None,
        launch: LaunchRequest {
            conversation_id: "preview-fixture".into(),
            backend: AgentBackend::Pi,
            project_uuid: Some(uuid.clone()),
            project_root: root.clone(),
            session_key: format!("preview-{}", std::process::id()),
            full_access_granted: false,
            unified_login: None,
            native_permission: None,
        },
    };
    runtime
        .start_turn(request("start", "start-preview"))
        .unwrap();
    let output = sink.terminal("start").await;
    let server: Value = serde_json::from_str(&output).unwrap();
    let port = server["preview"]["port"].as_u64().unwrap() as u16;
    let previews = runtime.previews("preview-fixture").unwrap();
    println!("server={server} previews={previews:?}");
    let preview = previews
        .iter()
        .find(|preview| preview.port == port)
        .expect("Host must verify the sandbox listener's Job membership")
        .clone();
    let host = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    drop(host);
    runtime.start_turn(request("http", "own-http")).unwrap();
    let http: Value = serde_json::from_str(&sink.terminal("http").await).unwrap();
    assert_eq!(http["status"], 200);
    runtime
        .start_turn(request("websocket", "own-websocket"))
        .unwrap();
    let websocket: Value = serde_json::from_str(&sink.terminal("websocket").await).unwrap();
    assert_eq!(websocket["websocket"], true);
    runtime
        .start_turn(request("direct", "direct-loopback"))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&sink.terminal("direct").await).unwrap()["direct"],
        false
    );
    assert!(
        sink.approvals.lock().unwrap().is_empty(),
        "only owned preview ports are automatic"
    );
    let foreign = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let foreign_port = foreign.local_addr().unwrap().port();
    let foreign_server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = foreign.accept().await.unwrap();
        let mut buffer = [0u8; 2048];
        socket.read(&mut buffer).await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nforeign")
            .await
            .unwrap();
    });
    runtime
        .start_turn(request("deny", &format!("foreign:{foreign_port}")))
        .unwrap();
    let id = sink.approval(1).await;
    assert_eq!(runtime.answer(&id, false), AnswerOutcome::Written);
    assert_eq!(
        serde_json::from_str::<Value>(&sink.terminal("deny").await).unwrap()["status"],
        403
    );
    runtime
        .start_turn(request("allow", &format!("foreign:{foreign_port}")))
        .unwrap();
    let id = sink.approval(2).await;
    assert_eq!(runtime.answer(&id, true), AnswerOutcome::Written);
    assert_eq!(
        serde_json::from_str::<Value>(&sink.terminal("allow").await).unwrap()["status"],
        200
    );
    foreign_server.await.unwrap();
    let install = if std::env::var("FOX_TEST_NPM_NETWORK").as_deref() == Ok("1") {
        runtime
            .start_turn(request("dependency", "install-dependency"))
            .unwrap();
        let id = sink.approval(3).await;
        sink.network_rules.lock().unwrap().push(NetworkTarget {
            host: "registry.npmjs.org".into(),
            port: 443,
        });
        assert_eq!(runtime.answer(&id, true), AnswerOutcome::Written);
        let result: Value = serde_json::from_str(&sink.terminal("dependency").await).unwrap();
        assert_eq!(result["code"], 0, "{result}");
        assert_eq!(result["installed"], "7.0.0", "{result}");
        result
    } else {
        json!({ "notRun": true })
    };
    runtime
        .start_turn(request("filesystem", "filesystem-boundary"))
        .unwrap();
    let filesystem: Value = serde_json::from_str(&sink.terminal("filesystem").await).unwrap();
    assert_eq!(filesystem["outsideWriteBlocked"], true, "{filesystem}");
    assert_eq!(filesystem["insideWriteSucceeded"], true, "{filesystem}");
    assert_eq!(std::fs::read_to_string(base.join("outside-canary.txt")).unwrap(), "HOST_CANARY");
    if let Some(previous) = &previous {
        assert_eq!(filesystem["previousProjectWriteBlocked"], true, "{filesystem}");
        assert_eq!(std::fs::read_to_string(previous.join("inside-write.txt")).unwrap(), "PROJECT_WRITE");
    }
    let hold = std::env::var("FOX_PREVIEW_HOLD_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
        .min(600);
    std::fs::write(
        base.join("preview-ready.json"),
        serde_json::to_vec_pretty(&preview).unwrap(),
    )
    .unwrap();
    println!("PREVIEW_READY {} evidence={}", preview.url, base.display());
    for _ in 0..hold * 2 {
        if base.join("browser-done").exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    runtime
        .stop_preview("preview-fixture", &preview)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
    assert!(runtime
        .stop_preview("preview-fixture", &preview)
        .await
        .is_err());
    let reports = runtime.shutdown().await;
    assert!(reports
        .iter()
        .all(|(_, report)| report.confirmation.is_confirmed()));
    std::fs::write(base.join("result.json"), serde_json::to_vec_pretty(&json!({ "http": http["status"], "websocket": true, "directLoopbackBlocked": true, "foreignDenied": true, "explicitGrant": true, "dependencyInstall": install, "filesystemObservation": filesystem, "stopConfirmed": true, "preview": preview })).unwrap()).unwrap();
}
