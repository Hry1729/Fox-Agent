//! srt isolation layer, real binaries (T04 for sandboxed mode).
//!
//! Runs only with `FOX_TEST_REAL_SRT=1`; otherwise each test prints SKIPPED and
//! returns (a skip is not a pass, plan section 2). Needs:
//!
//! * `FOX_SRT_PREFIX`: npm prefix with `@anthropic-ai/sandbox-runtime` (0.0.80+).
//! * `FOX_PI_ACP_PREFIX`: npm prefix with `pi-acp@0.0.34` and
//!   `@earendil-works/pi-coding-agent@0.84.2` (stands in for Fox's managed
//!   backend directory).
//! * Linux: bubblewrap, socat, ripgrep; Node >= 22.12 on PATH (or `FOX_SRT_NODE`).
//! * Network for the allowed probe: `FOX_T04_ALLOWED_DOMAIN` (default
//!   example.com, standing in for a model API) must be reachable from the
//!   host; `FOX_T04_BLOCKED_DOMAIN` (default example.org) is the off-list host.
//! * `FOX_T04_EXTRA_DENY`: extra read-denied roots, `:`-separated (the measured
//!   run used `/workspace`, where the repository and the test layout live).
//!
//! The model is `fixtures/sandbox/scripted-cmd-provider.ts` (test only); pi,
//! pi-acp, srt and bubblewrap are real. Every tool call goes through the Fox
//! approval gate and is allowed, so what stops it is the sandbox alone.

#![cfg(feature = "acp")]

use fox_agent_backend::acp::{AcpConnection, BackendEvent};
use fox_agent_backend::backends::pi::{check_initialize, parse_tool_approval, OPTION_ALLOW_ONCE};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::sandbox::{
    default_deny_read_roots, preflight, prepare_launch, AuthorizedDir, HostOs, PermissionMode, RealProbe,
    SandboxPolicy, SandboxSetup, SrtInstall, SrtLauncher, SystemProbe,
};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn real_enabled(test: &str) -> bool {
    if std::env::var("FOX_TEST_REAL_SRT").as_deref() == Ok("1") {
        return true;
    }
    eprintln!("SKIPPED {test}: set FOX_TEST_REAL_SRT=1 (see file header) to run against real srt (skip is not a pass)");
    false
}

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name} must be set for the real srt test")))
}

fn b64url(text: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = text.as_bytes();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        let len = chunk.len() + 1;
        for index in 0..len {
            out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    out
}

async fn next(connection: &mut AcpConnection) -> BackendEvent {
    tokio::time::timeout(Duration::from_secs(120), connection.events.recv())
        .await
        .expect("event within 120s")
        .expect("event channel open")
}

/// One prompt; every gate approval is allowed. Returns the whole turn as text.
async fn turn(connection: &mut AcpConnection, session: &str, prompt: &str) -> String {
    connection.prompt(session, prompt).unwrap();
    let mut transcript = String::new();
    loop {
        match next(connection).await {
            BackendEvent::PermissionRequested { native_request_id, request, .. } => {
                assert!(parse_tool_approval(&request).is_some(), "only gate approvals expected: {request}");
                connection.answer_permission(&native_request_id, Some(OPTION_ALLOW_ONCE)).unwrap();
            }
            BackendEvent::Update { update, .. } => transcript.push_str(&format!("{update}\n")),
            BackendEvent::TurnEnded { stop_reason, .. } => {
                transcript.push_str(&format!("STOP {stop_reason}\n"));
                return transcript;
            }
            BackendEvent::Failed(error) => panic!("backend failed: {error}\n{transcript}\n{}", connection.stderr_tail()),
            other => transcript.push_str(&format!("{other:?}\n")),
        }
    }
}

async fn run(connection: &mut AcpConnection, session: &str, id: &str, command: &str) -> String {
    let out = turn(connection, session, &format!("RUN:{} ID:{id}", b64url(command))).await;
    eprintln!("--- {id}: {command}\n{}", tail(&out));
    out
}

fn tail(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(1500)..].iter().collect()
}

fn processes_matching(needle: &str) -> Vec<String> {
    let output = std::process::Command::new("ps").args(["-eo", "pid,pgid,args"]).output().expect("ps");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.contains(needle) && !line.contains("ps -eo"))
        .map(str::to_owned)
        .collect()
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn real_srt_pi_acp_t04_filesystem_and_network_boundary() {
    if !real_enabled("real_srt_pi_acp_t04_filesystem_and_network_boundary") {
        return;
    }
    let probe = RealProbe;
    let node = std::env::var_os("FOX_SRT_NODE").map(PathBuf::from).or_else(|| probe.which("node")).expect("node");
    let install = SrtInstall::under_prefix(node, &env_path("FOX_SRT_PREFIX"));
    let ready = preflight(&install, &probe).unwrap_or_else(|error| panic!("preflight: {error}"));
    eprintln!("preflight ok: node {} srt {} helpers {:?}", ready.node_version, ready.srt_version, ready.helpers);

    // Layout under the real user home (read-denied): authorized project,
    // a secret outside it, the private HOME and Fox's state directory.
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let base = home.join(format!(".fox-t04-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("proj");
    let secret_dir = base.join("secret");
    let private_home = base.join("backend-home");
    let state = base.join("state");
    for dir in [&proj, &secret_dir, &private_home, &state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let token = format!("FOXSECRET{}", std::process::id());
    std::fs::write(secret_dir.join("secret.txt"), &token).unwrap();
    std::fs::write(proj.join("readme.txt"), "authorized-content").unwrap();
    let tmp_out = std::env::temp_dir().join(format!("fox-t04-out-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_out).unwrap();

    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    // pi-gated.sh is committed as 100644 (the contents API cannot set the
    // executable bit), so run a private executable copy next to the gate.
    let launcher_dir = base.join("pi-launcher");
    std::fs::create_dir_all(&launcher_dir).unwrap();
    for file in ["pi-gated.sh", "fox-approval-gate.ts"] {
        std::fs::copy(fixtures.join("pi").join(file), launcher_dir.join(file)).unwrap();
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(launcher_dir.join("pi-gated.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let pi_prefix = env_path("FOX_PI_ACP_PREFIX");
    let allowed = std::env::var("FOX_T04_ALLOWED_DOMAIN").unwrap_or_else(|_| "example.com".into());
    let blocked = std::env::var("FOX_T04_BLOCKED_DOMAIN").unwrap_or_else(|_| "example.org".into());
    let mut policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: proj.clone(), writable: true }],
        private_home.clone(),
        vec![fixtures.clone(), launcher_dir.clone(), pi_prefix.clone()],
        vec![allowed.clone()],
        HostOs::Linux,
    );
    assert_eq!(policy.deny_read_roots, default_deny_read_roots(HostOs::Linux));
    if let Ok(extra) = std::env::var("FOX_T04_EXTRA_DENY") {
        policy.deny_read_roots.extend(extra.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    }

    let bin = pi_prefix.join("node_modules/.bin");
    let spec = LaunchSpec {
        backend: AgentBackend::Pi,
        program: bin.join("pi-acp").display().to_string(),
        args: vec![],
        version_pin: fox_agent_backend::launch::PI_ACP_VERSION,
        credential_env: &[],
        experimental: false,
        session_env: vec![
            ("PI_ACP_PI_COMMAND".into(), launcher_dir.join("pi-gated.sh").display().to_string()),
            ("FOX_PI_BIN".into(), bin.join("pi").display().to_string()),
            (
                "FOX_PI_EXTRA_ARGS".into(),
                format!("-e {} --provider foxcmd --model scripted", fixtures.join("sandbox/scripted-cmd-provider.ts").display()),
            ),
        ],
    };
    let launcher = SrtLauncher::new(ready, state.clone());
    let prepared = prepare_launch(
        PermissionMode::Sandboxed,
        spec,
        "t04",
        SandboxSetup::Ready { launcher: &launcher, policy: &policy },
        HostOs::Linux,
    )
    .unwrap_or_else(|error| panic!("prepare: {error}"));
    assert!(prepared.declaration.directory_isolation);
    let settings_path = prepared.sandbox.as_ref().unwrap().settings_path().to_path_buf();
    eprintln!("settings: {}", std::fs::read_to_string(&settings_path).unwrap());

    let mut connection = AcpConnection::spawn_prepared(prepared, &proj).await.expect("spawn pi-acp inside srt");
    assert_eq!(connection.permission_mode(), PermissionMode::Sandboxed);
    let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
        panic!("expected Initialized first");
    };
    check_initialize(&initialize_response).unwrap();
    let session = connection
        .new_session(proj.clone())
        .await
        .unwrap_or_else(|error| panic!("session/new: {error}\n{}", connection.stderr_tail()));

    // 0. Authorized directory: read and write work.
    let out = run(&mut connection, &session, "inside", "cat readme.txt && echo inside-ok > inside.txt && echo wrote").await;
    assert!(out.contains("authorized-content"), "authorized read failed");
    assert_eq!(std::fs::read_to_string(proj.join("inside.txt")).unwrap().trim(), "inside-ok");

    // 1. Read outside the authorized directory fails (bash and pi's read tool).
    let out = run(&mut connection, &session, "readout", &format!("cat {} ; ls {}", secret_dir.join("secret.txt").display(), home.display())).await;
    assert!(!out.contains(&token), "secret leaked through bash");
    let out = turn(&mut connection, &session, &format!("READ:{} ID:readtool", b64url(&secret_dir.join("secret.txt").display().to_string()))).await;
    eprintln!("--- readtool\n{}", tail(&out));
    assert!(!out.contains(&token), "secret leaked through pi read tool");
    let out = run(&mut connection, &session, "settings", &format!("cat {}", settings_path.display())).await;
    assert!(!out.contains("allowedDomains"), "sandbox settings readable from inside");

    // 2. Write outside fails: a read-denied dir and a readable-but-not-writable one.
    let out = run(
        &mut connection,
        &session,
        "writeout",
        &format!("echo pwn > {0}/pwn.txt; echo pwn > {1}/pwn.txt; echo pwn > {2}/pwn.txt; echo done", secret_dir.display(), tmp_out.display(), state.display()),
    )
    .await;
    assert!(out.contains("done"));
    assert!(!secret_dir.join("pwn.txt").exists(), "wrote into a denied directory");
    assert!(!tmp_out.join("pwn.txt").exists(), "wrote outside the authorized directory");
    assert!(!state.join("pwn.txt").exists(), "wrote into Fox state");

    // 3. Network: off-list host refused, allowlisted host reachable.
    let out = run(&mut connection, &session, "netblocked", &format!("curl -sS -m 20 -o /dev/null -w code=%{{http_code}} https://{blocked}/; echo")).await;
    assert!(!out.contains("code=200"), "off-list domain reachable");
    let out = run(&mut connection, &session, "netallowed", &format!("curl -sS -m 20 -o /dev/null -w code=%{{http_code}} https://{allowed}/; echo")).await;
    assert!(out.contains("code=200"), "allowlisted domain not reachable");
    // Raw TCP to a public IP bypassing the proxy has no route (no netns).
    let out = run(&mut connection, &session, "rawtcp", "curl -sS -m 10 --noproxy '*' -o /dev/null -w code=%{http_code} http://1.1.1.1/; echo").await;
    assert!(!out.contains("code=200") && !out.contains("code=301"), "direct connection escaped the proxy");

    // 4. Private HOME is the backend's HOME.
    let out = run(&mut connection, &session, "home", "echo HOME=$HOME; echo x > $HOME/probe.txt && echo homewrite-ok").await;
    assert!(out.contains(&format!("HOME={}", private_home.display())));
    assert!(private_home.join("probe.txt").exists());

    // 5. Close: process tree gone, per-session settings removed.
    connection.close().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = processes_matching(&base.display().to_string());
    assert!(left.is_empty(), "processes left after close: {left:?}");
    assert!(!settings_path.exists(), "per-session settings not cleaned up");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&tmp_out);
}

/// Fail-closed with a real (missing) srt: sandboxed refuses before anything is
/// spawned; full access still launches.
#[tokio::test(flavor = "multi_thread")]
async fn real_missing_srt_refuses_sandboxed_launch() {
    if !real_enabled("real_missing_srt_refuses_sandboxed_launch") {
        return;
    }
    let install = SrtInstall::under_prefix("/nonexistent/node".into(), Path::new("/nonexistent/srt"));
    let error = preflight(&install, &RealProbe).unwrap_err();
    assert_eq!(error.code.as_str(), "sandbox_dependency_missing");
    let spec = LaunchSpec {
        backend: AgentBackend::Pi,
        program: "/bin/true".into(),
        args: vec![],
        version_pin: "x",
        credential_env: &[],
        experimental: false,
        session_env: vec![],
    };
    let refused = prepare_launch(PermissionMode::Sandboxed, spec.clone(), "s", SandboxSetup::Unavailable(error.clone()), HostOs::current());
    assert_eq!(refused.unwrap_err(), error);
    let full = prepare_launch(PermissionMode::FullAccess, spec.clone(), "s", SandboxSetup::Unavailable(error), HostOs::current()).unwrap();
    assert_eq!(full.spec, spec);
}
