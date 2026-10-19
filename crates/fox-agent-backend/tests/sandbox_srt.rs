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
//!
//! ## Windows T04 (`real_srt_pi_acp_t04_windows`)
//!
//! Same opt-in (`#[ignore]` + `FOX_TEST_REAL_SRT=1`). Needs, on a machine where
//! `srt windows-install` has run (one UAC prompt) and as a normal user:
//!
//! * `FOX_RUNTIME_ROOT`: Fox's managed runtime directory, a folder the test
//!   user created (e.g. `%LOCALAPPDATA%\Fox\runtime`). srt can only grant the
//!   sandbox account read access to user-owned paths, so the next three must
//!   live under it (Node may instead be a machine-wide install under
//!   Program Files, which needs no grant).
//! * `FOX_SRT_PREFIX`, `FOX_PI_ACP_PREFIX` (npm prefixes as above) and
//!   `FOX_SRT_NODE` (absolute path to `node.exe`, >= 22.12).
//! * Git for Windows installed machine-wide (pi's bash tool looks for
//!   `%ProgramFiles%\Git\bin\bash.exe`). Probes run as PowerShell /
//!   `cmd.exe` / `curl.exe` (System32) children of that bash, so every probe
//!   also checks that grandchildren inherit the boundary.
//! * Optional `FOX_T04_EXPOSED_DIR`: a first-level folder on another fixed
//!   drive that inherits the stock root ACL (`Authenticated Users: Modify`),
//!   created by the tester. The test records whether the sandbox can read and
//!   write it and checks the exposure audit lists it.
//!
//! ```text
//! set FOX_TEST_REAL_SRT=1& cargo test -p fox-agent-backend --features acp --test sandbox_srt real_srt_pi_acp_t04_windows -- --include-ignored --nocapture
//! ```
//!
//! ## Sandboxed DeepSeek smoke (`real_srt_dsh_initialize_inside_sandbox`)
//!
//! Linux and Windows. Same opt-in plus `FOX_DSH_BIN` (absolute path to the
//! `dsh` / `dsh.cmd` entry of `@deepseek-ai/dsh@0.2.1-rc.2`, under
//! `FOX_RUNTIME_ROOT` on Windows): version check outside, then initialize and
//! session/new inside srt. No prompt (needs a key; a model turn is not T04).

#![cfg(feature = "acp")]
#![cfg_attr(not(any(target_os = "linux", windows)), allow(unused_imports, dead_code))]

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

#[cfg(unix)]
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

// ---------------------------------------------------------------------------
// Windows T04
// ---------------------------------------------------------------------------

/// `C:\X\y` -> `/c/X/y` (how Git Bash names a Windows path).
#[cfg(windows)]
fn msys(path: &Path) -> String {
    let text = path.display().to_string().replace('\\', "/");
    match text.split_once(":/") {
        Some((drive, rest)) if drive.len() == 1 => format!("/{}/{rest}", drive.to_ascii_lowercase()),
        _ => text,
    }
}

#[cfg(windows)]
fn b64_std(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * index)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A bash command line that runs `script` in Windows PowerShell through
/// `-EncodedCommand` (UTF-16LE base64), so no quoting survives into bash.
#[cfg(windows)]
fn ps(script: &str) -> String {
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()));
    let exe = system_root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    format!("'{}' -NoProfile -NonInteractive -EncodedCommand {}", msys(&exe), b64_std(&utf16))
}

#[cfg(windows)]
fn ps_literal(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "''"))
}

#[cfg(windows)]
fn ps_read(path: &Path) -> String {
    ps(&format!(
        "try {{ $c = Get-Content -LiteralPath {} -Raw -ErrorAction Stop; 'READ-OK:' + $c }} catch {{ 'READ-DENIED:' + $_.Exception.GetType().Name }}",
        ps_literal(path)
    ))
}

#[cfg(windows)]
fn ps_write(path: &Path) -> String {
    ps(&format!(
        "try {{ Set-Content -LiteralPath {} -Value 'pwn' -ErrorAction Stop; 'WRITE-OK' }} catch {{ 'WRITE-DENIED:' + $_.Exception.GetType().Name }}",
        ps_literal(path)
    ))
}

#[cfg(windows)]
fn curl_exe() -> String {
    let system_root = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into()));
    format!("'{}'", msys(&system_root.join(r"System32\curl.exe")))
}

/// Visible processes whose command line mentions `needle`. Command lines of
/// processes running as the `srt-sandbox` account may be hidden from a
/// non-elevated caller: an empty list here is not proof (see docs/sandbox.md
/// section 7.4 for the elevated check).
#[cfg(windows)]
fn windows_processes_matching(needle: &str) -> Vec<String> {
    let script = format!(
        "Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -and $_.CommandLine.Contains({}) -and $_.ProcessId -ne $PID }} | ForEach-Object {{ \"$($_.ProcessId) $($_.Name)\" }}",
        ps_literal(Path::new(needle))
    );
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell");
    String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|line| !line.is_empty()).map(str::to_owned).collect()
}

#[cfg(windows)]
fn icacls(path: &Path) -> String {
    let output = std::process::Command::new("icacls").arg(path).output().expect("icacls");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt (Windows) + pi-acp; run with FOX_TEST_REAL_SRT=1 ... -- --include-ignored (see file header)"]
async fn real_srt_pi_acp_t04_windows() {
    use fox_agent_backend::winacl::{audit_broad_exposure, RealWindowsAcl, WindowsAclProbe};
    require_real("real_srt_pi_acp_t04_windows");
    let probe = RealProbe;
    let runtime_root = env_path("FOX_RUNTIME_ROOT");
    let node = env_path("FOX_SRT_NODE");
    let install = SrtInstall::under_prefix(node, &env_path("FOX_SRT_PREFIX"));
    let ready = preflight(&install, &probe).unwrap_or_else(|error| panic!("preflight: {error}"));
    eprintln!("preflight ok: node {} srt {}", ready.node_version, ready.srt_version);

    // Layout: everything private under the real profile (whose stock DACL
    // admits only the user, SYSTEM and Administrators).
    let pid = std::process::id();
    let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let base = profile.join(format!(".fox-t04-{pid}"));
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
    // Two stock world-writable places srt-win install stamps with a write
    // deny for the sandbox account (vendor/srt-win-src/src/ambient.rs).
    let public = PathBuf::from(std::env::var_os("PUBLIC").unwrap_or_else(|| r"C:\Users\Public".into()));
    let program_data = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()));

    // Fixtures copied into the managed runtime directory (the repo checkout
    // may be on a drive the sandbox account cannot be granted).
    let fixtures_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    let fixtures = runtime_root.join(format!("t04-fixtures-{pid}"));
    let _ = std::fs::remove_dir_all(&fixtures);
    std::fs::create_dir_all(fixtures.join("sandbox")).unwrap();
    for file in ["pi-gated.cmd", "fox-approval-gate.ts"] {
        std::fs::copy(fixtures_src.join("pi").join(file), fixtures.join(file)).unwrap();
    }
    std::fs::copy(fixtures_src.join("sandbox/scripted-cmd-provider.ts"), fixtures.join("sandbox/scripted-cmd-provider.ts")).unwrap();

    let exposed = std::env::var_os("FOX_T04_EXPOSED_DIR").map(PathBuf::from);
    if let Some(dir) = &exposed {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("fox-t04-exposed.txt"), &token).unwrap();
        eprintln!("exposed dir {}: broad access {:?}", dir.display(), RealWindowsAcl.broad_access(dir));
    }
    let audit = audit_broad_exposure();
    eprintln!("exposure audit: {} drive(s), {} exposed, {} error(s)", audit.drives.len(), audit.exposed.len(), audit.errors.len());
    for item in &audit.exposed {
        eprintln!("  exposed {} read={} write={} {:?}", item.path.display(), item.access.read, item.access.write, item.access.groups);
    }

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
        vec![fixtures.clone(), pi_prefix.clone()],
        vec![format!("127.0.0.1:{}", allowed.port)],
        HostOs::Windows,
    );
    assert_eq!(policy.deny_read_roots, default_deny_read_roots(HostOs::Windows));
    assert!(policy.deny_read_roots.is_empty(), "no denyRead on Windows by default");
    let private_home = private_home.path;
    let bin = pi_prefix.join(r"node_modules\.bin");
    let spec = LaunchSpec {
        backend: AgentBackend::Pi,
        program: bin.join("pi-acp.cmd").display().to_string(),
        args: vec![],
        version_pin: fox_agent_backend::launch::PI_ACP_VERSION,
        credential_env: &[],
        experimental: false,
        session_env: vec![
            ("PI_ACP_PI_COMMAND".into(), fixtures.join("pi-gated.cmd").display().to_string()),
            ("FOX_PI_BIN".into(), bin.join("pi.cmd").display().to_string()),
            // No quotes possible in session variables on Windows (they reach
            // the sandbox as `set "K=V"`): the path travels on its own.
            ("FOX_PI_TEST_EXTENSION".into(), fixtures.join(r"sandbox\scripted-cmd-provider.ts").display().to_string()),
            ("FOX_PI_EXTRA_ARGS".into(), "--provider foxcmd --model scripted".into()),
        ],
    };
    let launcher = SrtLauncher::new(ready, state.clone()).with_managed_runtime_root(runtime_root.clone());
    let session_tmp = launcher.session_tmp("t04");
    let prepared = prepare_launch(
        PermissionMode::Sandboxed,
        spec,
        "t04",
        SandboxSetup::Ready { launcher: &launcher, policy: &policy },
        HostOs::Windows,
    )
    .unwrap_or_else(|error| panic!("prepare (ownership preflight included): {error} remedy={:?}", error.remedy));
    assert_eq!(prepared.declaration.read_scope, ReadScope::UsersGroupReadable);
    assert!(!prepared.declaration.read_scope_verified && !prepared.declaration.directory_isolation);
    let settings_path = prepared.sandbox.as_ref().unwrap().settings_path().to_path_buf();
    let settings = std::fs::read_to_string(&settings_path).unwrap();
    eprintln!("settings: {settings}");
    let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap();
    assert_eq!(parsed["filesystem"]["denyRead"], serde_json::json!([]), "no denyRead C:\\Users");
    let allow_read = parsed["filesystem"]["allowRead"].to_string().to_ascii_lowercase();
    assert!(!allow_read.contains("program files"), "no ACE on Users-readable system paths");

    let mut connection = AcpConnection::spawn_prepared(prepared, &proj).await.expect("spawn pi-acp inside srt");
    let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
        panic!("expected Initialized first");
    };
    check_initialize(&initialize_response).unwrap();
    let session = connection
        .new_session(proj.clone())
        .await
        .unwrap_or_else(|error| panic!("session/new: {error}\n{}", connection.stderr_tail()));

    // 0. Positive controls: identity, authorized read (bash grandchild and
    //    pi's own read tool), authorized write.
    let out = run(&mut connection, &session, "whoami", &ps("'WHO=' + [Security.Principal.WindowsIdentity]::GetCurrent().Name")).await;
    assert!(out.to_ascii_lowercase().contains("srt-sandbox"), "probe must run as the srt-sandbox account");
    let out = run(&mut connection, &session, "inside", &ps_read(&proj.join("readme.txt"))).await;
    assert!(out.contains("READ-OK:authorized-content"), "authorized read failed");
    assert!(out.contains("STOP \"end_turn\""), "turn did not complete normally");
    let out = turn(&mut connection, &session, &format!("READ:{} ID:readtool-in", b64url(&proj.join("readme.txt").display().to_string()))).await;
    assert!(out.contains("authorized-content"), "pi read tool cannot read the authorized folder");
    let out = run(&mut connection, &session, "insidewrite", &ps_write(&proj.join("inside.txt"))).await;
    assert!(out.contains("WRITE-OK"));
    assert!(proj.join("inside.txt").exists());

    // 1. Reads outside: the user's own profile (beside the project), Fox
    //    state (settings), through PowerShell, cmd.exe and pi's read tool.
    for (id, path) in [("readprofile", secret_dir.join("secret.txt")), ("readsettings", settings_path.clone())] {
        let out = run(&mut connection, &session, id, &ps_read(&path)).await;
        assert!(!out.contains(&token) && !out.contains("allowedDomains"), "{id}: leaked");
        assert!(out.contains("READ-DENIED"), "{id}: expected a denial");
    }
    let out = run(&mut connection, &session, "readcmd", &ps(&format!("cmd.exe /d /c type {} 2>&1", ps_literal(&secret_dir.join("secret.txt")).replace('\'', "\"")))).await;
    assert!(!out.contains(&token), "secret leaked through a cmd.exe great-grandchild");
    let out = turn(&mut connection, &session, &format!("READ:{} ID:readtool-out", b64url(&secret_dir.join("secret.txt").display().to_string()))).await;
    assert!(!out.contains(&token), "secret leaked through pi read tool");

    // 2. Writes outside: profile, Fox state, %PUBLIC%, %ProgramData%.
    let public_probe = public.join(format!("fox-t04-{pid}.txt"));
    let program_data_probe = program_data.join(format!("fox-t04-{pid}.txt"));
    for (id, path) in [
        ("writeprofile", secret_dir.join("pwn.txt")),
        ("writestate", state.join("pwn.txt")),
        ("writepublic", public_probe.clone()),
        ("writeprogramdata", program_data_probe.clone()),
    ] {
        let out = run(&mut connection, &session, id, &ps_write(&path)).await;
        assert!(out.contains("WRITE-DENIED"), "{id}: expected a denial");
        assert!(!path.exists(), "{id}: wrote {}", path.display());
    }

    // 3. Exposure (optional): measured, and must agree with the audit.
    if let Some(dir) = &exposed {
        let out = run(&mut connection, &session, "readexposed", &ps_read(&dir.join("fox-t04-exposed.txt"))).await;
        let readable = out.contains(&token);
        let out = run(&mut connection, &session, "writeexposed", &ps_write(&dir.join("fox-t04-sandbox-wrote.txt"))).await;
        let writable = out.contains("WRITE-OK");
        eprintln!("MEASURE exposed dir {}: sandbox read={readable} write={writable}", dir.display());
        let first_level = dir.parent().is_some_and(|parent| parent.parent().is_none());
        if (readable || writable) && first_level {
            assert!(audit.exposed.iter().any(|item| item.path == *dir), "audit must list a folder the sandbox could reach");
        }
        let _ = std::fs::remove_file(dir.join("fox-t04-sandbox-wrote.txt"));
        let _ = std::fs::remove_file(dir.join("fox-t04-exposed.txt"));
    }

    // 4. Network: allowlisted through the proxy, blocked (HTTP and CONNECT),
    //    direct loopback and direct public IP refused by WFP.
    let curl = curl_exe();
    let out = run(&mut connection, &session, "netallowed", &format!("{curl} -sS -m 15 --noproxy '' http://127.0.0.1:{}/; echo; echo rc=$?", allowed.port)).await;
    assert!(out.contains(&allowed.token), "allowlisted target not reachable through the proxy");
    assert!(allowed.hits() > allowed_before);
    let out = run(&mut connection, &session, "netblocked", &format!("{curl} -sS -i -m 15 --noproxy '' http://127.0.0.1:{}/; echo; echo rc=$?", blocked.port)).await;
    assert!(out.contains("403"), "expected a 403 from the srt proxy");
    assert!(!out.contains(&blocked.token));
    let out = run(&mut connection, &session, "netconnect", &format!("{curl} -sS -p -m 15 --noproxy '' http://127.0.0.1:{}/; echo rc=$?", blocked.port)).await;
    assert!(out.contains("403"), "expected CONNECT 403 from the srt proxy");
    assert_eq!(blocked.hits(), blocked_before, "a refused request reached the blocked target");
    let allowed_hits = allowed.hits();
    let out = run(&mut connection, &session, "directloop", &format!("{curl} -sS -m 15 --noproxy '*' -o NUL -w code=%{{http_code}} http://127.0.0.1:{}/; echo rc=$?", allowed.port)).await;
    assert!(out.contains("rc=7") && out.contains("code=000"), "direct loopback must be refused by WFP (curl exit 7)");
    assert_eq!(allowed.hits(), allowed_hits, "direct connection reached the host target");
    let out = run(&mut connection, &session, "directip", &format!("{curl} -sS -m 15 --noproxy '*' -o NUL -w code=%{{http_code}} http://1.1.1.1/; echo rc=$?")).await;
    assert!(out.contains("rc=7") && out.contains("code=000"), "direct public IP must fail to connect (curl exit 7)");
    // DNS through the system resolver is NOT fenced by srt on Windows (srt
    // README, Known limitations): recorded, never asserted as blocked.
    let out = run(&mut connection, &session, "dns", &ps("try { 'DNS=' + (([System.Net.Dns]::GetHostAddresses('example.com') | ForEach-Object { $_.IPAddressToString }) -join ',') } catch { 'DNS-FAIL:' + $_.Exception.GetType().Name }")).await;
    eprintln!("MEASURE dns via system resolver: {}", if out.contains("DNS=") && !out.contains("DNS=,") { "resolves (unfenced, as documented)" } else { "did not resolve" });

    // 5. Private HOME / temp as seen by a grandchild.
    let out = run(&mut connection, &session, "home", &ps("'HOME=' + $env:HOME; 'USERPROFILE=' + $env:USERPROFILE; 'TEMP=' + $env:TEMP")).await;
    // Git Bash may hand HOME on in its own form (/c/...); either is the
    // private HOME. USERPROFILE / TEMP are recorded (Fox sets them through
    // the `set` prefix; a program reading known folders through the shell
    // API still gets the srt-sandbox profile).
    assert!(
        out.contains(&format!("HOME={}", private_home.display())) || out.contains(&format!("HOME={}", msys(&private_home))),
        "HOME is not the private HOME"
    );
    eprintln!("MEASURE USERPROFILE/TEMP inside: see the transcript of id=home above");
    let _ = session_tmp;

    // 6. Close: job terminated, per-session state removed; ACL residue and
    //    leftover processes measured.
    let report = connection.close_with_report().await;
    eprintln!("MEASURE close: {report:?}");
    tokio::time::sleep(Duration::from_secs(3)).await;
    let left = windows_processes_matching(&base.display().to_string());
    eprintln!("MEASURE visible processes left: {left:?} (srt-sandbox processes may be invisible without elevation)");
    assert!(left.is_empty(), "processes left after close: {left:?}");
    assert!(!settings_path.exists(), "per-session settings not cleaned up");
    let acl_after = icacls(&proj);
    eprintln!("MEASURE icacls proj after close (srt-sandbox ACE present = srt reset did not run):\n{acl_after}");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&fixtures);
}

// ---------------------------------------------------------------------------
// DeepSeek (dsh) initialize inside srt
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", windows))]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt + dsh 0.2.1-rc.2; run with FOX_TEST_REAL_SRT=1 FOX_DSH_BIN=<abs> ... -- --include-ignored"]
async fn real_srt_dsh_initialize_inside_sandbox() {
    use fox_agent_backend::backends::deepseek;
    require_real("real_srt_dsh_initialize_inside_sandbox");
    let os = HostOs::current();
    let dsh = env_path("FOX_DSH_BIN");
    assert!(dsh.is_absolute(), "FOX_DSH_BIN must be absolute");
    // <prefix>/node_modules/.bin/dsh -> <prefix>
    let dsh_prefix = dsh.parent().and_then(Path::parent).and_then(Path::parent).expect("dsh under <prefix>/node_modules/.bin").to_path_buf();
    let version = std::process::Command::new(&dsh).arg("--version").output().expect("dsh --version");
    deepseek::check_version(&String::from_utf8_lossy(&version.stdout)).expect("pinned dsh 0.2.1-rc.2");

    let probe = RealProbe;
    let node = std::env::var_os("FOX_SRT_NODE").map(PathBuf::from).or_else(|| probe.which("node")).expect("node");
    let install = SrtInstall::under_prefix(node, &env_path("FOX_SRT_PREFIX"));
    let ready = preflight(&install, &probe).unwrap_or_else(|error| panic!("preflight: {error}"));
    let pid = std::process::id();
    let home_base = PathBuf::from(std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).expect("home"));
    let base = home_base.join(format!(".fox-dsh-srt-{pid}"));
    let _ = std::fs::remove_dir_all(&base);
    let work = base.join("work");
    let state = base.join("fox-data").join("state");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    let patch_dir = base.join("fox-data").join("dsh-patch");
    std::fs::create_dir_all(&patch_dir).unwrap();
    let mapping = deepseek::policy_patch(&deepseek::HostPolicy { workspace_write: false, network: false });
    let patch = patch_dir.join("fox-dsh-patch.yml");
    std::fs::write(&patch, &mapping.patch_yaml).unwrap();
    let homes = PrivateHomes::new(base.join("fox-data").join("sandbox-homes"));
    let policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: work.clone(), writable: false }],
        homes.home_for(PROJECT_UUID, AgentBackend::Deepseek).unwrap(),
        vec![dsh_prefix, patch_dir],
        vec!["api.deepseek.com".into()],
        os,
    );
    let mut launcher = SrtLauncher::new(ready, state);
    if os == HostOs::Windows {
        launcher = launcher.with_managed_runtime_root(env_path("FOX_RUNTIME_ROOT"));
    }
    let spec = deepseek::launch_spec(&dsh.display().to_string(), &patch).unwrap();
    let prepared = prepare_launch(PermissionMode::Sandboxed, spec, "dsh1", SandboxSetup::Ready { launcher: &launcher, policy: &policy }, os)
        .unwrap_or_else(|error| panic!("prepare: {error} remedy={:?}", error.remedy));
    let mut connection = AcpConnection::spawn_prepared(prepared, &work).await.expect("spawn dsh inside srt");
    let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
        panic!("expected Initialized first");
    };
    deepseek::check_initialize(&initialize_response).expect("dsh acp initialize inside srt");
    let session = connection
        .new_session(work.clone())
        .await
        .unwrap_or_else(|error| panic!("session/new inside srt: {error}\n{}", connection.stderr_tail()));
    assert!(!session.is_empty());
    eprintln!("MEASURE dsh close: {:?}", connection.close_with_report().await);
    let _ = std::fs::remove_dir_all(&base);
}
