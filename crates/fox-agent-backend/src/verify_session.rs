//! A disposable ACP session for the explicit settings verification action.
use crate::{
    acp::{AcpConnection, BackendEvent, WriteOutcome},
    agent_config::{LayerResult, VerificationLayer, VerificationStatus},
    external::{ExternalLauncher, LaunchRequest},
    external_home::DEFAULT_CONVERSATIONS_HOME_ID,
    external_launch::{HostLaunchConfig, HostLauncher},
    onboarding::{self, ImportOptions, ImportTarget, RealHome},
    port::AgentBackend,
    settlement::{PendingRegistry, TerminalArbiter},
    unified_login::UnifiedLogin,
};
use serde_json::Value;
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};
use tokio::sync::{mpsc, watch};

const LIMIT: Duration = Duration::from_secs(120);

pub struct SessionVerification {
    pub config: HostLaunchConfig,
    pub backend: AgentBackend,
    pub login: Option<UnifiedLogin>,
    /// An already imported private HOME, never an unapproved real-home source.
    pub native_home: Option<PathBuf>,
    pub with_tool: bool,
    pub permission_mode: String,
}

pub trait VerificationApprovals: Sync {
    fn answer(&self, project: &Path, request: &Value) -> impl std::future::Future<Output = Result<Option<String>, String>> + Send;
}

pub async fn verify_session(input: SessionVerification, cancel: &mut watch::Receiver<bool>, approvals: &impl VerificationApprovals, mut progress: impl FnMut(&LayerResult)) -> [LayerResult; 2] {
    let mut layers = [
        LayerResult::new(VerificationLayer::AgentOutput, VerificationStatus::Skipped, None, None),
        LayerResult::new(VerificationLayer::ToolCall, VerificationStatus::Skipped, None, None),
    ];
    let started = Instant::now();
    layers[0] = LayerResult::new(VerificationLayer::AgentOutput, VerificationStatus::Running, None, None);
    progress(&layers[0]);
    let prepared = prepare(input);
    let (launcher, request, root, nonce, with_tool) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => { layers[0] = failed(VerificationLayer::AgentOutput, error, started); progress(&layers[0]); return layers; }
    };
    // Let initialize finish its own bounded cleanup. Dropping its future would
    // lose the connection before stop_for_revocation could confirm the tree.
    let launch = if *cancel.borrow() { Err(Failure::cancelled()) } else { launcher.launch(request.clone()).await.map_err(|e| Failure::agent(e.message)) };
    let mut launched = match launch {
        Ok(launched) => launched,
        Err(error) => {
            layers[0] = failed(VerificationLayer::AgentOutput, error, started);
            let _ = std::fs::remove_dir_all(&root);
            progress(&layers[0]); return layers;
        }
    };
    let mut events = launched.connection.take_events();
    let already_cancelled = *cancel.borrow();
    let setup = async {
        if already_cancelled { return Err(Failure::cancelled()); }
        let session = launched.connection.new_session(request.project_root.clone()).await.map_err(Failure::agent)?;
        if request.backend == AgentBackend::Codex { launched.connection.set_mode(&session, request.native_mode()).await.map_err(Failure::agent)?; }
        if let Some(model) = launcher.selected_model(&request) { launched.connection.set_model(&session, &model).await.map_err(Failure::agent)?; }
        Ok(session)
    };
    let session = tokio::select! {
        _ = cancel.changed() => Err(Failure::cancelled()),
        result = tokio::time::timeout(LIMIT.saturating_sub(started.elapsed()), setup) => result.unwrap_or_else(|_| Err(Failure::timeout())),
    };
    let mut session_id = String::new();
    match session {
        Ok(session) => {
            session_id = session;
            let first = turn(&launched.connection, &mut events, &session_id, "只回复 OK，不要调用任何工具。", &request.project_root, None, approvals, cancel, LIMIT.saturating_sub(started.elapsed())).await;
            layers[0] = match first {
                Ok(()) => LayerResult::new(VerificationLayer::AgentOutput, VerificationStatus::Passed, None, Some(started.elapsed().as_millis() as i64)),
                Err(error) => failed(VerificationLayer::AgentOutput, error, started),
            };
            progress(&layers[0]);
            if layers[0].status == VerificationStatus::Passed && with_tool {
                let tool_started = Instant::now();
                layers[1] = LayerResult::new(VerificationLayer::ToolCall, VerificationStatus::Running, None, None);
                progress(&layers[1]);
                let prompt = "只读取当前目录中的 fox-verify.txt，并原样回复文件里的字符串。使用你自己的工具，不要修改文件，不要访问其他路径，不要联网。";
                let result = turn(&launched.connection, &mut events, &session_id, prompt, &request.project_root, Some(&nonce), approvals, cancel, LIMIT).await;
                layers[1] = match result {
                    Ok(()) => LayerResult::new(VerificationLayer::ToolCall, VerificationStatus::Passed, None, Some(tool_started.elapsed().as_millis() as i64)),
                    Err(error) => failed(VerificationLayer::ToolCall, error, tool_started),
                };
                progress(&layers[1]);
            }
        }
        Err(error) => { layers[0] = failed(VerificationLayer::AgentOutput, error, started); progress(&layers[0]); }
    }
    let tail = launched.connection.diagnostic_stderr_tail();
    let stopped = launched.connection.stop_for_revocation(&session_id, &mut PendingRegistry::default(), &mut TerminalArbiter::default(), "settings verification finished").await;
    if let Some(close) = launched.on_closed.take() { close(); }
    if stopped.process_tree.confirmation.is_confirmed() {
        if let Err(error) = std::fs::remove_dir_all(&root) {
            let at = if with_tool && layers[0].status == VerificationStatus::Passed { 1 } else { 0 };
            layers[at] = failed(layers[at].layer, Failure { code: "agent_verify.cleanup_failed", detail: error.to_string() }, started);
        }
    } else {
        let at = if with_tool && layers[0].status == VerificationStatus::Passed { 1 } else { 0 };
        layers[at] = failed(layers[at].layer, Failure { code: "agent_verify.stop_unconfirmed", detail: stopped.process_tree.confirmation.reasons().join("; ") }, started);
    }
    for layer in &mut layers {
        if layer.status == VerificationStatus::Failed && !tail.is_empty() {
            layer.detail = Some(crate::acp::redact_diagnostic_tail(&format!("{}\n{tail}", layer.detail.as_deref().unwrap_or_default())));
        }
        progress(layer);
    }
    layers
}

type Prepared = (HostLauncher, LaunchRequest, PathBuf, String, bool);
fn prepare(input: SessionVerification) -> Result<Prepared, Failure> {
    let suffix = crate::launch::random_hex(16).map_err(Failure::agent)?;
    let root = input.config.state_root.join("verify").join(&suffix);
    std::fs::create_dir_all(&root).map_err(|e| Failure::agent(e.to_string()))?;
    let result = prepare_in(&root, &suffix, input);
    if result.is_err() { let _ = std::fs::remove_dir_all(&root); }
    result
}
fn prepare_in(root: &Path, suffix: &str, input: SessionVerification) -> Result<Prepared, Failure> {
    let project_root = root.join("project");
    std::fs::create_dir(&project_root).map_err(|e| Failure::agent(e.to_string()))?;
    let nonce = crate::launch::random_hex(24).map_err(Failure::agent)?;
    std::fs::write(project_root.join("fox-verify.txt"), &nonce).map_err(|e| Failure::agent(e.to_string()))?;
    let mut config = input.config;
    config.fox_data = root.join("data");
    config.state_root = config.fox_data.join("agent-state");
    config.native_login_home = None;
    let home = config.private_home_path(DEFAULT_CONVERSATIONS_HOME_ID, input.backend).map_err(|e| Failure::agent(e.message))?;
    crate::external_home::ensure_backend_home_dirs(input.backend, &home).map_err(|e| Failure::agent(e.to_string()))?;
    if let Some(source) = input.native_home {
        let real = RealHome { home: source, codex_home: None };
        let selected = onboarding::import_candidates(input.backend, &real).iter().map(|item| item.id.clone()).collect::<Vec<_>>();
        let target = ImportTarget { project: DEFAULT_CONVERSATIONS_HOME_ID.into(), label: "临时验证".into(), home: home.clone() };
        onboarding::import_into(input.backend, &real, &selected, &[target], ImportOptions { overwrite: false, backup_stamp: "verify" }).map_err(Failure::agent)?;
        let native = crate::model_endpoint::resolve(input.backend, &home, &project_root).map_err(Failure::agent)?;
        crate::model_endpoint::approve_import_snapshot(&config.state_root, input.backend, &home, &native).map_err(Failure::agent)?;
    } else {
        crate::login_source::choose_fox(input.backend, &config.state_root, &home, false).map_err(Failure::agent)?;
    }
    let full_access_granted = crate::native_permission::is_full_access(input.backend, &input.permission_mode);
    let request = LaunchRequest { conversation_id: format!("verify-{suffix}"), backend: input.backend, project_uuid: Some(DEFAULT_CONVERSATIONS_HOME_ID.into()), project_root, session_key: format!("verify-{suffix}"), full_access_granted, unified_login: input.login, native_permission: Some(input.permission_mode.clone()) };
    if request.native_mode() != input.permission_mode || (full_access_granted && config.full_access_forbidden) {
        return Err(Failure::agent("该权限在当前配置下不可用。".into()));
    }
    // Built-in file tools only. This disposable session receives no Fox MCP
    // token and cannot reach a user's conversations or workspace tools.
    Ok((HostLauncher { config, mcp: None }, request, root.to_owned(), nonce, input.with_tool))
}

struct Failure { code: &'static str, detail: String }
impl Failure {
    fn agent(detail: String) -> Self { Self { code: "agent_verify.agent_no_output", detail } }
    fn cancelled() -> Self { Self { code: "agent_verify.cancelled", detail: String::new() } }
    fn timeout() -> Self { Self { code: "agent_verify.timeout", detail: String::new() } }
}
fn failed(layer: VerificationLayer, error: Failure, started: Instant) -> LayerResult {
    let code = if layer == VerificationLayer::ToolCall && error.code == "agent_verify.agent_no_output" { "agent_verify.tool_failed" } else { error.code };
    let mut result = LayerResult::new(layer, VerificationStatus::Failed, Some(code), Some(started.elapsed().as_millis() as i64));
    if !error.detail.is_empty() { result.detail = Some(crate::acp::redact_diagnostic_tail(&error.detail)); }
    result
}
async fn turn(connection: &AcpConnection, events: &mut mpsc::UnboundedReceiver<BackendEvent>, session: &str, prompt: &str, project: &Path, nonce: Option<&str>, approvals: &impl VerificationApprovals, cancel: &mut watch::Receiver<bool>, timeout: Duration) -> Result<(), Failure> {
    if *cancel.borrow() { return Err(Failure::cancelled()); }
    connection.prompt(session, prompt).map_err(Failure::agent)?;
    let work = async {
        let mut text = String::new();
        let mut completed_tool = false;
        let mut started = false;
        while let Some(event) = events.recv().await {
            match event {
                BackendEvent::PromptStarted { session_id } if session_id == session => { started = true; }
                BackendEvent::Update { session_id, update } if started && session_id == session => {
                    match update["sessionUpdate"].as_str().unwrap_or_default() {
                        "agent_message_chunk" => { if let Some(chunk) = update.pointer("/content/text").and_then(Value::as_str) { text.push_str(chunk); } }
                        "tool_call" | "tool_call_update" if update["status"] == "completed" => { completed_tool = true; }
                        _ => {}
                    }
                }
                BackendEvent::PermissionRequested { native_request_id, request, .. } => {
                    let option = approvals.answer(project, &request).await.map_err(Failure::agent)?;
                    if !matches!(connection.answer_permission(&native_request_id, option.as_deref()).map_err(Failure::agent)?, WriteOutcome::Written) {
                        return Err(Failure::agent("未能确认审批结果已送达 Agent。".into()));
                    }
                }
                BackendEvent::TurnEnded { session_id, .. } if started && session_id == session => {
                    let success = nonce.map_or(!text.trim().is_empty(), |nonce| completed_tool && text.contains(nonce));
                    return if success { Ok(()) } else { Err(Failure::agent("没有收到预期输出或完整的工具调用回执".into())) };
                }
                BackendEvent::TurnFailed { error, .. } | BackendEvent::Failed(error) => return Err(Failure::agent(error)),
                BackendEvent::Closed => break,
                _ => {}
            }
        }
        Err(Failure::agent("Agent 连接已结束".into()))
    };
    tokio::select! {
        _ = cancel.changed() => Err(Failure::cancelled()),
        outcome = tokio::time::timeout(timeout, work) => outcome.unwrap_or_else(|_| Err(Failure::timeout())),
    }
}
