//! External backend runtime (feature `acp`): one ACP connection per
//! conversation, driven by the Host (plan 6.1, 6.3, 6.4, 6.7; external
//! backend wiring batch).
//!
//! The desktop Host only supplies two adapters:
//!
//! * [`ExternalSink`]: persist + publish a Fox runtime event of a run (the
//!   same events the yuxi runtime emits, see [`crate::projection`]), and show
//!   an approval card for a native permission request;
//! * [`ExternalLauncher`]: start the backend process for a conversation
//!   (private HOME, the Agent's own native permission mode) and hand back
//!   the initialized [`AcpConnection`].
//!
//! Everything else is here and tested on Linux against the mock ACP agent
//! (`tests/external_runtime.rs`):
//!
//! * **Start**: a turn reserves the conversation (one active turn per
//!   conversation, plan 6.2), emits `run.started`, launches the backend if
//!   it has no live connection, opens the native session (`session/load`
//!   with history replay suppressed when the backend declared `loadSession`
//!   and a native id is known; otherwise `session/new`), records the native
//!   id as an `agent_backend.session` event, and sends the prompt. A launch
//!   failure fails the run with the launcher's structured code.
//! * **Stream**: `session/update` -> [`AcpProjector`] -> sink. Events of an
//!   older connection generation are audit-only (plan 6.1 rule 3).
//! * **Approvals** (plan C2): `session/request_permission` -> approval card;
//!   the user's answer is written as the native one-shot option (`allow_once`
//!   / `reject_once`) and reported as written, never acknowledged (6.3 rule
//!   3). pi's requests (Fox's approval gate) are first put to the Host's own
//!   approval engine ([`ExternalSink::host_permission_answer`], the same
//!   engine, rules and block rules as PiNative); only what it leaves to the
//!   user becomes a card. Codex and dsh keep their own sandbox and approval:
//!   a mode that asks shows the card, a dsh preset with approval `never`
//!   rejects what still arrives (fail-closed), never shown as answerable.
//!   Requests that arrive after a stop began are auto-cancelled by the
//!   connection and only observed (`agent_backend.permission_auto_cancelled`).
//! * **End**: the stop reason decides the terminal
//!   ([`crate::projection::terminal_for_stop_reason`]); the first terminal
//!   wins ([`TerminalArbiter`]); a cancel that the backend does not confirm
//!   within [`CANCEL_CONFIRM_TIMEOUT`] ends the run `cancelled` with
//!   `outcome: "unknown"` and stops the process (6.4: cancel result may be
//!   unknown, never shown as confirmed).
//! * **Crash**: the connection closing while a turn runs fails the run
//!   (`agent_backend_exited`, with the redacted stderr tail); the next turn
//!   relaunches and reopens the native session.
//! * **Close / exit**: [`ExternalRuntime::close_conversation`] and
//!   [`ExternalRuntime::shutdown`] stop the process tree and report whether it
//!   is confirmed gone ([`crate::port::StopConfirmation`]).

use crate::acp::{AcpConnection, BackendEvent, TerminateReport};
use crate::capability_view::PiLaunch;
use crate::port::{AgentBackend, CapabilityDeclaration, StopConfirmation, STOP_UNCONFIRMED_REMEDY};
use crate::projection::{allow_option, permission_card, permission_tool_kind, reject_option, AcpProjector, AutoPermission, PermissionCard, PermissionOption, TurnTerminal};

/// What the Host's own approval engine answered for one permission request
/// ([`ExternalSink::host_permission_answer`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostPermissionAnswer {
    /// Allow / reject the native request now, or ask the user (card).
    pub decision: AutoPermission,
    /// Machine summary of the decision (mode, class, risk, findings, rule)
    /// for the run event and the audit; never tool input.
    pub policy: Value,
}
use crate::acp::WriteOutcome;
use crate::external_home::{LaunchDeclaration, PermissionMode};
use crate::settlement::{RunTerminal, TerminalArbiter};
use futures::future::BoxFuture;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedReceiver;

/// How long a cancelled turn may take to end with `stopReason: cancelled`
/// before the Host records the cancel outcome as unknown and stops the
/// process.
pub const CANCEL_CONFIRM_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the pump checks whether the backend process exited.
pub const EXIT_POLL: Duration = Duration::from_millis(250);

/// How long a failed `session/prompt` waits for the connection to close
/// (process exit) before it is settled as a protocol failure.
pub const PROMPT_FAILURE_SETTLE: Duration = Duration::from_millis(800);
/// After a `session/load` reply, how long the Host waits for the
/// [`BackendEvent::LoadSettled`] fence (it is queued right behind the reply,
/// so this only bounds a connection that died meanwhile).
pub const LOAD_FENCE_TIMEOUT: Duration = Duration::from_secs(5);
/// Bound the initialization control request; prompts retain their normal
/// long-running lifecycle and cancellation behavior.
pub const MODEL_SELECTION_TIMEOUT: Duration = Duration::from_secs(10);

/// What the launcher needs to start one conversation's backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    pub conversation_id: String,
    pub backend: AgentBackend,
    /// Host database id of the conversation's project (private HOME key).
    pub project_uuid: Option<String>,
    /// Authorized root, also the ACP `cwd`.
    pub project_root: PathBuf,
    /// Per-launch session id (`[A-Za-z0-9_-]{1,128}`), names the session temp.
    pub session_key: String,
    /// Full access granted to this conversation by the user when it was
    /// created (persisted per conversation, UX batch ②). OR-ed with the
    /// engineering switch `FOX_EXTERNAL_FULL_ACCESS`; both lose to
    /// `FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1`.
    pub full_access_granted: bool,
    /// Fox「模型服务」's default provider (settings batch, item 6): before
    /// the launch, the backend's own config is generated from it in the
    /// private HOME unless the user's own login is there
    /// ([`crate::unified_login`]). `None`: 模型服务 has no usable default
    /// provider; Fox-managed files are removed and the login card is the
    /// fallback. `Debug` never shows the key.
    pub unified_login: Option<crate::unified_login::UnifiedLogin>,
    /// The conversation's choice in its Agent's own permission list
    /// ([`crate::native_permission`]), as stored. `None` / unknown / greyed:
    /// the Agent's default.
    pub native_permission: Option<String>,
}

impl LaunchRequest {
    /// What the native permission list depends on for this launch.
    pub fn native_context(&self) -> crate::native_permission::CatalogContext {
        crate::native_permission::CatalogContext {
            provider_base_url: self.unified_login.as_ref().map(|login| login.base_url.clone()),
            full_access_forbidden: false,
        }
    }

    /// The native mode a session of this request runs in: the Agent's own
    /// stored permission choice. Fox's [`PermissionMode`] never rewrites it;
    /// a missing grant can never turn Codex or dsh into full access, nor the
    /// other way round.
    pub fn native_mode(&self) -> &'static str {
        crate::native_permission::effective_mode(self.backend, self.native_permission.as_deref(), false, &self.native_context())
    }
}

/// An initialized backend handed back by the launcher.
pub struct Launched {
    pub connection: AcpConnection,
    pub mode: PermissionMode,
    pub declaration: LaunchDeclaration,
    pub pi_launch: PiLaunch,
    /// Runs after the connection was closed.
    pub on_closed: Option<Box<dyn FnOnce() + Send>>,
}

/// Structured launch refusal / failure (codes of
/// [`crate::external_home::LaunchErrorCode`], the initialize codes of
/// [`crate::acp::InitializeFailureCode`], or `agent_backend_*`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchFailure {
    pub code: String,
    pub message: String,
    pub remedy: Option<String>,
    pub detail: Value,
    /// Redacted stderr tail for the run's diagnostic ([`stderr_diagnostic`]);
    /// written once, at the top level of `run.failed` as `diagnostic` (not
    /// repeated inside the serialized `launch` object).
    #[serde(skip)]
    pub diagnostic: Option<Value>,
    /// The JSON-RPC error's `data`, redacted ([`rpc_error_data_diagnostic`]);
    /// written next to the stderr tail (`diagnostics`), never into `message`.
    #[serde(skip)]
    pub rpc_error_data: Option<Value>,
}

impl LaunchFailure {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self { code: code.into(), message: message.into(), remedy: None, detail: Value::Null, diagnostic: None, rpc_error_data: None }
    }

    /// Attach the redacted stderr tail as the run diagnostic (nothing when
    /// the tail is empty).
    pub fn with_stderr_tail(mut self, tail: &str, source: &str) -> Self {
        self.diagnostic = stderr_diagnostic(tail, source);
        self
    }

    /// Attach the JSON-RPC error `data` (unredacted text; redacted here).
    pub fn with_rpc_error_data(mut self, data: Option<&str>) -> Self {
        self.rpc_error_data = data.and_then(rpc_error_data_diagnostic);
        self
    }

    pub fn remedy(mut self, remedy: impl Into<String>) -> Self {
        self.remedy = Some(remedy.into());
        self
    }

    pub fn detail(mut self, detail: Value) -> Self {
        self.detail = detail;
        self
    }

    fn user_message(&self) -> String {
        match &self.remedy {
            Some(remedy) => format!("{}（{}）", self.message, remedy),
            None => self.message.clone(),
        }
    }
}

impl From<crate::external_home::LaunchError> for LaunchFailure {
    fn from(error: crate::external_home::LaunchError) -> Self {
        let mut failure = Self::new(error.code.as_str(), error.message.clone());
        failure.remedy = error.remedy.clone();
        failure.detail = serde_json::to_value(&error).unwrap_or(Value::Null);
        failure
    }
}

impl From<crate::acp::InitializeFailure> for LaunchFailure {
    fn from(failure: crate::acp::InitializeFailure) -> Self {
        let mut message = failure.message.clone();
        if !failure.backend_stderr_tail.is_empty() {
            message.push_str(&format!("；后端输出（已脱敏）：{}", tail_chars(&failure.backend_stderr_tail, 600)));
        }
        // Already redacted once (InitializeFailure::attach_tails); not again.
        let diagnostic = diagnostic_value(&crate::acp::limit_failure_tail(&failure.stderr_tail), "initialize");
        let mut launch = Self::new(failure.code.as_str(), message)
            .detail(serde_json::to_value(&failure).unwrap_or(Value::Null))
            .with_rpc_error_data(failure.rpc_error_data.as_deref());
        launch.diagnostic = diagnostic;
        launch
    }
}

/// The `diagnostic` field of a failed run's terminal event: the backend's
/// stderr tail, redacted (once) before it is persisted
/// ([`crate::acp::redact_diagnostic_tail`]: keys, tokens, `Authorization` /
/// `Bearer` values, `*_API_KEY=` values, proxy credentials) and limited to the
/// last [`crate::acp::FAILURE_TAIL_BYTES`] (4 KiB). `None` when there is no
/// output. `source` says which failure it belongs to: `prompt_failed`,
/// `backend_exited`, `session_failed`, `initialize`.
///
/// `tail` is the unredacted text ([`crate::acp::AcpConnection::diagnostic_stderr_tail`]);
/// this is the one place it is redacted.
pub fn stderr_diagnostic(tail: &str, source: &str) -> Option<Value> {
    diagnostic_value(&crate::acp::redact_diagnostic_tail(tail), source)
}

/// The diagnostic object for an already redacted, already limited tail.
fn diagnostic_value(tail: &str, source: &str) -> Option<Value> {
    if tail.trim().is_empty() {
        return None;
    }
    Some(json!({
        "kind": "stderr_tail",
        "source": source,
        "redacted": true,
        "maxBytes": crate::acp::FAILURE_TAIL_BYTES,
        "stderrTail": tail,
    }))
}

/// Source of the JSON-RPC error `data` diagnostic.
pub const RPC_ERROR_DATA_SOURCE: &str = "rpc_error_data";

/// The diagnostic for a JSON-RPC error's `data` (serialized by
/// [`crate::acp::rpc_error_data_text`]): redacted once with
/// [`crate::acp::redact_diagnostic_tail`] and limited to
/// [`crate::acp::FAILURE_TAIL_BYTES`] (4 KiB). `None` when empty. The error's
/// `message` keeps only `code` and `message` ([`crate::acp::rpc_error_summary`]).
pub fn rpc_error_data_diagnostic(data: &str) -> Option<Value> {
    let text = crate::acp::redact_diagnostic_tail(data);
    if text.trim().is_empty() {
        return None;
    }
    Some(json!({
        "kind": RPC_ERROR_DATA_SOURCE,
        "source": RPC_ERROR_DATA_SOURCE,
        "redacted": true,
        "maxBytes": crate::acp::FAILURE_TAIL_BYTES,
        "text": text,
    }))
}

/// `extra` for a terminal event with its diagnostics. Shape (both optional,
/// present only when there is something):
/// - `diagnostic`: ONE object, kept for older UIs: the stderr tail when there
///   is one (`kind: "stderr_tail"`, `stderrTail`), else the JSON-RPC `data`
///   (`kind: "rpc_error_data"`, `text`);
/// - `diagnostics`: every diagnostic, stderr tail first, then the `data`.
fn with_diagnostics(mut extra: Value, stderr: Option<Value>, rpc_error_data: Option<Value>) -> Value {
    let all: Vec<Value> = stderr.into_iter().chain(rpc_error_data).collect();
    if let (Some(first), Value::Object(map)) = (all.first(), &mut extra) {
        map.insert("diagnostic".into(), first.clone());
        map.insert("diagnostics".into(), Value::Array(all.clone()));
    }
    extra
}

fn tail_chars(text: &str, max: usize) -> String {
    let count = text.chars().count();
    text.chars().skip(count.saturating_sub(max)).collect()
}

fn model_error_terminal(data: &str) -> Option<TurnTerminal> {
    let data: Value = serde_json::from_str(data).ok()?;
    if data.get("code")?.as_str()? != "agent_backend_model_failed" || data.get("redacted")?.as_bool()? != true { return None; }
    let (kind, message) = match data.get("failureKind")?.as_str()? {
        "endpoint_changed" => ("endpoint_changed", "pi 当前会话的模型端点已变化；请重新应用已授权的模型配置后重试"),
        "network_allowlist" => ("network_allowlist", "模型请求被沙箱网络允许列表阻止"),
        "network_unreachable" => ("network_unreachable", "沙箱内无法通过受管代理连接模型服务"),
        "authentication" => ("authentication", "模型服务拒绝了登录凭据"),
        "rate_limit" => ("rate_limit", "模型服务达到请求限额"),
        "model_error" => ("request_failed", "模型服务未能完成本轮请求"),
        _ => return None,
    };
    Some(TurnTerminal::Failed { code: format!("agent_backend_model_{kind}"), message: message.into() })
}

/// Starts a backend process for a conversation.
pub trait ExternalLauncher: Send + Sync + 'static {
    /// The mode a fresh launch would use now (Host policy may have tightened
    /// since the live process started).
    fn desired_mode(&self, request: &LaunchRequest) -> PermissionMode;
    /// Local configuration only; never performs a model request. A changed
    /// key requires a fresh process/policy before the next turn.
    fn connection_key(&self, _request: &LaunchRequest) -> Result<Option<String>, LaunchFailure> { Ok(None) }
    fn model_host(&self, request: &LaunchRequest) -> Option<String> {
        request.unified_login.as_ref().and_then(|login| crate::native_permission::host_of(&login.base_url))
    }
    fn selected_model(&self, _request: &LaunchRequest) -> Option<String> { None }
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'static, Result<Launched, LaunchFailure>>;
}

/// Where the runtime's output goes.
pub trait ExternalSink: Send + Sync + 'static {
    /// Persist and publish one runtime event of `run_id`.
    fn emit(&self, run_id: &str, conversation_id: &str, event: Value) -> Result<(), String>;
    /// Show an approval card; returns the Host approval id. `key` is unique
    /// per native request and connection generation.
    fn request_approval(&self, run_id: &str, conversation_id: &str, key: &str, card: &PermissionCard) -> Result<String, String>;
    /// Record one Host decision in the external-backend audit log (names
    /// and kinds only, never tool input). Default: nothing beyond the run
    /// event the runtime already emitted.
    fn audit(&self, _conversation_id: &str, _backend: AgentBackend, _event: &str, _details: &Value) {}
    /// pi (plan C2): answer one request of Fox's approval gate with the
    /// Host's approval engine, read against the conversation's tier
    /// (`native_mode`), project rules and block rules as they are NOW. The
    /// desktop always answers (an engine failure is a fail-closed reject).
    /// `None` = this sink has no engine (tests); the runtime then uses the
    /// built-in tier table ([`crate::native_permission::answer`]). Never
    /// called for Codex or dsh: their own approval decides.
    fn host_permission_answer(
        &self,
        _run_id: &str,
        _conversation_id: &str,
        _backend: AgentBackend,
        _native_mode: &str,
        _request: &Value,
        _options: &[PermissionOption],
    ) -> Option<HostPermissionAnswer> {
        None
    }
}

/// Why a turn could not start (nothing was emitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    /// One active turn per conversation (plan 6.2).
    TurnBusy,
    /// The conversation is being closed.
    Closing,
    /// `run.started` could not be recorded.
    Emit(String),
}

impl StartError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::TurnBusy => "agent_backend_turn_busy",
            Self::Closing => "agent_backend_session_closing",
            Self::Emit(_) => "agent_backend_emit_failed",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::TurnBusy => "此会话上一轮仍在运行，请等它结束或先取消".to_owned(),
            Self::Closing => "此会话的外部后端正在关闭".to_owned(),
            Self::Emit(error) => format!("无法记录运行开始：{error}"),
        }
    }
}

/// Result of [`ExternalRuntime::revoke_full_access`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevokeOutcome {
    /// No live process for the conversation: the next launch reads the
    /// (already withdrawn) grant and starts without full access.
    NotRunning,
    /// The live process already runs without full access.
    Unchanged,
    /// The full-access process was stopped; the next turn starts without
    /// full access. `confirmed: false`: the stop was not confirmed and no
    /// relaunch will happen. Only for an Agent whose full access lives in its
    /// process (dsh, [`crate::native_permission::full_access_needs_restart`]).
    RestartWithoutFullAccess { confirmed: bool },
    /// The live session left full access without a restart (Codex
    /// `session/set_mode`; pi: the Host answers by the new tier from the
    /// next request).
    SwitchedLive,
    /// The live switch was refused; the next turn retries it.
    SwitchFailed(String),
}

/// Result of [`ExternalRuntime::apply_native_mode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeModeOutcome {
    /// No live process: the next launch starts in the new mode.
    NotRunning,
    /// The live session runs in the new mode now.
    Applied,
    /// Takes effect when the backend restarts at the next turn.
    NextTurn,
    /// The live switch was refused (the next turn retries it).
    Failed(String),
}

impl NativeModeOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NotRunning => "not_running",
            Self::Applied => "applied",
            Self::NextTurn => "next_turn",
            Self::Failed(_) => "failed",
        }
    }
}

/// Result of [`ExternalRuntime::cancel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CancelOutcome {
    /// `session/cancel` sent (or the turn had not reached the backend yet);
    /// the terminal follows as an event. `answered_permissions` are native
    /// requests answered `cancelled` now.
    Requested { answered_permissions: Vec<String> },
    /// No active turn with this run id.
    NotActive,
}

/// Result of [`ExternalRuntime::answer`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerOutcome {
    /// Written to the backend (ACP has no acknowledgement).
    Written,
    /// The write could not be confirmed (transport closed): outcome unknown.
    Unconfirmed,
    /// Not (or no longer) waiting: cancelled, answered, or the turn ended.
    NotPending,
}

/// What the UI may know about a conversation's backend session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub backend: AgentBackend,
    pub live: bool,
    pub mode: Option<PermissionMode>,
    pub declaration: Option<LaunchDeclaration>,
    pub initialize_response: Option<Value>,
    pub capabilities: Option<CapabilityDeclaration>,
    pub pi_launch: PiLaunch,
    pub native_session_id: Option<String>,
    pub active_run: Option<String>,
}

struct PendingApproval {
    native_id: String,
    options: Vec<PermissionOption>,
}

struct Turn {
    last_activity: std::time::Instant,
    active_tools: std::collections::HashSet<String>,
    run_id: String,
    /// The Agent's own permission mode this turn runs in
    /// ([`crate::native_permission`]); decides the Host's fail-closed answer.
    native_mode: String,
    projector: AcpProjector,
    arbiter: TerminalArbiter,
    approvals: HashMap<String, PendingApproval>,
    cancel_requested: bool,
    prompt_sent: bool,
    /// The [`BackendEvent::PromptStarted`] fence of this turn's prompt has
    /// come through the event stream: only `session/update`s after it belong
    /// to this Run (anything before it is a history replay or other output
    /// from before the prompt, never projected into the new Run).
    prompt_fence_seen: bool,
    /// `session/prompt` failed; settled as a prompt failure unless the
    /// connection closes within [`PROMPT_FAILURE_SETTLE`] (then an exit).
    prompt_error: Option<String>,
    /// JSON-RPC `data` of the failed prompt (unredacted until the diagnostic).
    prompt_error_data: Option<String>,
}

impl Turn {
    fn new(run_id: &str, native_mode: &str) -> Self {
        Self {
            last_activity: std::time::Instant::now(),
            active_tools: std::collections::HashSet::new(),
            run_id: run_id.to_owned(),
            native_mode: native_mode.to_owned(),
            projector: AcpProjector::new(),
            arbiter: TerminalArbiter::default(),
            approvals: HashMap::new(),
            cancel_requested: false,
            prompt_sent: false,
            prompt_fence_seen: false,
            prompt_error: None,
            prompt_error_data: None,
        }
    }
}

#[derive(Default)]
struct SessionState {
    generation: u64,
    connection: Option<Arc<AcpConnection>>,
    native_session_id: Option<String>,
    native_session_open: bool,
    initialize_response: Option<Value>,
    capabilities: Option<CapabilityDeclaration>,
    mode: Option<PermissionMode>,
    /// The native permission mode the live process was started in or last
    /// switched to.
    native_mode: Option<String>,
    connection_key: Option<String>,
    /// Actual selected model destination (diagnostics only).
    model_host: Option<String>,
    declaration: Option<LaunchDeclaration>,
    pi_launch: Option<PiLaunch>,
    turn: Option<Turn>,
    closing: bool,
    on_closed: Option<Box<dyn FnOnce() + Send>>,
    /// A full-access process stopped for revocation was not confirmed gone:
    /// no relaunch until Fox restarts.
    stop_unconfirmed: bool,
}

struct Session {
    conversation_id: String,
    backend: AgentBackend,
    state: Mutex<SessionState>,
}

struct Inner<S: ExternalSink, L: ExternalLauncher> {
    sink: S,
    launcher: L,
    handle: tokio::runtime::Handle,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    cancel_timeout: Duration,
    model_selection_timeout: Duration,
    idle_timeout: Duration,
}

/// The Host's external backend runtime. Cheap to clone.
pub struct ExternalRuntime<S: ExternalSink, L: ExternalLauncher> {
    inner: Arc<Inner<S, L>>,
}

impl<S: ExternalSink, L: ExternalLauncher> Clone for ExternalRuntime<S, L> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

/// Everything a turn needs.
#[derive(Debug, Clone)]
pub struct TurnRequest {
    pub run_id: String,
    pub prompt: String,
    /// Native session id recorded for this conversation (from an earlier
    /// `agent_backend.session` event), used to reopen it after a restart.
    pub stored_native_session: Option<String>,
    pub launch: LaunchRequest,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<S: ExternalSink, L: ExternalLauncher> ExternalRuntime<S, L> {
    pub fn new(sink: S, launcher: L, handle: tokio::runtime::Handle) -> Self {
        Self::with_cancel_timeout(sink, launcher, handle, CANCEL_CONFIRM_TIMEOUT)
    }

    pub fn with_cancel_timeout(sink: S, launcher: L, handle: tokio::runtime::Handle, cancel_timeout: Duration) -> Self {
        Self::with_timeouts(sink, launcher, handle, cancel_timeout, MODEL_SELECTION_TIMEOUT)
    }

    pub fn with_timeouts(sink: S, launcher: L, handle: tokio::runtime::Handle, cancel_timeout: Duration, model_selection_timeout: Duration) -> Self {
        Self::with_idle_timeout(sink, launcher, handle, cancel_timeout, model_selection_timeout, Duration::from_secs(120))
    }

    pub fn with_idle_timeout(sink: S, launcher: L, handle: tokio::runtime::Handle, cancel_timeout: Duration, model_selection_timeout: Duration, idle_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner { sink, launcher, handle, sessions: Mutex::new(HashMap::new()), cancel_timeout, model_selection_timeout, idle_timeout }),
        }
    }

    pub fn sink(&self) -> &S {
        &self.inner.sink
    }

    pub fn launcher(&self) -> &L {
        &self.inner.launcher
    }

    fn session(&self, conversation_id: &str, backend: AgentBackend) -> Arc<Session> {
        let mut sessions = lock(&self.inner.sessions);
        Arc::clone(sessions.entry(conversation_id.to_owned()).or_insert_with(|| {
            Arc::new(Session { conversation_id: conversation_id.to_owned(), backend, state: Mutex::new(SessionState::default()) })
        }))
    }

    fn find_by_run(&self, run_id: &str) -> Option<Arc<Session>> {
        lock(&self.inner.sessions)
            .values()
            .find(|session| lock(&session.state).turn.as_ref().is_some_and(|turn| turn.run_id == run_id))
            .cloned()
    }

    /// Reserve the conversation, emit `run.started` and drive the turn in the
    /// background. Returns once the turn is reserved; its end arrives as a
    /// terminal event through the sink.
    pub fn start_turn(&self, request: TurnRequest) -> Result<(), StartError> {
        let backend = request.launch.backend;
        let session = self.session(&request.launch.conversation_id, backend);
        let native_mode = request.launch.native_mode();
        {
            let mut state = lock(&session.state);
            if state.closing {
                return Err(StartError::Closing);
            }
            if state.turn.is_some() {
                return Err(StartError::TurnBusy);
            }
            state.turn = Some(Turn::new(&request.run_id, native_mode));
        }
        let started = json!({ "type": "run.started", "agentBackend": backend.as_str() });
        if let Err(error) = self.inner.sink.emit(&request.run_id, &session.conversation_id, started) {
            lock(&session.state).turn = None;
            return Err(StartError::Emit(error));
        }
        let runtime = self.clone();
        self.inner.handle.spawn(async move {
            runtime.drive(session, request).await;
        });
        Ok(())
    }

    fn emit(&self, session: &Session, run_id: &str, event: Value) {
        if let Err(error) = self.inner.sink.emit(run_id, &session.conversation_id, event) {
            eprintln!(
                "fox.external conversation={} run={run_id} agent_backend={} emit_failed={error:?}",
                session.conversation_id,
                session.backend.as_str()
            );
        }
    }

    /// End the active turn of `run_id` once: close the assistant message and
    /// emit the terminal. No effect when that turn already ended.
    fn finish_turn(&self, session: &Session, state: &mut SessionState, run_id: &str, terminal: TurnTerminal, extra: Value) {
        let Some(turn) = state.turn.as_mut().filter(|turn| turn.run_id == run_id) else {
            return;
        };
        let terminal = if terminal == TurnTerminal::Completed && !turn.projector.has_delivery() {
            if turn.cancel_requested { TurnTerminal::Cancelled } else {
                TurnTerminal::Failed { code: "agent_backend_empty_output".into(), message: "外部 Agent 已结束本轮，但没有返回有效回答或完成工具交付；请检查模型连接后重试".into() }
            }
        } else { terminal };
        let terminal_kind = match &terminal {
            TurnTerminal::Completed => RunTerminal::Completed,
            TurnTerminal::Cancelled => RunTerminal::Cancelled,
            TurnTerminal::Interrupted { .. } | TurnTerminal::Failed { .. } => RunTerminal::Failed,
        };
        let unknown = extra.get("outcome").and_then(Value::as_str) == Some("unknown");
        if turn.arbiter.committed().is_some() {
            return;
        }
        turn.arbiter.commit(terminal_kind, unknown);
        let mut events = turn.projector.finish();
        let mut extra = match extra {
            Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        extra.insert("agentBackend".into(), json!(session.backend.as_str()));
        if !turn.projector.notices().is_empty() {
            // Backend warnings taken out of the answer (diagnostics area).
            extra.insert("backendNotices".into(), Value::Array(turn.projector.notices().to_vec()));
        }
        if turn.cancel_requested && terminal == TurnTerminal::Completed {
            // Plan 6.4 rule 3: the turn finished first; shown as completed
            // with the cancel request kept on record.
            extra.insert("cancelRequested".into(), json!(true));
        }
        events.push(terminal.event(Value::Object(extra)));
        state.turn = None;
        for event in events {
            self.emit(session, run_id, event);
        }
    }

    async fn drive(&self, session: Arc<Session>, request: TurnRequest) {
        let run_id = request.run_id.clone();
        if let Err(failure) = self.ensure_ready(&session, &request).await {
            let mut state = lock(&session.state);
            let cancelled = state.turn.as_ref().is_some_and(|turn| turn.run_id == run_id && turn.cancel_requested);
            let terminal = if cancelled {
                TurnTerminal::Cancelled
            } else {
                TurnTerminal::Failed { code: failure.code.clone(), message: failure.user_message() }
            };
            let (diagnostic, rpc_error_data) = if cancelled { (None, None) } else { (failure.diagnostic.clone(), failure.rpc_error_data.clone()) };
            self.finish_turn(&session, &mut state, &run_id, terminal, with_diagnostics(json!({ "launch": failure }), diagnostic, rpc_error_data));
            return;
        }
        let (connection, native) = {
            let mut state = lock(&session.state);
            let Some(turn) = state.turn.as_mut().filter(|turn| turn.run_id == run_id) else {
                return;
            };
            if turn.cancel_requested {
                // Cancelled before the prompt reached the backend: nothing ran.
                self.finish_turn(&session, &mut state, &run_id, TurnTerminal::Cancelled, json!({ "outcome": "not_started" }));
                return;
            }
            turn.prompt_sent = true;
            turn.last_activity = std::time::Instant::now();
            (state.connection.clone(), state.native_session_id.clone())
        };
        let (Some(connection), Some(native)) = (connection, native) else {
            let mut state = lock(&session.state);
            self.finish_turn(
                &session,
                &mut state,
                &run_id,
                TurnTerminal::Failed { code: "agent_backend_exited".into(), message: "外部后端在开始本轮之前退出".into() },
                Value::Null,
            );
            return;
        };
        if let Err(error) = connection.prompt(&native, &request.prompt) {
            let diagnostic = stderr_diagnostic(&connection.diagnostic_stderr_tail(), "prompt_failed");
            let mut state = lock(&session.state);
            self.finish_turn(
                &session,
                &mut state,
                &run_id,
                TurnTerminal::Failed { code: "agent_backend_prompt_failed".into(), message: error },
                with_diagnostics(json!({}), diagnostic, None),
            );
        }
    }

    /// Live connection with an open native session, launching and reopening
    /// as needed.
    async fn ensure_ready(&self, session: &Arc<Session>, request: &TurnRequest) -> Result<(), LaunchFailure> {
        let desired = self.inner.launcher.desired_mode(&request.launch);
        if lock(&session.state).stop_unconfirmed {
            return Err(LaunchFailure::new("agent_backend_stop_unconfirmed", "撤销完全访问时，原进程未确认全部结束，不会重建")
                .remedy(STOP_UNCONFIRMED_REMEDY));
        }
        let wanted_native = request.launch.native_mode();
        let wanted_key = self.inner.launcher.connection_key(&request.launch)?;
        let source_changed = {
            let state = lock(&session.state);
            state.connection.is_some() && state.connection_key != wanted_key
        };
        if source_changed {
            if let Some(report) = self.stop_connection(session, "model configuration changed").await {
                if !report.confirmation.is_confirmed() {
                    return Err(LaunchFailure::new("agent_backend_stop_unconfirmed", "模型配置已变化，但原进程未确认结束，暂不重建连接"));
                }
            }
        }
        let (live, running_mode, running_native) = {
            let state = lock(&session.state);
            (state.connection.is_some() && state.native_session_open, state.mode, state.native_mode.clone())
        };
        if live {
            // Tightening applies before the next continue (plan 6.5): a
            // process whose full access lives in it (dsh) is stopped when the
            // conversation left full access; never the other way round
            // silently. Codex leaves it live (`session/set_mode`) and pi by
            // the Host's next answer, below.
            if running_mode == Some(PermissionMode::FullAccess)
                && desired == PermissionMode::Standard
                && crate::native_permission::full_access_needs_restart(session.backend)
            {
                let report = self.stop_connection(session, "full access withdrawn").await;
                if let Some(report) = report {
                    if !report.confirmation.is_confirmed() {
                        return Err(LaunchFailure::new("agent_backend_stop_unconfirmed", "原“完全访问”进程未确认全部结束，不会重建")
                            .remedy(STOP_UNCONFIRMED_REMEDY)
                            .detail(json!({ "reasons": report.confirmation.reasons() })));
                    }
                }
            } else if running_native.as_deref() == Some(wanted_native) {
                return Ok(());
            } else {
                // The user picked another mode in the Agent's own list.
                match session.backend {
                    // Codex: switched in the live session (`session/set_mode`).
                    AgentBackend::Codex => {
                        self.switch_codex_mode(session, &request.run_id, wanted_native).await?;
                        return Ok(());
                    }
                    // dsh reads its preset only at start (no set_mode):
                    // restart it for this turn.
                    AgentBackend::Deepseek => {
                        let report = self.stop_connection(session, "dsh permission preset changed").await;
                        if let Some(report) = report {
                            if !report.confirmation.is_confirmed() {
                                return Err(LaunchFailure::new("agent_backend_stop_unconfirmed", "切换 DeepSeek 权限时原进程未确认全部结束，不会以新权限重建")
                                    .remedy(STOP_UNCONFIRMED_REMEDY)
                                    .detail(json!({ "reasons": report.confirmation.reasons() })));
                            }
                        }
                        self.emit(
                            session,
                            &request.run_id,
                            json!({ "type": "agent_backend.permission_mode_restart", "agentBackend": session.backend.as_str(), "from": running_native, "to": wanted_native }),
                        );
                    }
                    // pi / Fox: the Host answers; nothing to restart.
                    _ => {
                        let mut state = lock(&session.state);
                        state.native_mode = Some(wanted_native.to_owned());
                        state.mode = Some(desired);
                        return Ok(());
                    }
                }
            }
        }
        let run_id = request.run_id.clone();
        self.emit(session, &run_id, json!({ "type": "agent_backend.launching", "agentBackend": session.backend.as_str(), "mode": desired.as_str() }));
        let launched = self.inner.launcher.launch(request.launch.clone()).await?;
        let Launched { mut connection, mode, declaration, pi_launch, on_closed } = launched;
        let connection_key = match self.inner.launcher.connection_key(&request.launch) {
            Ok(key) => key,
            Err(error) => {
                let _ = connection.close_with_report().await;
                if let Some(callback) = on_closed { callback(); }
                return Err(error);
            }
        };
        let mut events = connection.take_events();
        let initialize_response = match tokio::time::timeout(Duration::from_secs(5), events.recv()).await {
            Ok(Some(BackendEvent::Initialized { initialize_response, .. })) => initialize_response,
            other => {
                let (_, tail) = connection.close_with_report_and_tail().await;
                if let Some(callback) = on_closed {
                    callback();
                }
                return Err(LaunchFailure::new("initialize_failed", format!("backend did not report initialize first: {other:?}"))
                    .with_stderr_tail(&tail, "session_failed"));
            }
        };
        let capabilities = crate::capability_view::backend_declaration(session.backend, &initialize_response, pi_launch);
        let can_load = capabilities.session.load;
        // Reopen: the id this connection already had (crash / restart within
        // this Fox), else the recorded one.
        let previous = lock(&session.state).native_session_id.clone().or_else(|| request.stored_native_session.clone());
        let cwd = request.launch.project_root.clone();
        let (native, reopened, load_error) = match previous {
            Some(id) if can_load => {
                // Everything from sending session/load until its reply (the
                // replayed history, as session/update) is dropped here and
                // never projected. The reply is followed in the event stream
                // by the LoadSettled fence, so draining up to the fence drops
                // exactly the updates that came before the reply, however the
                // two channels interleave. A permission request in that
                // window is refused (no turn can own it).
                let load_id = id.clone();
                let load = connection.load_session(&load_id, cwd.clone());
                tokio::pin!(load);
                let mut dropped = 0usize;
                let mut fenced = false;
                let drop_event = |event: BackendEvent, fenced: &mut bool, dropped: &mut usize| match event {
                    BackendEvent::PermissionRequested { native_request_id, .. } => {
                        let _ = connection.answer_permission(&native_request_id, None);
                    }
                    BackendEvent::LoadSettled { .. } => *fenced = true,
                    BackendEvent::Update { .. } => *dropped += 1,
                    _ => {}
                };
                let loaded = loop {
                    tokio::select! {
                        result = &mut load => break result,
                        event = events.recv() => match event {
                            Some(event) => drop_event(event, &mut fenced, &mut dropped),
                            None => {}
                        }
                    }
                };
                let deadline = tokio::time::Instant::now() + LOAD_FENCE_TIMEOUT;
                while !fenced {
                    match tokio::time::timeout_at(deadline, events.recv()).await {
                        Ok(Some(event)) => drop_event(event, &mut fenced, &mut dropped),
                        Ok(None) | Err(_) => break,
                    }
                }
                if dropped > 0 {
                    eprintln!(
                        "fox.external conversation={} agent_backend={} load_replay_dropped={dropped} fenced={fenced}",
                        session.conversation_id,
                        session.backend.as_str()
                    );
                }
                match loaded {
                    Ok(()) => (Ok(id), "load", None),
                    Err(error) => (connection.new_session(cwd.clone()).await, "new_after_load_failed", Some(error)),
                }
            }
            Some(_) => (connection.new_session(cwd.clone()).await, "new_without_history", None),
            None => (connection.new_session(cwd.clone()).await, "new", None),
        };
        let native = match native {
            Ok(native) => native,
            Err(error) => {
                let data = connection.take_rpc_error_data();
                let (_report, tail) = connection.close_with_report_and_tail().await;
                if let Some(callback) = on_closed {
                    callback();
                }
                return Err(LaunchFailure::new("agent_backend_session_failed", format!("外部后端无法打开会话：{error}"))
                    .with_stderr_tail(&tail, "session_failed")
                    .with_rpc_error_data(data.as_deref()));
            }
        };
        if let Some(model) = self.inner.launcher.selected_model(&request.launch) {
            let selected = tokio::time::timeout(self.inner.model_selection_timeout, connection.set_model(&native, &model))
                .await.unwrap_or_else(|_| Err("应用模型选择超时".to_owned()));
            if let Err(error) = selected {
                let (_, tail) = connection.close_with_report_and_tail().await;
                if let Some(callback) = on_closed { callback(); }
                return Err(LaunchFailure::new("agent_backend_model_selection_failed", format!("无法在载入会话后应用当前模型选择：{}", crate::acp::redact_diagnostic_tail(&error)))
                    .with_stderr_tail(&tail, "session_failed"));
            }
        }
        let connection = Arc::new(connection);
        let generation = {
            let mut state = lock(&session.state);
            state.generation += 1;
            state.connection = Some(Arc::clone(&connection));
            state.native_session_id = Some(native.clone());
            state.native_session_open = true;
            state.initialize_response = Some(initialize_response.clone());
            state.capabilities = Some(capabilities);
            state.mode = Some(mode);
            state.native_mode = Some(request.launch.native_mode().to_owned());
            state.connection_key = connection_key;
            state.model_host = self.inner.launcher.model_host(&request.launch);
            state.declaration = Some(declaration.clone());
            state.pi_launch = Some(pi_launch);
            state.on_closed = on_closed;
            state.generation
        };
        self.emit(
            session,
            &run_id,
            json!({
                "type": "agent_backend.session",
                "agentBackend": session.backend.as_str(),
                "nativeSessionId": native,
                "reopened": reopened,
                "loadError": load_error,
                "mode": mode.as_str(),
                "permissionMode": request.launch.native_mode(),
                "generation": generation,
                "declaration": declaration,
                "agentInfo": initialize_response.get("agentInfo").cloned().unwrap_or(Value::Null),
            }),
        );
        let runtime = self.clone();
        let pump_session = Arc::clone(session);
        self.inner.handle.spawn(async move {
            runtime.pump(pump_session, generation, events).await;
        });
        // A reopened Codex session keeps the mode it was saved in; apply the
        // one this turn runs in (a new session got it from
        // INITIAL_AGENT_MODE).
        if session.backend == AgentBackend::Codex && reopened == "load" {
            lock(&session.state).native_mode = None;
            self.switch_codex_mode(session, &run_id, request.launch.native_mode()).await?;
        }
        Ok(())
    }

    /// `session/set_mode` on the live Codex session. Fail-closed: when the
    /// switch is refused the turn does not run in the old mode.
    async fn switch_codex_mode(&self, session: &Arc<Session>, run_id: &str, wanted: &str) -> Result<(), LaunchFailure> {
        let (connection, native, from) = {
            let state = lock(&session.state);
            (state.connection.clone(), state.native_session_id.clone(), state.native_mode.clone())
        };
        let (Some(connection), Some(native)) = (connection, native) else {
            return Err(LaunchFailure::new("agent_backend_exited", "外部后端在切换权限之前退出"));
        };
        let mode = crate::native_permission::codex_mode(wanted);
        match connection.set_mode(&native, mode.id()).await {
            Ok(()) => {
                {
                    let mut state = lock(&session.state);
                    state.native_mode = Some(wanted.to_owned());
                    // Codex's full access is its own mode: the live session
                    // now records what it runs in.
                    state.mode = Some(if crate::native_permission::is_full_access(session.backend, wanted) { PermissionMode::FullAccess } else { PermissionMode::Standard });
                }
                let details = json!({ "from": from, "to": wanted, "method": "session/set_mode" });
                let mut event = details.clone();
                event["type"] = json!("agent_backend.permission_mode_switched");
                event["agentBackend"] = json!(session.backend.as_str());
                self.emit(session, run_id, event);
                self.inner.sink.audit(&session.conversation_id, session.backend, "agent_backend.permission_mode_switched", &details);
                Ok(())
            }
            Err(error) => Err(LaunchFailure::new("agent_backend_set_mode_failed", format!("Codex 拒绝切换到权限「{wanted}」：{error}"))
                .remedy("本轮未执行；可改回原来的权限后重试")
                .with_rpc_error_data(connection.take_rpc_error_data().as_deref())),
        }
    }

    /// The user changed the conversation's native permission mode (plan
    /// C2), full access included. The active turn answers later permission
    /// requests by the new mode at once; a live Codex session is switched now
    /// (`session/set_mode`); dsh restarts at the next turn; pi needs nothing
    /// else (the Host answers its next request by the new tier).
    pub async fn apply_native_mode(&self, conversation_id: &str, wanted: &str) -> NativeModeOutcome {
        let Some(session) = lock(&self.inner.sessions).get(conversation_id).cloned() else {
            return NativeModeOutcome::NotRunning;
        };
        let (live, run_id, running) = {
            let mut state = lock(&session.state);
            if session.backend == AgentBackend::Deepseek {
                // dsh reads its preset only at start: the running turn keeps
                // its preset; the next turn restarts dsh in the new one.
                return if state.connection.is_some() { NativeModeOutcome::NextTurn } else { NativeModeOutcome::NotRunning };
            }
            if let Some(turn) = state.turn.as_mut() {
                turn.native_mode = wanted.to_owned();
            }
            (state.connection.is_some() && state.native_session_open, state.turn.as_ref().map(|turn| turn.run_id.clone()), state.native_mode.clone())
        };
        if !live {
            return NativeModeOutcome::NotRunning;
        }
        if running.as_deref() == Some(wanted) {
            return NativeModeOutcome::Applied;
        }
        match session.backend {
            AgentBackend::Codex => match self.switch_codex_mode(&session, run_id.as_deref().unwrap_or("-"), wanted).await {
                Ok(()) => NativeModeOutcome::Applied,
                Err(failure) => NativeModeOutcome::Failed(failure.message),
            },
            AgentBackend::Deepseek => NativeModeOutcome::NextTurn,
            _ => {
                let mut state = lock(&session.state);
                state.native_mode = Some(wanted.to_owned());
                state.mode = Some(if crate::native_permission::is_full_access(session.backend, wanted) { PermissionMode::FullAccess } else { PermissionMode::Standard });
                NativeModeOutcome::Applied
            }
        }
    }

    async fn pump(&self, session: Arc<Session>, generation: u64, mut events: UnboundedReceiver<BackendEvent>) {
        let mut tick = tokio::time::interval(EXIT_POLL);
        loop {
            tokio::select! {
                event = events.recv() => match event {
                    None | Some(BackendEvent::Closed) => break,
                    Some(event) => self.handle_event(&session, generation, event),
                },
                _ = tick.tick() => {
                    if self.check_idle(&session, generation).await { break; }
                    let (current, connection) = {
                        let state = lock(&session.state);
                        (state.generation == generation, state.connection.clone())
                    };
                    if !current {
                        break;
                    }
                    if connection.and_then(|connection| connection.has_exited()) == Some(true) {
                        // Deliver what the process wrote before it died.
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        while let Ok(event) = events.try_recv() {
                            if !matches!(event, BackendEvent::Closed) {
                                self.handle_event(&session, generation, event);
                            }
                        }
                        break;
                    }
                }
            }
        }
        self.connection_ended(&session, generation).await;
    }

    fn handle_event(&self, session: &Arc<Session>, generation: u64, event: BackendEvent) {
        let mut state = lock(&session.state);
        if state.generation != generation {
            eprintln!(
                "fox.external conversation={} agent_backend={} stale_generation={generation} current={} event={event:?}",
                session.conversation_id,
                session.backend.as_str(),
                state.generation
            );
            return;
        }
        let connection = state.connection.clone();
        match event {
            BackendEvent::PromptStarted { .. } => {
                if let Some(turn) = state.turn.as_mut().filter(|turn| turn.prompt_sent) {
                    turn.prompt_fence_seen = true;
                }
            }
            BackendEvent::LoadSettled { .. } => {}
            BackendEvent::Update { update, .. } => {
                // Only output after this turn's prompt fence belongs to it.
                let Some(turn) = state.turn.as_mut().filter(|turn| turn.prompt_sent && turn.prompt_fence_seen) else {
                    return;
                };
                let run_id = turn.run_id.clone();
                let events = turn.projector.project(&update);
                turn.last_activity = std::time::Instant::now();
                for event in &events {
                    if let Some(id) = event.get("toolCallId").and_then(Value::as_str) {
                        match event.get("type").and_then(Value::as_str) {
                            Some("tool.started") => { turn.active_tools.insert(id.to_owned()); }
                            Some("tool.completed") => { turn.active_tools.remove(id); }
                            _ => {}
                        }
                    }
                }
                for event in events {
                    self.emit(session, &run_id, event);
                }
            }
            BackendEvent::PermissionRequested { native_request_id, option_ids, request, .. } => {
                let Some(connection) = connection else { return };
                let _ = &option_ids;
                let Some(turn) = state.turn.as_mut().filter(|turn| !turn.cancel_requested) else {
                    // No turn to attach it to, or a cancel is under way.
                    let _ = connection.answer_permission(&native_request_id, None);
                    return;
                };
                let card = permission_card(&request);
                let key = format!("acp-permission:{generation}:{native_request_id}");
                let run_id = turn.run_id.clone();
                // Plan C2: pi's gate requests go to the Host's approval
                // engine (same as PiNative); Codex / dsh keep their own
                // approval: a mode that asks shows the card, a mode in which
                // nothing should ask rejects what still arrives (fail-closed).
                // Fox never auto-allows on behalf of Codex or dsh.
                let host = if session.backend == AgentBackend::Pi {
                    self.inner.sink.host_permission_answer(&run_id, &session.conversation_id, session.backend, &turn.native_mode, &request, &card.options)
                } else {
                    None
                };
                let (decision, policy) = match host {
                    Some(answer) => (answer.decision, Some(answer.policy)),
                    None => (crate::native_permission::answer(session.backend, &turn.native_mode, &request, &card.options), None),
                };
                if decision != AutoPermission::Ask {
                    let option = match &decision {
                        AutoPermission::Allow { option_id } => Some(option_id.as_str()),
                        AutoPermission::Reject { option_id } => option_id.as_deref(),
                        AutoPermission::Ask => None,
                    };
                    let written = match connection.answer_permission(&native_request_id, option) {
                        Ok(WriteOutcome::Written) => "response_written",
                        Ok(WriteOutcome::Unconfirmed) => "outcome_unknown",
                        Err(_) => "not_pending",
                    };
                    let mut details = json!({
                        "permissionMode": turn.native_mode,
                        "decision": decision.decision(),
                        "toolKind": permission_tool_kind(&request),
                        "title": card.title,
                        "nativeToolCallId": card.native_tool_call_id,
                        "nativeAnswer": written,
                        "runId": run_id,
                    });
                    if let Some(policy) = policy {
                        details["answeredBy"] = json!("host_approval_policy");
                        details["policy"] = policy;
                    }
                    let mut event = details.clone();
                    event["type"] = json!("agent_backend.permission_auto_answered");
                    self.emit(session, &run_id, event);
                    self.inner.sink.audit(&session.conversation_id, session.backend, "agent_backend.permission_auto_answered", &details);
                    return;
                }
                match self.inner.sink.request_approval(&run_id, &session.conversation_id, &key, &card) {
                    Ok(approval_id) => {
                        turn.approvals.insert(approval_id, PendingApproval { native_id: native_request_id, options: card.options.clone() });
                    }
                    Err(error) => {
                        // Fail closed: an approval Fox cannot show is never allowed.
                        let _ = connection.answer_permission(&native_request_id, None);
                        self.emit(session, &run_id, json!({ "type": "agent_backend.permission_failed", "error": error, "request": request }));
                    }
                }
            }
            BackendEvent::PermissionAutoCancelled { reason, request, .. } => {
                if let Some(turn) = state.turn.as_ref() {
                    let run_id = turn.run_id.clone();
                    self.emit(session, &run_id, json!({ "type": "agent_backend.permission_auto_cancelled", "reason": reason, "request": request }));
                }
            }
            BackendEvent::TurnEnded { stop_reason, .. } => {
                let Some(run_id) = state.turn.as_ref().filter(|turn| turn.prompt_sent).map(|turn| turn.run_id.clone()) else {
                    return;
                };
                let terminal = crate::projection::terminal_for_stop_reason(&stop_reason);
                self.finish_turn(session, &mut state, &run_id, terminal, json!({ "stopReason": stop_reason }));
            }
            BackendEvent::TurnFailed { error, data, .. } => {
                let Some(turn) = state.turn.as_mut().filter(|turn| turn.prompt_sent) else {
                    return;
                };
                // A prompt that fails because the process died is reported
                // as an exit: wait briefly for the stream to close before
                // calling it a protocol failure.
                turn.prompt_error = Some(crate::acp::redact_diagnostic_tail(&error));
                turn.prompt_error_data = data;
                let run_id = turn.run_id.clone();
                let runtime = self.clone();
                let session = Arc::clone(session);
                self.inner.handle.spawn(async move {
                    tokio::time::sleep(PROMPT_FAILURE_SETTLE).await;
                    let mut state = lock(&session.state);
                    if state.generation != generation {
                        return;
                    }
                    let Some((error, data)) = state
                        .turn
                        .as_ref()
                        .filter(|turn| turn.run_id == run_id)
                        .and_then(|turn| turn.prompt_error.clone().map(|error| (error, turn.prompt_error_data.clone())))
                    else {
                        return;
                    };
                    let tail = state.connection.as_ref().map(|connection| connection.diagnostic_stderr_tail()).unwrap_or_default();
                    let terminal = data.as_deref().and_then(model_error_terminal).unwrap_or(TurnTerminal::Failed { code: "agent_backend_prompt_failed".into(), message: error });
                    runtime.finish_turn(
                        &session,
                        &mut state,
                        &run_id,
                        terminal,
                        with_diagnostics(json!({}), stderr_diagnostic(&tail, "prompt_failed"), data.as_deref().and_then(rpc_error_data_diagnostic)),
                    );
                });
            }
            BackendEvent::Failed(error) => {
                eprintln!("fox.external conversation={} agent_backend={} connection_failed={error:?}", session.conversation_id, session.backend.as_str());
            }
            BackendEvent::Initialized { .. } | BackendEvent::Closed => {}
        }
    }

    async fn check_idle(&self, session: &Arc<Session>, generation: u64) -> bool {
        let expired = {
            let mut state = lock(&session.state);
            if state.generation != generation { return false; }
            let Some(turn) = state.turn.as_mut().filter(|turn| turn.prompt_sent && !turn.cancel_requested) else { return false; };
            if !turn.approvals.is_empty() {
                turn.last_activity = std::time::Instant::now();
                return false;
            }
            let timeout = if turn.active_tools.is_empty() { self.inner.idle_timeout } else { Duration::from_secs(600) };
            if turn.last_activity.elapsed() < timeout { return false; }
            let run_id = turn.run_id.clone();
            let phase = if turn.active_tools.is_empty() { "模型响应" } else { "工具执行" };
            let message = format!("等待{phase}超过 {} 秒，外部 Agent 没有返回进展。请检查配置来源、模型端点或本机网关后重试。", timeout.as_secs());
            self.finish_turn(session, &mut state, &run_id, TurnTerminal::Failed { code: "agent_backend_response_timeout".into(), message }, json!({ "outcome": "unknown", "phase": phase, "timeoutSeconds": timeout.as_secs() }));
            // Reserve the session until the timed-out process has been stopped.
            state.closing = true;
            true
        };
        if expired {
            let report = self.stop_connection(session, "response timeout").await;
            let mut state = lock(&session.state);
            state.stop_unconfirmed = report.is_some_and(|report| !report.confirmation.is_confirmed());
            state.closing = false;
        }
        expired
    }

    /// The event stream of `generation` ended: the process exited or the
    /// transport closed. A running turn fails; the next turn relaunches.
    async fn connection_ended(&self, session: &Arc<Session>, generation: u64) {
        let (connection, on_closed) = {
            let mut state = lock(&session.state);
            if state.generation != generation {
                return;
            }
            let connection = state.connection.take();
            state.native_session_open = false;
            let on_closed = state.on_closed.take();
            if !state.closing {
                if let Some((run_id, prompt_error, prompt_error_data)) =
                    state.turn.as_ref().map(|turn| (turn.run_id.clone(), turn.prompt_error.clone(), turn.prompt_error_data.clone()))
                {
                    let tail = connection.as_ref().map(|connection| connection.diagnostic_stderr_tail()).unwrap_or_default();
                    let message = if tail.trim().is_empty() {
                        "外部后端进程意外退出".to_owned()
                    } else {
                        format!("外部后端进程意外退出；后端输出（已脱敏）：{}", tail_chars(&tail, 600))
                    };
                    self.finish_turn(
                        session,
                        &mut state,
                        &run_id,
                        TurnTerminal::Failed { code: "agent_backend_exited".into(), message },
                        with_diagnostics(
                            json!({ "outcome": "unknown", "promptError": prompt_error }),
                            stderr_diagnostic(&tail, "backend_exited"),
                            prompt_error_data.as_deref().and_then(rpc_error_data_diagnostic),
                        ),
                    );
                }
            }
            (connection, on_closed)
        };
        if let Some(connection) = connection {
            let _ = close_shared(connection).await;
        }
        if let Some(callback) = on_closed {
            callback();
        }
    }

    /// Send `session/cancel` for the run's turn (plan 6.4). Pending native
    /// permission requests are answered `cancelled` by the connection. When
    /// the backend does not end the turn within the cancel timeout the run is
    /// recorded `cancelled` with `outcome: "unknown"` and the process stopped.
    pub fn cancel(&self, run_id: &str) -> CancelOutcome {
        let Some(session) = self.find_by_run(run_id) else {
            return CancelOutcome::NotActive;
        };
        let (connection, native, generation) = {
            let mut state = lock(&session.state);
            let generation = state.generation;
            let connection = state.connection.clone();
            let native = state.native_session_id.clone().filter(|_| state.native_session_open);
            let Some(turn) = state.turn.as_mut().filter(|turn| turn.run_id == run_id) else {
                return CancelOutcome::NotActive;
            };
            turn.cancel_requested = true;
            turn.arbiter.request_cancel();
            turn.approvals.clear();
            (connection.filter(|_| turn.prompt_sent), native, generation)
        };
        let answered = match (connection, native) {
            (Some(connection), Some(native)) => connection.cancel(&native),
            _ => Vec::new(),
        };
        let runtime = self.clone();
        let run_id = run_id.to_owned();
        let timeout = self.inner.cancel_timeout;
        self.inner.handle.spawn(async move {
            tokio::time::sleep(timeout).await;
            let still_running = {
                let mut state = lock(&session.state);
                let running = state.turn.as_ref().is_some_and(|turn| turn.run_id == run_id) && state.generation == generation;
                if running {
                    runtime.finish_turn(
                        &session,
                        &mut state,
                        &run_id,
                        TurnTerminal::Cancelled,
                        json!({ "outcome": "unknown", "reason": "取消请求在限定时间内未得到外部后端确认，结果未知；已结束该后端进程" }),
                    );
                }
                running
            };
            if still_running {
                let _ = runtime.stop_connection(&session, "cancel not confirmed").await;
            }
        });
        CancelOutcome::Requested { answered_permissions: answered }
    }

    /// The user's decision on an approval card: the native one-shot option
    /// is written (`allow_once` / `reject_once`, `cancelled` when the backend
    /// offered no rejecting option).
    pub fn answer(&self, approval_id: &str, approved: bool) -> AnswerOutcome {
        let sessions: Vec<Arc<Session>> = lock(&self.inner.sessions).values().cloned().collect();
        for session in sessions {
            let (connection, pending) = {
                let mut state = lock(&session.state);
                let connection = state.connection.clone();
                let Some(pending) = state.turn.as_mut().and_then(|turn| turn.approvals.remove(approval_id)) else {
                    continue;
                };
                if let Some(turn) = state.turn.as_mut() { turn.last_activity = std::time::Instant::now(); }
                (connection, pending)
            };
            let Some(connection) = connection else {
                return AnswerOutcome::Unconfirmed;
            };
            let option = if approved { allow_option(&pending.options) } else { reject_option(&pending.options) };
            if approved && option.is_none() {
                // No native "allow once": never widened to "always".
                let _ = connection.answer_permission(&pending.native_id, None);
                return AnswerOutcome::NotPending;
            }
            return match connection.answer_permission(&pending.native_id, option.map(|option| option.option_id.as_str())) {
                Ok(WriteOutcome::Written) => AnswerOutcome::Written,
                Ok(WriteOutcome::Unconfirmed) => AnswerOutcome::Unconfirmed,
                Err(_) => AnswerOutcome::NotPending,
            };
        }
        AnswerOutcome::NotPending
    }

    pub fn previews(&self, conversation_id: &str) -> Result<Vec<crate::preview::Preview>, String> {
        let session = lock(&self.inner.sessions).get(conversation_id).cloned();
        let connection = session.and_then(|session| lock(&session.state).connection.clone());
        connection.map(|connection| connection.previews()).unwrap_or(Ok(Vec::new()))
    }

    pub async fn stop_preview(&self, conversation_id: &str, preview: &crate::preview::Preview) -> Result<(), String> {
        if !self.previews(conversation_id)?.contains(preview) { return Err("预览已结束或端口归属已变化".into()); }
        match self.close_conversation(conversation_id, "用户停止预览和当前 Agent").await {
            Some(report) if report.confirmation.is_confirmed() => Ok(()),
            _ => Err("当前 Agent 与预览未确认全部结束".into()),
        }
    }

    /// Stop the live process of a session (keeps the session entry and its
    /// native id for a later reopen). `None` when nothing was running.
    async fn stop_connection(&self, session: &Arc<Session>, reason: &str) -> Option<TerminateReport> {
        let (connection, on_closed) = {
            let mut state = lock(&session.state);
            state.generation += 1;
            state.native_session_open = false;
            (state.connection.take(), state.on_closed.take())
        };
        let connection = connection?;
        connection.begin_stopping();
        let report = close_shared(connection).await;
        if let Some(callback) = on_closed {
            callback();
        }
        audit_stop(session, reason, &report);
        Some(report)
    }

    /// Revoke full access mid-session (UX batch ②, semantics A): the Host has
    /// already withdrawn the grant, so [`ExternalLauncher::desired_mode`]
    /// now answers [`PermissionMode::Standard`]. Uses [`crate::external_home::recheck_permission_mode`]:
    /// on `RestartWithoutFullAccess` the running turn is cancelled (ended
    /// `interrupted`, pending approvals settled `cancelled` by the stopping
    /// connection), the process tree is stopped and confirmed, and the next
    /// turn starts a new connection (nothing is replayed). An unconfirmed
    /// stop blocks any relaunch. `Unchanged` when nothing runs in full access.
    ///
    /// Plan C2: `request.native_permission` is the restricted mode the
    /// conversation now runs in. An Agent whose full access does not live in
    /// its process (Codex, pi) leaves it live ([`RevokeOutcome::SwitchedLive`]);
    /// dsh is stopped ([`RevokeOutcome::RestartWithoutFullAccess`]).
    pub async fn revoke_full_access(&self, request: &LaunchRequest) -> RevokeOutcome {
        let conversation_id = request.conversation_id.as_str();
        let Some(session) = lock(&self.inner.sessions).get(conversation_id).cloned() else {
            return RevokeOutcome::NotRunning;
        };
        if !crate::native_permission::full_access_needs_restart(session.backend) {
            let running_full = {
                let state = lock(&session.state);
                state.connection.is_some() && state.mode == Some(PermissionMode::FullAccess)
            };
            if !running_full {
                return RevokeOutcome::Unchanged;
            }
            return match self.apply_native_mode(conversation_id, request.native_mode()).await {
                NativeModeOutcome::Applied => RevokeOutcome::SwitchedLive,
                NativeModeOutcome::NotRunning => RevokeOutcome::NotRunning,
                NativeModeOutcome::NextTurn => RevokeOutcome::Unchanged,
                NativeModeOutcome::Failed(message) => RevokeOutcome::SwitchFailed(message),
            };
        }
        let outcome = self.revoke_running(&session, request).await;
        // 13.19 fix ②: a stopped process must not keep reporting its old
        // (full-access) declaration; the UI then shows the planned view until
        // the next launch reports its own.
        forget_stopped_launch(&session);
        outcome
    }

    async fn revoke_running(&self, session: &Arc<Session>, request: &LaunchRequest) -> RevokeOutcome {
        let conversation_id = request.conversation_id.as_str();
        let running = lock(&session.state).mode;
        let Some(running) = running.filter(|_| lock(&session.state).connection.is_some()) else {
            return RevokeOutcome::NotRunning;
        };
        // The mode the Host wants now, as a policy + snapshot for the shared
        // recheck (a still-granted conversation keeps full access).
        let desired = self.inner.launcher.desired_mode(request);
        let mut policy = crate::config::HostPolicy::default();
        if desired == PermissionMode::FullAccess {
            policy.full_access_grants.insert(crate::external_home::FullAccessGrant::Conversation(conversation_id.to_owned()).key());
        }
        let snapshot = crate::config::SessionSnapshot {
            backend: request.backend,
            project_root: request.project_root.to_string_lossy().into_owned(),
            egress_allowed: false,
            settings: std::collections::BTreeMap::from([(crate::external_home::SETTING_PERMISSION_MODE.to_owned(), desired.as_str().to_owned())]),
        };
        match crate::external_home::recheck_permission_mode(&policy, &snapshot, conversation_id, running) {
            crate::external_home::ModeRecheck::RestartWithoutFullAccess(reason) => {
                {
                    let mut state = lock(&session.state);
                    if let Some(run_id) = state.turn.as_ref().map(|turn| turn.run_id.clone()) {
                        if let Some(turn) = state.turn.as_mut() {
                            turn.cancel_requested = true;
                            turn.arbiter.request_cancel();
                            turn.approvals.clear();
                        }
                        self.finish_turn(
                            &session,
                            &mut state,
                            &run_id,
                            TurnTerminal::Interrupted {
                                code: "agent_backend_full_access_revoked".into(),
                                message: "已撤销完全访问：本轮已取消，外部后端将在下一轮重新启动（不重放本轮）".into(),
                            },
                            json!({ "outcome": "unknown" }),
                        );
                    }
                }
                let report = self.stop_connection(session, &reason).await;
                let confirmed = report.as_ref().is_none_or(|report| report.confirmation.is_confirmed());
                if !confirmed {
                    lock(&session.state).stop_unconfirmed = true;
                }
                RevokeOutcome::RestartWithoutFullAccess { confirmed }
            }
            _ => RevokeOutcome::Unchanged,
        }
    }

    /// Close a conversation's backend (conversation deleted / archived): a
    /// running turn ends `interrupted`, the process tree is stopped and
    /// confirmed. `None` when the conversation had no session.
    pub async fn close_conversation(&self, conversation_id: &str, reason: &str) -> Option<TerminateReport> {
        let session = lock(&self.inner.sessions).remove(conversation_id)?;
        {
            let mut state = lock(&session.state);
            state.closing = true;
            if let Some(run_id) = state.turn.as_ref().map(|turn| turn.run_id.clone()) {
                self.finish_turn(
                    &session,
                    &mut state,
                    &run_id,
                    TurnTerminal::Interrupted { code: "agent_backend_session_closed".into(), message: format!("外部后端会话已关闭：{reason}") },
                    json!({ "outcome": "unknown" }),
                );
            }
        }
        self.stop_connection(&session, reason).await
    }

    /// Stop every backend (application exit). Returns one report per live
    /// process.
    pub async fn shutdown(&self) -> Vec<(String, TerminateReport)> {
        let ids: Vec<String> = lock(&self.inner.sessions).keys().cloned().collect();
        let closes = ids.into_iter().map(|id| {
            let runtime = self.clone();
            async move { runtime.close_conversation(&id, "Fox 退出").await.map(|report| (id, report)) }
        });
        futures::future::join_all(closes).await.into_iter().flatten().collect()
    }

    /// What is known about a conversation's backend session.
    pub fn session_info(&self, conversation_id: &str) -> Option<SessionInfo> {
        let session = lock(&self.inner.sessions).get(conversation_id).cloned()?;
        let state = lock(&session.state);
        Some(SessionInfo {
            backend: session.backend,
            live: state.connection.is_some() && state.native_session_open,
            mode: state.mode,
            declaration: state.declaration.clone(),
            initialize_response: state.initialize_response.clone(),
            capabilities: state.capabilities.clone(),
            pi_launch: state.pi_launch.unwrap_or(PiLaunch::Stock),
            native_session_id: state.native_session_id.clone(),
            active_run: state.turn.as_ref().map(|turn| turn.run_id.clone()),
        })
    }

    /// Run ids of every active turn (for recovery checks and tests).
    pub fn active_runs(&self) -> Vec<String> {
        lock(&self.inner.sessions)
            .values()
            .filter_map(|session| lock(&session.state).turn.as_ref().map(|turn| turn.run_id.clone()))
            .collect()
    }

    /// Whether an approval id is waiting for an answer in this runtime.
    pub fn approval_pending(&self, approval_id: &str) -> bool {
        lock(&self.inner.sessions)
            .values()
            .any(|session| lock(&session.state).turn.as_ref().is_some_and(|turn| turn.approvals.contains_key(approval_id)))
    }
}

/// Drop what a session reported about a process that no longer runs (its
/// launch declaration and handshake), so capability views fall back to the
/// planned launch. `mode` is kept: [`ExternalRuntime::apply_native_mode`]
/// still answers `NextTurn` for leaving full access (the relaunch happens at
/// the next turn). No-op while a connection is open.
fn forget_stopped_launch(session: &Session) {
    let mut state = lock(&session.state);
    if state.connection.is_none() {
        state.declaration = None;
        state.initialize_response = None;
        state.capabilities = None;
    }
}

fn audit_stop(session: &Session, reason: &str, report: &TerminateReport) {
    let confirmation = match &report.confirmation {
        StopConfirmation::Confirmed => "confirmed".to_owned(),
        StopConfirmation::FailedOrUnconfirmed { reasons } => format!("unconfirmed:{}", reasons.join("; ")),
    };
    eprintln!(
        "fox.external conversation={} agent_backend={} stop reason={reason:?} force_killed={} confirmation={confirmation:?}",
        session.conversation_id,
        session.backend.as_str(),
        report.force_killed
    );
}

/// Close a connection that may still be shared with a running call (a
/// `cancel` or `answer_permission` in another task). Waits briefly for the
/// other holders to let go; if they do not, the stopping state is entered
/// and the last holder's drop kills the process (kill-on-drop / Job Object),
/// reported as unconfirmed.
async fn close_shared(mut connection: Arc<AcpConnection>) -> TerminateReport {
    connection.begin_stopping();
    for _ in 0..50 {
        match Arc::try_unwrap(connection) {
            Ok(owned) => return owned.close_with_report().await,
            Err(shared) => {
                connection = shared;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    let mut report = TerminateReport::default();
    report.confirmation = StopConfirmation::FailedOrUnconfirmed {
        reasons: vec!["connection still in use by another task; process killed when it is released".into()],
    };
    report
}
