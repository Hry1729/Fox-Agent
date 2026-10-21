//! Windows only, `#[ignore]`d: the bridge pipe Fox creates, served for an
//! outside probe by a second (non-administrator) account.
//!
//! `scripts/windows/diag-srt-stdio.ps1 -Variants pipe-other-user
//! -OtherUserCredential (Get-Credential) -PipeServerTest <this test binary>`
//! runs this test as the current user; the test creates the pipe with Fox's
//! own code ([`BridgeListener::bind`]: protected DACL, current user full
//! control + srt sandbox SID read/write, first instance, one instance, no
//! remote clients), prints `FOX_PIPE_NAME=<name>` and waits. The script then
//! starts a process as the other account (credentials only in the
//! PSCredential, never on a command line, in a file or in the log) that tries
//! to open the pipe and exits with the Win32 error (expected 5, access
//! denied), and writes a line to this test's stdin. The test then checks that
//! nobody connected, and connects once itself as the owner (control: the pipe
//! was alive and openable during the attempt).
//!
//! ```text
//! set FOX_TEST_PIPE_SERVE=1& cargo test -p fox-agent-backend --features acp --test bridge_pipe_windows -- --ignored --nocapture
//! ```
//!
//! Optional `FOX_TEST_PIPE_SANDBOX_SID` (default: `SandboxUserSid` from
//! `HKLM\SOFTWARE\sandbox-runtime`) and `FOX_TEST_PIPE_SERVE_SECONDS` (default
//! 120: how long to wait for the "done" line).

#![cfg(all(windows, feature = "acp"))]

use fox_agent_backend::bridge::BridgeListener;
use fox_agent_backend::launch::BridgeEndpoint;
use fox_agent_backend::sandbox::{read_srt_sandbox_sid, RealProbe};
use std::io::Write;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::ClientOptions;

#[tokio::test(flavor = "multi_thread")]
#[ignore = "Windows hardware check driven by diag-srt-stdio.ps1 -Variants pipe-other-user; set FOX_TEST_PIPE_SERVE=1"]
async fn bridge_pipe_serve_for_other_user_probe() {
    if std::env::var("FOX_TEST_PIPE_SERVE").as_deref() != Ok("1") {
        panic!("set FOX_TEST_PIPE_SERVE=1 and run it through diag-srt-stdio.ps1 -Variants pipe-other-user (not run = not passed)");
    }
    let sid = std::env::var("FOX_TEST_PIPE_SANDBOX_SID")
        .ok()
        .or_else(|| read_srt_sandbox_sid(&RealProbe))
        .expect("srt sandbox SID (HKLM\\SOFTWARE\\sandbox-runtime SandboxUserSid or FOX_TEST_PIPE_SANDBOX_SID)");
    let seconds = std::env::var("FOX_TEST_PIPE_SERVE_SECONDS").ok().and_then(|value| value.parse().ok()).unwrap_or(120u64);
    let mut endpoint = BridgeEndpoint::new_local(Some(sid.clone())).unwrap();
    // As in a real launch: the handshake answer carries the backend's cwd and
    // environment (`OK {"cwd":..,"env":{..}}\n`, since eeea84e).
    let probe_cwd = std::env::current_dir().unwrap().to_string_lossy().into_owned();
    endpoint.child_cwd = Some(probe_cwd.clone());
    endpoint.child_env = vec![("FOX_PIPE_PROBE".into(), "owner-control".into())];
    let listener = BridgeListener::bind(&endpoint).unwrap_or_else(|error| panic!("bind: {error}"));
    println!("FOX_PIPE_NAME={}", endpoint.address);
    println!("FOX_PIPE_SANDBOX_SID={sid}");
    std::io::stdout().flush().unwrap();

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = done_tx.send(());
    });
    let accept = listener.accept();
    tokio::pin!(accept);
    let early = tokio::select! {
        result = &mut accept => Some(result.is_ok()),
        _ = done_rx => None,
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => None,
    };
    if let Some(handshake_ok) = early {
        println!("FOX_PIPE_RESULT=connected handshake_ok={handshake_ok}");
        std::io::stdout().flush().unwrap();
        panic!("a client connected to the bridge pipe before the owner control (handshake ok: {handshake_ok})");
    }
    println!("FOX_PIPE_RESULT=no-connection");

    // Owner control: the same pipe, still listening, opens for the owner.
    let address = endpoint.address.clone();
    let token = endpoint.token.clone();
    let client = async move {
        let mut client = ClientOptions::new().open(&address).map_err(|error| format!("owner open: {error}"))?;
        client.write_all(format!("FOX-ACP-BRIDGE/1 {token}\n").as_bytes()).await.map_err(|error| error.to_string())?;
        // Read the whole answer line (up to and excluding '\n'), byte by byte,
        // never past the newline.
        let mut answer = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            client.read_exact(&mut byte).await.map_err(|error| error.to_string())?;
            if byte[0] == b'\n' {
                break;
            }
            answer.push(byte[0]);
            if answer.len() > 64 * 1024 {
                return Err("handshake answer has no newline within 64 KiB".to_owned());
            }
        }
        Ok::<_, String>(answer)
    };
    let (accepted, answer) = tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(accept, client) }).await.expect("owner control within 10 s");
    let answer = answer.unwrap_or_else(|error| panic!("owner control failed: {error}"));
    let line = String::from_utf8(answer).expect("handshake answer is UTF-8");
    let json = line.strip_prefix("OK ").unwrap_or_else(|| panic!("handshake answer must start with \"OK \", got {line:?}"));
    let payload: serde_json::Value = serde_json::from_str(json).unwrap_or_else(|error| panic!("handshake answer after \"OK \" is not JSON ({error}): {json:?}"));
    assert_eq!(payload["cwd"], serde_json::Value::String(probe_cwd), "handshake carries the backend cwd");
    assert!(payload["env"].is_object(), "handshake carries the backend env object: {payload}");
    assert_eq!(payload["env"]["FOX_PIPE_PROBE"], "owner-control");
    assert!(accepted.is_ok(), "owner handshake");
    println!("FOX_PIPE_CONTROL=owner-connected");
    std::io::stdout().flush().unwrap();
}
