//! PiBackend tests.
//!
//! * Unit tests run always: recorded pi-acp 0.0.34 `initialize` response
//!   (fixtures/pi/initialize-pi-acp-0.0.34.json, captured from a real
//!   `npx -y pi-acp@0.0.34` on 2026-10-19) mapped to the capability declaration.
//! * `real_*` tests drive the real pi-acp 0.0.34 and real pi 0.84.2. Entry:
//!   `FOX_PI_ACP_PROGRAM`, else the managed runtime (`FOX_AGENT_RUNTIME_ROOT`;
//!   Windows also `FOX_RUNTIME_ROOT` / the installed runtime), else
//!   `npx -y pi-acp@0.0.34` (`launch::dev_launch_spec`, tests only).
//!   They only run with `FOX_TEST_REAL_ACP=1`. Without it they `return` early:
//!   that is a SKIP and does not count as passing (plan section 2).
//!   Needs Node 22, npx with network (first run downloads pi-acp), and pi at
//!   `FOX_PI_BIN` or services/agent-runtime/node_modules/.bin/pi.

use fox_agent_backend::backends::pi::{
    capabilities, capabilities_with_approval_gate, check_initialize, classify_failure, parse_tool_approval,
    PiFailure, OPTION_ALLOW_ONCE, OPTION_REJECT_ONCE,
};
use fox_agent_backend::port::{ApprovalGate, CancelReach, Enforcement, ToolCategory, Visibility};
use serde_json::{json, Value};

fn recorded_initialize() -> Value {
    let raw = include_str!("../fixtures/pi/initialize-pi-acp-0.0.34.json");
    serde_json::from_str(raw).expect("recorded initialize is JSON")
}

#[test]
fn recorded_initialize_is_the_pinned_adapter() {
    let init = recorded_initialize();
    check_initialize(&init).expect("pinned pi-acp accepted");
    let mut other = init.clone();
    other["agentInfo"]["version"] = json!("0.0.35");
    assert!(check_initialize(&other).is_err(), "unvalidated pi-acp version must be refused");
    let mut foreign = init;
    foreign["agentInfo"]["name"] = json!("codex-acp");
    assert!(check_initialize(&foreign).is_err());
}

#[test]
fn stock_pi_acp_declares_no_approval_and_no_sandbox() {
    let declared = capabilities(&recorded_initialize());
    for category in [ToolCategory::FileRead, ToolCategory::FileWrite, ToolCategory::Shell, ToolCategory::Network] {
        assert_eq!(declared.gate(category), ApprovalGate::None, "{category:?}");
        assert!(!declared.ui_claims_per_call_approval(category));
    }
    assert_eq!(declared.gate(ToolCategory::McpBusinessTool), ApprovalGate::Unverified, "MCP is not wired to pi");
    assert_eq!(declared.sandbox_filesystem, Enforcement::Absent);
    assert_eq!(declared.sandbox_network, Enforcement::Absent);
    assert_eq!(declared.cancel_turn, CancelReach::TurnConfirmed);
    assert!(declared.cancel_process_tree, "detached bash is reached through descendant tracking (measured on Linux)");
    assert!(!declared.approval_answer_acknowledged);
    assert_eq!(declared.observability, Visibility::ToolLifecycle);
}

#[test]
fn session_capabilities_follow_initialize() {
    let session = capabilities(&recorded_initialize()).session;
    assert!(session.load, "loadSession: true");
    assert!(session.list, "sessionCapabilities.list declared");
    assert!(!session.resume, "no session/resume");
    assert!(!session.close, "declares delete, not close");
    assert!(!session.steer, "ACP has no steer; pi-acp queues prompts as new turns");
    let mut init = recorded_initialize();
    init["agentCapabilities"]["loadSession"] = json!(false);
    assert!(!capabilities(&init).session.load, "never claim load the agent did not declare");
}

#[test]
fn gated_launch_only_claims_measured_per_call_gates() {
    let declared = capabilities_with_approval_gate(&recorded_initialize());
    assert!(declared.ui_claims_per_call_approval(ToolCategory::Shell));
    assert!(declared.ui_claims_per_call_approval(ToolCategory::FileWrite));
    assert!(declared.ui_claims_per_call_approval(ToolCategory::FileRead));
    assert!(!declared.ui_claims_per_call_approval(ToolCategory::Network));
    assert!(!declared.ui_claims_per_call_approval(ToolCategory::McpBusinessTool));
    assert_eq!(declared.sandbox_filesystem, Enforcement::Absent, "a gate is not a sandbox");
}

fn gate_request(title: &str, message: &str, options: &[&str]) -> Value {
    json!({
        "sessionId": "s1",
        "toolCall": {
            "toolCallId": "pi-ui-1234",
            "title": title,
            "kind": "other",
            "status": "pending",
            "rawInput": {"method": "confirm", "title": title, "message": message}
        },
        "options": options.iter().map(|id| json!({"optionId": id, "name": id, "kind": "allow_once"})).collect::<Vec<_>>()
    })
}

#[test]
fn tool_approval_is_decoded_only_from_the_fox_gate() {
    let message = r#"{"fox":"tool-approval","v":1,"toolCallId":"call_x","toolName":"bash","input":{"command":"ls"},"cwd":"/w"}"#;
    let approval = parse_tool_approval(&gate_request("Fox approval: bash", message, &[OPTION_ALLOW_ONCE, OPTION_REJECT_ONCE]))
        .expect("gate request decoded");
    assert_eq!(approval.tool_call_id, "call_x");
    assert_eq!(approval.tool_name, "bash");
    assert_eq!(approval.input["command"], "ls");
    assert_eq!(approval.dialog_tool_call_id, "pi-ui-1234");
    // Another extension's confirm is not a tool approval.
    assert!(parse_tool_approval(&gate_request("Delete?", "really?", &["yes", "no"])).is_none());
    // Spoofed title without the marker.
    assert!(parse_tool_approval(&gate_request("Fox approval: bash", "{}", &["yes", "no"])).is_none());
    // Options other than the native yes/no are not ours.
    assert!(parse_tool_approval(&gate_request("Fox approval: bash", message, &["choice-0", "choice-1"])).is_none());
}

#[test]
fn failures_from_pi_acp_are_classified() {
    // Messages as measured from pi-acp 0.0.34 (see docs/pi.md).
    assert_eq!(
        classify_failure("Authentication required: Configure an API key or log in with an OAuth provider."),
        PiFailure::AuthRequired
    );
    assert_eq!(
        classify_failure("Internal error: Could not start pi: executable not found (command: pi)."),
        PiFailure::PiNotFound
    );
    assert_eq!(classify_failure("boom"), PiFailure::Other);
}

// ---------------------------------------------------------------------------
// Real binaries (FOX_TEST_REAL_ACP=1).

#[cfg(feature = "acp")]
mod real {
    use super::*;
    use fox_agent_backend::acp::{AcpConnection, BackendEvent};
    use fox_agent_backend::launch::{dev_launch_spec, DevEntrySource, LaunchSpec};
    use fox_agent_backend::port::AgentBackend;
    #[cfg(unix)]
    use fox_agent_backend::port::StopConfirmation;
    use fox_agent_backend::acp::WriteOutcome;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn real_enabled(test: &str) -> bool {
        if std::env::var("FOX_TEST_REAL_ACP").as_deref() == Ok("1") {
            return true;
        }
        eprintln!("SKIPPED {test}: set FOX_TEST_REAL_ACP=1 to run against real pi-acp (skip is not a pass)");
        false
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("fox-pi-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn next(connection: &mut AcpConnection) -> BackendEvent {
        tokio::time::timeout(Duration::from_secs(90), connection.events.recv())
            .await
            .expect("event within 90s")
            .expect("event channel open")
    }

    async fn until<F: Fn(&BackendEvent) -> bool>(connection: &mut AcpConnection, wanted: F) -> Vec<BackendEvent> {
        let mut seen = Vec::new();
        loop {
            let event = next(connection).await;
            if let BackendEvent::Failed(error) | BackendEvent::TurnFailed { error, .. } = &event {
                panic!("backend failed: {error}; seen so far: {seen:?}");
            }
            let done = wanted(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
    }

    fn tool_updates<'a>(events: &'a [BackendEvent], tool_call_id: &str) -> Vec<&'a Value> {
        events
            .iter()
            .filter_map(|event| match event {
                BackendEvent::Update { update, .. }
                    if update.get("toolCallId").and_then(Value::as_str) == Some(tool_call_id) =>
                {
                    Some(update)
                }
                _ => None,
            })
            .collect()
    }

    fn stop_reason(events: &[BackendEvent]) -> String {
        match events.last() {
            Some(BackendEvent::TurnEnded { stop_reason, .. }) => stop_reason.as_str().unwrap_or("").to_owned(),
            other => panic!("expected TurnEnded, got {other:?}"),
        }
    }

    /// Stock launch spec (`launch::dev_launch_spec`), real environment allowlist.
    /// pi is whatever is on PATH; without credentials session/new fails.
    #[tokio::test(flavor = "multi_thread")]
    async fn real_stock_initialize_and_session_new() {
        if !real_enabled("real_stock_initialize_and_session_new") {
            return;
        }
        let (spec, source) = dev_launch_spec(AgentBackend::Pi).unwrap();
        eprintln!("pi-acp entry ({source:?}): {} {:?}", spec.program, spec.args);
        let cwd = temp_dir("stock");
        let mut connection = AcpConnection::spawn(&spec, &cwd).await.expect("spawn pi-acp");
        let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
            panic!("expected Initialized first");
        };
        eprintln!("pi-acp initialize: {initialize_response}");
        check_initialize(&initialize_response).expect("pinned pi-acp");
        assert_eq!(capabilities(&initialize_response), capabilities(&recorded_initialize()), "recording is current");
        match connection.new_session(cwd.clone()).await {
            Ok(session) => eprintln!("session/new succeeded (pi on PATH has credentials): {session}"),
            Err(error) => {
                eprintln!("session/new failed as expected without pi credentials: {error}");
                assert_ne!(classify_failure(&error), PiFailure::Other, "unexpected failure shape: {error}");
            }
        }
        connection.close().await;
    }

    fn fixtures() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/pi")
    }

    fn pi_bin() -> PathBuf {
        std::env::var_os("FOX_PI_BIN").map(PathBuf::from).unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../services/agent-runtime/node_modules/.bin/pi")
        })
    }

    /// Real pi-acp + real pi 0.84.2 + Fox approval gate, with the test-only
    /// scripted model (fixtures/pi/scripted-provider.ts) so no credentials are
    /// needed. Isolated HOME.
    fn gated_spec(home: &Path) -> LaunchSpec {
        let pi = pi_bin();
        assert!(pi.exists(), "pi binary not found at {} (set FOX_PI_BIN)", pi.display());
        let extra = format!(
            "-e {} --provider foxscripted --model scripted",
            fixtures().join("scripted-provider.ts").display()
        );
        LaunchSpec {
            backend: AgentBackend::Pi,
            program: "node".into(),
            args: vec![
                fixtures().join("launch-pi-acp.mjs").display().to_string(),
                "--home".into(),
                home.display().to_string(),
                "--pi-bin".into(),
                pi.display().to_string(),
                "--extra".into(),
                extra,
            ]
            .into_iter()
            .chain(match dev_launch_spec(AgentBackend::Pi) {
                Some((spec, DevEntrySource::Override | DevEntrySource::Managed(_))) => vec!["--program".to_owned(), spec.program],
                _ => vec![],
            })
            .collect(),
            version_pin: fox_agent_backend::launch::PI_ACP_VERSION,
            credential_env: &[],
            experimental: false,
            session_env: vec![],
        }
    }

    fn processes_matching(needle: &str) -> Vec<String> {
        let output = std::process::Command::new("ps").args(["-eo", "pid,pgid,args"]).output().expect("ps");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| line.contains(needle) && !line.contains("ps -eo"))
            .map(str::to_owned)
            .collect()
    }

    async fn ask(connection: &mut AcpConnection, session: &str, prompt: &str) -> (Vec<BackendEvent>, String, Value) {
        connection.prompt(session, prompt).unwrap();
        let events = until(connection, |event| matches!(event, BackendEvent::PermissionRequested { .. })).await;
        let Some(BackendEvent::PermissionRequested { native_request_id, request, option_ids, .. }) = events.last().cloned()
        else {
            unreachable!()
        };
        assert_eq!(option_ids, [OPTION_ALLOW_ONCE, OPTION_REJECT_ONCE], "only native options");
        (events, native_request_id, request)
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn real_gated_allow_reject_cancel_and_process_tree() {
        if !real_enabled("real_gated_allow_reject_cancel_and_process_tree") {
            return;
        }
        let home = temp_dir("home");
        let cwd = temp_dir("cwd");
        let mut connection = AcpConnection::spawn(&gated_spec(&home), &cwd).await.expect("spawn gated pi-acp");
        let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
            panic!("expected Initialized first");
        };
        check_initialize(&initialize_response).unwrap();
        let session = connection.new_session(cwd.clone()).await.expect("session/new with scripted model");
        eprintln!("gated session: {session}");

        // 1. Allow: bash waits for the Host, runs only after "yes".
        let (events, id, request) = ask(&mut connection, &session, "please GATE:allow1").await;
        let approval = parse_tool_approval(&request).expect("gate approval");
        eprintln!("request_permission params: {request}");
        assert_eq!(approval.tool_name, "bash");
        assert_eq!(approval.tool_call_id, "call_allow1");
        let pre: Vec<_> = tool_updates(&events, "call_allow1").iter().filter_map(|u| u.get("status").cloned()).collect();
        eprintln!("allow1 tool statuses BEFORE approval: {pre:?}");
        assert!(!pre.is_empty(), "tool_call update precedes approval");
        assert!(!cwd.join("fox-proof-allow1.txt").exists(), "tool ran before approval");
        assert_eq!(connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).unwrap(), WriteOutcome::Written);
        assert!(connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).is_err(), "second answer refused");
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        assert_eq!(stop_reason(&events), "end_turn");
        let statuses: Vec<_> = tool_updates(&events, "call_allow1").iter().filter_map(|u| u.get("status").cloned()).collect();
        eprintln!("allow1 tool statuses after approval: {statuses:?}");
        assert!(statuses.contains(&json!("completed")));
        assert_eq!(std::fs::read_to_string(cwd.join("fox-proof-allow1.txt")).unwrap().trim(), "allow1");

        // 2. Reject: blocked, no side effect, turn still ends normally.
        let (_, id, _) = ask(&mut connection, &session, "please GATE:deny1").await;
        connection.answer_permission(&id, Some(OPTION_REJECT_ONCE)).unwrap();
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        assert_eq!(stop_reason(&events), "end_turn");
        let statuses: Vec<_> = tool_updates(&events, "call_deny1").iter().filter_map(|u| u.get("status").cloned()).collect();
        eprintln!("deny1 tool statuses: {statuses:?}");
        assert!(statuses.contains(&json!("failed")), "blocked call reported as failed");
        assert!(!cwd.join("fox-proof-deny1.txt").exists(), "rejected command ran");

        // 3. File write tool goes through the same gate.
        let (_, id, request) = ask(&mut connection, &session, "please GATEWRITE:w1").await;
        assert_eq!(parse_tool_approval(&request).unwrap().tool_name, "write");
        assert!(!cwd.join("fox-write-w1.txt").exists(), "write ran before approval");
        connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).unwrap();
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        assert_eq!(stop_reason(&events), "end_turn");
        assert_eq!(std::fs::read_to_string(cwd.join("fox-write-w1.txt")).unwrap(), "w1");

        // 3b. File read tool: gated too; rejected read returns no content.
        let (_, id, request) = ask(&mut connection, &session, "please GATEREAD:w1").await;
        assert_eq!(parse_tool_approval(&request).unwrap().tool_name, "read");
        connection.answer_permission(&id, Some(OPTION_REJECT_ONCE)).unwrap();
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        let read_updates = tool_updates(&events, "call_read_w1");
        eprintln!("rejected read updates: {read_updates:?}");
        assert!(read_updates.iter().any(|u| u.get("status") == Some(&json!("failed"))));
        assert!(!read_updates.iter().any(|u| u.to_string().contains("\"w1\"") && u.get("status") == Some(&json!("completed"))));

        // 4. Cancel while approval is pending: Host answers `cancelled`, the
        //    turn ends `cancelled`, the tool never runs.
        let (_, _, _) = ask(&mut connection, &session, "please GATE:cancel1").await;
        let settled = connection.cancel(&session);
        assert_eq!(settled.len(), 1, "pending approval settled as cancelled");
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        assert_eq!(stop_reason(&events), "cancelled");
        assert!(!cwd.join("fox-proof-cancel1.txt").exists(), "cancelled command ran");

        // 5. Cancel while an approved bash command runs: pi abort kills it.
        let (_, id, _) = ask(&mut connection, &session, "please GATESLEEP:s2").await;
        connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let running = processes_matching("fox-sleep-s2");
        eprintln!("running before cancel: {running:?}");
        assert!(!running.is_empty(), "approved sleep should be running");
        connection.cancel(&session);
        let events = until(&mut connection, |event| matches!(event, BackendEvent::TurnEnded { .. })).await;
        assert_eq!(stop_reason(&events), "cancelled");
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(processes_matching("fox-sleep-s2").is_empty(), "pi abort left the bash command running");

        // 6. Host closes the connection while an approved bash runs. bash runs
        //    in its own process group (pi spawns it `detached`), so a group
        //    kill alone would not reach it. Re-measured 2026-10-19 before this
        //    change: with TERM -> grace -> KILL, pi's own SIGTERM handler
        //    already ended the detached bash (the old "bash survives"
        //    assertion failed). Now asserted positively: every process
        //    recorded under the backend (pi-acp, pi, bash, sleep) is gone and
        //    the stop is confirmed.
        let (_, id, _) = ask(&mut connection, &session, "please GATESLEEP:s3").await;
        connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let root = connection.process_id().expect("backend pid");
        let tree = process_tree(root);
        eprintln!("process tree before close: {tree:#?}");
        let recorded = check_tree_shape(&tree, root, "fox-sleep-s3");
        let report = connection.close_with_report().await;
        eprintln!("close report: {report:?}");
        assert_eq!(report.confirmation, StopConfirmation::Confirmed, "stop not confirmed: {report:?}");
        let alive = still_alive(&recorded);
        assert!(alive.is_empty(), "processes survived the close: {alive:?}");
        assert!(processes_matching("fox-sleep-s3").is_empty());
    }

    /// (pid, ppid, pgid, args) of `root` and all its descendants.
    #[cfg(unix)]
    fn process_tree(root: u32) -> Vec<(u32, u32, u32, String)> {
        let output = std::process::Command::new("ps").args(["-eo", "pid=,ppid=,pgid=,args="]).output().expect("ps");
        let rows: Vec<(u32, u32, u32, String)> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.split_whitespace();
                let pid = parts.next()?.parse().ok()?;
                let ppid = parts.next()?.parse().ok()?;
                let pgid = parts.next()?.parse().ok()?;
                Some((pid, ppid, pgid, parts.collect::<Vec<_>>().join(" ")))
            })
            .collect();
        let mut tree: Vec<_> = rows.iter().filter(|row| row.0 == root).cloned().collect();
        let mut index = 0;
        while index < tree.len() {
            let parent = tree[index].0;
            tree.extend(rows.iter().filter(|row| row.1 == parent).cloned());
            index += 1;
        }
        tree
    }

    #[cfg(unix)]
    /// The tree holds pi-acp, pi and the detached bash running `marker`
    /// (own process group); returns every pid with its args.
    fn check_tree_shape(tree: &[(u32, u32, u32, String)], root: u32, marker: &str) -> Vec<(u32, String)> {
        assert!(tree.iter().any(|row| row.3.contains("pi-acp")), "pi-acp not under the backend: {tree:?}");
        let bash = tree.iter().find(|row| row.3.contains(marker) && row.3.contains("bash")).expect("approved bash under the backend");
        assert_ne!(bash.2, root, "bash expected in its own process group (pi spawns it detached)");
        let pi = tree.iter().find(|row| row.0 == bash.1).expect("bash parent");
        eprintln!("pi process (bash parent): {pi:?}; bash: {bash:?}");
        tree.iter().map(|row| (row.0, row.3.clone())).collect()
    }

    #[cfg(unix)]
    /// Recorded processes still alive with the same command line (zombies
    /// count as gone).
    fn still_alive(recorded: &[(u32, String)]) -> Vec<(u32, String)> {
        recorded
            .iter()
            .filter(|(pid, args)| {
                let output = std::process::Command::new("ps").args(["-o", "stat=,args=", "-p", &pid.to_string()]).output().expect("ps");
                let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                !text.is_empty() && !text.starts_with('Z') && text.contains(args.split_whitespace().next().unwrap_or(""))
            })
            .cloned()
            .collect()
    }

    /// pi does not get to clean up (SIGSTOP before the close, so its SIGTERM
    /// handler never runs): the group kill cannot reach the detached bash
    /// (its own process group); whatever of that subtree is still running is
    /// killed by Fox's descendant tracking, and the stop is confirmed only
    /// when nothing recorded is left.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn real_gated_close_kills_detached_bash_even_when_pi_cannot_clean_up() {
        if !real_enabled("real_gated_close_kills_detached_bash_even_when_pi_cannot_clean_up") {
            return;
        }
        let home = temp_dir("home-stop");
        let cwd = temp_dir("cwd-stop");
        let mut connection = AcpConnection::spawn(&gated_spec(&home), &cwd).await.expect("spawn gated pi-acp");
        let _ = next(&mut connection).await;
        let session = connection.new_session(cwd.clone()).await.expect("session/new with scripted model");
        let (_, id, _) = ask(&mut connection, &session, "please GATESLEEP:s4").await;
        connection.answer_permission(&id, Some(OPTION_ALLOW_ONCE)).unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        let root = connection.process_id().expect("backend pid");
        let tree = process_tree(root);
        let recorded = check_tree_shape(&tree, root, "fox-sleep-s4");
        let bash = tree.iter().find(|row| row.3.contains("fox-sleep-s4") && row.3.contains("bash")).unwrap().clone();
        let pi_pid = bash.1;
        assert!(std::process::Command::new("kill").args(["-STOP", &pi_pid.to_string()]).status().unwrap().success());
        let report = connection.close_with_report().await;
        eprintln!("close report with pi stopped: {report:?}");
        assert_eq!(report.confirmation, StopConfirmation::Confirmed, "stop not confirmed: {report:?}");
        // Whether descendant tracking had to kill part of bash's subtree
        // depends on timing (measured: in one run it killed the `sleep`,
        // in another the subtree was already gone); the mechanism itself is
        // covered deterministically by proctree's unit tests. What must hold
        // is the outcome: confirmed, nothing left.
        eprintln!("killed by descendant tracking: {:?}", report.notes.iter().filter(|note| note.contains("left process group")).collect::<Vec<_>>());
        let alive = still_alive(&recorded);
        assert!(alive.is_empty(), "processes survived the close: {alive:?}");
        assert!(processes_matching("fox-sleep-s4").is_empty());
    }
}
