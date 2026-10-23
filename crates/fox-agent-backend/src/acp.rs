//! Host-side ACP v1 client (plan 6.7) on the official `agent-client-protocol`
//! crate. One process per bound session (6.1): spawned with an allowlisted
//! environment, in its own process group on Unix, and killed as a tree on
//! close. On Windows the child is created suspended, placed in a Job Object of
//! its own (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, no breakaway) and only then
//! resumed, so it and every descendant that stays in the job die with the job
//! (`win_job`). In sandboxed mode the backend itself is NOT a descendant in
//! that sense: srt-win starts it through the Secondary Logon service as the
//! `srt-sandbox` account (two-hop launch), outside this job; its cleanup rests
//! on srt-win's own job chain (docs/sandbox.md section 7.4).
//!
//! What this module guarantees, matching the port rules:
//! * `session/cancel` answers every pending permission request of that session
//!   with `cancelled` (ACP spec requirement; plan 6.4 rule 1).
//! * Permission answers are reported as written, never as acknowledged: ACP v1
//!   has no acknowledgement for a permission response (plan 6.3 rule 3).
//! * Capabilities are read from `initialize` and default to "unsupported".
//! * Once a stop has begun ([`AcpConnection::begin_stopping`]) the connection
//!   refuses new work and answers every permission request that still arrives
//!   with `cancelled` itself; such a request never reaches the user as
//!   answerable ([`BackendEvent::PermissionAutoCancelled`] is observation only).
//! * A stop reports whether the whole process tree is confirmed gone
//!   ([`crate::port::StopConfirmation`], see [`crate::proctree`]).

use crate::launch::{BridgeEndpoint, LaunchSpec};
use crate::port::{CapabilityDeclaration, SessionCapabilities};
use crate::reverse::WriteOutcome;
use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, InitializeRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SelectedPermissionOutcome, SessionNotification, SetSessionModeRequest, TextContent,
};
use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::{Agent, ByteStreams, Client, ConnectionTo, Responder};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Something the backend told the Host.
#[derive(Debug, Clone)]
pub enum BackendEvent {
    Initialized {
        capabilities: CapabilityDeclaration,
        initialize_response: Value,
    },
    /// A `session/update` notification, kept as JSON for the Host mapper.
    Update { session_id: String, update: Value },
    /// The agent asks for permission. Answer with [`AcpConnection::answer_permission`].
    PermissionRequested {
        native_request_id: String,
        session_id: String,
        option_ids: Vec<String>,
        request: Value,
    },
    /// A permission request that arrived after the connection began stopping
    /// (revocation). The Host already answered it `cancelled`; it is NOT
    /// answerable and must not be shown as a pending approval. Observation
    /// only.
    PermissionAutoCancelled {
        session_id: String,
        reason: String,
        request: Value,
    },
    /// Ordering fence: the `session/load` reply for `session_id` arrived.
    /// Every `session/update` the agent sent before that reply (the history
    /// replay) is ahead of this event in the stream. Queued by the command
    /// task after the reply, so it can never overtake those updates (the
    /// connection dispatches incoming messages on one task, in order).
    LoadSettled { session_id: String },
    /// Ordering fence: Fox is about to write `session/prompt` for
    /// `session_id`. Queued before the request is written, so nothing the
    /// agent sends in answer to that prompt can be ahead of it; anything ahead
    /// of it (a late history replay) is not part of the turn.
    PromptStarted { session_id: String },
    TurnEnded { session_id: String, stop_reason: Value },
    /// `session/prompt` itself failed (JSON-RPC error, transport closed while
    /// the turn ran). The turn is over; the connection may still be alive.
    ///
    /// `error` carries only the JSON-RPC `code` and `message`
    /// ([`rpc_error_summary`]); the error's `data` (which may echo request
    /// headers or keys) is kept apart, unredacted, in `data`
    /// ([`rpc_error_data_text`]) and redacted where it leaves the process
    /// ([`crate::external::rpc_error_data_diagnostic`]).
    TurnFailed { session_id: String, error: String, data: Option<String> },
    /// The connection failed (not tied to one turn).
    Failed(String),
    Closed,
}

enum Command {
    NewSession {
        cwd: PathBuf,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// `session/load` (history replay arrives as `session/update` before the
    /// reply). Only sent when the agent declared `loadSession`.
    LoadSession {
        session_id: String,
        cwd: PathBuf,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Prompt {
        session_id: String,
        text: String,
    },
    Cancel {
        session_id: String,
    },
    /// `session/set_mode` (Codex permission modes, mid-session).
    SetMode {
        session_id: String,
        mode_id: String,
        reply: oneshot::Sender<Result<(), String>>,
    },
}

/// Pending permission requests plus the stopping flag, under ONE lock: the
/// request handler checks the flag and registers the request atomically, and
/// [`AcpConnection::begin_stopping`] sets the flag and drains the map
/// atomically, so a request can never slip in between.
#[derive(Default)]
struct PermissionRegistry {
    stopping: bool,
    map: HashMap<String, (String, Responder<RequestPermissionResponse>)>,
}

type PendingPermissions = Arc<Mutex<PermissionRegistry>>;

/// How long [`AcpConnection::set_mode`] waits for the agent's reply.
pub const SET_MODE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Error text for work refused while the connection is stopping.
pub const STOPPING_REFUSAL: &str = "backend connection is stopping (full access revoked); no new tasks are accepted";

/// The user-facing text of a JSON-RPC error: its `message` and `code`, never
/// its `data`. `agent_client_protocol::Error`'s `Display` appends `data` as
/// pretty JSON, unredacted (979313c closeout); this replaces it everywhere a
/// backend error becomes a run / session failure message.
pub fn rpc_error_summary(error: &agent_client_protocol::Error) -> String {
    let code = i32::from(error.code);
    if error.message.trim().is_empty() {
        format!("JSON-RPC error {code}")
    } else {
        format!("{} (JSON-RPC {code})", error.message)
    }
}

fn lock_data(slot: &Mutex<Option<String>>) -> std::sync::MutexGuard<'_, Option<String>> {
    slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The JSON-RPC error's `data`, serialized (pretty, so each field is on its
/// own line for line-based redaction), UNREDACTED. `None` when absent or
/// `null`. Redact before it leaves the process
/// ([`crate::external::rpc_error_data_diagnostic`]).
pub fn rpc_error_data_text(error: &agent_client_protocol::Error) -> Option<String> {
    let data = error.data.as_ref().filter(|data| !data.is_null())?;
    Some(serde_json::to_string_pretty(data).unwrap_or_else(|_| data.to_string()))
}

pub struct AcpConnection {
    commands: mpsc::UnboundedSender<Command>,
    pub events: mpsc::UnboundedReceiver<BackendEvent>,
    pending: PendingPermissions,
    child_id: Option<u32>,
    child: Arc<tokio::sync::Mutex<tokio::process::Child>>,
    /// Windows: the Job Object holding the backend tree (`None` elsewhere).
    job: ProcessJob,
    stderr_tail: Tail,
    /// `data` of the last failed `session/new` / `session/load` reply
    /// (unredacted; [`Self::take_rpc_error_data`]).
    rpc_error_data: Arc<Mutex<Option<String>>>,
    /// Per-session srt state (settings file), removed when the connection
    /// goes away. `None` in full-access mode.
    sandbox: Option<crate::sandbox::SandboxSession>,
    permission_mode: crate::sandbox::PermissionMode,
}

/// Map an ACP `initialize` response onto the port's capability declaration.
/// Anything the agent does not declare stays unsupported.
pub fn capabilities_from_initialize(response: &Value) -> CapabilityDeclaration {
    let caps = response.get("agentCapabilities").cloned().unwrap_or(Value::Null);
    let session_caps = caps.get("sessionCapabilities").cloned().unwrap_or(Value::Null);
    let present = |value: &Value, key: &str| match value.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Null) | None => false,
        Some(_) => true,
    };
    let mut declared = CapabilityDeclaration::unverified();
    declared.session = SessionCapabilities {
        load: present(&caps, "loadSession"),
        resume: present(&session_caps, "resume"),
        close: present(&session_caps, "close"),
        list: present(&session_caps, "list"),
        steer: false,
    };
    declared.approval_answer_acknowledged = false;
    declared
}

/// How long `initialize` may take. Generous: a cold `npx` install or a slow
/// filesystem (dsh measured ~76 s from a network-backed workspace) is normal.
pub const INITIALIZE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);
/// Grace between SIGTERM and SIGKILL on close. pi cleans up its own bash
/// children (which run in their own process groups) on SIGTERM; a bare
/// SIGKILL of Fox's group orphaned them in the pi branch measurement.
pub const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
const STDERR_TAIL_BYTES: usize = 8 * 1024;


/// How long a failed start waits for stderr to drain after the process ended.
const STDERR_DRAIN: std::time::Duration = std::time::Duration::from_secs(1);
/// After a transport error during `initialize`: how long to wait for the
/// process to exit so the failure can be reported as an exit.
const EXIT_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);
/// At most this much of each stderr / stdout tail goes into a failure report.
pub const FAILURE_TAIL_BYTES: usize = 4 * 1024;

type Tail = Arc<Mutex<std::collections::VecDeque<String>>>;

/// Keep the last bytes of a stream for diagnostics, split into lines. NOT
/// redacted here: a text is redacted exactly once, where it leaves the
/// process ([`InitializeFailure::attach_tails`],
/// [`crate::external::stderr_diagnostic`], [`AcpConnection::stderr_tail`]);
/// redacting at every stage left `[redacted]]]` and ate JSON braces (5f2f2d8
/// re-test).
fn collect_tail<R: tokio::io::AsyncRead + Unpin + Send + 'static>(stream: R, tail: Tail) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Ok(mut tail) = tail.lock() {
                tail.push_back(line);
                while tail.iter().map(String::len).sum::<usize>() > STDERR_TAIL_BYTES {
                    tail.pop_front();
                }
            }
        }
    })
}

fn tail_text(tail: &Tail) -> String {
    tail.lock().map(|lines| lines.iter().cloned().collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// The last `max` bytes of `text`, cut at a line (or char) boundary.
fn limit_tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let cut = &text[start..];
    match cut.find('\n') {
        Some(newline) if newline + 1 < cut.len() => cut[newline + 1..].to_owned(),
        _ => cut.to_owned(),
    }
}

fn describe_exit(status: Option<std::process::ExitStatus>) -> String {
    match status.and_then(|status| status.code()) {
        Some(code) => format!("exit code {code}"),
        None => match status {
            Some(_) => "terminated by a signal".to_owned(),
            None => "exit status unknown".to_owned(),
        },
    }
}

/// Whether a stderr line comes from srt / srt-win / the Fox bridge rather than
/// the backend. Heuristic by prefix: all of them share one stderr pipe.
pub fn is_sandbox_stderr_line(line: &str) -> bool {
    let line = line.trim_start();
    ["srt-win:", "srt:", "[SandboxDebug]", "[sandbox-runtime]", "fox-acp-bridge:", "Sandbox ", "sandbox-runtime:"]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

/// Why a backend did not get through `initialize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializeFailureCode {
    /// The process could not be started or contained.
    SpawnFailed,
    /// The process (the backend, or srt in sandboxed mode) exited, with any
    /// exit code including 0, before the `initialize` response arrived, or
    /// before the bridge connected (`stage = bridge_not_connected`).
    BackendExitedBeforeInitialize,
    /// No `initialize` response within [`INITIALIZE_TIMEOUT`].
    InitializeTimeout,
    /// `initialize` failed while the process kept running (protocol error).
    InitializeFailed,
    BridgeListenFailed,
    /// The bridge did not connect and authenticate within its timeout.
    BridgeConnectTimeout,
    BridgeHandshakeFailed,
    /// The client presented a wrong token.
    BridgeHandshakeRejected,
}

impl InitializeFailureCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpawnFailed => "spawn_failed",
            Self::BackendExitedBeforeInitialize => "backend_exited_before_initialize",
            Self::InitializeTimeout => "initialize_timeout",
            Self::InitializeFailed => "initialize_failed",
            Self::BridgeListenFailed => "bridge_listen_failed",
            Self::BridgeConnectTimeout => "bridge_connect_timeout",
            Self::BridgeHandshakeFailed => "bridge_handshake_failed",
            Self::BridgeHandshakeRejected => "bridge_handshake_rejected",
        }
    }
}

/// Structured start failure. Every text field is redacted
/// ([`redact_line`]: API keys, tokens, proxy credentials; user-name paths are
/// kept) and each tail is at most [`FAILURE_TAIL_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InitializeFailure {
    pub code: InitializeFailureCode,
    pub message: String,
    /// `bridge_not_connected` or `initialize`.
    pub stage: Option<String>,
    /// Exit code of the process Fox started (the srt broker in sandboxed mode).
    pub exit_code: Option<i32>,
    /// Whole stderr tail as received.
    pub stderr_tail: String,
    /// Lines from srt / srt-win / the bridge ([`is_sandbox_stderr_line`]).
    pub srt_stderr_tail: String,
    /// Every other line (the backend's own stderr).
    pub backend_stderr_tail: String,
    /// Bridge mode: stdout of the process (not used for ACP there).
    pub stdout_tail: String,
    /// `data` of the `initialize` JSON-RPC error, unredacted; never
    /// serialized (it is redacted into the run diagnostic,
    /// `source: "rpc_error_data"`).
    #[serde(skip)]
    pub rpc_error_data: Option<String>,
}

impl InitializeFailure {
    pub fn new(code: InitializeFailureCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            stage: None,
            exit_code: None,
            stderr_tail: String::new(),
            srt_stderr_tail: String::new(),
            backend_stderr_tail: String::new(),
            stdout_tail: String::new(),
            rpc_error_data: None,
        }
    }

    fn stage(mut self, stage: &str) -> Self {
        self.stage = Some(stage.to_owned());
        self
    }

    fn from_bridge(error: crate::bridge::BridgeError) -> Self {
        use crate::bridge::BridgeErrorKind as Kind;
        let code = match error.kind {
            Kind::ListenFailed => InitializeFailureCode::BridgeListenFailed,
            Kind::ConnectTimeout => InitializeFailureCode::BridgeConnectTimeout,
            Kind::HandshakeFailed => InitializeFailureCode::BridgeHandshakeFailed,
            Kind::HandshakeRejected => InitializeFailureCode::BridgeHandshakeRejected,
        };
        Self::new(code, error.message).stage("bridge_not_connected")
    }

    /// `stderr` / `stdout` arrive unredacted; this is where they are redacted
    /// (once) for the report.
    fn attach_tails(&mut self, stderr: &str, stdout: &str) {
        let stderr = &redact_text(stderr);
        let stdout = &redact_text(stdout);
        let (sandbox, backend): (Vec<&str>, Vec<&str>) = stderr.lines().partition(|line| is_sandbox_stderr_line(line));
        self.stderr_tail = limit_tail(stderr, FAILURE_TAIL_BYTES);
        self.srt_stderr_tail = limit_tail(&sandbox.join("\n"), FAILURE_TAIL_BYTES);
        self.backend_stderr_tail = limit_tail(&backend.join("\n"), FAILURE_TAIL_BYTES);
        self.stdout_tail = limit_tail(stdout, FAILURE_TAIL_BYTES);
    }
}

impl std::fmt::Display for InitializeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)?;
        if let Some(code) = self.exit_code {
            write!(f, " [exit code {code}]")?;
        }
        if !self.stderr_tail.is_empty() {
            write!(f, "; backend stderr (redacted):\n{}", self.stderr_tail)?;
        }
        if !self.stdout_tail.is_empty() {
            write!(f, "\nstdout (redacted):\n{}", self.stdout_tail)?;
        }
        Ok(())
    }
}

impl std::error::Error for InitializeFailure {}

/// Mask secret-shaped tokens in one diagnostic line. Masked: `sk-...`
/// tokens (anywhere in a word); the value of a sensitive assignment
/// (`OPENAI_API_KEY=...`, `token: ...`, `"api_key":"..."`, `password=...`;
/// names containing key / token / secret / password / passwd / credential /
/// authorization / cookie, or exactly `auth`), the name kept; the word(s)
/// after a sensitive header name that ends the word (`Authorization: Bearer
/// <x>`, `x-api-key: <x>`) and after `Bearer` / `Basic`;
/// long opaque strings (the bridge token included); and the user-info part of
/// URLs (`http://user:pass@host` -> `http://<redacted>@host`, the srt proxy
/// credentials). Paths, user names in paths included, are kept.
pub(crate) fn redact_line(line: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // How many following words to mask (header form / auth scheme).
    let mut mask_next = 0usize;
    for word in line.split_whitespace() {
        if mask_next > 0 {
            mask_next -= 1;
            let bare = word.trim_matches(|c: char| c == '"' || c == '\'' || c == ',');
            if is_auth_scheme(bare) {
                // `Authorization: Bearer x`: keep the scheme, mask what follows.
                out.push(word.to_owned());
                mask_next = 1;
            } else {
                let (masked, dangling) = mask_word(word);
                if dangling {
                    mask_next = 1;
                }
                out.push(masked);
            }
            continue;
        }
        let bare = word.trim_matches(|c: char| c == '"' || c == '\'' || c == ',' || c == '{' || c == '(');
        if is_auth_scheme(bare) {
            out.push(word.to_owned());
            mask_next = 1;
            continue;
        }
        if word.len() >= 32 && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            out.push(REDACTED.to_owned());
            continue;
        }
        let (masked, dangling) = redact_assignments(word);
        if dangling {
            mask_next = 1;
        }
        out.push(redact_sk_tokens(&redact_url_userinfo(&masked)));
    }
    out.join(" ")
}

/// The mask. Chosen so a second pass finds nothing in it: no `[` `]` (a
/// value stops at `]`, so `KEY=[redacted]` grew a `]` per pass), no quote,
/// comma or brace, not an auth scheme, shorter than the opaque-string rule.
/// [`redact_line`] is idempotent: `redact(redact(x)) == redact(x)`.
pub const REDACTED: &str = "<redacted>";

/// Mask the credential in one word that follows `Bearer` / a header name.
/// The credential runs from after any leading quotes / brackets to the first
/// JSON / quoting boundary (`"` `'` `,` `}` `]` `)` `;`); everything after it
/// is kept and redacted like any other text (so `zzz"}` -> `<redacted>"}` and
/// `v","x-api-key":"k"}` -> `<redacted>","x-api-key":"<redacted>"}`). An
/// already masked credential is left alone. Returns the word and whether it
/// ends with a sensitive name whose value is the next word.
fn mask_word(word: &str) -> (String, bool) {
    let is_lead = |c: char| matches!(c, '"' | '\'' | '(' | '[' | '{');
    let is_boundary = |c: char| matches!(c, '"' | '\'' | ',' | '}' | ']' | ')' | ';');
    let core_start = word.char_indices().find(|(_, c)| !is_lead(*c)).map_or(word.len(), |(i, _)| i);
    let core_end = word[core_start..].char_indices().find(|(_, c)| is_boundary(*c)).map_or(word.len(), |(i, _)| core_start + i);
    let core = &word[core_start..core_end];
    let (rest, dangling) = redact_assignments(&word[core_end..]);
    let rest = redact_sk_tokens(&redact_url_userinfo(&rest));
    let core = if core.is_empty() { "" } else { REDACTED };
    (format!("{}{core}{rest}", &word[..core_start]), dangling)
}

fn is_auth_scheme(word: &str) -> bool {
    ["bearer", "basic"].iter().any(|scheme| word.eq_ignore_ascii_case(scheme))
}

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-')).to_ascii_lowercase();
    if lower.is_empty() {
        return false;
    }
    lower == "auth"
        || ["key", "token", "secret", "password", "passwd", "credential", "authorization", "cookie", "proxy-authorization"]
            .iter()
            .any(|marker| lower.contains(marker))
}

/// Mask the value of every `name=value` / `name: value` / `"name":"value"`
/// in `word` whose name is sensitive. Returns the masked word and whether it
/// ended with a sensitive name and an empty value (header form: the value is
/// the next word).
fn redact_assignments(word: &str) -> (String, bool) {
    let bytes = word.as_bytes();
    let mut out = String::with_capacity(word.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    let mut dangling = false;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'=' || c == b':' {
            // `://` is a URL scheme, never an assignment.
            if c == b':' && word[i..].starts_with("://") {
                i += 3;
                continue;
            }
            // The name: identifier characters right before the separator,
            // skipping one closing quote.
            let mut name_end = i;
            if name_end > 0 && (bytes[name_end - 1] == b'"' || bytes[name_end - 1] == b'\'') {
                name_end -= 1;
            }
            let mut name_start = name_end;
            while name_start > 0 {
                let b = bytes[name_start - 1];
                if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' {
                    name_start -= 1;
                } else {
                    break;
                }
            }
            let name = &word[name_start..name_end];
            if is_sensitive_name(name) {
                // The value: after the separator (and an opening quote), up
                // to a closing quote or a structural delimiter.
                let mut value_start = i + 1;
                while value_start < bytes.len() && (bytes[value_start] == b'"' || bytes[value_start] == b'\'') {
                    value_start += 1;
                }
                let mut value_end = value_start;
                while value_end < bytes.len() && !matches!(bytes[value_end], b'"' | b'\'' | b',' | b'}' | b')' | b'&' | b';' | b']') {
                    value_end += 1;
                }
                if &word[value_start..value_end] == REDACTED {
                    i = value_end;
                    continue;
                }
                if value_end == value_start {
                    if value_end >= bytes.len() {
                        dangling = true;
                    }
                    i += 1;
                    continue;
                }
                if is_auth_scheme(&word[value_start..value_end]) {
                    // `"authorization":"Bearer <x>"`: the credential is the
                    // next word when the scheme ends this one.
                    if value_end >= bytes.len() {
                        dangling = true;
                    }
                    i = value_end;
                    continue;
                }
                out.push_str(&word[copied..value_start]);
                out.push_str(REDACTED);
                copied = value_end;
                i = value_end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&word[copied..]);
    (out, dangling)
}

/// Mask `sk-<token>` (OpenAI / DeepSeek / Anthropic style keys) anywhere in a
/// word; `sk-` must start the word or follow a non-alphanumeric character
/// (so `task-runner` and `risk-free` are kept) and be followed by at least
/// six token characters.
fn redact_sk_tokens(word: &str) -> String {
    let bytes = word.as_bytes();
    let mut out = String::with_capacity(word.len());
    let mut copied = 0usize;
    let mut i = 0usize;
    while i + 3 <= bytes.len() {
        if &bytes[i..i + 3] == b"sk-" && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric()) {
            let mut end = i + 3;
            while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'-' || bytes[end] == b'_') {
                end += 1;
            }
            if end - (i + 3) >= 6 {
                out.push_str(&word[copied..i]);
                out.push_str(REDACTED);
                copied = end;
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out.push_str(&word[copied..]);
    out
}

/// Redact a multi-line diagnostic text line by line ([`redact_line`]) and keep
/// its last [`FAILURE_TAIL_BYTES`]. Used before a stderr tail is persisted in
/// a run's terminal event.
pub fn redact_diagnostic_tail(text: &str) -> String {
    limit_tail(&redact_text(text), FAILURE_TAIL_BYTES)
}

/// The last [`FAILURE_TAIL_BYTES`] of `text`, cut at a line boundary.
pub fn limit_failure_tail(text: &str) -> String {
    limit_tail(text, FAILURE_TAIL_BYTES)
}

/// [`redact_line`] on every line of `text`.
pub fn redact_text(text: &str) -> String {
    text.lines().map(redact_line).collect::<Vec<_>>().join("\n")
}

/// `scheme://user:pass@host...` -> `scheme://<redacted>@host...` (every URL
/// in the word).
fn redact_url_userinfo(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut rest = word;
    while let Some(index) = rest.find("://") {
        let (head, tail) = rest.split_at(index + 3);
        out.push_str(head);
        let authority_end = tail.find(['/', '?', '#', '"', '\'', ',', ')']).unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match authority.rfind('@') {
            Some(at) => {
                out.push_str(REDACTED);
                out.push_str(&authority[at..]);
            }
            None => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

/// Windows Job Object owning the backend process tree.
///
/// Race: the child is spawned with `CREATE_SUSPENDED`, assigned, and only then
/// resumed (every thread of the new process, found with a Toolhelp snapshot;
/// a fresh suspended process has exactly one). It cannot run, and so cannot
/// spawn anything, before it is in the job. The job handle is not inheritable
/// and the limit has no `BREAKAWAY_OK`, so a descendant can leave only through
/// a different creation path: a process started by a service on its behalf
/// (Secondary Logon / `CreateProcessWithLogonW`, Task Scheduler, COM servers,
/// WMI) is created outside the job. srt-win's runner is exactly that case.
#[cfg(windows)]
pub mod win_job {
    use std::os::windows::io::RawHandle;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};

    /// `CREATE_SUSPENDED` for `Command::creation_flags`.
    pub const CREATE_SUSPENDED: u32 = windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    pub struct Job(HANDLE);

    // The handle is a kernel object handle owned only by this value.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn last_error() -> u32 {
        unsafe { GetLastError() }
    }

    impl Job {
        /// Unnamed job, kill-on-close, no breakaway allowed.
        pub fn new() -> Result<Self, String> {
            let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if handle.is_null() {
                return Err(format!("CreateJobObjectW failed (Win32 error {})", last_error()));
            }
            let job = Self(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &limits as *const _ as *const core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if ok == 0 {
                return Err(format!("SetInformationJobObject(KILL_ON_JOB_CLOSE) failed (Win32 error {})", last_error()));
            }
            Ok(job)
        }

        pub fn assign(&self, process: RawHandle) -> Result<(), String> {
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                return Err(format!("AssignProcessToJobObject failed (Win32 error {})", last_error()));
            }
            Ok(())
        }

        /// Whether `process` is in this job (diagnostics and tests).
        pub fn contains(&self, process: RawHandle) -> Option<bool> {
            let mut result = 0;
            (unsafe { IsProcessInJob(process as HANDLE, self.0, &mut result) } != 0).then_some(result != 0)
        }

        /// Number of processes currently in the job
        /// (`JobObjectBasicAccountingInformation.ActiveProcesses`). `Err`
        /// carries the Win32 error.
        pub fn active_processes(&self) -> Result<u32, u32> {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            let ok = unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    &mut info as *mut _ as *mut core::ffi::c_void,
                    std::mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(last_error());
            }
            Ok(info.ActiveProcesses)
        }

        /// Poll [`Self::active_processes`] until it is zero or `timeout`
        /// passes. Only an observed zero is `Confirmed`.
        pub fn confirm_empty(&self, timeout: std::time::Duration) -> crate::port::StopConfirmation {
            let deadline = std::time::Instant::now() + timeout;
            loop {
                let reason = match self.active_processes() {
                    Ok(0) => return crate::port::StopConfirmation::Confirmed,
                    Ok(count) => format!("Job Object still holds {count} active process(es)"),
                    Err(code) => format!("QueryInformationJobObject failed (Win32 error {code})"),
                };
                if std::time::Instant::now() >= deadline {
                    return crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec![reason] };
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
        }

        /// Kill every process in the job now. `Err` carries the Win32 error.
        pub fn terminate(&self, exit_code: u32) -> Result<(), u32> {
            if unsafe { TerminateJobObject(self.0, exit_code) } == 0 {
                return Err(last_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // KILL_ON_JOB_CLOSE: closing the last handle kills what is left.
            unsafe { CloseHandle(self.0) };
        }
    }

    /// Resume every thread of a process created with `CREATE_SUSPENDED`.
    /// Returns how many threads were resumed (must be at least one).
    pub fn resume_process(pid: u32) -> Result<usize, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot == INVALID_HANDLE_VALUE || snapshot.is_null() {
            return Err(format!("CreateToolhelp32Snapshot failed (Win32 error {})", last_error()));
        }
        let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut resumed = 0usize;
        let mut failure = None;
        let mut more = unsafe { Thread32First(snapshot, &mut entry) } != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
                if thread.is_null() {
                    failure = Some(format!("OpenThread({}) failed (Win32 error {})", entry.th32ThreadID, last_error()));
                } else {
                    if unsafe { ResumeThread(thread) } == u32::MAX {
                        failure = Some(format!("ResumeThread({}) failed (Win32 error {})", entry.th32ThreadID, last_error()));
                    } else {
                        resumed += 1;
                    }
                    unsafe { CloseHandle(thread) };
                }
            }
            more = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
        }
        unsafe { CloseHandle(snapshot) };
        match (resumed, failure) {
            (0, Some(error)) => Err(error),
            (0, None) => Err(format!("no thread of process {pid} found to resume")),
            (count, _) => Ok(count),
        }
    }
}

/// How a close ended, for the Host's process-cleanup record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminateReport {
    /// Whether the whole process tree is confirmed gone: Unix, process group
    /// gone (or zombies only) and every tracked descendant gone; Windows,
    /// Fox's Job Object empty (`taskkill` fallback: never confirmed).
    pub confirmation: crate::port::StopConfirmation,
    /// Set when "confirmed" covers less than the whole tree (Windows
    /// sandboxed: the srt-win runner subtree is outside Fox's job).
    pub confirmation_scope_note: Option<String>,
    /// Unix: the backend's process group.
    pub process_group: Option<u32>,
    /// Unix: every descendant seen before the kill (pid + start time).
    pub tracked: Vec<crate::proctree::ProcIdentity>,
    /// The backend did not exit within [`TERMINATE_GRACE`] and was killed.
    pub force_killed: bool,
    /// Windows: `Some(true)` when `TerminateJobObject` succeeded, `Some(false)`
    /// when it failed (see `notes`), `None` when there was no job (not
    /// Windows, or the job was never created).
    pub job_terminated: Option<bool>,
    /// What happened beyond the normal path (job failure, taskkill fallback
    /// and its result). Also written to stderr with a `fox-agent-backend:`
    /// prefix.
    pub notes: Vec<String>,
}

impl Default for TerminateReport {
    fn default() -> Self {
        Self {
            confirmation: crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["not checked".into()] },
            confirmation_scope_note: None,
            process_group: None,
            tracked: Vec::new(),
            force_killed: false,
            job_terminated: None,
            notes: Vec::new(),
        }
    }
}

impl TerminateReport {
    /// The port-level record of this stop.
    pub fn process_tree_stop(&self) -> crate::port::ProcessTreeStop {
        crate::port::ProcessTreeStop {
            force_killed: self.force_killed,
            confirmation: self.confirmation.clone(),
            confirmation_scope_note: self.confirmation_scope_note.clone(),
            process_group: self.process_group,
            tracked: self.tracked.clone(),
        }
    }
}

/// What "confirmed" means for a Windows sandboxed process (two-hop launch).
pub const WINDOWS_SANDBOXED_SCOPE_NOTE: &str = "Windows sandboxed: confirmed = Fox's Job Object is empty (node + srt CLI). The srt-win runner and the backend it starts as srt-sandbox are outside that job; they end through srt-win's own job chain when srt exits, which is not yet measured (docs/sandbox.md section 7.4).";

#[cfg(windows)]
type ProcessJob = Option<win_job::Job>;
#[cfg(not(windows))]
type ProcessJob = Option<NoJob>;
/// Placeholder: no Job Object off Windows (process groups instead).
#[cfg(not(windows))]
pub struct NoJob;

fn note(report: &mut TerminateReport, text: String) {
    eprintln!("fox-agent-backend: {text}");
    report.notes.push(text);
}

/// Kill the backend tree. Unix: SIGKILL to the process group. Windows:
/// `TerminateJobObject`; `taskkill /T /F` only as a fallback when there is no
/// job or terminating it failed, with its exit status checked and recorded.
fn kill_tree(pid: u32, job: &ProcessJob, report: &mut TerminateReport) {
    let _ = &job;
    #[cfg(windows)]
    {
        if let Some(job) = job {
            match job.terminate(1) {
                Ok(()) => {
                    report.job_terminated = Some(true);
                    return;
                }
                Err(code) => {
                    report.job_terminated = Some(false);
                    note(report, format!("TerminateJobObject failed (Win32 error {code}); falling back to taskkill /T /F for pid {pid}"));
                }
            }
        } else {
            note(report, format!("no Job Object for pid {pid}; falling back to taskkill /T /F"));
        }
    }
    #[cfg(unix)]
    {
        if let Err(error) = crate::proctree::signal_group(pid, libc::SIGKILL) {
            note(report, error);
        }
    }
    #[cfg(windows)]
    {
        let result = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match result {
            // 128: no such process (it had already exited).
            Ok(status) if status.success() => note(report, format!("taskkill /T /F pid {pid}: ok")),
            Ok(status) => note(report, format!("taskkill /T /F pid {pid}: exit {:?} (128 = process already gone)", status.code())),
            Err(error) => note(report, format!("taskkill /T /F pid {pid}: could not run ({error})")),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (pid, report);
    }
}

impl AcpConnection {
    /// Spawn the backend and complete `initialize` over its stdin / stdout.
    /// Returns after the `Initialized` event has been queued. The error text
    /// starts with the [`InitializeFailureCode`] (see [`Self::spawn_detailed`]).
    pub async fn spawn(spec: &LaunchSpec, cwd: &std::path::Path) -> Result<Self, String> {
        Self::spawn_detailed(spec, cwd, None).await.map_err(|failure| failure.to_string())
    }

    /// [`Self::spawn`] with a structured failure, and optionally over the ACP
    /// bridge instead of stdio (Windows sandboxed mode, docs/sandbox.md 7.12):
    /// the endpoint is created first, the process is started with stdin
    /// `null` (its stdout / stderr are kept for diagnostics only), and ACP
    /// runs over the one connection that completes the token handshake.
    ///
    /// Initialization succeeds only when the `initialize` response arrived.
    /// A process that exits first, with any exit code including 0, is
    /// [`InitializeFailureCode::BackendExitedBeforeInitialize`] (stage
    /// `bridge_not_connected` when the bridge never connected).
    pub async fn spawn_detailed(spec: &LaunchSpec, cwd: &std::path::Path, bridge: Option<BridgeEndpoint>) -> Result<Self, InitializeFailure> {
        use InitializeFailureCode as Code;
        // What the backend sees is the ordinary path form (no `\\?\`), see
        // launch::backend_visible_path. Over the bridge (Windows sandboxed)
        // the bridge sets it as the backend's cwd explicitly: srt-win does
        // not carry the caller's cwd into the sandbox.
        let cwd = crate::launch::backend_visible_path(cwd);
        let cwd = cwd.as_path();
        let bridge = bridge.map(|mut endpoint| {
            if endpoint.child_cwd.is_none() {
                endpoint.child_cwd = Some(cwd.to_string_lossy().into_owned());
            }
            endpoint
        });
        // Bridge: listen before the process exists.
        let listener = match &bridge {
            Some(endpoint) => Some(crate::bridge::BridgeListener::bind(endpoint).map_err(InitializeFailure::from_bridge)?),
            None => None,
        };
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(cwd)
            .env_clear()
            .envs(crate::launch::process_environment(spec, std::env::vars()))
            .stdin(if bridge.is_some() { std::process::Stdio::null() } else { std::process::Stdio::piped() })
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        command.creation_flags(win_job::CREATE_SUSPENDED);
        let mut child = command
            .spawn()
            .map_err(|error| InitializeFailure::new(Code::SpawnFailed, format!("failed to start {}: {error}", spec.program)))?;
        // Windows: into the job while suspended, then resume. Any failure
        // kills the suspended child; nothing runs outside the job.
        #[cfg(windows)]
        let job: ProcessJob = {
            let placed = (|| {
                let job = win_job::Job::new()?;
                let handle = child.raw_handle().ok_or("backend exited before it could be placed in a Job Object")?;
                job.assign(handle)?;
                let pid = child.id().ok_or("backend has no pid")?;
                win_job::resume_process(pid)?;
                Ok::<_, String>(job)
            })();
            match placed {
                Ok(job) => Some(job),
                Err(error) => {
                    let _ = child.start_kill();
                    return Err(InitializeFailure::new(Code::SpawnFailed, format!("could not contain {} in a Job Object: {error}", spec.program)));
                }
            }
        };
        #[cfg(not(windows))]
        let job: ProcessJob = None;
        let child_id = child.id();
        let stderr_tail: Tail = Arc::default();
        let mut stderr_task = child.stderr.take().map(|stderr| collect_tail(stderr, Arc::clone(&stderr_tail)));
        let stdout_tail: Tail = Arc::default();
        let stdout = child.stdout.take().ok_or_else(|| InitializeFailure::new(Code::SpawnFailed, "backend stdout unavailable"))?;

        // Fail: stop what was started, give stderr a moment to drain, attach
        // the tails. `exit` = the process had already exited.
        async fn fail(
            mut failure: InitializeFailure,
            child_id: Option<u32>,
            job: &ProcessJob,
            exit: Option<std::process::ExitStatus>,
            stderr_task: &mut Option<tokio::task::JoinHandle<()>>,
            stderr_tail: &Tail,
            stdout_tail: &Tail,
        ) -> InitializeFailure {
            if let Some(pid) = child_id {
                kill_tree(pid, job, &mut TerminateReport::default());
            }
            if let Some(task) = stderr_task.as_mut() {
                let _ = tokio::time::timeout(STDERR_DRAIN, task).await;
            }
            failure.exit_code = exit.and_then(|status| status.code());
            failure.attach_tails(&tail_text(stderr_tail), &tail_text(stdout_tail));
            failure
        }

        let (reader, writer): (Box<dyn tokio::io::AsyncRead + Unpin + Send>, Box<dyn tokio::io::AsyncWrite + Unpin + Send>) = match (listener, &bridge) {
            (Some(listener), Some(endpoint)) => {
                // stdout of srt / the bridge is diagnostics only.
                let _ = collect_tail(stdout, Arc::clone(&stdout_tail));
                enum Connect {
                    Accepted(Result<Box<dyn crate::bridge::BridgeStream>, crate::bridge::BridgeError>),
                    TimedOut,
                    Exited(std::io::Result<std::process::ExitStatus>),
                }
                let outcome = tokio::select! {
                    biased;
                    accepted = listener.accept() => Connect::Accepted(accepted),
                    status = child.wait() => Connect::Exited(status),
                    _ = tokio::time::sleep(endpoint.connect_timeout) => Connect::TimedOut,
                };
                match outcome {
                    Connect::Accepted(Ok(stream)) => {
                        let (read, write) = tokio::io::split(stream);
                        (Box::new(read), Box::new(write))
                    }
                    Connect::Accepted(Err(error)) => {
                        let failure = InitializeFailure::from_bridge(error);
                        return Err(fail(failure, child_id, &job, None, &mut stderr_task, &stderr_tail, &stdout_tail).await);
                    }
                    Connect::TimedOut => {
                        let failure = InitializeFailure::new(
                            Code::BridgeConnectTimeout,
                            format!("the sandbox bridge did not connect within {} s", endpoint.connect_timeout.as_secs_f32()),
                        )
                        .stage("bridge_not_connected");
                        return Err(fail(failure, child_id, &job, None, &mut stderr_task, &stderr_tail, &stdout_tail).await);
                    }
                    Connect::Exited(status) => {
                        let status = status.ok();
                        let failure = InitializeFailure::new(
                            Code::BackendExitedBeforeInitialize,
                            format!("the sandboxed process exited ({}) before the bridge pipe connected", describe_exit(status)),
                        )
                        .stage("bridge_not_connected");
                        return Err(fail(failure, child_id, &job, status, &mut stderr_task, &stderr_tail, &stdout_tail).await);
                    }
                }
            }
            _ => {
                let stdin = child.stdin.take().ok_or_else(|| InitializeFailure::new(Code::SpawnFailed, "backend stdin unavailable"))?;
                (Box::new(stdout), Box::new(stdin))
            }
        };
        let transport = ByteStreams::new(writer.compat_write(), reader.compat());

        let (event_tx, events) = mpsc::unbounded_channel();
        let (commands, mut command_rx) = mpsc::unbounded_channel::<Command>();
        let pending: PendingPermissions = Arc::default();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), (String, Option<String>)>>();

        let notify_tx = event_tx.clone();
        let permission_tx = event_tx.clone();
        let permission_pending = Arc::clone(&pending);
        let main_tx = event_tx.clone();
        let next_request = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rpc_error_data: Arc<Mutex<Option<String>>> = Arc::default();
        let session_error_data = Arc::clone(&rpc_error_data);

        tokio::spawn(async move {
            let result = Client
                .builder()
                .name("fox-host")
                .on_receive_notification(
                    async move |notification: SessionNotification, _cx| {
                        let update = serde_json::to_value(&notification.update).unwrap_or(Value::Null);
                        let _ = notify_tx.send(BackendEvent::Update {
                            session_id: notification.session_id.to_string(),
                            update,
                        });
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, responder: Responder<RequestPermissionResponse>, _cx| {
                        let id = format!(
                            "perm-{}",
                            next_request.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        );
                        let session_id = request.session_id.to_string();
                        let option_ids = request.options.iter().map(|option| option.option_id.to_string()).collect();
                        let raw = serde_json::to_value(&request).unwrap_or(Value::Null);
                        let mut registry = permission_pending.lock().expect("permission registry poisoned");
                        if registry.stopping {
                            // Late request after the stop began: answer it
                            // `cancelled` here; it never becomes answerable.
                            drop(registry);
                            let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
                            let _ = permission_tx.send(BackendEvent::PermissionAutoCancelled {
                                session_id,
                                reason: "connection stopping (full access revoked)".to_owned(),
                                request: raw,
                            });
                            return Ok(());
                        }
                        registry.map.insert(id.clone(), (session_id.clone(), responder));
                        // Queued while holding the lock, so it is ordered
                        // before any drain by begin_stopping.
                        let _ = permission_tx.send(BackendEvent::PermissionRequested {
                            native_request_id: id,
                            session_id,
                            option_ids,
                            request: raw,
                        });
                        drop(registry);
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(transport, async move |connection: ConnectionTo<Agent>| {
                    let init = connection
                        .send_request(InitializeRequest::new(ProtocolVersion::V1))
                        .block_task()
                        .await;
                    let init = match init {
                        Ok(init) => init,
                        Err(error) => {
                            let _ = ready_tx.send(Err((format!("initialize failed: {}", rpc_error_summary(&error)), rpc_error_data_text(&error))));
                            return Ok(());
                        }
                    };
                    let initialize_response = serde_json::to_value(&init).unwrap_or(Value::Null);
                    let _ = main_tx.send(BackendEvent::Initialized {
                        capabilities: capabilities_from_initialize(&initialize_response),
                        initialize_response,
                    });
                    let _ = ready_tx.send(Ok(()));
                    while let Some(command) = command_rx.recv().await {
                        match command {
                            Command::NewSession { cwd, reply } => {
                                let cwd = crate::launch::backend_visible_path(&cwd);
                                let created = connection
                                    .send_request(NewSessionRequest::new(cwd))
                                    .block_task()
                                    .await
                                    .map(|response| response.session_id.to_string())
                                    .map_err(|error| {
                                        *lock_data(&session_error_data) = rpc_error_data_text(&error);
                                        rpc_error_summary(&error)
                                    });
                                let _ = reply.send(created);
                            }
                            Command::LoadSession { session_id, cwd, reply } => {
                                let cwd = crate::launch::backend_visible_path(&cwd);
                                let fence_session = session_id.clone();
                                let loaded = connection
                                    .send_request(LoadSessionRequest::new(session_id, cwd))
                                    .block_task()
                                    .await
                                    .map(|_| ())
                                    .map_err(|error| {
                                        *lock_data(&session_error_data) = rpc_error_data_text(&error);
                                        rpc_error_summary(&error)
                                    });
                                let _ = main_tx.send(BackendEvent::LoadSettled { session_id: fence_session });
                                let _ = reply.send(loaded);
                            }
                            Command::Prompt { session_id, text } => {
                                // Fence first: queued before the request is
                                // written (see BackendEvent::PromptStarted).
                                let _ = main_tx.send(BackendEvent::PromptStarted { session_id: session_id.to_string() });
                                // Prompt completion arrives on its own task so
                                // cancel and permission answers keep flowing.
                                let connection = connection.clone();
                                let tx = main_tx.clone();
                                tokio::spawn(async move {
                                    let outcome = connection
                                        .send_request(PromptRequest::new(
                                            session_id.clone(),
                                            vec![ContentBlock::Text(TextContent::new(text))],
                                        ))
                                        .block_task()
                                        .await;
                                    let _ = tx.send(match outcome {
                                        Ok(response) => BackendEvent::TurnEnded {
                                            session_id,
                                            stop_reason: serde_json::to_value(&response.stop_reason)
                                                .unwrap_or(Value::Null),
                                        },
                                        Err(error) => BackendEvent::TurnFailed {
                                            session_id,
                                            error: format!("prompt failed: {}", rpc_error_summary(&error)),
                                            data: rpc_error_data_text(&error),
                                        },
                                    });
                                });
                            }
                            Command::Cancel { session_id } => {
                                let _ = connection.send_notification(CancelNotification::new(session_id));
                            }
                            Command::SetMode { session_id, mode_id, reply } => {
                                // Its own task: a mode switch during a turn
                                // must not block cancel / permission answers.
                                let connection = connection.clone();
                                let session_error_data = session_error_data.clone();
                                tokio::spawn(async move {
                                    let switched = connection
                                        .send_request(SetSessionModeRequest::new(session_id, mode_id))
                                        .block_task()
                                        .await
                                        .map(|_| ())
                                        .map_err(|error| {
                                            *lock_data(&session_error_data) = rpc_error_data_text(&error);
                                            rpc_error_summary(&error)
                                        });
                                    let _ = reply.send(switched);
                                });
                            }
                        }
                    }
                    Ok(())
                })
                .await;
            if let Err(error) = result {
                let _ = event_tx.send(BackendEvent::Failed(error.to_string()));
            }
            let _ = event_tx.send(BackendEvent::Closed);
        });

        // Only an `initialize` response counts. A process exit first (exit
        // code 0 included) is a failure; so is a transport error, which is
        // reported as an exit when the process is gone shortly after.
        enum Init {
            Ready(Result<Result<(), (String, Option<String>)>, oneshot::error::RecvError>),
            Exited(std::io::Result<std::process::ExitStatus>),
            TimedOut,
        }
        let mut ready_rx = ready_rx;
        let outcome = tokio::select! {
            biased;
            ready = &mut ready_rx => Init::Ready(ready),
            status = child.wait() => Init::Exited(status),
            _ = tokio::time::sleep(INITIALIZE_TIMEOUT) => Init::TimedOut,
        };
        let failure = match outcome {
            Init::Ready(Ok(Ok(()))) => None,
            Init::Ready(other) => {
                let (error, rpc_error_data) = match other {
                    Ok(Err(error)) => error,
                    _ => ("backend connection closed before initialize completed".to_owned(), None),
                };
                let (mut failure, status) = match tokio::time::timeout(EXIT_SETTLE, child.wait()).await {
                    Ok(status) => {
                        let status = status.ok();
                        (
                            InitializeFailure::new(
                                Code::BackendExitedBeforeInitialize,
                                format!("backend process exited ({}) before initialize completed ({error})", describe_exit(status)),
                            )
                            .stage("initialize"),
                            status,
                        )
                    }
                    Err(_) => (InitializeFailure::new(Code::InitializeFailed, error).stage("initialize"), None),
                };
                failure.rpc_error_data = rpc_error_data;
                Some((failure, status))
            }
            Init::Exited(status) => {
                let status = status.ok();
                Some((
                    InitializeFailure::new(
                        Code::BackendExitedBeforeInitialize,
                        format!("backend process exited ({}) before initialize completed", describe_exit(status)),
                    )
                    .stage("initialize"),
                    status,
                ))
            }
            Init::TimedOut => Some((
                InitializeFailure::new(Code::InitializeTimeout, format!("initialize did not complete within {} s", INITIALIZE_TIMEOUT.as_secs()))
                    .stage("initialize"),
                None,
            )),
        };
        if let Some((failure, status)) = failure {
            return Err(fail(failure, child_id, &job, status, &mut stderr_task, &stderr_tail, &stdout_tail).await);
        }
        Ok(Self {
            commands,
            events,
            pending,
            child_id,
            child: Arc::new(tokio::sync::Mutex::new(child)),
            job,
            stderr_tail,
            rpc_error_data,
            sandbox: None,
            permission_mode: crate::sandbox::PermissionMode::FullAccess,
        })
    }

    /// Spawn what [`crate::sandbox::prepare_launch`] produced. In sandboxed
    /// mode the backend runs inside srt and the per-session settings live as
    /// long as this connection; on Windows ACP runs over the bridge the
    /// launcher set up ([`crate::sandbox::PreparedLaunch::acp_bridge`]).
    /// Sandbox refusals happen before this call (fail-closed); a sandboxed
    /// launch that cannot start is an error here too, never a retry outside
    /// the sandbox.
    pub async fn spawn_prepared(prepared: crate::sandbox::PreparedLaunch, cwd: &std::path::Path) -> Result<Self, String> {
        Self::spawn_prepared_detailed(prepared, cwd).await.map_err(|failure| failure.to_string())
    }

    /// [`Self::spawn_prepared`] with a structured failure; in sandboxed mode
    /// the message is prefixed `sandboxed launch failed:`.
    pub async fn spawn_prepared_detailed(prepared: crate::sandbox::PreparedLaunch, cwd: &std::path::Path) -> Result<Self, InitializeFailure> {
        let crate::sandbox::PreparedLaunch { mode, spec, sandbox, acp_bridge, .. } = prepared;
        let mut connection = Self::spawn_detailed(&spec, cwd, acp_bridge).await.map_err(|mut failure| {
            if mode == crate::sandbox::PermissionMode::Sandboxed {
                failure.message = format!("sandboxed launch failed: {}", failure.message);
            }
            failure
        })?;
        connection.sandbox = sandbox;
        connection.permission_mode = mode;
        Ok(connection)
    }

    /// The mode this process runs in. A plain [`Self::spawn`] has no sandbox
    /// and reports `FullAccess`, which is what it is.
    pub fn permission_mode(&self) -> crate::sandbox::PermissionMode {
        self.permission_mode
    }

    pub async fn new_session(&self, cwd: PathBuf) -> Result<String, String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::NewSession { cwd, reply })
            .map_err(|_| "backend connection closed".to_string())?;
        response.await.map_err(|_| "backend connection closed".to_string())?
    }

    /// Reopen an existing native session with history replay
    /// (`session/load`). Call only when `initialize` declared `loadSession`
    /// ([`crate::port::SessionCapabilities::load`]); the replayed
    /// `session/update`s arrive on [`Self::events`] before this returns.
    pub async fn load_session(&self, session_id: &str, cwd: PathBuf) -> Result<(), String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::LoadSession { session_id: session_id.to_owned(), cwd, reply })
            .map_err(|_| "backend connection closed".to_string())?;
        response.await.map_err(|_| "backend connection closed".to_string())?
    }

    /// Switch the session's mode (`session/set_mode`); Codex permission
    /// modes. `Err` carries the JSON-RPC error summary.
    pub async fn set_mode(&self, session_id: &str, mode_id: &str) -> Result<(), String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::SetMode { session_id: session_id.to_owned(), mode_id: mode_id.to_owned(), reply })
            .map_err(|_| "backend connection closed".to_string())?;
        // A backend that never answers must not hang the turn.
        match tokio::time::timeout(SET_MODE_TIMEOUT, response).await {
            Ok(result) => result.map_err(|_| "backend connection closed".to_string())?,
            Err(_) => Err(format!("session/set_mode not answered within {} s", SET_MODE_TIMEOUT.as_secs())),
        }
    }

    /// `data` of the last failed `session/new` / `session/load` JSON-RPC
    /// reply (unredacted), taken once. The error text those calls return
    /// carries only `code` and `message` ([`rpc_error_summary`]).
    pub fn take_rpc_error_data(&self) -> Option<String> {
        lock_data(&self.rpc_error_data).take()
    }

    /// Move the event stream out of the connection, for a Host task that
    /// pumps it while other tasks keep calling `cancel` / `answer_permission`
    /// on the connection. [`Self::events`] is left with a closed receiver.
    pub fn take_events(&mut self) -> mpsc::UnboundedReceiver<BackendEvent> {
        let (_closed_tx, closed) = mpsc::unbounded_channel();
        std::mem::replace(&mut self.events, closed)
    }

    /// Start a turn. Its end is reported as `TurnEnded` or `TurnFailed`.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<(), String> {
        if self.is_stopping() {
            return Err(STOPPING_REFUSAL.to_owned());
        }
        self.commands
            .send(Command::Prompt {
                session_id: session_id.to_owned(),
                text: text.to_owned(),
            })
            .map_err(|_| "backend connection closed".to_string())
    }

    /// Send `session/cancel` and answer every pending permission request of
    /// the session with `cancelled`. Returns the native ids that were settled.
    pub fn cancel(&self, session_id: &str) -> Vec<String> {
        let _ = self.commands.send(Command::Cancel {
            session_id: session_id.to_owned(),
        });
        let mut registry = self.pending.lock().expect("permission registry poisoned");
        let ids: Vec<String> = registry
            .map
            .iter()
            .filter(|(_, (session, _))| session == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            if let Some((_, responder)) = registry.map.remove(id) {
                let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            }
        }
        ids
    }

    /// Answer a permission request with one of the offered option ids, or
    /// `None` to reject as cancelled. ACP has no acknowledgement for this
    /// response, so success is `Written`, never acknowledged; a closed
    /// transport is `Unconfirmed` (outcome unknown).
    pub fn answer_permission(&self, native_request_id: &str, option_id: Option<&str>) -> Result<WriteOutcome, String> {
        let (_, responder) = self
            .pending
            .lock()
            .expect("permission registry poisoned")
            .map
            .remove(native_request_id)
            .ok_or_else(|| "permission request already answered or no longer pending".to_string())?;
        let outcome = match option_id {
            Some(id) => RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(id.to_owned())),
            None => RequestPermissionOutcome::Cancelled,
        };
        Ok(match responder.respond(RequestPermissionResponse::new(outcome)) {
            Ok(()) => WriteOutcome::Written,
            Err(_) => WriteOutcome::Unconfirmed,
        })
    }

    /// Number of permission requests still waiting for the user.
    pub fn pending_permissions(&self) -> usize {
        self.pending.lock().map(|registry| registry.map.len()).unwrap_or(0)
    }

    /// Enter the stopping state (idempotent): new prompts and sessions are
    /// refused with [`STOPPING_REFUSAL`], every pending permission request (all
    /// sessions) is answered `cancelled` now, and any that arrives later is
    /// answered `cancelled` on arrival and reported only as
    /// [`BackendEvent::PermissionAutoCancelled`]. Returns the native ids that
    /// were pending.
    pub fn begin_stopping(&self) -> Vec<String> {
        let mut registry = self.pending.lock().expect("permission registry poisoned");
        registry.stopping = true;
        let drained: Vec<_> = registry.map.drain().collect();
        drop(registry);
        let mut ids = Vec::with_capacity(drained.len());
        for (id, (_, responder)) in drained {
            let _ = responder.respond(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled));
            ids.push(id);
        }
        ids.sort();
        ids
    }

    /// Whether [`Self::begin_stopping`] has run.
    pub fn is_stopping(&self) -> bool {
        self.pending.lock().map(|registry| registry.stopping).unwrap_or(true)
    }

    /// The stderr tail for a failed run's diagnostic, UNREDACTED (lines
    /// only, at most the ring buffer). [`crate::external::stderr_diagnostic`]
    /// redacts it once and cuts it to [`FAILURE_TAIL_BYTES`] when it is
    /// written into `run.failed`; never persist this text any other way.
    pub fn diagnostic_stderr_tail(&self) -> String {
        tail_text(&self.stderr_tail)
    }

    /// [`Self::close_with_report`], then the diagnostic stderr tail as it
    /// stands after the process ended (what it wrote while dying included).
    /// Unredacted, like [`Self::diagnostic_stderr_tail`].
    pub async fn close_with_report_and_tail(self) -> (TerminateReport, String) {
        let tail = Arc::clone(&self.stderr_tail);
        let report = self.close_with_report().await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        (report, tail_text(&tail))
    }

    /// Last lines of the backend's stderr, secret-shaped tokens masked.
    pub fn stderr_tail(&self) -> String {
        redact_text(&tail_text(&self.stderr_tail))
    }

    /// Terminate the backend process tree: SIGTERM to the group, a grace
    /// period for the backend to stop its own children, then SIGKILL.
    /// Pending permission requests are dropped with the connection; the Host
    /// marks them per `reverse` rules.
    pub async fn close(self) {
        self.close_with_report().await;
    }

    /// [`Self::close`], returning how it ended (force kill, Windows job
    /// termination, taskkill fallback, whether the tree is confirmed gone)
    /// for the Host's cleanup record. Enters the stopping state first.
    pub async fn close_with_report(self) -> TerminateReport {
        self.begin_stopping();
        self.terminate().await
    }

    /// Stop the backend and confirm its process tree is gone.
    ///
    /// Unix: snapshot the descendants, SIGTERM to the group,
    /// [`TERMINATE_GRACE`], snapshot again, SIGKILL to the group, reap the
    /// child, SIGKILL tracked descendants that left the group, then poll
    /// ([`crate::proctree::confirm_tree_gone`]). Windows has no SIGTERM: the
    /// command channel is dropped first (the ACP connection then ends and the
    /// backend's stdin closes, which lets srt run its own reset and remove its
    /// ACEs), then the grace, then `TerminateJobObject` (taskkill only as a
    /// checked fallback), then `ActiveProcesses == 0` is polled.
    async fn terminate(self) -> TerminateReport {
        let Self { commands, child, child_id, job, sandbox, permission_mode, .. } = self;
        let mut report = TerminateReport::default();
        #[cfg(windows)]
        drop(commands);
        #[cfg(not(windows))]
        let _commands = commands;
        let _ = permission_mode;
        let mut child = child.lock().await;
        if let Some(pid) = child_id {
            #[cfg(unix)]
            {
                report.process_group = Some(pid);
                report.tracked = crate::proctree::snapshot_tree(pid);
                if let Err(error) = crate::proctree::signal_group(pid, libc::SIGTERM) {
                    note(&mut report, error);
                }
            }
            report.force_killed = tokio::time::timeout(TERMINATE_GRACE, child.wait()).await.is_err();
            #[cfg(unix)]
            {
                let late = crate::proctree::snapshot_tree(pid);
                crate::proctree::merge(&mut report.tracked, late);
            }
            kill_tree(pid, &job, &mut report);
        }
        let _ = child.kill().await;
        let _ = child.wait().await;
        drop(child);
        report.confirmation = match child_id {
            None => crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["backend had no process id".into()] },
            Some(pid) => {
                #[cfg(unix)]
                {
                    let tracked = report.tracked.clone();
                    let (notes, confirmation) = tokio::task::spawn_blocking(move || {
                        let notes = crate::proctree::kill_escaped(pid, &tracked);
                        (notes, crate::proctree::confirm_tree_gone(pid, &tracked, crate::proctree::STOP_CONFIRM_TIMEOUT))
                    })
                    .await
                    .unwrap_or_else(|error| {
                        (Vec::new(), crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec![format!("confirmation task failed: {error}")] })
                    });
                    for text in notes {
                        note(&mut report, text);
                    }
                    confirmation
                }
                #[cfg(windows)]
                {
                    let _ = pid;
                    match &job {
                        Some(job) => {
                            if permission_mode == crate::sandbox::PermissionMode::Sandboxed {
                                report.confirmation_scope_note = Some(WINDOWS_SANDBOXED_SCOPE_NOTE.to_owned());
                            }
                            job.confirm_empty(crate::proctree::STOP_CONFIRM_TIMEOUT)
                        }
                        None => crate::port::StopConfirmation::FailedOrUnconfirmed {
                            reasons: vec!["no Job Object: taskkill /T /F cannot prove that every descendant ended".into()],
                        },
                    }
                }
                #[cfg(not(any(unix, windows)))]
                {
                    let _ = pid;
                    crate::port::StopConfirmation::FailedOrUnconfirmed { reasons: vec!["no tree confirmation on this platform".into()] }
                }
            }
        };
        if let crate::port::StopConfirmation::FailedOrUnconfirmed { reasons } = &report.confirmation {
            eprintln!("fox-agent-backend: process tree stop NOT confirmed: {}", reasons.join("; "));
        }
        drop(job);
        drop(sandbox);
        report
    }

    /// Whether the process Fox started (the srt broker in sandboxed mode) has
    /// exited. `None` when it cannot be checked right now (a stop holds the
    /// child). The ACP stream itself does not end on exit, so the Host polls
    /// this to notice a crash.
    pub fn has_exited(&self) -> Option<bool> {
        let mut child = self.child.try_lock().ok()?;
        match child.try_wait() {
            Ok(Some(_)) => Some(true),
            Ok(None) => Some(false),
            Err(_) => None,
        }
    }

    /// OS process id of the backend (the srt broker in sandboxed mode).
    pub fn process_id(&self) -> Option<u32> {
        self.child_id
    }

    /// Revocation stop for a live connection: full access was withdrawn while
    /// this process runs outside the sandbox
    /// ([`crate::sandbox::ModeRecheck::RestartSandboxed`]).
    ///
    /// 1. stopping state ([`Self::begin_stopping`]): new tasks refused, every
    ///    pending permission answered `cancelled`, late ones auto-cancelled;
    /// 2. `session/cancel`;
    /// 3. the run's pending requests settled (in flight -> `outcome_unknown`);
    /// 4. the process tree terminated and confirmed gone, the connection
    ///    consumed;
    /// 5. the run's terminal committed. Confirmed -> `NeedsSandboxedRebuild`
    ///    with `replay_last_operation = false`; not confirmed ->
    ///    `StoppedUnconfirmed` and [`crate::port::rebuild_after_revocation`]
    ///    refuses.
    pub async fn stop_for_revocation(
        self,
        session_id: &str,
        pending: &mut crate::settlement::PendingRegistry,
        arbiter: &mut crate::settlement::TerminalArbiter,
        reason: &str,
    ) -> crate::port::RevocationStop {
        let mut cancelled = self.begin_stopping();
        for id in self.cancel(session_id) {
            if !cancelled.contains(&id) {
                cancelled.push(id);
            }
        }
        let settled = crate::port::settle_for_revocation(pending, arbiter, reason);
        let report = self.terminate().await;
        crate::port::finish_revocation(arbiter, reason, cancelled, settled, report.process_tree_stop())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stderr_redaction_masks_secret_shaped_tokens() {
        let line = redact_line("auth failed OPENAI_API_KEY=abc sk-1234567890 token ok");
        assert!(!line.contains("abc") && !line.contains("sk-1234567890"));
        assert!(line.contains("auth failed") && line.contains("ok"));
    }

    #[test]
    fn redaction_masks_proxy_credentials_and_keeps_paths() {
        let line = redact_line(r"HTTPS_PROXY=http://srt:abc123@127.0.0.1:60080 failed at C:\Users\张三\x.js fox-acp-bridge: 0123456789abcdef0123456789abcdef");
        assert!(!line.contains("abc123"), "{line}");
        assert!(line.contains("http://<redacted>@127.0.0.1:60080") || line.contains("<redacted>"), "{line}");
        assert!(line.contains(r"C:\Users\张三\x.js"), "user-name paths kept: {line}");
        assert!(!line.contains("0123456789abcdef0123456789abcdef"));
        assert_eq!(redact_url_userinfo("see https://u:p@h.example/x?y and http://plain/z"), "see https://<redacted>@h.example/x?y and http://plain/z");
    }

    #[test]
    fn redaction_masks_headers_assignments_and_json_values() {
        let cases = [
            ("DEEPSEEK_API_KEY=sk-live-0123456789 started", "DEEPSEEK_API_KEY=<redacted> started"),
            ("MAGPIE_API_KEY=plainvalue", "MAGPIE_API_KEY=<redacted>"),
            ("Authorization: Bearer abc.def.ghi next", "Authorization: Bearer <redacted> next"),
            ("authorization=Basic dXNlcjpwYXNz", "authorization=Basic <redacted>"),
            ("x-api-key: k-123 ok", "x-api-key: <redacted> ok"),
            ("token: t0ps3cret ok", "token: <redacted> ok"),
            (r#"{"api_key":"abc123","model":"deepseek-chat"}"#, r#"{"api_key":"<redacted>","model":"deepseek-chat"}"#),
            (r#"{"authorization":"Bearer zzz"}"#, r#"{"authorization":"Bearer <redacted>"}"#),
            ("request failed key=sk-abcdef123456&x=1", "request failed key=<redacted>&x=1"),
            ("got \"sk-proj-abcdefgh\" back", "got \"<redacted>\" back"),
            ("task-runner risk-free sk-x ok", "task-runner risk-free sk-x ok"),
            (r"open C:\Users\张三\.dsh\cordis.patch.yml https://api.deepseek.com/v1", r"open C:\Users\张三\.dsh\cordis.patch.yml https://api.deepseek.com/v1"),
        ];
        for (input, expected) in cases {
            let got = redact_line(input);
            assert_eq!(got, expected, "{input}");
        }
        let tail = redact_diagnostic_tail(&format!("{}\nError: 401 DEEPSEEK_API_KEY=sk-zzzzzzzz", "x ".repeat(5000)));
        assert!(tail.len() <= FAILURE_TAIL_BYTES);
        assert!(tail.ends_with("Error: 401 DEEPSEEK_API_KEY=<redacted>"), "{tail}");
    }

    /// 5f2f2d8 re-test: the same text redacted at three stages gave
    /// `DEEPSEEK_API_KEY=[redacted]]]` and lost the closing `}` of
    /// `{"authorization":"Bearer [redacted]`. Redaction is idempotent and keeps
    /// JSON structure.
    #[test]
    fn rpc_error_text_keeps_code_and_message_and_splits_data_out() {
        let data = serde_json::json!({ "headers": { "authorization": "Bearer unit-secret-bearer" }, "apiKey": "unit-secret-key", "env": "OPENAI_API_KEY=sk-unitsecret0123456789" });
        let error = agent_client_protocol::Error::new(-32603, "Internal error").data(data);
        // Display (what Fox used before) appends the data verbatim.
        assert!(error.to_string().contains("unit-secret-bearer"));
        let summary = rpc_error_summary(&error);
        assert_eq!(summary, "Internal error (JSON-RPC -32603)");
        let text = rpc_error_data_text(&error).expect("data");
        assert!(text.contains("unit-secret-key"), "unredacted until the diagnostic");
        let diagnostic = crate::external::rpc_error_data_diagnostic(&text).expect("diagnostic");
        assert_eq!(diagnostic["source"], "rpc_error_data");
        let redacted = diagnostic["text"].as_str().unwrap();
        for secret in ["unit-secret-bearer", "unit-secret-key", "sk-unitsecret0123456789"] {
            assert!(!redacted.contains(secret), "{secret} in {redacted}");
            assert!(!diagnostic.to_string().contains(secret));
        }
        assert!(redacted.contains("Bearer <redacted>") && redacted.contains("OPENAI_API_KEY=<redacted>"), "{redacted}");
        // No data / null data: nothing; an empty message falls back to the code.
        assert_eq!(rpc_error_data_text(&agent_client_protocol::Error::new(-32000, "x")), None);
        assert_eq!(rpc_error_data_text(&agent_client_protocol::Error::new(-32000, "x").data(Value::Null)), None);
        assert_eq!(rpc_error_summary(&agent_client_protocol::Error::new(-32000, "")), "JSON-RPC error -32000");
        // Large data is limited to 4 KiB after redaction.
        let big = agent_client_protocol::Error::new(1, "big").data(serde_json::json!({ "lines": (0..400).map(|i| format!("line {i} token=unit-secret-{i}")).collect::<Vec<_>>() }));
        let diagnostic = crate::external::rpc_error_data_diagnostic(&rpc_error_data_text(&big).unwrap()).unwrap();
        let redacted = diagnostic["text"].as_str().unwrap();
        assert!(redacted.len() <= FAILURE_TAIL_BYTES, "{}", redacted.len());
        assert!(!redacted.contains("unit-secret-"), "{redacted}");
    }

    #[test]
    fn redaction_is_idempotent_and_keeps_json_structure() {
        let inputs = [
            "Error: 401 DEEPSEEK_API_KEY=sk-live-0123456789abcdef",
            "MAGPIE_API_KEY=plainvalue, next",
            r#"request headers {"authorization":"Bearer zzz.yyy.xxx"}"#,
            r#"{"headers":{"Authorization":"Bearer abc"},"api_key":"k1","model":"deepseek-chat"}"#,
            r#"{"x-api-key":"abc","token":"t"} [{"password":"p"}]"#,
            "Authorization: Bearer abc.def.ghi next",
            "x-api-key: k-123, ok",
            "token: t0ps3cret ok",
            "HTTPS_PROXY=http://srt:abc123@127.0.0.1:60080 failed",
            "got \"sk-proj-abcdefgh\" back key=sk-abcdef123456&x=1",
            "opaque 0123456789abcdef0123456789abcdef0123 done",
            "already DEEPSEEK_API_KEY=<redacted> and Bearer <redacted> and http://<redacted>@h",
            "长行 中文 密钥 DEEPSEEK_API_KEY=sk-中文-not-a-key-0123456789 结束",
        ];
        for input in inputs {
            let once = redact_line(input);
            let twice = redact_line(&once);
            let thrice = redact_line(&twice);
            assert_eq!(twice, once, "second pass changed {input:?}");
            assert_eq!(thrice, once, "third pass changed {input:?}");
            assert!(!once.contains("]]") && !once.contains("[redacted]"), "{once}");
        }
        assert_eq!(redact_line(r#"request headers {"authorization":"Bearer zzz.yyy.xxx"}"#), r#"request headers {"authorization":"Bearer <redacted>"}"#);
        assert_eq!(
            redact_line(r#"{"headers":{"Authorization":"Bearer abc"},"api_key":"k1","model":"deepseek-chat"}"#),
            r#"{"headers":{"Authorization":"Bearer <redacted>"},"api_key":"<redacted>","model":"deepseek-chat"}"#
        );
        assert_eq!(redact_line(r#"{"x-api-key":"abc","token":"t"} [{"password":"p"}]"#), r#"{"x-api-key":"<redacted>","token":"<redacted>"} [{"password":"<redacted>"}]"#);
        assert_eq!(redact_line("x-api-key: k-123, ok"), "x-api-key: <redacted>, ok");
        // Braces / brackets / quotes balance after redaction, every pass.
        let json = r#"{"error":{"message":"bad key","headers":{"authorization":"Bearer sk-live-0123456789"}},"DEEPSEEK_API_KEY":"sk-zzzzzzzzzz"}"#;
        let once = redact_line(json);
        let count = |text: &str, c: char| text.chars().filter(|x| *x == c).count();
        for c in ['{', '}', '"', '[', ']'] {
            assert_eq!(count(&once, c), count(json, c), "{c} in {once}");
        }
        assert!(serde_json::from_str::<serde_json::Value>(&once).is_ok(), "still JSON: {once}");
        let text = "a DEEPSEEK_API_KEY=sk-1234567890\nb {\"authorization\":\"Bearer q\"}";
        assert_eq!(redact_diagnostic_tail(&redact_diagnostic_tail(&redact_diagnostic_tail(text))), redact_diagnostic_tail(text));
    }

    #[test]
    fn failure_tails_are_split_limited_and_coded() {
        let mut failure = InitializeFailure::new(InitializeFailureCode::BackendExitedBeforeInitialize, "exited");
        let mut stderr = String::from("srt-win: WFP self-check ok\nfox-acp-bridge: connected\nError: cannot find module\n");
        stderr.push_str(&"x".repeat(10_000));
        failure.attach_tails(&stderr, "");
        assert!(failure.stderr_tail.len() <= FAILURE_TAIL_BYTES);
        let mut short = InitializeFailure::new(InitializeFailureCode::BackendExitedBeforeInitialize, "exited");
        short.exit_code = Some(0);
        short.attach_tails("srt-win: WFP self-check ok\nfox-acp-bridge: connected\nError: cannot find module", "");
        assert_eq!(short.srt_stderr_tail, "srt-win: WFP self-check ok\nfox-acp-bridge: connected");
        assert_eq!(short.backend_stderr_tail, "Error: cannot find module");
        let text = short.to_string();
        assert!(text.starts_with("backend_exited_before_initialize: exited [exit code 0]"), "{text}");
        assert_eq!(serde_json::to_value(&short).unwrap()["code"], "backend_exited_before_initialize");
        assert_eq!(limit_tail("a\nb\nc", 3), "c");
    }

    #[test]
    fn capabilities_default_to_unsupported_and_read_declared_fields() {
        let empty = capabilities_from_initialize(&serde_json::json!({"protocolVersion": 1}));
        assert_eq!(empty.session, SessionCapabilities::default());
        let declared = capabilities_from_initialize(&serde_json::json!({
            "protocolVersion": 1,
            "agentCapabilities": {
                "loadSession": true,
                "sessionCapabilities": {"resume": {}, "close": {}, "list": null}
            }
        }));
        assert!(declared.session.load && declared.session.resume && declared.session.close);
        assert!(!declared.session.list);
        assert!(!declared.approval_answer_acknowledged);
    }
}
