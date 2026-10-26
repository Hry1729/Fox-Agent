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
//! The model is [`ScriptedModel`], an HTTP server in this test process,
//! outside the sandbox, on a port chosen before srt starts and listed in the
//! allowlist (`127.0.0.1:<port>`); pi's provider
//! `fixtures/sandbox/scripted-cmd-provider.ts` (test only, client only) gets
//! its address as `FOX_SCRIPTED_MODEL_URL`. So the model call takes the same
//! path as a real model API (srt proxy, allowlist). pi, pi-acp, srt and
//! bubblewrap are real. Every tool call goes through the Fox
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
//! * The one-time setup done with `scripts/windows/enable-sandbox.ps1`
//!   (one UAC prompt): the runtime package copied to
//!   `%ProgramFiles%\Fox\sandbox-runtime\<version>` (administrator-owned,
//!   Users read+execute), `%ProgramData%\Fox\sandbox` created, srt
//!   `windows-install` run.
//! * `FOX_RUNTIME_ROOT`: that runtime directory (or an override with the same
//!   ACL rules; a folder under the user profile is refused with
//!   `sandbox_runtime_not_installed`). The next three live under it.
//! * `FOX_SRT_PREFIX`, `FOX_PI_ACP_PREFIX` (npm prefixes as above) and
//!   `FOX_SRT_NODE` (absolute path to `node.exe`, >= 22.12).
//! * `<FOX_RUNTIME_ROOT>\t04-fixtures`: `pi-gated.cmd`,
//!   `fox-approval-gate.ts` (from `fixtures/pi`) and
//!   `sandbox\scripted-cmd-provider.ts` (from `fixtures/sandbox`), placed
//!   there by the setup (put them in the source package) or by an
//!   administrator; the test cannot write into the runtime.
//! * Optional `FOX_T04_PROJECT_PARENT` (default
//!   `%USERPROFILE%\Documents\中文 空格`): the project is created there, so
//!   the ancestor metadata ACEs (profile root, Documents, `中文 空格`) are
//!   exercised; the test checks the sandbox cannot list Documents and that
//!   the ACEs are gone after close.
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
//! The `--patch` file is the runtime asset
//! `<runtime>/dsh-patches/read-only-noweb.yml` (read-only assets must be in
//! the managed runtime; on Windows the setup installs it, on Linux the test
//! writes it into `FOX_RUNTIME_ROOT` or a private runtime directory).

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

/// TEST ONLY. The scripted "model" for T04, served from this test process,
/// outside the sandbox (docs/sandbox.md 7.13). pi reaches it through the srt
/// proxy like any model API: `127.0.0.1:<port>` is in the network allowlist,
/// and the address reaches pi's scripted provider
/// (`fixtures/sandbox/scripted-cmd-provider.ts`, client only) as
/// `FOX_SCRIPTED_MODEL_URL`. Before round 6 the server ran inside pi, inside
/// the sandbox, on a random port that was not in the allowlist: on Windows
/// srt drops NO_PROXY, so pi's request went to the proxy and came back
/// `403 Connection blocked by network allowlist`.
///
/// OpenAI Chat Completions compatible (stream only), same script as before:
///
///  - `RUN:<base64url command>`  -> one `bash` tool call running that command
///  - `READ:<base64url path>`    -> one `read` tool call of that path
///  - last message is a tool result -> text `tool-result: <first 400 chars>`
///  - anything else -> text `scripted hello`
struct ScriptedModel {
    port: u16,
    requests: Arc<AtomicUsize>,
}

impl ScriptedModel {
    fn start() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind scripted model");
        let port = listener.local_addr().unwrap().port();
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let counter = Arc::clone(&counter);
                std::thread::spawn(move || {
                    if let Some((path, body)) = read_http_request(&stream) {
                        counter.fetch_add(1, Ordering::SeqCst);
                        let (content_type, payload) = scripted_reply(&path, &body);
                        let mut stream = stream;
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\ncache-control: no-cache\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                            payload.len()
                        );
                        let _ = stream.flush();
                    }
                });
            }
        });
        Self { port, requests }
    }

    /// The base URL pi's provider uses (`/v1` like the OpenAI API).
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// The allowlist entry for it.
    fn allow_entry(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
}

/// One HTTP/1.1 request: (path, body). Content-Length and chunked bodies.
fn read_http_request(stream: &std::net::TcpStream) -> Option<(String, Vec<u8>)> {
    use std::io::BufRead;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let mut reader = std::io::BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let path = line.split_whitespace().nth(1)?.to_owned();
    let (mut length, mut chunked) = (0usize, false);
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            return None;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            let value = value.trim();
            if name.eq_ignore_ascii_case("content-length") {
                length = value.parse().ok()?;
            } else if name.eq_ignore_ascii_case("transfer-encoding") && value.to_ascii_lowercase().contains("chunked") {
                chunked = true;
            }
        }
    }
    let mut body = Vec::new();
    if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).ok()?;
            let size = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            let mut chunk = vec![0u8; size + 2];
            reader.read_exact(&mut chunk).ok()?;
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    } else {
        body.resize(length, 0);
        reader.read_exact(&mut body).ok()?;
    }
    Some((path, body))
}

fn content_text(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|part| part["text"].as_str()).collect(),
        _ => String::new(),
    }
}

fn b64url_decode(text: &str) -> Option<String> {
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        bits = bits << 6 | value as u32;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `(RUN|READ):<b64url>( ID:<id>)?` anywhere in the text, first match wins.
fn scripted_directive(text: &str) -> Option<(&'static str, String, String)> {
    let is_b64 = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    for (index, _) in text.char_indices() {
        let rest = &text[index..];
        let (kind, after) = if let Some(after) = rest.strip_prefix("RUN:") {
            ("RUN", after)
        } else if let Some(after) = rest.strip_prefix("READ:") {
            ("READ", after)
        } else {
            continue;
        };
        let end = after.find(|c: char| !is_b64(c)).unwrap_or(after.len());
        if end == 0 {
            continue;
        }
        let payload = b64url_decode(&after[..end])?;
        let tail = &after[end..];
        let trimmed = tail.trim_start();
        let id = if trimmed.len() < tail.len() {
            trimmed.strip_prefix("ID:").map(|id| id[..id.find(|c: char| !is_b64(c)).unwrap_or(id.len())].to_owned()).filter(|id| !id.is_empty())
        } else {
            None
        };
        return Some((kind, payload, id.unwrap_or_else(|| "x".to_owned())));
    }
    None
}

/// (content type, body) for one request.
fn scripted_reply(path: &str, body: &[u8]) -> (&'static str, String) {
    if path.ends_with("/models") {
        return ("application/json", serde_json::json!({ "data": [{ "id": "scripted" }] }).to_string());
    }
    let request: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
    let empty = Vec::new();
    let messages = request["messages"].as_array().unwrap_or(&empty);
    let base = |choices: serde_json::Value| serde_json::json!({ "id": "scripted-1", "object": "chat.completion.chunk", "created": 0, "model": "scripted", "choices": choices });
    let mut events = Vec::new();
    let finish = |events: &mut Vec<serde_json::Value>, reason: &str| {
        events.push(base(serde_json::json!([{ "index": 0, "delta": {}, "finish_reason": reason }])));
        let mut usage = base(serde_json::json!([]));
        usage["usage"] = serde_json::json!({ "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 });
        events.push(usage);
    };
    let text = |events: &mut Vec<serde_json::Value>, value: String| {
        events.push(base(serde_json::json!([{ "index": 0, "delta": { "role": "assistant", "content": value }, "finish_reason": null }])));
        finish(events, "stop");
    };
    let last = messages.last();
    if last.is_some_and(|message| message["role"] == "tool") {
        let result: String = content_text(&last.unwrap()["content"]).chars().take(400).collect();
        text(&mut events, format!("tool-result: {result}"));
    } else {
        let last_user = messages.iter().rev().find(|message| message["role"] == "user");
        match last_user.and_then(|message| scripted_directive(&content_text(&message["content"]))) {
            None => text(&mut events, "scripted hello".to_owned()),
            Some((kind, payload, id)) => {
                let (name, args) = if kind == "RUN" {
                    ("bash", serde_json::json!({ "command": payload }))
                } else {
                    ("read", serde_json::json!({ "path": payload }))
                };
                events.push(base(serde_json::json!([{
                    "index": 0,
                    "delta": {
                        "role": "assistant",
                        "tool_calls": [{ "index": 0, "id": format!("call_{id}"), "type": "function", "function": { "name": name, "arguments": args.to_string() } }],
                    },
                    "finish_reason": null,
                }])));
                finish(&mut events, "tool_calls");
            }
        }
    }
    let mut out = String::new();
    for event in events {
        out.push_str(&format!("data: {event}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    ("text/event-stream", out)
}

/// The scripted model's parsing and replies, without srt (always runs).
#[test]
fn scripted_model_serves_tool_calls_over_http() {
    let model = ScriptedModel::start();
    let post = |body: &str| -> String {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", model.port)).unwrap();
        write!(stream, "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    let run = serde_json::json!({ "messages": [{ "role": "user", "content": format!("RUN:{} ID:who", b64url("echo 中文 && ls")) }] });
    let out = post(&run.to_string());
    assert!(out.starts_with("HTTP/1.1 200") && out.contains("text/event-stream"), "{out}");
    assert!(out.contains("\"call_who\"") && out.contains("\"bash\"") && out.contains("echo 中文 && ls") && out.contains("tool_calls"), "{out}");
    assert!(out.ends_with("data: [DONE]\n\n"));
    let read = serde_json::json!({ "messages": [{ "role": "user", "content": [{ "type": "text", "text": format!("READ:{}", b64url("C:\\x y\\a.txt")) }] }] });
    let out = post(&read.to_string());
    assert!(out.contains("\"read\"") && out.contains("\"call_x\"") && out.contains("a.txt"), "{out}");
    let tool = serde_json::json!({ "messages": [{ "role": "user", "content": "RUN:eA" }, { "role": "tool", "content": "R".repeat(500) }] });
    let out = post(&tool.to_string());
    assert!(out.contains(&format!("tool-result: {}\"", "R".repeat(400))), "{out}");
    let hello = serde_json::json!({ "messages": [{ "role": "user", "content": "hi" }] });
    assert!(post(&hello.to_string()).contains("scripted hello"));
    // Chunked request body and the /models listing.
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", model.port)).unwrap();
    let body = hello.to_string();
    write!(stream, "POST /v1/chat/completions HTTP/1.1\r\nHost: x\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{body}\r\n0\r\n\r\n", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("scripted hello"), "{response}");
    let mut stream = std::net::TcpStream::connect(("127.0.0.1", model.port)).unwrap();
    write!(stream, "GET http://127.0.0.1:{}/v1/models HTTP/1.1\r\nHost: x\r\n\r\n", model.port).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.contains("\"scripted\"") && response.contains("application/json"));
    assert_eq!(model.requests(), 6);
    assert_eq!(model.url(), format!("http://127.0.0.1:{}/v1", model.port));
    assert_eq!(model.allow_entry(), format!("127.0.0.1:{}", model.port));
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
            BackendEvent::Failed(error) | BackendEvent::TurnFailed { error, .. } => panic!("backend failed: {error}\n{transcript}\n{}", connection.stderr_tail()),
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
    // The scripted model runs here, outside the sandbox, on a port chosen
    // before srt starts; only that port (and the allowed target) is listed.
    let model = ScriptedModel::start();

    let pi_prefix = env_path("FOX_PI_ACP_PREFIX");
    let homes = PrivateHomes::new(fox_data.join("sandbox-homes"));
    let private_home = homes.home_for(PROJECT_UUID, AgentBackend::Pi).unwrap();
    let policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: proj.clone(), writable: true }],
        private_home.clone(),
        vec![fixtures.clone(), launcher_dir.clone(), pi_prefix.clone()],
        vec![format!("127.0.0.1:{}", allowed.port), model.allow_entry()],
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
            ("FOX_SCRIPTED_MODEL_URL".into(), model.url()),
            // Linux only: srt sets NO_PROXY=localhost,127.0.0.1,... (bwrap
            // has its own loopback), so pi would connect to 127.0.0.1 inside
            // the network namespace and find nothing. Cleared for the pi
            // process so its model request goes through the srt proxy to the
            // host loopback like a real model API (srt on Windows already
            // drops NO_PROXY). Probes below pass --noproxy explicitly.
            ("NO_PROXY".into(), String::new()),
            ("no_proxy".into(), String::new()),
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
    // The UI declaration lists exactly what the settings file says.
    let written: serde_json::Value = serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    let listed = |key: &str| -> Vec<String> {
        written["filesystem"][key].as_array().unwrap().iter().map(|entry| entry["path"].as_str().unwrap().to_owned()).collect()
    };
    assert_eq!(prepared.declaration.readable_roots, listed("allowRead"));
    assert_eq!(prepared.declaration.writable_roots, listed("allowWrite"));
    assert_eq!(prepared.declaration.denied_read_roots, listed("denyRead"));
    assert!(prepared.declaration.readable_roots.contains(&session_tmp.display().to_string()));

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
    assert!(model.requests() >= 2, "the model turn did not reach the scripted model outside the sandbox");
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

    // 5. Close: process tree gone (and confirmed: process group plus the
    //    tracked descendants, bwrap's included), per-session settings and
    //    temp removed, the project x backend HOME kept.
    let report = connection.close_with_report().await;
    eprintln!("close report: {report:?}");
    assert_eq!(report.confirmation, fox_agent_backend::port::StopConfirmation::Confirmed, "{report:?}");
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

/// Windows sandbox state shared by the real tests: srt's sandbox SID (also
/// recorded in the SID ledger), the per-user private root under
/// `%ProgramData%\Fox\sandbox\<SID>` and the ancestor ACE coordinator.
#[cfg(windows)]
struct WindowsSandbox {
    sid: String,
    layout: fox_agent_backend::winruntime::WindowsPrivateLayout,
    coordinator: fox_agent_backend::winancestor::AncestorAceCoordinator,
}

#[cfg(windows)]
fn windows_sandbox(state: &Path) -> WindowsSandbox {
    use fox_agent_backend::winacl::RealWindowsAcl;
    use fox_agent_backend::winancestor::{srt_acl_lock_path, AncestorAceCoordinator};
    use fox_agent_backend::winruntime::{prepare_windows_private_root, read_srt_registry, record_srt_sid, WindowsPrivateLayout};
    let registry = read_srt_registry(&RealProbe).unwrap_or_else(|error| panic!("srt registry: {error}"));
    let known = record_srt_sid(state, &registry).unwrap();
    let layout = WindowsPrivateLayout::for_current_user(&RealWindowsAcl).unwrap();
    prepare_windows_private_root(&layout, &RealWindowsAcl, &RealWindowsAcl, &known)
        .unwrap_or_else(|error| panic!("private root: {error} remedy={:?}", error.remedy));
    let acl = Arc::new(RealWindowsAcl);
    let coordinator = AncestorAceCoordinator::new(state.to_path_buf(), acl.clone(), acl).with_srt_acl_lock(srt_acl_lock_path());
    WindowsSandbox { sid: registry.sandbox_user_sid, layout, coordinator }
}

#[cfg(windows)]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt (Windows) + pi-acp; run with FOX_TEST_REAL_SRT=1 ... -- --include-ignored (see file header)"]
async fn real_srt_pi_acp_t04_windows() {
    use fox_agent_backend::winacl::{audit_broad_exposure, RealWindowsAcl, WindowsAclProbe, WindowsAclWriter};
    use fox_agent_backend::winancestor::ANCESTOR_ACE_MASK;
    require_real("real_srt_pi_acp_t04_windows");
    let probe = RealProbe;
    let runtime_root = env_path("FOX_RUNTIME_ROOT");
    let node = env_path("FOX_SRT_NODE");
    let install = SrtInstall::under_prefix(node, &env_path("FOX_SRT_PREFIX"));
    let ready = preflight(&install, &probe).unwrap_or_else(|error| panic!("preflight: {error}"));
    eprintln!("preflight ok: node {} srt {}", ready.node_version, ready.srt_version);

    // Layout: Fox state and a secret under the real profile (stock DACL:
    // the user, SYSTEM, Administrators); the project under
    // Documents\中文 空格 (ancestor ACEs); private HOME / temp under
    // %ProgramData%\Fox\sandbox\<SID>.
    let pid = std::process::id();
    let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("USERPROFILE"));
    let base = profile.join(format!(".fox-t04-{pid}"));
    let _ = std::fs::remove_dir_all(&base);
    let project_parent = std::env::var_os("FOX_T04_PROJECT_PARENT").map(PathBuf::from).unwrap_or_else(|| profile.join("Documents").join("中文 空格"));
    let proj = project_parent.join(format!("proj-{pid}"));
    let secret_dir = base.join("secret");
    let fox_data = base.join("fox-data");
    let state = fox_data.join("state");
    for dir in [&proj, &secret_dir, &state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let sandbox = windows_sandbox(&state);
    let token = format!("FOXSECRET{pid}");
    std::fs::write(secret_dir.join("secret.txt"), &token).unwrap();
    std::fs::write(proj.join("readme.txt"), "authorized-content").unwrap();
    // Two stock world-writable places srt-win install stamps with a write
    // deny for the sandbox account (vendor/srt-win-src/src/ambient.rs).
    let public = PathBuf::from(std::env::var_os("PUBLIC").unwrap_or_else(|| r"C:\Users\Public".into()));
    let program_data = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into()));

    // Fixtures live in the managed runtime directory (administrator-owned:
    // installed by the setup, not written by this test).
    let fixtures = runtime_root.join("t04-fixtures");
    for file in ["pi-gated.cmd", "fox-approval-gate.ts", r"sandbox\scripted-cmd-provider.ts"] {
        assert!(
            fixtures.join(file).is_file(),
            "{} missing: copy fixtures/pi/{{pi-gated.cmd,fox-approval-gate.ts}} and fixtures/sandbox/scripted-cmd-provider.ts into <runtime>\\t04-fixtures as administrator (or include them in the enable-sandbox source package)",
            fixtures.join(file).display()
        );
    }

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
    // Scripted model outside the sandbox (fixes round 5's 403: the in-pi
    // server's random port was not in the allowlist and srt-win drops
    // NO_PROXY, so the request went to the proxy and was refused).
    let model = ScriptedModel::start();

    let pi_prefix = env_path("FOX_PI_ACP_PREFIX");
    let homes = PrivateHomes::new(sandbox.layout.homes_root());
    let private_home = homes.home_for(PROJECT_UUID, AgentBackend::Pi).unwrap();
    let policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: proj.clone(), writable: true }],
        private_home.clone(),
        vec![fixtures.clone(), pi_prefix.clone()],
        vec![format!("127.0.0.1:{}", allowed.port), model.allow_entry()],
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
            ("FOX_SCRIPTED_MODEL_URL".into(), model.url()),
        ],
    };
    let launcher = SrtLauncher::new(ready, state.clone()).with_managed_runtime_root(runtime_root.clone()).with_tmp_root(sandbox.layout.tmp_root());
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
    let ancestors = prepared.windows_ancestor_aces.clone();
    eprintln!("ancestor ACEs: {ancestors:?}");
    assert!(ancestors.iter().any(|path| path == &profile), "profile root needs an ancestor ACE");
    assert!(ancestors.iter().any(|path| path == &project_parent), "{} needs an ancestor ACE", project_parent.display());

    // Global srt transition: ancestor ACEs before srt starts, reconcile after.
    let mut transition = sandbox.coordinator.transition().unwrap();
    let added = transition.begin_session("t04", &sandbox.sid, &ancestors).unwrap_or_else(|error| panic!("ancestor ACEs: {error} remedy={:?}", error.remedy));
    eprintln!("MEASURE ancestor ACEs added: {added:?}");
    let mut connection = AcpConnection::spawn_prepared(prepared, &proj).await.expect("spawn pi-acp inside srt");
    let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
        panic!("expected Initialized first");
    };
    eprintln!("MEASURE reconcile after start: {:?}", transition.finish());
    for path in &ancestors {
        assert!(RealWindowsAcl.has_exact_allow_ace(path, &sandbox.sid, ANCESTOR_ACE_MASK).unwrap(), "ancestor ACE missing on {}", path.display());
    }
    check_initialize(&initialize_response).unwrap();
    let session = connection
        .new_session(proj.clone())
        .await
        .unwrap_or_else(|error| panic!("session/new: {error}\n{}", connection.stderr_tail()));

    // 0. Positive controls: identity, authorized read (bash grandchild and
    //    pi's own read tool), authorized write.
    let out = run(&mut connection, &session, "whoami", &ps("'WHO=' + [Security.Principal.WindowsIdentity]::GetCurrent().Name")).await;
    assert!(model.requests() >= 2, "the model turn did not reach the scripted model outside the sandbox (403 from the srt proxy?)\n{}", tail(&out));
    assert!(out.to_ascii_lowercase().contains("srt-sandbox"), "probe must run as the srt-sandbox account");
    let out = run(&mut connection, &session, "inside", &ps_read(&proj.join("readme.txt"))).await;
    assert!(out.contains("READ-OK:authorized-content"), "authorized read failed");
    assert!(out.contains("STOP \"end_turn\""), "turn did not complete normally");
    let out = turn(&mut connection, &session, &format!("READ:{} ID:readtool-in", b64url(&proj.join("readme.txt").display().to_string()))).await;
    assert!(out.contains("authorized-content"), "pi read tool cannot read the authorized folder");
    let out = run(&mut connection, &session, "insidewrite", &ps_write(&proj.join("inside.txt"))).await;
    assert!(out.contains("WRITE-OK"));
    assert!(proj.join("inside.txt").exists());

    // 0b. An ancestor ACE allows stat, not listing: Documents stays closed.
    let documents = project_parent.parent().unwrap_or(&profile).to_path_buf();
    let out = run(&mut connection, &session, "listdocs", &ps(&format!("try {{ Get-ChildItem -LiteralPath {} -ErrorAction Stop | Select-Object -First 1 | Out-Null; 'LIST-OK' }} catch {{ 'LIST-DENIED:' + $_.Exception.GetType().Name }}", ps_literal(&documents)))).await;
    assert!(out.contains("LIST-DENIED") && !out.contains("LIST-OK"), "the sandbox could list {}", documents.display());
    let out = run(&mut connection, &session, "listprofile", &ps(&format!("try {{ Get-ChildItem -LiteralPath {} -ErrorAction Stop | Select-Object -First 1 | Out-Null; 'LIST-OK' }} catch {{ 'LIST-DENIED:' + $_.Exception.GetType().Name }}", ps_literal(&profile)))).await;
    assert!(out.contains("LIST-DENIED"), "the sandbox could list the profile root");

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

    // 6. Close (inside the srt transition): job terminated, per-session
    //    state removed, ancestor ACEs revoked; ACL residue and leftover
    //    processes measured.
    let mut transition = sandbox.coordinator.transition().unwrap();
    let report = connection.close_with_report().await;
    eprintln!("MEASURE close: {report:?}");
    eprintln!("MEASURE ancestor ACEs removed: {:?}", transition.end_session("t04").unwrap());
    eprintln!("MEASURE reconcile after stop: {:?}", transition.finish());
    for path in &ancestors {
        assert!(!RealWindowsAcl.has_exact_allow_ace(path, &sandbox.sid, ANCESTOR_ACE_MASK).unwrap(), "ancestor ACE left on {}", path.display());
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
    let left = windows_processes_matching(&base.display().to_string());
    eprintln!("MEASURE visible processes left: {left:?} (srt-sandbox processes may be invisible without elevation)");
    assert!(left.is_empty(), "processes left after close: {left:?}");
    assert!(!settings_path.exists(), "per-session settings not cleaned up");
    let acl_after = icacls(&proj);
    eprintln!("MEASURE icacls proj after close (srt-sandbox ACE present = srt reset did not run):\n{acl_after}");
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&proj);
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
    // The --patch file is a read-only runtime asset: it must sit inside the
    // managed runtime directory (rule unchanged; the USERPROFILE location used
    // before was refused with sandbox_path_not_owned on Windows).
    let host_policy = deepseek::HostPolicy { workspace_write: false, network: false };
    let runtime_root = match std::env::var_os("FOX_RUNTIME_ROOT") {
        Some(root) => PathBuf::from(root),
        None if os == HostOs::Windows => panic!("FOX_RUNTIME_ROOT must be set on Windows"),
        None => base.join("fox-data").join("runtime"),
    };
    let patch = deepseek::default_patch_path(&runtime_root, &host_policy);
    if deepseek::verify_installed_patch(&patch, &host_policy).is_err() {
        // Linux / a writable test runtime: install it; Windows: the setup must have.
        let written = std::fs::create_dir_all(patch.parent().unwrap())
            .and_then(|()| std::fs::write(&patch, deepseek::policy_patch(&host_policy).patch_yaml));
        assert!(
            written.is_ok() && deepseek::verify_installed_patch(&patch, &host_policy).is_ok(),
            "{} missing and not writable: install runtime-assets/dsh-patches into <runtime>\\dsh-patches (enable-sandbox source package)",
            patch.display()
        );
    }
    let mut extra = vec![dsh_prefix];
    if os != HostOs::Windows {
        extra.push(patch.parent().unwrap().to_path_buf());
    }
    #[cfg(windows)]
    let sandbox = windows_sandbox(&state);
    #[cfg(windows)]
    let homes = PrivateHomes::new(sandbox.layout.homes_root());
    #[cfg(not(windows))]
    let homes = PrivateHomes::new(base.join("fox-data").join("sandbox-homes"));
    let policy = SandboxPolicy::new(
        vec![AuthorizedDir { path: work.clone(), writable: false }],
        homes.home_for(PROJECT_UUID, AgentBackend::Deepseek).unwrap(),
        extra,
        vec!["api.deepseek.com".into()],
        os,
    );
    #[allow(unused_mut)]
    let mut launcher = SrtLauncher::new(ready, state);
    #[cfg(windows)]
    {
        launcher = launcher.with_managed_runtime_root(runtime_root.clone()).with_tmp_root(sandbox.layout.tmp_root());
    }
    let spec = deepseek::launch_spec(&dsh.display().to_string(), &patch).unwrap();
    let prepared = prepare_launch(PermissionMode::Sandboxed, spec, "dsh1", SandboxSetup::Ready { launcher: &launcher, policy: &policy }, os)
        .unwrap_or_else(|error| panic!("prepare: {error} remedy={:?}", error.remedy));
    #[cfg(windows)]
    let mut transition = sandbox.coordinator.transition().unwrap();
    #[cfg(windows)]
    transition.begin_session("dsh1", &sandbox.sid, &prepared.windows_ancestor_aces).unwrap_or_else(|error| panic!("ancestor ACEs: {error}"));
    let mut connection = AcpConnection::spawn_prepared(prepared, &work).await.expect("spawn dsh inside srt");
    let BackendEvent::Initialized { initialize_response, .. } = next(&mut connection).await else {
        panic!("expected Initialized first");
    };
    #[cfg(windows)]
    eprintln!("MEASURE reconcile after start: {:?}", transition.finish());
    deepseek::check_initialize(&initialize_response).expect("dsh acp initialize inside srt");
    let session = connection
        .new_session(work.clone())
        .await
        .unwrap_or_else(|error| panic!("session/new inside srt: {error}\n{}", connection.stderr_tail()));
    assert!(!session.is_empty());
    #[cfg(windows)]
    let mut transition = sandbox.coordinator.transition().unwrap();
    eprintln!("MEASURE dsh close: {:?}", connection.close_with_report().await);
    #[cfg(windows)]
    {
        eprintln!("MEASURE ancestor ACEs removed: {:?}", transition.end_session("dsh1").unwrap());
        transition.finish();
    }
    let _ = std::fs::remove_dir_all(&base);
}

// ---------------------------------------------------------------------------
// External runtime, real srt + real backends, whole session (Linux)
// ---------------------------------------------------------------------------
//
// `real_srt_external_runtime_pi_session` and `real_srt_external_runtime_dsh_session`
// drive the Host's `ExternalRuntime` with the production `HostLauncher`
// (managed runtime layout, `prepare_launch`, srt + bubblewrap, initialize
// check), not a mock launcher. Opt-in twice: `#[ignore]` plus
// `FOX_TEST_REAL_SRT=1` and `FOX_TEST_REAL_ACP=1`.
//
// ```text
// FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 FOX_SRT_PREFIX=<srt prefix> \
//   FOX_PI_ACP_PREFIX=<pi prefix> [FOX_DSH_BIN=<abs dsh>] [FOX_TEST_APPROVAL_WAIT_SECS=20] \
//   cargo test -j2 -p fox-agent-backend --features acp --test sandbox_srt real_srt_external_runtime \
//   -- --include-ignored --nocapture --test-threads=1
// ```
//
// The managed runtime is `FOX_AGENT_RUNTIME_ROOT` when set (a writable Linux
// layout: `srt/`, `backends/pi/`, `backends/dsh/`), else assembled under
// cargo's target tmp dir from the prefixes (`cp -al`, falling back to
// `cp -a`; it must be a real directory tree, a symlink out of the runtime is
// not readable inside the sandbox). The test writes `pi-gate/` (a test
// wrapper around `fixtures/pi/pi-gated.sh` that loads the scripted provider)
// and `dsh-patches/` into it. The model is `ScriptedModel`, outside the
// sandbox, its `127.0.0.1:<port>` on the Host allowlist
// (`extra_domains`), which also clears NO_PROXY for the backend.

#[cfg(target_os = "linux")]
mod external_real {
    use super::*;
    use fox_agent_backend::capability_view::PiLaunch;
    use fox_agent_backend::external::{AnswerOutcome, CancelOutcome, ExternalRuntime, ExternalSink, LaunchRequest, TurnRequest};
    use fox_agent_backend::external_launch::{FullAccessSwitch, HostLaunchConfig, HostLauncher};
    use fox_agent_backend::port::StopConfirmation;
    use fox_agent_backend::projection::PermissionCard;
    use serde_json::Value;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    pub(super) fn require_real_acp(test: &str) {
        require_real(test);
        if std::env::var("FOX_TEST_REAL_ACP").as_deref() != Ok("1") {
            panic!("{test} launches real ACP backends: set FOX_TEST_REAL_ACP=1 as well (not run = not passed)");
        }
        for name in ["FOX_PI_ACP_PROGRAM", "FOX_DSH_PROGRAM", "FOX_EXTERNAL_FULL_ACCESS"] {
            assert!(std::env::var_os(name).is_none(), "{name} must be unset: the test uses the managed runtime in sandboxed mode");
        }
    }

    #[derive(Default)]
    struct Recorded {
        events: Vec<(String, Value)>,
        approvals: Vec<(String, String, PermissionCard)>,
    }

    #[derive(Clone, Default)]
    pub(super) struct Sink(Arc<Mutex<Recorded>>);

    impl ExternalSink for Sink {
        fn emit(&self, run_id: &str, _conversation_id: &str, event: Value) -> Result<(), String> {
            self.0.lock().unwrap().events.push((run_id.to_owned(), event));
            Ok(())
        }

        fn request_approval(&self, run_id: &str, _conversation_id: &str, _key: &str, card: &PermissionCard) -> Result<String, String> {
            let mut recorded = self.0.lock().unwrap();
            let id = format!("appr-{}", recorded.approvals.len() + 1);
            recorded.approvals.push((run_id.to_owned(), id.clone(), card.clone()));
            Ok(id)
        }
    }

    const TERMINALS: [&str; 4] = ["run.completed", "run.cancelled", "run.failed", "run.interrupted"];

    impl Sink {
        pub(super) fn events(&self, run_id: &str) -> Vec<Value> {
            self.0.lock().unwrap().events.iter().filter(|(run, _)| run == run_id).map(|(_, event)| event.clone()).collect()
        }

        pub(super) fn types(&self, run_id: &str) -> Vec<String> {
            self.events(run_id).iter().map(|event| event["type"].as_str().unwrap_or_default().to_owned()).collect()
        }

        pub(super) fn text(&self, run_id: &str) -> String {
            self.events(run_id)
                .iter()
                .filter(|event| event["type"] == "message.delta")
                .filter_map(|event| event["delta"].as_str().map(str::to_owned))
                .collect()
        }

        fn approvals(&self) -> Vec<(String, String, PermissionCard)> {
            self.0.lock().unwrap().approvals.clone()
        }

        fn terminal_now(&self, run_id: &str) -> Option<Value> {
            self.events(run_id).into_iter().find(|event| event["type"].as_str().is_some_and(|kind| TERMINALS.contains(&kind)))
        }

        pub(super) async fn terminal(&self, run_id: &str) -> Value {
            self.terminal_within(run_id, 120).await
        }

        pub(super) async fn terminal_within(&self, run_id: &str, secs: u64) -> Value {
            for _ in 0..secs * 10 {
                if let Some(event) = self.terminal_now(run_id) {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("no terminal for {run_id} within {secs}s; got {:?}", self.types(run_id));
        }

        /// The `count`-th approval, or the run's terminal if it ended first.
        async fn approval(&self, run_id: &str, count: usize) -> (String, PermissionCard) {
            for _ in 0..1200 {
                let approvals = self.approvals();
                if approvals.len() >= count {
                    let (run, id, card) = approvals[count - 1].clone();
                    assert_eq!(run, run_id);
                    return (id, card);
                }
                if let Some(terminal) = self.terminal_now(run_id) {
                    panic!("{run_id} ended before approval #{count}: {terminal}\n{:?}", self.types(run_id));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("no approval #{count} for {run_id} within 120s; got {:?}", self.types(run_id));
        }
    }

    /// `cp -al src dst`, else `cp -a` (hard links need one file system).
    fn copy_tree(src: &Path, dst: &Path) {
        if dst.exists() {
            return;
        }
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        let linked = std::process::Command::new("cp").arg("-al").arg(src).arg(dst).status().map(|status| status.success()).unwrap_or(false);
        if !linked {
            let _ = std::fs::remove_dir_all(dst);
            let copied = std::process::Command::new("cp").arg("-a").arg(src).arg(dst).status().expect("cp");
            assert!(copied.success(), "copy {} -> {}", src.display(), dst.display());
        }
    }

    /// The managed runtime (see the section comment).
    pub(super) fn runtime_root(with_dsh: bool) -> PathBuf {
        if let Some(root) = std::env::var_os("FOX_AGENT_RUNTIME_ROOT") {
            return PathBuf::from(root);
        }
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("fox-linux-runtime");
        copy_tree(&env_path("FOX_SRT_PREFIX"), &root.join("srt"));
        copy_tree(&env_path("FOX_PI_ACP_PREFIX"), &root.join("backends").join("pi"));
        if with_dsh {
            let dsh = env_path("FOX_DSH_BIN");
            let prefix = dsh.parent().and_then(Path::parent).and_then(Path::parent).expect("dsh under <prefix>/node_modules/.bin");
            copy_tree(prefix, &root.join("backends").join("dsh"));
        }
        root
    }

    /// Writes the test pi gate into `<runtime>/pi-gate`: the committed gate
    /// and launcher, plus a `pi-gated.sh` wrapper that points pi at the
    /// scripted provider and model (the Host clears the environment, so they
    /// cannot come from the test process).
    fn install_test_pi_gate(runtime: &Path, model: &ScriptedModel) -> PathBuf {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
        let gate = runtime.join("pi-gate");
        std::fs::create_dir_all(&gate).unwrap();
        std::fs::copy(fixtures.join("pi/fox-approval-gate.ts"), gate.join("fox-approval-gate.ts")).unwrap();
        std::fs::copy(fixtures.join("pi/pi-gated.sh"), gate.join("pi-gated-fixture.sh")).unwrap();
        std::fs::copy(fixtures.join("sandbox/scripted-cmd-provider.ts"), gate.join("scripted-cmd-provider.ts")).unwrap();
        let wrapper = format!(
            "#!/bin/sh\n# TEST ONLY (tests/sandbox_srt.rs external_real).\nDIR=$(CDPATH= cd -- \"$(dirname -- \"$0\")\" && pwd)\nexport FOX_SCRIPTED_MODEL_URL='{}'\nexport FOX_PI_EXTRA_ARGS=\"-e $DIR/scripted-cmd-provider.ts --provider foxcmd --model scripted\"\nexec /bin/sh \"$DIR/pi-gated-fixture.sh\" \"$@\"\n",
            model.url()
        );
        std::fs::write(gate.join("pi-gated.sh"), wrapper).unwrap();
        use std::os::unix::fs::PermissionsExt;
        for file in ["pi-gated.sh", "pi-gated-fixture.sh"] {
            std::fs::set_permissions(gate.join(file), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        gate
    }

    pub(super) fn config(runtime: PathBuf, fox_data: &Path, extra: BTreeMap<AgentBackend, Vec<String>>) -> HostLaunchConfig {
        HostLaunchConfig {
            os: HostOs::Linux,
            fox_data: fox_data.to_path_buf(),
            state_root: fox_data.join("agent-state"),
            runtime_root: runtime,
            native_login_home: None,
            full_access: FullAccessSwitch::Off,
            full_access_forbidden: false,
            pi_providers: vec![],
            extra_domains: extra,
            env_passthrough: vec![],
        }
    }

    pub(super) fn turn(conversation: &str, run: &str, prompt: &str, backend: AgentBackend, project: &Path) -> TurnRequest {
        TurnRequest {
            run_id: run.into(),
            prompt: prompt.into(),
            stored_native_session: None,
            launch: LaunchRequest {
                conversation_id: conversation.into(),
                backend,
                project_uuid: Some(PROJECT_UUID.into()),
                project_root: project.to_path_buf(),
                session_key: format!("ext-{run}"),
                full_access_granted: false,
                unified_login: None,
                native_permission: None,
            },
        }
    }

    fn run_prompt(id: &str, command: &str) -> String {
        format!("RUN:{} ID:{id}", b64url(command))
    }

    fn assert_no_terminal(sink: &Sink, run: &str) {
        assert!(sink.terminal_now(run).is_none(), "{run} ended on its own while the approval was unanswered: {:?}", sink.types(run));
    }

    pub(super) async fn pi_session() {
        let pid = std::process::id();
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let base = home.join(format!(".fox-ext-{pid}"));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("proj");
        let fox_data = base.join("fox-data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&fox_data).unwrap();

        let model = ScriptedModel::start();
        let runtime = runtime_root(false);
        let gate = install_test_pi_gate(&runtime, &model);
        let config = config(runtime.clone(), &fox_data, BTreeMap::from([(AgentBackend::Pi, vec![model.allow_entry()])]));
        assert_eq!(config.pi_launch(), PiLaunch::Gated);
        assert!(config.allows_loopback(AgentBackend::Pi));
        let sink = Sink::default();
        let runtime_handle = ExternalRuntime::new(sink.clone(), HostLauncher::new(config), tokio::runtime::Handle::current());
        let rt = &runtime_handle;
        let conv = "ext-pi";
        let wait = Duration::from_secs(std::env::var("FOX_TEST_APPROVAL_WAIT_SECS").ok().and_then(|value| value.parse().ok()).unwrap_or(20));

        // 1. Start, streamed text, completion.
        rt.start_turn(turn(conv, "e1", "hello", AgentBackend::Pi, &project)).unwrap();
        let terminal = sink.terminal("e1").await;
        eprintln!("MEASURE e1 types {:?}\nterminal {terminal}", sink.types("e1"));
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        let types = sink.types("e1");
        assert_eq!(types.first().map(String::as_str), Some("run.started"));
        for expected in ["agent_backend.launching", "agent_backend.session", "message.started", "message.delta", "message.completed"] {
            assert!(types.iter().any(|kind| kind == expected), "missing {expected}: {types:?}");
        }
        assert!(sink.text("e1").contains("scripted hello"), "{}", sink.text("e1"));
        assert!(model.requests() >= 1, "the turn did not reach the scripted model outside the sandbox");
        let info = rt.session_info(conv).expect("session");
        assert!(info.live && info.active_run.is_none());
        assert_eq!(info.mode, Some(PermissionMode::Sandboxed));
        assert_eq!(info.pi_launch, PiLaunch::Gated);
        assert!(info.declaration.as_ref().is_some_and(|declaration| declaration.directory_isolation));

        // 2. Permission request -> allow -> the tool runs, the turn completes.
        rt.start_turn(turn(conv, "e2", &run_prompt("allow", "echo allowed-ok > allowed.txt && echo ran"), AgentBackend::Pi, &project)).unwrap();
        let (approval, card) = sink.approval("e2", 1).await;
        eprintln!("MEASURE e2 card {card:?}");
        assert!(card.available_decisions.contains(&"allow_once") && card.available_decisions.contains(&"deny"), "{card:?}");
        assert!(!card.available_decisions.contains(&"allow_conversation"));
        assert!(rt.approval_pending(&approval));
        assert_eq!(rt.answer(&approval, true), AnswerOutcome::Written);
        let terminal = sink.terminal("e2").await;
        eprintln!("MEASURE e2 terminal {terminal} text {:?}", sink.text("e2"));
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert_eq!(std::fs::read_to_string(project.join("allowed.txt")).unwrap_or_default().trim(), "allowed-ok");
        assert!(sink.text("e2").contains("tool-result"), "{}", sink.text("e2"));

        // 3. Permission request -> deny -> the tool is blocked, the turn completes.
        rt.start_turn(turn(conv, "e3", &run_prompt("deny", "echo denied > denied.txt"), AgentBackend::Pi, &project)).unwrap();
        let (approval, _) = sink.approval("e3", 2).await;
        assert_eq!(rt.answer(&approval, false), AnswerOutcome::Written);
        let terminal = sink.terminal("e3").await;
        eprintln!("MEASURE e3 terminal {terminal} text {:?}", sink.text("e3"));
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert!(!project.join("denied.txt").exists(), "a denied tool ran");
        let completed = sink.events("e3").into_iter().find(|event| event["type"] == "tool.completed");
        eprintln!("MEASURE e3 tool.completed {completed:?}");

        // 4. No deadline: the request stays pending, the turn keeps running,
        //    nothing ends it on its own; a late allow still runs the tool.
        rt.start_turn(turn(conv, "e4", &run_prompt("late", "echo late-ok > late.txt"), AgentBackend::Pi, &project)).unwrap();
        let (approval, _) = sink.approval("e4", 3).await;
        let started = std::time::Instant::now();
        while started.elapsed() < wait {
            tokio::time::sleep(Duration::from_secs(1)).await;
            assert!(rt.approval_pending(&approval), "approval ended on its own after {:?}", started.elapsed());
            assert_no_terminal(&sink, "e4");
        }
        assert_eq!(rt.active_runs(), vec!["e4".to_owned()]);
        assert!(rt.session_info(conv).unwrap().live, "backend process stopped while waiting");
        assert!(!project.join("late.txt").exists());
        eprintln!("MEASURE e4 still pending after {:?}", started.elapsed());
        assert_eq!(rt.answer(&approval, true), AnswerOutcome::Written);
        let terminal = sink.terminal("e4").await;
        assert_eq!(terminal["type"], "run.completed", "{terminal}");
        assert_eq!(std::fs::read_to_string(project.join("late.txt")).unwrap_or_default().trim(), "late-ok");

        // 5. Cancel the turn while its approval is pending: the native request
        //    is answered `cancelled`, the run ends cancelled, a late answer is
        //    not written, the tool never runs.
        rt.start_turn(turn(conv, "e5", &run_prompt("cancel", "echo cancelled > cancelled.txt"), AgentBackend::Pi, &project)).unwrap();
        let (approval, _) = sink.approval("e5", 4).await;
        match rt.cancel("e5") {
            CancelOutcome::Requested { answered_permissions } => assert_eq!(answered_permissions.len(), 1, "pending request answered `cancelled`"),
            other => panic!("{other:?}"),
        }
        assert!(!rt.approval_pending(&approval));
        let terminal = sink.terminal("e5").await;
        eprintln!("MEASURE e5 terminal {terminal}");
        assert_eq!(terminal["type"], "run.cancelled", "{terminal}");
        assert_eq!(rt.answer(&approval, true), AnswerOutcome::NotPending, "a late approval is never written");
        assert_eq!(rt.cancel("e5"), CancelOutcome::NotActive);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!project.join("cancelled.txt").exists(), "the tool of a cancelled turn ran");

        // 6. The conversation takes another turn after the cancel.
        rt.start_turn(turn(conv, "e6", "hello again", AgentBackend::Pi, &project)).unwrap();
        let terminal = sink.terminal("e6").await;
        eprintln!("MEASURE e6 terminal {terminal} live-before-close {:?}", rt.session_info(conv).map(|info| info.live));
        assert_eq!(terminal["type"], "run.completed", "{terminal}");

        // 7. Close: confirmed stop, nothing left running from this session.
        let report = rt.close_conversation(conv, "test done").await.expect("live process");
        eprintln!("MEASURE close {report:?}");
        assert_eq!(report.confirmation, StopConfirmation::Confirmed, "{report:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let left = processes_matching(&base.display().to_string());
        assert!(left.is_empty(), "processes left after close: {left:?}");
        let _ = std::fs::remove_dir_all(&gate);
        let _ = std::fs::remove_dir_all(&base);
    }

    pub(super) async fn dsh_session() {
        use fox_agent_backend::backends::deepseek;
        let pid = std::process::id();
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let base = home.join(format!(".fox-ext-dsh-{pid}"));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("proj");
        let fox_data = base.join("fox-data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&fox_data).unwrap();
        let runtime = runtime_root(true);
        let host_policy = deepseek::HostPolicy { workspace_write: true, network: false };
        let patch = deepseek::default_patch_path(&runtime, &host_policy);
        if deepseek::verify_installed_patch(&patch, &host_policy).is_err() {
            std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
            std::fs::write(&patch, deepseek::policy_patch(&host_policy).patch_yaml).unwrap();
        }
        let config = config(runtime, &fox_data, BTreeMap::new());
        let sink = Sink::default();
        let rt = ExternalRuntime::new(sink.clone(), HostLauncher::new(config), tokio::runtime::Handle::current());
        rt.start_turn(turn("ext-dsh", "d1", "Reply with the single word: pong", AgentBackend::Deepseek, &project)).unwrap();
        // dsh's cold start inside srt (many small files) is slow.
        let started = std::time::Instant::now();
        let terminal = sink.terminal_within("d1", 600).await;
        eprintln!("MEASURE d1 settled after {:?}", started.elapsed());
        eprintln!("MEASURE d1 types {:?}\nterminal {terminal}\ntext {:?}", sink.types("d1"), sink.text("d1"));
        let types = sink.types("d1");
        assert!(types.iter().any(|kind| kind == "agent_backend.session"), "dsh session/new inside srt: {types:?}");
        assert_eq!(types.iter().filter(|kind| TERMINALS.contains(&kind.as_str())).count(), 1, "one terminal");
        let info = rt.session_info("ext-dsh").expect("session");
        assert_eq!(info.mode, Some(PermissionMode::Sandboxed));
        // A real prompt passes only with a real reply: a model error (no key,
        // refused key, no outside network) fails the test.
        let close_first = rt.close_conversation("ext-dsh", "test done");
        let check = || {
            assert_eq!(terminal["type"], "run.completed", "the model call failed (needs DEEPSEEK_API_KEY and the network): {terminal}");
            assert_eq!(terminal["stopReason"], "end_turn", "{terminal}");
            assert!(!sink.text("d1").trim().is_empty(), "end_turn without agent text");
        };
        let report = close_first.await;
        eprintln!("MEASURE dsh close {report:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let left = processes_matching(&base.display().to_string());
        let _ = std::fs::remove_dir_all(&base);
        if let Some(report) = &report {
            assert_eq!(report.confirmation, StopConfirmation::Confirmed, "{report:?}");
        }
        assert!(left.is_empty(), "processes left after close: {left:?}");
        check();
    }

    /// DeepSeek with a loopback model gateway (Magpie-style, sandbox_md 13.10):
    /// the base URL lives in the private HOME's dsh patch, the Host derives
    /// its one port and puts `127.0.0.1:<port>` on the srt allowlist, and the
    /// gateway is a local HTTP server outside the sandbox. Passes only when a
    /// request from dsh inside srt reaches the gateway.
    pub(super) async fn dsh_loopback_gateway_session() {
        use fox_agent_backend::backends::deepseek;
        let pid = std::process::id();
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let base = home.join(format!(".fox-ext-dshgw-{pid}"));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("proj");
        let fox_data = base.join("fox-data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&fox_data).unwrap();
        let gateway = ScriptedModel::start();
        // Control: the gateway answers from outside the sandbox.
        {
            let mut stream = std::net::TcpStream::connect(("127.0.0.1", gateway.port)).unwrap();
            write!(stream, "GET /control HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n").unwrap();
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            assert!(answer.starts_with("HTTP/1.1"), "gateway control: {answer:?}");
        }
        let control_hits = gateway.requests();
        let runtime = runtime_root(true);
        let host_policy = deepseek::HostPolicy { workspace_write: true, network: false };
        let patch = deepseek::default_patch_path(&runtime, &host_policy);
        if deepseek::verify_installed_patch(&patch, &host_policy).is_err() {
            std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
            std::fs::write(&patch, deepseek::policy_patch(&host_policy).patch_yaml).unwrap();
        }
        // The user's own dsh configuration in the private HOME: a fake key and
        // a loopback base URL. The fake gateway answers any path, so the
        // `/anthropic` suffix here is not Magpie's form: a real Magpie needs
        // `http://127.0.0.1:3425/v1` (dsh's messagesApiRoot() appends `/v1`
        // only when the path does not already end in it; sandbox_md 13.12).
        let private_home = PrivateHomes::new(fox_data.join("sandbox-homes")).home_for(PROJECT_UUID, AgentBackend::Deepseek).unwrap().path;
        std::fs::create_dir_all(private_home.join(".dsh")).unwrap();
        // dsh takes DEEPSEEK_API_KEY from `$DSH_HOME/.env` but refuses
        // DEEPSEEK_BASE_URL there (launch environment only), so the base URL
        // is the `llm-deepseek` row of the private HOME's own patch.
        std::fs::write(private_home.join(".dsh").join(".env"), "DEEPSEEK_API_KEY=sk-fox-loopback-test\n").unwrap();
        std::fs::write(
            private_home.join(".dsh").join("cordis.patch.yml"),
            format!("- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:{}/anthropic\n", gateway.port),
        )
        .unwrap();
        let config = config(runtime, &fox_data, BTreeMap::new());
        let sink = Sink::default();
        let rt = ExternalRuntime::new(sink.clone(), HostLauncher::new(config), tokio::runtime::Handle::current());
        rt.start_turn(turn("ext-dshgw", "g1", "Reply with the single word: pong", AgentBackend::Deepseek, &project)).unwrap();
        let started = std::time::Instant::now();
        if !deepseek::LOOPBACK_VIA_PROXY_SUPPORTED {
            // Pinned dsh dials loopback directly: the sandboxed launch must be
            // refused before anything starts, with the dedicated code.
            let terminal = sink.terminal_within("g1", 60).await;
            let elapsed = started.elapsed();
            let hits = gateway.requests() - control_hits;
            let info = rt.session_info("ext-dshgw");
            eprintln!("MEASURE g1 refused after {elapsed:?}\nterminal {terminal}\ntypes {:?}", sink.types("g1"));
            let _ = rt.close_conversation("ext-dshgw", "test done").await;
            let _ = std::fs::remove_dir_all(&base);
            assert_eq!(terminal["type"], "run.failed", "{terminal}");
            assert_eq!(terminal["code"], fox_agent_backend::external_launch::LOOPBACK_REQUIRES_FULL_ACCESS, "{terminal}");
            let message = terminal["message"].as_str().unwrap_or_default();
            assert!(message.contains("本机网关在沙箱模式下不可达") && message.contains("需要完全访问权限") && message.contains("在设置中打开“新外部会话使用完全访问”后新建会话") && message.contains("api.deepseek.com"), "{message}");
            assert!(!message.contains("请切换到"), "{message}");
            assert!(elapsed < Duration::from_secs(10), "refused before launch, not after a prompt: {elapsed:?}");
            assert!(!sink.types("g1").iter().any(|kind| kind == "agent_backend.session"), "no backend session was opened");
            assert!(info.and_then(|info| info.declaration).is_none(), "nothing was launched");
            assert_eq!(hits, 0, "nothing reached the gateway");
            return;
        }
        // Acceptance once dsh has an upstream "loopback through the proxy"
        // switch (LOOPBACK_VIA_PROXY_SUPPORTED = true, switch enabled for the
        // session): exactly the derived port is allowed and a request from
        // dsh inside srt reaches the gateway.
        let terminal = sink.terminal_within("g1", 600).await;
        let info = rt.session_info("ext-dshgw");
        let declaration = info.as_ref().and_then(|info| info.declaration.clone());
        let hits = gateway.requests() - control_hits;
        eprintln!("MEASURE g1 settled after {:?}", started.elapsed());
        eprintln!("MEASURE g1 types {:?}\nterminal {terminal}", sink.types("g1"));
        eprintln!(
            "MEASURE declaration allowed_domains {:?} loopback_gateway {:?} diagnostics {:?}",
            declaration.as_ref().map(|d| d.allowed_domains.clone()),
            declaration.as_ref().map(|d| d.loopback_gateway.clone()),
            declaration.as_ref().map(|d| d.loopback_diagnostics.clone())
        );
        eprintln!("MEASURE gateway requests from inside the sandbox: {hits}");
        let report = rt.close_conversation("ext-dshgw", "test done").await;
        eprintln!("MEASURE dsh close {report:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let _ = std::fs::remove_dir_all(&base);
        let declaration = declaration.expect("a launched dsh session has a declaration");
        assert_eq!(declaration.loopback_gateway.as_ref().map(|g| g.port), Some(gateway.port), "derived gateway port");
        assert!(declaration.allowed_domains.contains(&gateway.allow_entry()), "{:?}", declaration.allowed_domains);
        assert!(hits >= 1, "no request from dsh inside srt reached the loopback gateway; terminal {terminal}");
    }

    /// Entries of `dir` (recursively), for before/after comparisons.
    fn tree(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(next) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&next) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path.clone());
                }
                out.push(path);
            }
        }
        out.sort();
        out
    }

    /// DeepSeek in FULL ACCESS with a loopback gateway (5f2f2d8 re-test:
    /// full access used the real HOME and dialled api.deepseek.com). The dsh
    /// configuration lives only in the private HOME and points at a local
    /// fake gateway; full access must start dsh without srt but with HOME /
    /// DSH_HOME in the private HOME, so the request reaches the gateway and
    /// dsh writes its state there, not under the real HOME.
    pub(super) async fn dsh_full_access_private_home_gateway_session() {
        use fox_agent_backend::backends::deepseek;
        let pid = std::process::id();
        let real_home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
        let base = real_home.join(format!(".fox-ext-dshfull-{pid}"));
        let _ = std::fs::remove_dir_all(&base);
        let project = base.join("proj");
        let fox_data = base.join("fox-data");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&fox_data).unwrap();
        let gateway = ScriptedModel::start();
        let runtime = runtime_root(true);
        let host_policy = deepseek::HostPolicy { workspace_write: true, network: false };
        let patch = deepseek::default_patch_path(&runtime, &host_policy);
        if deepseek::verify_installed_patch(&patch, &host_policy).is_err() {
            std::fs::create_dir_all(patch.parent().unwrap()).unwrap();
            std::fs::write(&patch, deepseek::policy_patch(&host_policy).patch_yaml).unwrap();
        }
        let private_home = PrivateHomes::new(fox_data.join("sandbox-homes")).home_for(PROJECT_UUID, AgentBackend::Deepseek).unwrap().path;
        std::fs::create_dir_all(private_home.join(".dsh")).unwrap();
        std::fs::write(private_home.join(".dsh").join(".env"), "DEEPSEEK_API_KEY=sk-fox-full-access-test\n").unwrap();
        std::fs::write(
            private_home.join(".dsh").join("cordis.patch.yml"),
            format!("- id: llm-deepseek\n  config:\n    baseURL: http://127.0.0.1:{}/anthropic\n", gateway.port),
        )
        .unwrap();
        let seeded = tree(&private_home);
        let real_before: Vec<PathBuf> = [".dsh", ".config", ".cache", ".local/share", ".local/state"].iter().flat_map(|dir| tree(&real_home.join(dir))).collect();
        let mut config = config(runtime, &fox_data, BTreeMap::new());
        config.full_access = FullAccessSwitch::All;
        let sink = Sink::default();
        let rt = ExternalRuntime::new(sink.clone(), HostLauncher::new(config), tokio::runtime::Handle::current());
        rt.start_turn(turn("ext-dshfull", "f1", "Reply with the single word: pong", AgentBackend::Deepseek, &project)).unwrap();
        let started = std::time::Instant::now();
        // Wait for the request (the fake gateway does not speak the Anthropic
        // API, so the turn itself may fail afterwards).
        while gateway.requests() == 0 && sink.terminal_now("f1").is_none() && started.elapsed() < Duration::from_secs(180) {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let terminal = sink.terminal_within("f1", 120).await;
        let hits = gateway.requests();
        let info = rt.session_info("ext-dshfull");
        let mode = info.as_ref().and_then(|info| info.mode);
        eprintln!("MEASURE f1 after {:?}: gateway requests {hits}; mode {mode:?}\ntypes {:?}\nterminal {terminal}", started.elapsed(), sink.types("f1"));
        let report = rt.close_conversation("ext-dshfull", "test done").await;
        eprintln!("MEASURE dsh close {report:?}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        let written: Vec<PathBuf> = tree(&private_home).into_iter().filter(|path| !seeded.contains(path)).collect();
        let real_after: Vec<PathBuf> = [".dsh", ".config", ".cache", ".local/share", ".local/state"].iter().flat_map(|dir| tree(&real_home.join(dir))).collect();
        let real_new: Vec<&PathBuf> = real_after.iter().filter(|path| !real_before.contains(path)).collect();
        eprintln!("MEASURE written under the private HOME: {written:?}");
        eprintln!("MEASURE new under the real HOME config dirs: {real_new:?}");
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(mode, Some(fox_agent_backend::sandbox::PermissionMode::FullAccess), "{terminal}");
        assert!(hits >= 1, "no request from dsh in full access reached the gateway named only in the private HOME; terminal {terminal}");
        assert!(written.iter().any(|path| path.starts_with(private_home.join(".dsh"))), "dsh wrote nothing under the private DSH_HOME: {written:?}");
        assert!(real_new.is_empty(), "dsh wrote under the real HOME: {real_new:?}");
    }
}

/// Whole external session through `ExternalRuntime` + `HostLauncher`, real
/// pi-acp / pi inside real srt (Linux): start, streamed output, completion;
/// permission allow and deny; an unanswered permission keeps waiting (no
/// deadline) and a late allow still works; cancelling the turn answers the
/// pending request `cancelled` and ends the run cancelled; close confirmed.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt + pi-acp through ExternalRuntime; FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 ... -- --include-ignored"]
async fn real_srt_external_runtime_pi_session() {
    external_real::require_real_acp("real_srt_external_runtime_pi_session");
    external_real::pi_session().await;
}

/// DeepSeek (dsh) through `ExternalRuntime` + `HostLauncher` inside real srt
/// (Linux): launch, initialize check, session/new, one turn that must end
/// `run.completed` / `end_turn` with non-empty agent text (a model error fails
/// the test; needs `DEEPSEEK_API_KEY` and a reachable api.deepseek.com), close
/// confirmed and no process left. Needs `FOX_DSH_BIN` unless the runtime has
/// `backends/dsh`.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt + dsh through ExternalRuntime; FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 FOX_DSH_BIN=<abs> ... -- --include-ignored"]
async fn real_srt_external_runtime_dsh_session() {
    external_real::require_real_acp("real_srt_external_runtime_dsh_session");
    external_real::dsh_session().await;
}

/// DeepSeek through `ExternalRuntime` + `HostLauncher` (Linux, sandboxed)
/// with a loopback model gateway named by the private HOME's dsh patch
/// (docs/sandbox.md 13.10 / 13.11). With the pinned dsh
/// (`deepseek::LOOPBACK_VIA_PROXY_SUPPORTED = false`) the launch must be
/// refused before anything starts: `run.failed` with
/// `agent_backend_loopback_requires_full_access` and the Chinese hint, within
/// seconds, no backend session, no request at the gateway. Once dsh has an
/// upstream "loopback through the proxy" switch (flag `true`), the same test
/// is the acceptance case: exactly that port allowed and a request from dsh
/// inside srt reaching the gateway.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real srt + dsh through ExternalRuntime; FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 FOX_DSH_BIN=<abs> ... -- --include-ignored"]
async fn real_srt_external_runtime_dsh_loopback_gateway() {
    external_real::require_real_acp("real_srt_external_runtime_dsh_loopback_gateway");
    external_real::dsh_loopback_gateway_session().await;
}

/// DeepSeek in full access through `ExternalRuntime` + `HostLauncher`
/// (Linux, no srt involved): the dsh configuration exists only in the
/// private HOME and points at a local fake gateway; the request must reach
/// it and dsh must write under the private HOME, nothing under the real one.
/// Needs no API key (the gateway only counts requests).
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "real dsh through ExternalRuntime in full access; FOX_TEST_REAL_SRT=1 FOX_TEST_REAL_ACP=1 FOX_AGENT_RUNTIME_ROOT=<runtime with backends/dsh> ... -- --include-ignored"]
async fn real_external_runtime_dsh_full_access_uses_the_private_home() {
    external_real::require_real_acp("real_external_runtime_dsh_full_access_uses_the_private_home");
    external_real::dsh_full_access_private_home_gateway_session().await;
}
