//! srt isolation layer, real binaries (T04 for sandboxed mode).
//!
//! **A green `cargo test` is not a measured pass.** The real test below is
//! `#[ignore]`d, so a plain `cargo test` reports it under "ignored" (that is
//! the skip count) and never runs it. To run it:
//!
//! ```text
//! FOX_TEST_REAL_SRT=1 FOX_SRT_PREFIX=<srt prefix> FOX_PI_ACP_PREFIX=<pi prefix> \
//!   cargo test -p fox-agent-backend --features acp --test sandbox_srt -- --include-ignored --nocapture
//! ```
//!
//! Running it (`--include-ignored` / `--ignored`) without `FOX_TEST_REAL_SRT=1`
//! fails instead of passing silently. Needs:
//!
//! * `FOX_SRT_PREFIX`: npm prefix with `@anthropic-ai/sandbox-runtime` (0.0.80+).
//! * `FOX_PI_ACP_PREFIX`: npm prefix with `pi-acp@0.0.34` and
//!   `@earendil-works/pi-coding-agent@0.84.2` (stands in for Fox's managed
//!   backend directory).
//! * Linux: bubblewrap, socat, ripgrep; Node >= 22.12 on PATH (or `FOX_SRT_NODE`).
//! * No outside network: the allowed and the blocked targets are two HTTP
//!   servers this test starts on the host loopback, checked from outside the
//!   sandbox first so a refusal inside cannot be a dead target.
//!
//! The model is `fixtures/sandbox/scripted-cmd-provider.ts` (test only); pi,
//! pi-acp, srt and bubblewrap are real. Every tool call goes through the Fox
//! approval gate and is allowed, so what stops it is the sandbox alone.
//!
//! `fail_closed_without_srt_refuses_sandboxed_launch` needs no srt and always
//! runs.

#![cfg(feature = "acp")]

use fox_agent_backend::acp::{AcpConnection, BackendEvent};
use fox_agent_backend::backends::pi::{check_initialize, parse_tool_approval, OPTION_ALLOW_ONCE};
use fox_agent_backend::launch::LaunchSpec;
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::sandbox::{
    default_deny_read_roots, preflight, prepare_launch, AuthorizedDir, HostOs, PermissionMode, PrivateHomes, ReadScope,
    RealProbe, SandboxPolicy, SandboxSetup, SrtInstall, SrtLauncher, SystemProbe,
};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const PROJECT_UUID: &str = "5d1c0f2e-7a3b-4c6d-9e8f-0a1b2c3d4e5f";

/// The real test is `#[ignore]`d; if someone runs it anyway without the
/// environment, fail loudly rather than report a pass.
fn require_real(test: &str) {
    if std::env::var("FOX_TEST_REAL_SRT").as_deref() != Ok("1") {
        panic!("{test} is a real-srt test: set FOX_TEST_REAL_SRT=1 and the variables in the file header (not run = not passed)");
    }
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

/// A loopback HTTP server that answers every request with `token` and counts
/// the requests that reached it.
struct Target {
    port: u16,
    token: String,
    hits: Arc<AtomicUsize>,
}

impl Target {
    fn start(token: &str) -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let body = token.to_owned();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 4096];
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let _ = stream.read(&mut buffer);
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        Self { port, token: token.to_owned(), hits }
    }

    fn hits(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }

    /// Control request from outside the sandbox: the target is up and answers
    /// with its token.
    fn assert_reachable_from_host(&self) {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port)).expect("control connect");
        write!(stream, "GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 200") && response.contains(&self.token), "control request failed: {response}");
    }
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
#[ignore = "real srt + pi-acp; run with FOX_TEST_REAL_SRT=1 ... -- --include-ignored (see file header)"]
async fn real_srt_pi_acp_t04_filesystem_and_network_boundary() {
    require_real("real_srt_pi_acp_t04_filesystem_and_network_boundary");
    let probe = RealProbe;
    let node = std::env::var_os("FOX_SRT_NODE").map(PathBuf::from).or_else(|| probe.which("node")).expect("node");
    let install = SrtInstall::under_prefix(node, &env_path("FOX_SRT_PREFIX"));
    let ready = preflight(&install, &probe).unwrap_or_else(|error| panic!("preflight: {error}"));
    eprintln!("preflight ok: node {} srt {} helpers {:?}", ready.node_version, ready.srt_version, ready.helpers);

    // Layout under the real user home: authorized project, a secret beside it,
    // Fox's data directory (private HOMEs, state, session temp). Two more
    // secrets outside any home (host /tmp and cargo's target dir) prove the
    // `/` deny reaches beyond the home roots.
    let pid = std::process::id();
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let base = home.join(format!(".fox-t04-{pid}"));
    let _ = std::fs::remove_dir_all(&base);
    let proj = base.join("proj");
    let secret_dir = base.join("secret");
    let fox_data = base.join("fox-data");
    let state = fox_data.join("state");
    for dir in [&proj, &secret_dir, &state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let token = format!("FOXSECRET{pid}");
    std::fs::write(secret_dir.join("secret.txt"), &token).unwrap();
    std::fs::write(proj.join("readme.txt"), "authorized-content").unwrap();
    let host_tmp = std::env::temp_dir().join(format!("fox-t04-out-{pid}"));
    std::fs::create_dir_all(&host_tmp).unwrap();
    std::fs::write(host_tmp.join("secret.txt"), &token).unwrap();
    let data_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("fox-t04-data-{pid}"));
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("secret.txt"), &token).unwrap();

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

    // Network targets on the host loopback, checked from outside first.
    let allowed = Target::start(&format!("FOX-T04-ALLOWED-{pid}"));
    let blocked = Target::start(&format!("FOX-T04-BLOCKED-{pid}"));
    allowed.assert_reachable_from_host();
    blocked.assert_reachable_from_host();
    let (allowed_before, blocked_before) = (allowed.hits(), blocked.hits());

    let pi_prefix = env_path("FOX_PI_ACP_PREFIX");
    let homes = PrivateHomes::new(fox_data.join("sandbox-homes"));
    let private_home = homes.home_for(PROJECT_UUID, AgentBackend::Pi).unwrap();
    let policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: proj.clone(), writable: true }],
        private_home.clone(),
        vec![fixtures.clone(), launcher_dir.clone(), pi_prefix.clone()],
        vec![format!("127.0.0.1:{}", allowed.port)],
        HostOs::Linux,
    );
    assert_eq!(policy.deny_read_roots, default_deny_read_roots(HostOs::Linux));
    assert_eq!(policy.deny_read_roots, [PathBuf::from("/")]);
    let private_home = private_home.path;

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
    let session_tmp = launcher.session_tmp("t04");
    let prepared = prepare_launch(
        PermissionMode::Sandboxed,
        spec,
        "t04",
        SandboxSetup::Ready { launcher: &launcher, policy: &policy },
        HostOs::Linux,
    )
    .unwrap_or_else(|error| panic!("prepare: {error}"));
    assert_eq!(prepared.declaration.read_scope, ReadScope::AuthorizedOnly);
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

    // 0. Authorized directory: read and write work; pi completes the turn.
    let out = run(&mut connection, &session, "inside", "cat readme.txt && echo inside-ok > inside.txt && echo wrote").await;
    assert!(out.contains("authorized-content"), "authorized read failed");
    assert!(out.contains("STOP \"end_turn\""), "turn did not complete normally");
    assert_eq!(std::fs::read_to_string(proj.join("inside.txt")).unwrap().trim(), "inside-ok");

    // 1. Reads outside the authorized directory fail: beside it in the home,
    //    in the host /tmp, in a non-home data directory, and Fox's settings.
    for (id, path) in [
        ("readhome", secret_dir.join("secret.txt")),
        ("readtmp", host_tmp.join("secret.txt")),
        ("readdata", data_dir.join("secret.txt")),
    ] {
        let out = run(&mut connection, &session, id, &format!("cat {}; echo rc=$?", path.display())).await;
        assert!(!out.contains(&token), "secret leaked through bash from {}", path.display());
        assert!(out.contains("No such file or directory"), "{id}: expected the path to be hidden");
    }
    let out = turn(&mut connection, &session, &format!("READ:{} ID:readtool", b64url(&data_dir.join("secret.txt").display().to_string()))).await;
    eprintln!("--- readtool\n{}", tail(&out));
    assert!(!out.contains(&token), "secret leaked through pi read tool");
    let out = run(&mut connection, &session, "settings", &format!("cat {}", settings_path.display())).await;
    assert!(!out.contains("allowedDomains"), "sandbox settings readable from inside");
    // /etc is re-allowed only in part: hosts yes, hostname / shadow no.
    let out = run(&mut connection, &session, "etc", "test -r /etc/hosts && echo hosts-readable; cat /etc/hostname /etc/shadow; ls /etc | tr '\\n' ' '; echo").await;
    assert!(out.contains("hosts-readable"));
    assert!(out.contains("/etc/hostname: No such file or directory"), "/etc must not be allowed as a whole");

    // 2. Writes outside fail: a denied home dir, the host /tmp, Fox state.
    let out = run(
        &mut connection,
        &session,
        "writeout",
        &format!("echo pwn > {0}/pwn.txt; echo pwn > {1}/pwn.txt; echo pwn > {2}/pwn.txt; echo done", secret_dir.display(), host_tmp.display(), state.display()),
    )
    .await;
    assert!(out.contains("done"));
    assert!(!secret_dir.join("pwn.txt").exists(), "wrote into a denied directory");
    assert!(!host_tmp.join("pwn.txt").exists(), "wrote into the host /tmp");
    assert!(!state.join("pwn.txt").exists(), "wrote into Fox state");

    // 3. Network, with controlled targets (both proven reachable above).
    //    Allowed target through the srt proxy: its token comes back.
    let out = run(&mut connection, &session, "netallowed", &format!("curl -sS -m 15 --noproxy '' http://127.0.0.1:{}/; echo; echo rc=$?", allowed.port)).await;
    assert!(out.contains(&allowed.token), "allowlisted target not reachable through the proxy");
    assert!(allowed.hits() > allowed_before);
    //    Blocked target, plain HTTP: the proxy's own 403 refusal.
    let out = run(&mut connection, &session, "netblocked", &format!("curl -sS -i -m 15 --noproxy '' http://127.0.0.1:{}/; echo; echo rc=$?", blocked.port)).await;
    assert!(out.contains("403"), "expected a 403 from the srt proxy");
    assert!(out.contains("blocked by network allowlist") || out.contains("blocked-by-allowlist"), "expected srt's allowlist refusal text");
    assert!(!out.contains(&blocked.token));
    //    Blocked target, CONNECT tunnel: refused with 403 at CONNECT.
    let out = run(&mut connection, &session, "netconnect", &format!("curl -sS -p -m 15 --noproxy '' http://127.0.0.1:{}/; echo rc=$?", blocked.port)).await;
    assert!(out.contains("CONNECT tunnel failed, response 403"), "expected CONNECT 403 from the srt proxy");
    assert_eq!(blocked.hits(), blocked_before, "a refused request reached the blocked target");
    //    Bypassing the proxy: connection refused at once (curl exit 7), not a
    //    timeout (28) and not an HTTP status.
    let allowed_hits = allowed.hits();
    let out = run(&mut connection, &session, "directloop", &format!("curl -sS -m 15 --noproxy '*' -o /dev/null -w code=%{{http_code}} http://127.0.0.1:{}/; echo rc=$?", allowed.port)).await;
    assert!(out.contains("rc=7") && out.contains("code=000"), "direct loopback must be refused (curl exit 7)");
    assert_eq!(allowed.hits(), allowed_hits, "direct connection reached the host target");
    let out = run(&mut connection, &session, "directip", "curl -sS -m 15 --noproxy '*' -o /dev/null -w code=%{http_code} http://1.1.1.1/; echo rc=$?").await;
    assert!(out.contains("rc=7") && out.contains("code=000"), "direct public IP must fail to connect (curl exit 7), not time out");

    // 4. Private HOME (per project x backend) and per-session temp.
    let out = run(&mut connection, &session, "home", "echo HOME=$HOME TMPDIR=$TMPDIR; echo x > $HOME/probe.txt && echo homewrite-ok; echo t > $TMPDIR/t.txt && echo tmpwrite-ok").await;
    assert!(out.contains(&format!("HOME={}", private_home.display())));
    assert!(out.contains(&format!("TMPDIR={}", session_tmp.display())));
    assert!(private_home.join("probe.txt").exists());
    assert!(session_tmp.join("t.txt").exists());

    // 5. Close: process tree gone, per-session settings and temp removed,
    //    the project x backend HOME kept.
    connection.close().await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let left = processes_matching(&base.display().to_string());
    assert!(left.is_empty(), "processes left after close: {left:?}");
    assert!(!settings_path.exists(), "per-session settings not cleaned up");
    assert!(!session_tmp.exists(), "per-session temp not cleaned up");
    assert!(private_home.join("probe.txt").exists(), "private HOME must survive the session");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&host_tmp);
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// Fail-closed with a real (missing) srt: sandboxed refuses before anything is
/// spawned; full access still launches. Needs no srt, so it always runs.
#[tokio::test(flavor = "multi_thread")]
async fn fail_closed_without_srt_refuses_sandboxed_launch() {
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
