//! Host side of the ACP bridge (Windows sandboxed mode, docs/sandbox.md 7.12).
//!
//! srt 0.0.80 on Windows does not deliver the caller's stdin to the sandboxed
//! process: the srt-win broker gives its runner a fresh stdin pipe, writes the
//! length-prefixed `RunnerCmd` into it and closes it
//! (`vendor/srt-win-src/src/logon.rs`, `spawn_runner`: `make_pipe` at 197,
//! `WriteFile` at 385, `drop(stdin.broker)` at 396); the runner hands that same
//! handle to the child as `hStdInput` (`launch.rs` 243). The backend reads EOF
//! at once (measured by the engineer on 2026-10-20: EOF ~1 ms after start
//! while the parent still held stdin open). stdout / stderr are pumped back
//! and arrive in real time.
//!
//! So ACP runs over a one-shot local endpoint instead: Fox listens first, srt
//! starts `node bridge.mjs -- <backend>` inside the sandbox, the bridge
//! connects back, sends `FOX-ACP-BRIDGE/1 <token>\n`, Fox compares the token
//! in constant time and answers `OK {"cwd":..,"env":{..}}\n` (one line of
//! compact JSON; see [`crate::launch::BridgeEndpoint::handshake_answer`]) or
//! `DENIED\n` and drops the connection, and only then does the bridge start
//! the backend (in that cwd, with that env) and relay bytes both ways.
//!
//! * Windows: named pipe `\\.\pipe\fox-acp-<128-bit hex>`, protected DACL
//!   granting only the current user (full) and the recorded srt sandbox SID
//!   (read / write), `FILE_FLAG_FIRST_PIPE_INSTANCE` (fails if the name
//!   already exists, so nobody can pre-create it), one instance,
//!   `PIPE_REJECT_REMOTE_CLIENTS`. After the single connection no new instance
//!   is created: a second client gets `ERROR_PIPE_BUSY` / not found.
//! * Unix: a socket file (mode 0600) in the temp directory. Production on
//!   Linux / macOS does not use the bridge (stdin works there); this exists so
//!   the handshake, the relay and the bridge script are tested on any host.

use crate::launch::{constant_time_eq, BridgeEndpoint, BRIDGE_PROTOCOL};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A connected, authenticated bridge stream.
pub trait BridgeStream: AsyncRead + AsyncWrite + Unpin + Send + 'static {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send + 'static> BridgeStream for T {}

/// Longest handshake line accepted (protocol + space + 32 hex + newline is 50).
const MAX_HANDSHAKE_LINE: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeErrorKind {
    /// The endpoint could not be created (name taken, DACL, no SID).
    ListenFailed,
    /// Nobody connected and finished the handshake within the timeout.
    ConnectTimeout,
    /// The connection broke or the first line was not a handshake.
    HandshakeFailed,
    /// A well-formed handshake with the wrong token.
    HandshakeRejected,
}

impl BridgeErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ListenFailed => "bridge_listen_failed",
            Self::ConnectTimeout => "bridge_connect_timeout",
            Self::HandshakeFailed => "bridge_handshake_failed",
            Self::HandshakeRejected => "bridge_handshake_rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeError {
    pub kind: BridgeErrorKind,
    pub message: String,
}

impl BridgeError {
    fn new(kind: BridgeErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

/// Server side of the handshake on an accepted stream. Reads one line byte by
/// byte (never past the newline), checks protocol and token, answers
/// `answer` (normally [`BridgeEndpoint::handshake_answer`]:
/// `OK {"cwd":..,"env":{..}}\n`, the backend's working directory and
/// environment) or `DENIED\n`. The token never appears in an error message.
pub async fn server_handshake<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, token: &str, answer: &[u8]) -> Result<(), BridgeError> {
    let mut line = Vec::with_capacity(64);
    loop {
        let mut byte = [0u8; 1];
        let read = stream
            .read(&mut byte)
            .await
            .map_err(|error| BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("reading the handshake failed: {error}")))?;
        if read == 0 {
            return Err(BridgeError::new(BridgeErrorKind::HandshakeFailed, "the bridge closed the connection before sending a handshake"));
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > MAX_HANDSHAKE_LINE {
            return Err(BridgeError::new(BridgeErrorKind::HandshakeFailed, "handshake line too long"));
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let prefix = format!("{BRIDGE_PROTOCOL} ");
    let Some(presented) = line.strip_prefix(prefix.as_bytes()) else {
        let _ = stream.write_all(b"DENIED\n").await;
        return Err(BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("first line is not a {BRIDGE_PROTOCOL} handshake")));
    };
    if !constant_time_eq(presented, token.as_bytes()) {
        let _ = stream.write_all(b"DENIED\n").await;
        let _ = stream.flush().await;
        return Err(BridgeError::new(BridgeErrorKind::HandshakeRejected, "the bridge presented a wrong token"));
    }
    stream
        .write_all(answer)
        .await
        .map_err(|error| BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("answering the handshake failed: {error}")))?;
    stream
        .flush()
        .await
        .map_err(|error| BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("answering the handshake failed: {error}")))?;
    Ok(())
}

/// A bound, not yet connected endpoint. Accepts exactly one connection.
pub struct BridgeListener {
    inner: imp::Listener,
    token: String,
    answer: Vec<u8>,
}

impl std::fmt::Debug for BridgeListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeListener").finish_non_exhaustive()
    }
}

impl BridgeListener {
    /// Create the endpoint. Must happen before the sandboxed process starts.
    pub fn bind(endpoint: &BridgeEndpoint) -> Result<Self, BridgeError> {
        Ok(Self { inner: imp::Listener::bind(endpoint)?, token: endpoint.token.clone(), answer: endpoint.handshake_answer() })
    }

    /// Wait for the one connection and run the handshake. The listener is
    /// consumed: no second connection is ever accepted. No timeout here; the
    /// caller races this against the connect timeout and the process exit.
    pub async fn accept(self) -> Result<Box<dyn BridgeStream>, BridgeError> {
        let Self { inner, token, answer } = self;
        let mut stream = inner.accept_one().await?;
        server_handshake(&mut stream, &token, &answer).await?;
        Ok(stream)
    }
}

#[cfg(windows)]
mod imp {
    use super::{BridgeError, BridgeErrorKind, BridgeStream};
    use crate::launch::BridgeEndpoint;
    use crate::winacl::{is_string_sid, WindowsAclProbe};
    use tokio::net::windows::named_pipe::{NamedPipeServer, PipeMode, ServerOptions};

    pub struct Listener(NamedPipeServer);

    /// Protected DACL: current user full control, srt sandbox account read +
    /// write, nobody else (no SYSTEM, no Administrators, no Everyone).
    pub fn pipe_sddl(user_sid: &str, sandbox_sid: &str) -> String {
        format!("D:P(A;;FA;;;{user_sid})(A;;FRFW;;;{sandbox_sid})")
    }

    impl Listener {
        pub fn bind(endpoint: &BridgeEndpoint) -> Result<Self, BridgeError> {
            use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
            use windows_sys::Win32::Security::Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
            use windows_sys::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
            let listen = |message: String| BridgeError::new(BridgeErrorKind::ListenFailed, message);
            let sandbox_sid = endpoint
                .allowed_sid
                .as_deref()
                .ok_or_else(|| listen("no srt sandbox SID recorded (HKLM\\SOFTWARE\\sandbox-runtime SandboxUserSid); run the sandbox setup".into()))?;
            let user_sid = crate::winacl::RealWindowsAcl.current_user_sid().map_err(listen)?;
            if !is_string_sid(sandbox_sid) || !is_string_sid(&user_sid) {
                return Err(listen(format!("invalid SID for the pipe DACL ({sandbox_sid:?} / {user_sid:?})")));
            }
            if !endpoint.address.starts_with(r"\\.\pipe\fox-acp-") {
                return Err(listen(format!("unexpected pipe name {}", endpoint.address)));
            }
            let sddl: Vec<u16> = pipe_sddl(&user_sid, sandbox_sid).encode_utf16().chain(std::iter::once(0)).collect();
            let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
            let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, std::ptr::null_mut()) };
            if ok == 0 {
                return Err(listen(format!("cannot build the pipe security descriptor (Win32 error {})", unsafe { GetLastError() })));
            }
            let mut attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor,
                bInheritHandle: 0,
            };
            let created = unsafe {
                ServerOptions::new()
                    .first_pipe_instance(true)
                    .max_instances(1)
                    .reject_remote_clients(true)
                    .pipe_mode(PipeMode::Byte)
                    .access_inbound(true)
                    .access_outbound(true)
                    .create_with_security_attributes_raw(&endpoint.address, &mut attributes as *mut SECURITY_ATTRIBUTES as *mut core::ffi::c_void)
            };
            unsafe { LocalFree(descriptor as _) };
            created.map(Self).map_err(|error| listen(format!("cannot create {}: {error}", endpoint.address)))
        }

        pub async fn accept_one(self) -> Result<Box<dyn BridgeStream>, BridgeError> {
            let server = self.0;
            server
                .connect()
                .await
                .map_err(|error| BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("pipe connect failed: {error}")))?;
            Ok(Box::new(server))
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::{BridgeError, BridgeErrorKind, BridgeStream};
    use crate::launch::BridgeEndpoint;
    use std::path::PathBuf;

    pub struct Listener {
        listener: tokio::net::UnixListener,
        path: PathBuf,
    }

    impl Listener {
        pub fn bind(endpoint: &BridgeEndpoint) -> Result<Self, BridgeError> {
            let path = PathBuf::from(&endpoint.address);
            let listen = |message: String| BridgeError::new(BridgeErrorKind::ListenFailed, message);
            if path.exists() {
                return Err(listen(format!("{} already exists", path.display())));
            }
            let listener = tokio::net::UnixListener::bind(&path).map_err(|error| listen(format!("cannot bind {}: {error}", path.display())))?;
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            Ok(Self { listener, path })
        }

        pub async fn accept_one(self) -> Result<Box<dyn BridgeStream>, BridgeError> {
            let accepted = self.listener.accept().await;
            // One connection only: the socket file goes away now (and in Drop).
            let _ = std::fs::remove_file(&self.path);
            let (stream, _) = accepted.map_err(|error| BridgeError::new(BridgeErrorKind::HandshakeFailed, format!("accept failed: {error}")))?;
            Ok(Box::new(stream))
        }
    }

    impl Drop for Listener {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod imp {
    use super::{BridgeError, BridgeErrorKind, BridgeStream};
    use crate::launch::BridgeEndpoint;
    pub struct Listener;
    impl Listener {
        pub fn bind(_: &BridgeEndpoint) -> Result<Self, BridgeError> {
            Err(BridgeError::new(BridgeErrorKind::ListenFailed, "no bridge endpoint on this platform"))
        }
        pub async fn accept_one(self) -> Result<Box<dyn BridgeStream>, BridgeError> {
            Err(BridgeError::new(BridgeErrorKind::ListenFailed, "no bridge endpoint on this platform"))
        }
    }
}

#[cfg(windows)]
pub use imp::pipe_sddl;

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(client_line: &'static [u8], token: &str) -> (Result<(), BridgeError>, Vec<u8>) {
        let (mut host, mut bridge) = tokio::io::duplex(1024);
        let token = token.to_owned();
        let server = tokio::spawn(async move { server_handshake(&mut host, &token, b"OK\n").await });
        bridge.write_all(client_line).await.unwrap();
        let result = server.await.unwrap();
        let mut answer = vec![0u8; 16];
        let read = tokio::time::timeout(std::time::Duration::from_secs(2), bridge.read(&mut answer)).await.ok().and_then(Result::ok).unwrap_or(0);
        answer.truncate(read);
        (result, answer)
    }

    #[tokio::test]
    async fn handshake_accepts_the_right_token_only() {
        let token = "0123456789abcdef0123456789abcdef";
        let (ok, answer) = run(b"FOX-ACP-BRIDGE/1 0123456789abcdef0123456789abcdef\n", token).await;
        assert!(ok.is_ok());
        assert_eq!(answer, b"OK\n");
        let (wrong, answer) = run(b"FOX-ACP-BRIDGE/1 ffffffffffffffffffffffffffffffff\n", token).await;
        let error = wrong.unwrap_err();
        assert_eq!(error.kind, BridgeErrorKind::HandshakeRejected);
        assert!(!error.to_string().contains("ffff") && !error.to_string().contains(token), "no token in messages");
        assert_eq!(answer, b"DENIED\n");
        let (garbage, _) = run(b"{\"jsonrpc\":\"2.0\"}\n", token).await;
        assert_eq!(garbage.unwrap_err().kind, BridgeErrorKind::HandshakeFailed);
        let (short, _) = run(b"FOX-ACP-BRIDGE/1 0123\n", token).await;
        assert_eq!(short.unwrap_err().kind, BridgeErrorKind::HandshakeRejected);
    }

    #[tokio::test]
    async fn handshake_fails_on_early_close_and_overlong_lines() {
        let (mut host, bridge) = tokio::io::duplex(1024);
        drop(bridge);
        assert_eq!(server_handshake(&mut host, "t", b"OK\n").await.unwrap_err().kind, BridgeErrorKind::HandshakeFailed);
        let (long, _) = run(&[b'A'; 300], "t").await;
        assert_eq!(long.unwrap_err().kind, BridgeErrorKind::HandshakeFailed);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_listener_accepts_one_connection_and_removes_the_socket() {
        let endpoint = BridgeEndpoint::new_local(None).unwrap();
        let listener = BridgeListener::bind(&endpoint).unwrap();
        assert!(BridgeListener::bind(&endpoint).is_err(), "a second bind on the same name fails");
        let address = endpoint.address.clone();
        let token = endpoint.token.clone();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::UnixStream::connect(&address).await.unwrap();
            stream.write_all(format!("FOX-ACP-BRIDGE/1 {token}\n").as_bytes()).await.unwrap();
            let expected = b"OK {\"env\":{}}\n";
            let mut answer = vec![0u8; expected.len()];
            stream.read_exact(&mut answer).await.unwrap();
            assert_eq!(answer, expected.to_vec());
            stream.write_all(b"ping").await.unwrap();
        });
        let mut stream = listener.accept().await.unwrap();
        let mut ping = [0u8; 4];
        stream.read_exact(&mut ping).await.unwrap();
        assert_eq!(&ping, b"ping");
        client.await.unwrap();
        assert!(!std::path::Path::new(&endpoint.address).exists(), "no further connections possible");
    }

    /// Windows: the real named pipe. FIRST_PIPE_INSTANCE (the name cannot be
    /// created twice), one instance (a second client is refused while the
    /// first is connected), the handshake, then the stream. The DACL against
    /// another account is a hardware check (docs/sandbox.md 9).
    #[cfg(windows)]
    #[tokio::test]
    async fn windows_pipe_is_first_instance_single_client() {
        use crate::winacl::{RealWindowsAcl, WindowsAclProbe};
        use tokio::net::windows::named_pipe::ClientOptions;
        let sid = RealWindowsAcl.current_user_sid().unwrap();
        assert_eq!(pipe_sddl(&sid, "S-1-5-21-1-2-3-1010"), format!("D:P(A;;FA;;;{sid})(A;;FRFW;;;S-1-5-21-1-2-3-1010)"));
        let endpoint = BridgeEndpoint::new_local(Some(sid)).unwrap();
        assert!(endpoint.address.starts_with(r"\\.\pipe\fox-acp-"));
        let listener = BridgeListener::bind(&endpoint).unwrap();
        assert_eq!(BridgeListener::bind(&endpoint).unwrap_err().kind, BridgeErrorKind::ListenFailed, "name cannot be taken twice");
        let token = endpoint.token.clone();
        let address = endpoint.address.clone();
        let accept = tokio::spawn(listener.accept());
        let mut first = loop {
            match ClientOptions::new().open(&address) {
                Ok(client) => break client,
                Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
            }
        };
        assert!(ClientOptions::new().open(&address).is_err(), "one instance only");
        first.write_all(format!("FOX-ACP-BRIDGE/1 {token}\n").as_bytes()).await.unwrap();
        let expected = b"OK {\"env\":{}}\n";
        let mut answer = vec![0u8; expected.len()];
        first.read_exact(&mut answer).await.unwrap();
        assert_eq!(answer, expected.to_vec());
        let mut stream = accept.await.unwrap().unwrap();
        first.write_all(b"ping").await.unwrap();
        let mut ping = [0u8; 4];
        stream.read_exact(&mut ping).await.unwrap();
        assert_eq!(&ping, b"ping");
        assert!(ClientOptions::new().open(&address).is_err(), "no second connection after the first");
    }

    #[cfg(windows)]
    #[test]
    fn windows_pipe_without_a_sandbox_sid_is_refused() {
        let endpoint = BridgeEndpoint::new_local(None).unwrap();
        assert_eq!(BridgeListener::bind(&endpoint).unwrap_err().kind, BridgeErrorKind::ListenFailed);
    }

    /// The backend's environment and cwd travel in the handshake answer, so
    /// values cmd.exe cannot carry (quotes, `%`, line breaks) arrive intact.
    #[cfg(unix)]
    #[tokio::test]
    async fn listener_answers_with_the_child_env_and_cwd() {
        let mut endpoint = BridgeEndpoint::new_local(None).unwrap();
        endpoint.child_env = vec![("CODEX_CONFIG".into(), "{\"web_search\":\"disabled\"}".into()), ("PCT".into(), "a%b%\nc".into())];
        endpoint.child_cwd = Some("/w/proj".into());
        let listener = BridgeListener::bind(&endpoint).unwrap();
        let address = endpoint.address.clone();
        let token = endpoint.token.clone();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::UnixStream::connect(&address).await.unwrap();
            stream.write_all(format!("FOX-ACP-BRIDGE/1 {token}\n").as_bytes()).await.unwrap();
            let mut line = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                stream.read_exact(&mut byte).await.unwrap();
                if byte[0] == b'\n' {
                    break;
                }
                line.push(byte[0]);
            }
            line
        });
        let _stream = listener.accept().await.unwrap();
        let line = client.await.unwrap();
        assert!(line.starts_with(b"OK "));
        let payload: serde_json::Value = serde_json::from_slice(&line[3..]).unwrap();
        assert_eq!(payload["cwd"], "/w/proj");
        assert_eq!(payload["env"]["CODEX_CONFIG"], "{\"web_search\":\"disabled\"}");
        assert_eq!(payload["env"]["PCT"], "a%b%\nc");
    }

    #[test]
    fn bridge_script_is_embedded() {
        assert!(crate::launch::BRIDGE_SCRIPT.contains(BRIDGE_PROTOCOL));
        assert!(crate::launch::BRIDGE_SCRIPT.contains(crate::launch::BRIDGE_TOKEN_ENV) && crate::launch::BRIDGE_SCRIPT.contains(crate::launch::BRIDGE_PIPE_ENV));
    }
}
