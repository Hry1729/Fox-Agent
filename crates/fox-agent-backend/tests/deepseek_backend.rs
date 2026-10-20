//! DeepSeek Harness backend: pure unit tests over a recorded `initialize`
//! response plus a smoke test against the real `dsh --profile acp` binary.
//!
//! The real-binary test is gated by `FOX_TEST_REAL_ACP=1`. When the variable is
//! not set the test returns early: that is a SKIP, not a pass, and says nothing
//! about the real backend. Binary path: `FOX_DSH_BIN`, an absolute path to the
//! `dsh` entry of an `npm --prefix <dir> install @deepseek-ai/dsh@0.2.1-rc.2`
//! (`<dir>/node_modules/.bin/dsh`, on Windows `...\dsh.cmd`). Linux defaults
//! to `/workspace/work/tools/dsh/node_modules/.bin/dsh`; Windows has no
//! default and the test fails if the variable is missing or relative.

use fox_agent_backend::backends::deepseek::{
    self, capabilities, check_initialize, check_version, filesystem_detail, floor_verdict,
    host_permission_answer, policy_patch, FloorStatus, HostPolicy, SandboxMode, PINNED_VERSION,
};
use fox_agent_backend::port::{ApprovalGate, CancelReach, Enforcement, ToolCategory, Visibility};

/// `initialize` response recorded on the wire from dsh 0.2.1-rc.2
/// (`dsh --profile acp --patch <host patch>`) on 2026-10-19, Linux aarch64 (Debian 13)
/// sandbox, Node 22, no DEEPSEEK_API_KEY (raw JSON-RPC `result`, before any
/// client-side re-serialisation).
const RECORDED_INITIALIZE: &str = r#"{"protocolVersion":1,"agentInfo":{"name":"deepseek-harness-acp","version":"0.0.1"},"agentCapabilities":{"mcpCapabilities":{"http":true},"promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},"sessionCapabilities":{"close":{},"list":{},"resume":{}}},"authMethods":[]}"#;

/// The same response after `agent-client-protocol` 3.4.0 re-serialised it
/// (what `BackendEvent::Initialized` carries): defaults made explicit.
const CLIENT_SERIALISED_INITIALIZE: &str = r#"{"protocolVersion":1,"agentCapabilities":{"loadSession":false,"promptCapabilities":{"image":false,"audio":false,"embeddedContext":false},"mcpCapabilities":{"http":true,"sse":false},"sessionCapabilities":{"list":{},"resume":{},"close":{}},"auth":{}},"authMethods":[],"agentInfo":{"name":"deepseek-harness-acp","version":"0.0.1"}}"#;

fn recorded() -> serde_json::Value {
    serde_json::from_str(RECORDED_INITIALIZE).expect("fixture is JSON")
}

#[test]
fn version_check_accepts_only_the_pinned_version() {
    assert_eq!(PINNED_VERSION, "0.2.1-rc.2");
    assert!(check_version("0.2.1-rc.2\n").is_ok(), "exact `dsh --version` output");
    assert!(check_version("dsh 0.2.1-rc.2").is_ok());
    assert!(check_version("v0.2.1-rc.2").is_ok());
    for bad in ["0.2.1-rc.1", "0.2.1", "0.2.1-alpha.5", "0.1", "", "  ", "0.2.1-rc.2 extra", "0.2.1-rc.2-patched"] {
        assert!(check_version(bad).is_err(), "{bad:?} must be refused");
    }
}

#[test]
fn recorded_initialize_is_dsh_acp_v1() {
    let init = recorded();
    assert!(check_initialize(&init).is_ok());
    let mut other = init.clone();
    other["agentInfo"]["name"] = "something-else".into();
    assert!(check_initialize(&other).is_err());
    let mut v2 = init;
    v2["protocolVersion"] = 2.into();
    assert!(check_initialize(&v2).is_err());
}

#[test]
fn capabilities_follow_evidence_and_never_overclaim() {
    let declared = capabilities(&recorded());
    // Session capabilities from initialize: list/resume/close, no load.
    assert!(declared.session.list && declared.session.resume && declared.session.close);
    assert!(!declared.session.load, "dsh has no session/load");
    assert!(!declared.session.steer);
    // No category is gated per call in the acp composition.
    for category in [
        ToolCategory::FileRead,
        ToolCategory::FileWrite,
        ToolCategory::Shell,
        ToolCategory::Network,
        ToolCategory::McpBusinessTool,
    ] {
        assert_eq!(declared.gate(category), ApprovalGate::None, "{category:?}");
        assert!(!declared.ui_claims_per_call_approval(category));
    }
    assert_eq!(declared.sandbox_network, Enforcement::Absent);
    assert_ne!(declared.sandbox_filesystem, Enforcement::Enforced);
    assert_eq!(filesystem_detail().read_outside_workspace, Enforcement::Absent);
    assert_eq!(declared.cancel_turn, CancelReach::TurnRequested);
    assert_eq!(declared.observability, Visibility::Unverified);
    assert!(!declared.approval_answer_acknowledged);
}

#[test]
fn wire_and_client_serialised_initialize_declare_the_same() {
    let client: serde_json::Value = serde_json::from_str(CLIENT_SERIALISED_INITIALIZE).unwrap();
    assert!(check_initialize(&client).is_ok());
    assert_eq!(capabilities(&client), capabilities(&recorded()));
}

#[test]
fn capabilities_default_to_unsupported_on_empty_initialize() {
    let declared = capabilities(&serde_json::json!({"protocolVersion": 1}));
    assert!(!declared.session.list && !declared.session.resume && !declared.session.close && !declared.session.load);
}

#[test]
fn policy_patch_maps_host_policy_without_widening() {
    let ro = policy_patch(&HostPolicy { workspace_write: false, network: false });
    assert_eq!(ro.mode, SandboxMode::ReadOnly);
    assert!(ro.patch_yaml.contains("mode: read-only"));
    assert!(ro.patch_yaml.contains("policy: never"));
    assert!(ro.patch_yaml.contains("defaultPreset: read-only"));
    assert!(ro.patch_yaml.contains("mode: DISABLED"));
    assert!(ro.patch_yaml.contains("- id: tool-web\n  disabled: true"));
    assert!(!ro.patch_yaml.contains("danger-full-access"));
    assert!(ro.unenforced.iter().any(|gap| gap.contains("network")));
    assert!(ro.unenforced.iter().any(|gap| gap.contains("reads")));

    let rw = policy_patch(&HostPolicy { workspace_write: true, network: true });
    assert_eq!(rw.mode, SandboxMode::WorkspaceWrite);
    assert!(rw.patch_yaml.contains("mode: workspace-write"));
    assert!(!rw.patch_yaml.contains("tool-web"));
    assert!(!rw.patch_yaml.contains("danger-full-access"));
}

#[test]
fn launch_spec_requires_absolute_patch_and_pins_version() {
    assert!(deepseek::launch_spec("dsh", std::path::Path::new("rel.yml")).is_err());
    // Absolute on the host running the test: `/abs/...` is not absolute on
    // Windows (no drive), so build the path from the temp directory.
    let patch = std::env::temp_dir().join("fox-dsh.yml");
    assert!(patch.is_absolute());
    let spec = deepseek::launch_spec("dsh", &patch).unwrap();
    assert_eq!(spec.args, ["--profile".to_owned(), "acp".to_owned(), "--patch".to_owned(), patch.display().to_string()]);
    #[cfg(windows)]
    assert!(deepseek::launch_spec("dsh", std::path::Path::new("/abs/fox-dsh.yml")).is_err(), "drive-less path is relative on Windows");
    assert_eq!(spec.version_pin, PINNED_VERSION);
    assert_eq!(spec.credential_env, ["DEEPSEEK_API_KEY"]);
    assert!(spec.experimental);
}

#[test]
fn permission_requests_are_only_ever_rejected() {
    let offered = vec!["allow-once".to_string(), "reject-once".to_string()];
    assert_eq!(host_permission_answer(&offered), Some("reject-once"));
    assert_eq!(host_permission_answer(&["allow-once".to_string()]), None, "never picks allow");
}

#[test]
fn floor_verdict_keeps_backend_disabled() {
    let verdict = floor_verdict();
    assert!(matches!(verdict.t04_isolation, FloorStatus::NotMet { .. }));
    assert!(!verdict.may_enable());
}

#[cfg(feature = "acp")]
mod real_binary {
    use super::*;
    use fox_agent_backend::acp::{AcpConnection, BackendEvent};
    use std::time::Duration;

    fn dsh_bin() -> String {
        let bin = match std::env::var("FOX_DSH_BIN") {
            Ok(bin) => bin,
            Err(_) if cfg!(windows) => {
                panic!("FOX_DSH_BIN must be set on Windows: absolute path to dsh.cmd of @deepseek-ai/dsh@0.2.1-rc.2")
            }
            Err(_) => "/workspace/work/tools/dsh/node_modules/.bin/dsh".into(),
        };
        assert!(std::path::Path::new(&bin).is_absolute(), "FOX_DSH_BIN must be an absolute path, got {bin:?}");
        bin
    }

    fn enabled() -> bool {
        std::env::var("FOX_TEST_REAL_ACP").as_deref() == Ok("1")
    }

    async fn next(connection: &mut AcpConnection, secs: u64) -> Option<BackendEvent> {
        tokio::time::timeout(Duration::from_secs(secs), connection.events.recv()).await.ok().flatten()
    }

    /// Real `dsh --profile acp --patch <host patch>`: version, initialize,
    /// session/new, and a prompt. Without DEEPSEEK_API_KEY the prompt must end
    /// in a failure, never a fabricated success.
    #[tokio::test(flavor = "multi_thread")]
    async fn real_dsh_initialize_session_and_prompt() {
        if !enabled() {
            eprintln!("SKIPPED: set FOX_TEST_REAL_ACP=1 to run against the real dsh binary (not a pass)");
            return;
        }
        let bin = dsh_bin();
        let version = std::process::Command::new(&bin).arg("--version").output().expect("run dsh --version");
        let version = String::from_utf8_lossy(&version.stdout).to_string();
        eprintln!("dsh --version: {}", version.trim());
        check_version(&version).expect("pinned dsh version");

        let work = std::env::temp_dir().join(format!("fox-dsh-smoke-{}", std::process::id()));
        std::fs::create_dir_all(&work).unwrap();
        let mapping = policy_patch(&HostPolicy { workspace_write: false, network: false });
        let patch = work.join("fox-dsh-patch.yml");
        std::fs::write(&patch, &mapping.patch_yaml).unwrap();
        let spec = deepseek::launch_spec(&bin, &patch).unwrap();

        let mut connection = AcpConnection::spawn(&spec, &work).await.expect("spawn dsh");
        let init = match next(&mut connection, 60).await {
            Some(BackendEvent::Initialized { initialize_response, .. }) => initialize_response,
            other => panic!("expected Initialized, got {other:?}"),
        };
        eprintln!("initialize: {}", serde_json::to_string(&init).unwrap());
        check_initialize(&init).expect("dsh acp initialize");
        let declared = capabilities(&init);
        assert!(declared.session.list && declared.session.resume && declared.session.close);
        assert!(!declared.session.load);

        let session = connection.new_session(work.clone()).await.expect("session/new");
        eprintln!("session/new id: {session}");
        assert!(!session.is_empty());

        connection.prompt(&session, "Reply with the single word: pong").unwrap();
        // A model is expected to answer when a key is present or the tester
        // says the private HOME configures one (e.g. a local gateway through
        // the `acp` row, see deepseek::USER_OWNED_ROWS):
        // FOX_TEST_DSH_EXPECT_REPLY=1.
        let has_key = std::env::var("DEEPSEEK_API_KEY").map(|key| !key.is_empty()).unwrap_or(false)
            || std::env::var("FOX_TEST_DSH_EXPECT_REPLY").as_deref() == Ok("1");
        let mut outcome = None;
        let mut reply = String::new();
        while let Some(event) = next(&mut connection, 120).await {
            match event {
                BackendEvent::TurnEnded { stop_reason, .. } => {
                    outcome = Some(Ok(stop_reason));
                    break;
                }
                BackendEvent::Failed(error) | BackendEvent::TurnFailed { error, .. } => {
                    outcome = Some(Err(error));
                    break;
                }
                BackendEvent::PermissionRequested { native_request_id, option_ids, .. } => {
                    // Under the Host patch dsh should never ask; reject if it does.
                    if let Some(answer) = host_permission_answer(&option_ids) {
                        let _ = connection.answer_permission(&native_request_id, Some(answer));
                    }
                }
                BackendEvent::Update { update, .. } => {
                    eprintln!("update: {update}");
                    if update.get("sessionUpdate").and_then(|kind| kind.as_str()) == Some("agent_message_chunk") {
                        if let Some(text) = update.pointer("/content/text").and_then(|text| text.as_str()) {
                            reply.push_str(text);
                        }
                    }
                }
                BackendEvent::Closed => break,
                BackendEvent::Initialized { .. } | BackendEvent::PermissionAutoCancelled { .. } => {}
            }
        }
        eprintln!("prompt outcome (key present: {has_key}): {outcome:?}");
        if !has_key {
            assert!(
                matches!(outcome, Some(Err(_))),
                "without DEEPSEEK_API_KEY the prompt must fail, got {outcome:?}"
            );
        } else {
            // A refused model (wrong provider routing, rejected key) used to
            // pass here; it must now fail the test.
            match &outcome {
                Some(Ok(stop_reason)) => assert_eq!(stop_reason, &serde_json::json!("end_turn"), "turn did not end normally"),
                other => panic!("expected end_turn with a reply, got {other:?}"),
            }
            assert!(!reply.trim().is_empty(), "end_turn without any agent text (model refused or empty)");
            eprintln!("reply: {}", reply.trim());
        }
        connection.close().await;
        let _ = std::fs::remove_dir_all(&work);
    }
}
