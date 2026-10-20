//! CodexBackend (codex-acp) tests.
//!
//! * Unit tests: capability mapping from the `initialize` response recorded
//!   against the real `@agentclientprotocol/codex-acp@2.2.2` (see docs/codex.md).
//! * `real_codex_acp_smoke_skips_unless_fox_test_real_acp`: starts the real adapter
//!   (`FOX_CODEX_ACP_PROGRAM`, else the managed runtime, else npx; see
//!   `launch::dev_launch_spec`). Gated by
//!   `FOX_TEST_REAL_ACP=1`; without it the test returns immediately and prints
//!   "SKIPPED" - that is a skip, not a pass (plan section 2).

use fox_agent_backend::backends::codex::{
    capabilities_for, check_initialize, mode_for_policy, session_env, set_mode_params, steer_supported,
    steering_params, verify_session_mode, CodexMode, HostPolicy, Platform,
};
use fox_agent_backend::port::{ApprovalGate, CancelReach, Enforcement, PortError, ToolCategory, Visibility};
use serde_json::{json, Value};

/// Raw `initialize` result from codex-acp 2.2.2, Linux aarch64, 2026-10-19
/// (client sent protocolVersion 1, fs read/write false, terminal false).
const RECORDED_INITIALIZE: &str = r#"{"protocolVersion":1,"agentInfo":{"name":"@agentclientprotocol/codex-acp","title":"Codex","version":"2.2.2"},"agentCapabilities":{"auth":{"logout":{}},"providers":{},"loadSession":true,"promptCapabilities":{"embeddedContext":true,"image":true},"sessionCapabilities":{"resume":{},"list":{},"close":{},"delete":{},"fork":{},"additionalDirectories":{},"subagents":{}},"mcpCapabilities":{"acp":false,"http":true,"sse":false},"_meta":{"authStatus":{}}},"authMethods":[{"id":"api-key","name":"API Key","description":"Use an API key to authenticate","_meta":{"api-key":{"provider":"openai"}}},{"id":"chat-gpt","name":"ChatGPT","description":"Use ChatGPT to authenticate"}],"_meta":{"steering":{"supported":true}}}"#;

fn recorded() -> Value {
    serde_json::from_str(RECORDED_INITIALIZE).unwrap()
}

/// Trimmed `session/new` result recorded with INITIAL_AGENT_MODE unset
/// (codex-acp default) - models list omitted.
fn recorded_session_new_default() -> Value {
    json!({
        "sessionId": "01a15342-314f-75b0-853f-3743c3fd019e",
        "modes": {"availableModes": [
            {"id": "read-only"}, {"id": "workspace-write"}, {"id": "agent"}, {"id": "agent-full-access"}
        ], "currentModeId": "agent"},
        "configOptions": [{"id": "mode", "currentValue": "agent"}]
    })
}

#[test]
fn recorded_initialize_passes_startup_check_and_wrong_version_is_refused() {
    assert_eq!(check_initialize(&recorded()), Ok(()));
    let mut other = recorded();
    other["agentInfo"]["version"] = json!("2.2.3-preview.15");
    assert!(matches!(check_initialize(&other), Err(PortError::Refused(_))));
    let mut wrong_protocol = recorded();
    wrong_protocol["protocolVersion"] = json!(2);
    assert!(check_initialize(&wrong_protocol).is_err());
}

#[test]
fn session_capabilities_and_steer_come_from_recorded_initialize() {
    let declared = capabilities_for(&recorded(), CodexMode::ReadOnly, Platform::Linux);
    assert!(declared.session.load, "loadSession: true");
    assert!(declared.session.resume && declared.session.close && declared.session.list);
    assert!(declared.session.steer, "_meta.steering.supported");
    assert!(steer_supported(&recorded()));

    let mut bare = recorded();
    bare.as_object_mut().unwrap().remove("_meta");
    bare["agentCapabilities"]["sessionCapabilities"] = json!({});
    bare["agentCapabilities"]["loadSession"] = json!(false);
    let declared = capabilities_for(&bare, CodexMode::ReadOnly, Platform::Linux);
    assert!(!declared.session.steer && !declared.session.load && !declared.session.resume && !declared.session.close);
}

#[cfg(feature = "acp")]
#[test]
fn session_capabilities_agree_with_shared_acp_reading() {
    let shared = fox_agent_backend::acp::capabilities_from_initialize(&recorded()).session;
    let ours = capabilities_for(&recorded(), CodexMode::ReadOnly, Platform::Linux).session;
    assert_eq!((shared.load, shared.resume, shared.close, shared.list), (ours.load, ours.resume, ours.close, ours.list));
}

#[test]
fn no_category_claims_per_call_approval_without_a_live_measurement() {
    for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
        for mode in [CodexMode::ReadOnly, CodexMode::WorkspaceWrite, CodexMode::AutoReview, CodexMode::FullAccess] {
            let declared = capabilities_for(&recorded(), mode, platform);
            for category in [
                ToolCategory::FileRead,
                ToolCategory::FileWrite,
                ToolCategory::Shell,
                ToolCategory::Network,
                ToolCategory::McpBusinessTool,
            ] {
                assert!(!declared.ui_claims_per_call_approval(category), "{mode:?} {category:?}");
            }
            assert!(!declared.approval_answer_acknowledged);
            assert_eq!(declared.cancel_turn, CancelReach::TurnRequested, "cancel confirmation not measured");
        }
    }
}

#[test]
fn read_only_mode_declaration() {
    let declared = capabilities_for(&recorded(), CodexMode::ReadOnly, Platform::Linux);
    assert_eq!(declared.gate(ToolCategory::FileRead), ApprovalGate::None, "reads are never asked");
    assert_eq!(declared.gate(ToolCategory::FileWrite), ApprovalGate::Unverified);
    assert_eq!(declared.gate(ToolCategory::Shell), ApprovalGate::None, "on-request: in-sandbox commands run unasked");
    assert_eq!(declared.gate(ToolCategory::Network), ApprovalGate::None, "server-side web search is not Host-approved");
    assert_eq!(declared.gate(ToolCategory::McpBusinessTool), ApprovalGate::Unverified);
    assert_eq!(declared.observability, Visibility::ToolLifecycle);
    assert!(declared.cancel_process_tree);
}

#[test]
fn workspace_write_runs_writes_inside_root_without_approval() {
    let declared = capabilities_for(&recorded(), CodexMode::WorkspaceWrite, Platform::Linux);
    assert_eq!(declared.gate(ToolCategory::FileWrite), ApprovalGate::None);
}

#[test]
fn sandbox_is_enforced_only_where_measured() {
    let linux = capabilities_for(&recorded(), CodexMode::ReadOnly, Platform::Linux);
    assert_eq!((linux.sandbox_filesystem, linux.sandbox_network), (Enforcement::Enforced, Enforcement::Enforced));
    for platform in [Platform::MacOs, Platform::Windows, Platform::Other] {
        let declared = capabilities_for(&recorded(), CodexMode::WorkspaceWrite, platform);
        assert_eq!(declared.sandbox_filesystem, Enforcement::Unverified, "{platform:?}");
        assert_eq!(declared.sandbox_network, Enforcement::Unverified, "{platform:?}");
    }
    let full = capabilities_for(&recorded(), CodexMode::FullAccess, Platform::Linux);
    assert_eq!((full.sandbox_filesystem, full.sandbox_network), (Enforcement::Absent, Enforcement::Absent));
}

#[test]
fn auto_review_and_full_access_never_count_as_host_gated() {
    for mode in [CodexMode::AutoReview, CodexMode::FullAccess] {
        assert!(!mode.host_answers_approvals());
        let declared = capabilities_for(&recorded(), mode, Platform::Linux);
        assert!(declared.approval_coverage.iter().all(|(_, gate)| *gate == ApprovalGate::None));
    }
}

#[test]
fn host_policy_maps_to_mode_env_and_is_verified_fail_closed() {
    let policy = HostPolicy { cwd: "/w".into(), writable: false, network: false };
    let env = session_env(&policy).unwrap();
    assert!(env.contains(&("INITIAL_AGENT_MODE".to_string(), "read-only".to_string())));
    assert!(env.iter().any(|(key, value)| key == "CODEX_CONFIG" && value.contains("\"web_search\":\"disabled\"")));
    let networked = HostPolicy { network: true, ..policy.clone() };
    assert!(session_env(&networked).is_err(), "network on would need full access: refused, not widened");

    // codex-acp's own default is the automatic reviewer: must be refused.
    let default_session = recorded_session_new_default();
    assert!(verify_session_mode(&default_session, mode_for_policy(&policy).unwrap()).is_err());
    let mut read_only = default_session.clone();
    read_only["modes"]["currentModeId"] = json!("read-only");
    assert_eq!(verify_session_mode(&read_only, CodexMode::ReadOnly), Ok(()));
    let only_config_option = json!({"configOptions": [{"id": "mode", "currentValue": "workspace-write"}]});
    assert_eq!(verify_session_mode(&only_config_option, CodexMode::WorkspaceWrite), Ok(()));
    assert!(verify_session_mode(&json!({}), CodexMode::ReadOnly).is_err(), "unknown mode fails closed");

    assert_eq!(set_mode_params("s1", CodexMode::ReadOnly), json!({"sessionId": "s1", "modeId": "read-only"}));
    assert_eq!(steering_params("s1", "hi")["prompt"][0]["text"], "hi");
}

// ---------------------------------------------------------------------------
// Real binary smoke test.

#[cfg(feature = "acp")]
#[tokio::test(flavor = "multi_thread")]
async fn real_codex_acp_smoke_skips_unless_fox_test_real_acp() {
    use fox_agent_backend::acp::{AcpConnection, BackendEvent};
    use fox_agent_backend::launch::dev_launch_spec;
    use fox_agent_backend::port::AgentBackend;
    use std::time::Duration;

    if std::env::var("FOX_TEST_REAL_ACP").as_deref() != Ok("1") {
        eprintln!("SKIPPED real_codex_acp_smoke_skips_unless_fox_test_real_acp: set FOX_TEST_REAL_ACP=1 (skip is not a pass)");
        return;
    }
    let cwd = std::env::temp_dir().join(format!("fox-codex-smoke-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    let (spec, source) = dev_launch_spec(AgentBackend::Codex).unwrap();
    eprintln!("codex-acp entry ({source:?}): {} {:?}", spec.program, spec.args);
    let mut connection = tokio::time::timeout(Duration::from_secs(180), AcpConnection::spawn(&spec, &cwd))
        .await
        .expect("codex-acp initialize within 180s (npx may download)")
        .expect("codex-acp spawn + initialize");

    let event = tokio::time::timeout(Duration::from_secs(10), connection.events.recv()).await.unwrap().unwrap();
    let BackendEvent::Initialized { initialize_response, .. } = event else { panic!("expected Initialized, got {event:?}") };
    eprintln!("INITIALIZE {initialize_response}");
    check_initialize(&initialize_response).expect("pinned codex-acp");
    let declared = capabilities_for(&initialize_response, CodexMode::ReadOnly, Platform::current());
    eprintln!("DECLARED {}", serde_json::to_string(&declared).unwrap());
    assert!(declared.session.load && declared.session.resume && declared.session.close);
    assert!(declared.session.steer, "steering _meta survives the shared client's re-serialization");

    #[cfg(target_os = "linux")]
    let before = processes_in(&cwd);
    #[cfg(target_os = "linux")]
    eprintln!("PROCESSES_IN_CWD_BEFORE_CLOSE {before:?}");

    let created = tokio::time::timeout(Duration::from_secs(60), connection.new_session(cwd.clone()))
        .await
        .expect("session/new answered within 60s");
    eprintln!("SESSION_NEW {created:?}");
    match created {
        Err(error) => {
            assert!(error.contains("Authentication required"), "failure form changed: {error}");
            // Also the result with OPENAI_API_KEY set: codex-acp wants an
            // explicit `authenticate`, which the Host does not send (docs/codex.md).
            eprintln!("NOT AUTHENTICATED: session/new fails with Authentication required; prompt not attempted");
        }
        Ok(session_id) => {
            connection.prompt(&session_id, "Reply with the single word: ok").unwrap();
            let outcome = tokio::time::timeout(Duration::from_secs(120), async {
                loop {
                    match connection.events.recv().await {
                        Some(event @ (BackendEvent::TurnEnded { .. } | BackendEvent::TurnFailed { .. } | BackendEvent::Failed(_))) => return Some(event),
                        Some(other) => eprintln!("EVENT {other:?}"),
                        None => return None,
                    }
                }
            })
            .await
            .expect("prompt settled within 120s");
            // Recorded as-is; an invalid key ends in Failed("... Authentication required").
            eprintln!("PROMPT_OUTCOME {outcome:?}");
            assert!(outcome.is_some(), "prompt left without a terminal event");
        }
    }

    connection.close().await;
    #[cfg(target_os = "linux")]
    {
        assert!(!before.is_empty(), "expected codex-acp processes rooted in the session cwd");
        tokio::time::sleep(Duration::from_millis(500)).await;
        let after = processes_in(&cwd);
        eprintln!("PROCESSES_IN_CWD_AFTER_CLOSE {after:?}");
        assert!(after.is_empty(), "process tree survived close: {after:?}");
    }
    let _ = std::fs::remove_dir_all(&cwd);
}

/// Live processes whose working directory is `dir`, as (pid, argv0..).
#[cfg(all(feature = "acp", target_os = "linux"))]
fn processes_in(dir: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let pid = entry.file_name().to_string_lossy().to_string();
        if !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(cwd) = std::fs::read_link(entry.path().join("cwd")) else { continue };
        if cwd != dir {
            continue;
        }
        let status = std::fs::read_to_string(entry.path().join("status")).unwrap_or_default();
        if status.lines().any(|line| line.starts_with("State:") && line.contains('Z')) {
            continue;
        }
        let cmdline = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        let cmd: String = cmdline.split(|b| *b == 0).take(3).map(|part| String::from_utf8_lossy(part).into_owned()).collect::<Vec<_>>().join(" ");
        found.push(format!("{pid} {cmd}"));
    }
    found
}
