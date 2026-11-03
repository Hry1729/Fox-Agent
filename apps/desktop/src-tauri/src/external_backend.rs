//! Thin Host glue for the external Agent backends (pi / Codex / DeepSeek over
//! ACP; external backend wiring batch). Every decision lives in
//! `fox_agent_backend::external` (runtime: start, stream, approvals, cancel,
//! crash, close) and `fox_agent_backend::external_launch` (managed runtime,
//! private HOME, native permission mode). This file only:
//!
//! * persists + publishes runtime events exactly like the yuxi runtime
//!   (`record_external_event` -> `fox://runtime-event`), so the existing
//!   projection, thread and tool cards are reused;
//! * turns a native permission request into the existing approval card (a
//!   Host tool call `agent_permission` pending approval + `approvals` row +
//!   `fox://approval-requested`) and settles that row after the answer;
//! * answers pi's gate requests with the Host approval engine first (plan
//!   C2, [`crate::external_policy`]: the same engine, rules and block rules
//!   as PiNative);
//! * offers the commands' entry points (start / cancel / approve / close /
//!   capabilities) and the startup / exit hooks.
use crate::database::{ApiResponse, ApprovalDecision, Database, StartRunRequest, StartRunResult};
use crate::runtime_host::RuntimeEventNotification;
use fox_agent_backend::capability_view::{capability_view, CapabilityView};
use fox_agent_backend::external::{AnswerOutcome, CancelOutcome, ExternalRuntime, ExternalSink, HostPermissionAnswer, LaunchRequest, RevokeOutcome, TurnRequest};
use fox_agent_backend::external_launch::{HostLaunchConfig, HostLauncher};
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::{PermissionCard, PermissionOption};
use fox_agent_backend::external_home::LaunchDeclaration;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

/// Tool name of the Host tool call that carries an external permission card.
pub const PERMISSION_TOOL_NAME: &str = "agent_permission";

pub struct DesktopSink {
    app: AppHandle,
    database: Database,
    event_lock: Arc<Mutex<()>>,
    /// 无项目会话的默认对话目录放在这个数据目录下。
    data_dir: std::path::PathBuf,
}

impl ExternalSink for DesktopSink {
    fn emit(&self, run_id: &str, conversation_id: &str, event: Value) -> Result<(), String> {
        let _guard = self.event_lock.lock().map_err(|_| "event lock poisoned".to_owned())?;
        let Some(seq) = self.database.record_external_event(run_id, &event)? else {
            // Not applied (cancelling run, duplicate): nothing to publish.
            return Ok(());
        };
        let _ = self.app.emit(
            "fox://runtime-event",
            RuntimeEventNotification {
                conversation_id: conversation_id.to_owned(),
                runtime_session_id: None,
                run_id: run_id.to_owned(),
                seq,
                timestamp: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                event,
            },
        );
        Ok(())
    }

    fn request_approval(&self, run_id: &str, conversation_id: &str, key: &str, card: &PermissionCard) -> Result<String, String> {
        let backend = self
            .database
            .conversation_agent_backend(conversation_id)?
            .unwrap_or_default();
        let input = json!({
            "title": card.title,
            "tool": card.tool,
            "input": card.input,
            "nativeToolCallId": card.native_tool_call_id,
            "options": card.options,
        });
        let tool = self
            .database
            .create_host_tool_call_once(run_id, key, PERMISSION_TOOL_NAME, &input, "pending", true)?
            .0;
        let label = AgentBackend::parse(&backend).map(|backend| match backend {
            AgentBackend::Pi => "pi",
            AgentBackend::Codex => "Codex",
            AgentBackend::Deepseek => "DeepSeek",
            AgentBackend::Fox => "Fox",
        });
        let request = external_permission_request(label.unwrap_or("外部 Agent"), &backend, card);
        let approval = self.database.create_approval(&tool.id, &card.title, &request)?;
        let _ = self.emit(run_id, conversation_id, json!({ "type": "run.waiting_approval", "approvalId": approval.id, "toolCallId": key }));
        let _ = self.app.emit("fox://approval-requested", &approval);
        Ok(approval.id)
    }

    /// Settings batch, item 7: a permission request Fox answered itself
    /// under 自动 / 只读 (names and kinds only, never tool input).
    fn audit(&self, conversation_id: &str, backend: AgentBackend, event: &str, details: &Value) {
        let _ = self.database.append_agent_backend_audit(Some(conversation_id), backend.as_str(), event, details);
    }

    /// Pi 由 Host 审批引擎完整作答（失败则 fail-closed）。
    /// Codex / dsh 只在命令命中 block 时拒绝；否则 `None`，仍由它们自己的审批决定。
    /// 数据库出错同样 fail-closed，不放宽。
    fn host_permission_answer(&self, run_id: &str, conversation_id: &str, backend: AgentBackend, native_mode: &str, request: &Value, options: &[PermissionOption]) -> Option<HostPermissionAnswer> {
        if backend == AgentBackend::Pi {
            return Some(match crate::external_policy::answer_pi_request(&self.database, &self.data_dir, run_id, conversation_id, native_mode, request, options) {
                Ok(answer) => answer,
                Err(error) => crate::external_policy::fail_closed(&error, options),
            });
        }
        if matches!(backend, AgentBackend::Codex | AgentBackend::Deepseek) {
            return match crate::external_policy::block_for_native_request(&self.database, &self.data_dir, run_id, conversation_id, backend, native_mode, request, options) {
                Ok(answer) => answer,
                Err(error) => Some(crate::external_policy::fail_closed(&error, options)),
            };
        }
        None
    }
}

pub type DesktopRuntime = ExternalRuntime<DesktopSink, HostLauncher>;

/// Wording for a settled approval that did not grant the action. Approvals no
/// longer expire, but rows written by older builds may still be `expired`;
/// they read "已过期", never "用户拒绝".
pub(crate) fn approval_outcome_label(status: &str) -> &'static str {
    match status {
        "denied" => "用户拒绝",
        "expired" => "已过期",
        "cancelled" => "已取消",
        "approved" => "已批准",
        _ => "未批准",
    }
}

/// Managed Tauri state.
pub struct ExternalBackendHost {
    runtime: DesktopRuntime,
    database: Database,
    app: AppHandle,
    /// Fox data root, named in `run.create_failed` messages.
    data_dir: std::path::PathBuf,
    /// Plan C3: why the Fox tool MCP bridge could not start (then no
    /// external turn starts: an Agent never runs without the Fox tools).
    mcp_error: Option<String>,
}

fn failure<T: serde::Serialize>(code: &str, message: impl Into<String>, retryable: bool) -> ApiResponse<T> {
    ApiResponse::failure(code, message.into(), retryable)
}

/// `permission.` 开头的错误原样作为 failure code；其余归到审批结算失败。
fn permission_failure<T: serde::Serialize>(error: String) -> ApiResponse<T> {
    let code = error.split(':').next().filter(|code| code.starts_with("permission.")).unwrap_or("approval.resolve_failed").to_owned();
    failure(&code, error, false)
}

/// Host 策略允许「本项目总是允许」时，从 host_policy 取出要写进卡片的字段。
/// 没有 eligible 的 projectRule，或没有命令原文，就不提供该选项（fail-closed）。
fn pi_project_rule_offer(card: &PermissionCard) -> Option<(Value, String, Option<String>, Value)> {
    let policy = card.host_policy.as_ref()?;
    let rule = policy.get("projectRule")?;
    if rule.get("eligible").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let command = policy.get("command").and_then(Value::as_str).map(str::trim).filter(|command| !command.is_empty())?;
    let cwd = policy.get("cwd").and_then(Value::as_str).filter(|cwd| !cwd.is_empty()).map(str::to_owned);
    let mut summary = policy.clone();
    if let Some(object) = summary.as_object_mut() {
        object.remove("projectRule");
        object.remove("command");
        object.remove("cwd");
    }
    Some((rule.clone(), command.to_owned(), cwd, summary))
}

/// 外部审批卡的 request。不 eligible 时与原来完全一样；eligible 时在 allow_once 与 deny
/// 之间插入 `allow_project_rule`，并带上 projectRule、policy 摘要、命令、cwd 和 hostExecution: null。
fn external_permission_request(agent_label: &str, backend: &str, card: &PermissionCard) -> Value {
    let mut decisions: Vec<&str> = card.available_decisions.clone();
    let offer = pi_project_rule_offer(card);
    if offer.is_some() && !decisions.iter().any(|item| *item == "allow_project_rule") {
        let at = decisions.iter().position(|item| *item == "deny").unwrap_or(decisions.len());
        decisions.insert(at, "allow_project_rule");
    }
    let mut request = json!({
        "title": format!("允许 {agent_label} 执行此操作？"),
        "summary": card.title,
        "input": card.input,
        "command": card.input.get("command"),
        "availableDecisions": decisions,
        "agentBackend": backend,
        "nativeToolCallId": card.native_tool_call_id,
        "options": card.options,
        "answerAcknowledged": false,
    });
    if let Some((rule, command, cwd, summary)) = offer {
        request["command"] = json!(command);
        if let Some(cwd) = cwd {
            request["cwd"] = json!(cwd);
        }
        request["projectRule"] = rule;
        request["policy"] = summary;
        request["hostExecution"] = Value::Null;
    }
    request
}

/// 会话 id 只能是目录名里的一段，避免 `..` 或分隔符把默认对话目录指到数据目录外面。
fn safe_workspace_id(id: &str) -> bool {
    let mut chars = id.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn project_is_bound(project_id: Option<&str>, project_root: Option<&str>) -> bool {
    project_id.is_some_and(|id| !id.is_empty()) || project_root.is_some_and(|root| !root.is_empty())
}

fn project_inaccessible_message(project_id: Option<&str>, project_root: Option<&str>) -> String {
    let path = project_root.filter(|root| !root.is_empty()).or(project_id.filter(|id| !id.is_empty())).unwrap_or("（没有路径）");
    format!("项目文件夹不可访问：{path}")
}

/// 外部会话的授权目录。
/// 有项目：规范化后的项目根；不存在或不是目录则 `agent_backend_project_required`（这个码只表示项目文件夹不可访问）。
/// 无项目：`<数据目录>/conversation-workspaces/<会话 id>`，由 Host 创建。创建失败返回 `agent_backend_default_workspace_unavailable`，不改用更宽的目录。
pub(crate) fn authorized_root(
    data_dir: &std::path::Path,
    conversation_id: &str,
    project_id: Option<&str>,
    project_root: Option<&str>,
) -> Result<std::path::PathBuf, ApiResponse<()>> {
    if project_is_bound(project_id, project_root) {
        return match project_root.filter(|root| !root.is_empty()).map(std::path::PathBuf::from).map(|root| root.canonicalize()) {
            Some(Ok(root)) if root.is_dir() => Ok(root),
            _ => Err(failure("agent_backend_project_required", project_inaccessible_message(project_id, project_root), false)),
        };
    }
    if !safe_workspace_id(conversation_id) {
        return Err(failure("agent_backend_default_workspace_unavailable", "默认对话目录不可用：会话标识不能作为目录名", false));
    }
    let path = crate::data_paths::default_conversation_workspace(data_dir, conversation_id);
    if let Err(reason) = crate::data_paths::ensure_directory(&path, "默认对话目录") {
        return Err(failure("agent_backend_default_workspace_unavailable", format!("默认对话目录不可用：{reason}"), false));
    }
    path.canonicalize().map_err(|error| failure("agent_backend_default_workspace_unavailable", format!("默认对话目录不可用：{error}"), false))
}

/// 「套用到我所有项目」时把无项目会话的共用 HOME 也算进去，回执标签是「未选项目的对话」。
pub(crate) fn import_home_targets(all_projects: bool, project_uuid: Option<&str>, known: &[(String, String)]) -> Vec<(String, String)> {
    const LABEL: &str = "未选项目的对话";
    let default_id = fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID;
    if all_projects {
        let mut projects: Vec<(String, String)> = known.iter().filter(|(id, _)| id != default_id).cloned().collect();
        projects.push((default_id.to_owned(), LABEL.to_owned()));
        return projects;
    }
    let Some(id) = project_uuid.map(str::trim).filter(|id| !id.is_empty()) else {
        return Vec::new();
    };
    if id == default_id {
        return vec![(id.to_owned(), LABEL.to_owned())];
    }
    let label = known.iter().find(|(known_id, _)| known_id == id).map(|(_, name)| name.clone()).unwrap_or_else(|| id.to_owned());
    vec![(id.to_owned(), label)]
}

/// The Agent's name in user-facing sentences.
fn agent_label(backend: AgentBackend) -> &'static str {
    match backend {
        AgentBackend::Pi => "Pi",
        AgentBackend::Codex => "Codex",
        AgentBackend::Deepseek => "DeepSeek",
        AgentBackend::Fox => "Fox",
    }
}

/// Send refusal for a missing / unverified / ambiguous model connection of an
/// external Agent. Every sentence points at 「设置 → 模型服务」: the composer's
/// backend bar and login cards no longer exist (UI 1103), and the address an
/// Agent uses is chosen there (「用作 X 默认协议」 on the protocol address).
pub(crate) fn login_gap_failure(gap: Option<crate::external_model::LoginGap>, backend: AgentBackend) -> (&'static str, String) {
    use crate::external_model::LoginGap;
    let name = agent_label(backend);
    match gap {
        Some(LoginGap::Unverified) => ("model.connection_unverified",
            "模型服务的配置改过了，当前地址还没有通过连接测试：请到「设置 → 模型服务」重新测试连接，通过后再发送".to_owned()),
        Some(LoginGap::ChooseAddress) => ("model.connection_choose_address",
            format!("当前供应商有多个地址都能给 {name} 用：请到「设置 → 模型服务」，在要用的地址上打开「用作 {name} 默认协议」后再发送")),
        _ => ("model.connection_required", "请先到「设置 → 模型服务」配置并测试当前供应商，再发送".to_owned()),
    }
}

/// What to do with Pi's pending 「配置来源」 choice before a send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PiSourceStep {
    /// Nothing pending.
    Proceed,
    /// Never chosen, and 模型服务 can serve Pi: use it for this project's
    /// private HOME (not remembered as the default; recorded in the audit).
    DefaultToFox,
    /// The confirmed native source changed, or 模型服务 cannot serve Pi.
    Refuse,
}

/// `implicit_fox`：没有待确认的选择，也从没选过来源，本机也没有 Pi 原生配置，当前来源只是
/// 默认落到「模型服务」上（13.38 B3-4）。这时 Fox 模型服务可用就按 DefaultToFox 记下并写审计。
pub(crate) fn pi_source_step(choice_required: bool, source_changed: bool, fox_login_ready: bool, implicit_fox: bool) -> PiSourceStep {
    if !choice_required {
        if implicit_fox && fox_login_ready { PiSourceStep::DefaultToFox } else { PiSourceStep::Proceed }
    } else if !source_changed && fox_login_ready {
        PiSourceStep::DefaultToFox
    } else {
        PiSourceStep::Refuse
    }
}

pub(crate) const PI_SOURCE_CHOICE_REQUIRED: &str = "agent_backend_pi_source_choice_required";
pub(crate) const PI_SOURCE_CHOICE_MESSAGE: &str = "Pi 的配置来源需要确认：请先到「设置 → 模型服务」配置并测试一个供应商（Fox 会把它交给 Pi），再发送";

/// Carry a refusal over to another payload type.
fn retype<T: serde::Serialize>(refusal: ApiResponse<()>) -> ApiResponse<T> {
    match refusal.error {
        Some(error) => ApiResponse::failure(&error.code, error.message, error.retryable),
        None => ApiResponse::failure("internal", "unexpected empty refusal", false),
    }
}

/// Which private HOME a login probe / import / "打开文件夹" is about: an
/// existing conversation (`conversationId`), or a NEW conversation that has
/// not been created yet (`backend`, optional `projectId` / `projectRoot`, plus the
/// draft's full-access choice), so the login card can show before the first
/// message (Windows acceptance of 80b1622). 两者都缺省时走默认对话目录的共用 HOME。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBackendTarget {
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
    /// Draft only: the user chose "完全访问" for the new conversation.
    #[serde(default)]
    pub full_access: bool,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub previews: Vec<fox_agent_backend::preview::Preview>,
    pub project_id: Option<String>,
}

fn network_project(database: &Database, conversation_id: &str) -> Result<Option<String>, String> {
    database.with_connection(|connection| connection.query_row("SELECT project_id FROM conversations WHERE id = ?1", [conversation_id], |row| row.get(0)))
}

impl ExternalBackendHost {
    pub fn network_status(&self, conversation_id: &str) -> Result<NetworkStatus, String> {
        let project_id = network_project(&self.database, conversation_id)?;
        let previews = self.runtime.previews(conversation_id)?;
        Ok(NetworkStatus { previews, project_id })
    }

    pub async fn preview_action(&self, conversation_id: &str, preview: &fox_agent_backend::preview::Preview, stop: bool) -> Result<String, String> {
        if !self.runtime.previews(conversation_id)?.contains(preview) { return Err("预览已结束或端口归属已变化，请刷新".into()); }
        if stop { self.runtime.stop_preview(conversation_id, preview).await?; }
        Ok(preview.url.clone())
    }

}

impl ExternalBackendHost {
    pub fn new(app: AppHandle, database: Database, fox_data: &std::path::Path) -> Self {
        let config = HostLaunchConfig::from_env(fox_data);
        let sink = DesktopSink { app: app.clone(), database: database.clone(), event_lock: Arc::new(Mutex::new(())), data_dir: fox_data.to_path_buf() };
        let handle = tauri::async_runtime::handle().inner().clone();
        // Plan C3: the Fox tool MCP bridge (loopback, per-launch tokens).
        let runtime_slot: Arc<std::sync::OnceLock<DesktopRuntime>> = Arc::default();
        let tools = Arc::new(crate::external_tools::DesktopFoxTools::new(app.clone(), Arc::clone(&runtime_slot)));
        let (launcher, mcp_error) = match fox_agent_backend::fox_mcp::McpBridge::start(tools, &handle) {
            Ok(bridge) => (HostLauncher::with_mcp(config, bridge), None),
            Err(error) => {
                eprintln!("fox.external mcp_bridge_failed={error}");
                (HostLauncher::new(config), Some(error.to_string()))
            }
        };
        let runtime = ExternalRuntime::new(sink, launcher, handle);
        let _ = runtime_slot.set(runtime.clone());
        let host = Self { runtime, database, app, data_dir: fox_data.to_path_buf(), mcp_error };
        host.interrupt_runs_left_active();
        host
    }

    /// Runs of external conversations a previous Fox process left
    /// non-terminal: their backend died with that process.
    fn interrupt_runs_left_active(&self) {
        // The generic `repair_interrupted_runs` skips these runs; this is the
        // only place they are ended (`agent_backend_host_restarted`, pending
        // approvals `cancelled` / `app_restarted`).
        if let Err(error) = self
            .database
            .interrupt_external_runs_left_active("Fox 已重启，外部后端的这一轮随上次进程结束，结果未知")
        {
            eprintln!("fox.external left_active_lookup_failed={error:?}");
        }
    }

    /// `run_start` for an external conversation (already routed).
    pub fn start_run(&self, request: &StartRunRequest, backend: AgentBackend) -> ApiResponse<StartRunResult> {
        if !request.attachment_ids.is_empty() {
            return failure("agent_backend_attachments_unsupported", "外部 Agent 后端暂不支持附件，请只发送文字", false);
        }
        if let Some(error) = &self.mcp_error {
            return failure("agent_backend_fox_tools_unavailable", format!("Fox 工具桥（MCP）没有启动，外部 Agent 不会在缺少 Fox 工具的情况下运行：{error}（请重启 Fox）"), true);
        }
        // Second check right before anything is launched (plan: re-verify the
        // frozen backend before prepare_launch). A failed read refuses.
        match self.database.conversation_agent_backend(&request.conversation_id) {
            Ok(Some(stored)) if stored == backend.as_str() => {}
            Ok(_) => return failure("agent_backend_misrouted", "会话绑定的 Agent 后端与路由结果不一致，已拒绝", false),
            Err(error) => return failure("agent_backend_lookup_failed", format!("无法读取会话绑定的 Agent 后端：{error}"), true),
        }
        let context = match self.database.external_conversation_context(&request.conversation_id) {
            Ok(Some(context)) => context,
            Ok(None) => return failure("conversation.not_found", "会话不存在", false),
            Err(error) => return failure("storage.operation_failed", error, true),
        };
        let project_root = match authorized_root(&self.data_dir, &request.conversation_id, context.project_id.as_deref(), context.project_root.as_deref()) {
            Ok(root) => root,
            Err(refusal) => return retype(refusal),
        };
        let target = AgentBackendTarget { conversation_id: Some(request.conversation_id.clone()), backend: None, project_id: None, project_root: None, full_access: false };
        // e2e-1101: Pi's 「配置来源」 choice (Fox 模型服务 or the native ~/.pi
        // config) is settled here, before any Run exists. The UI that offered
        // the choice is gone (UI 1103 removed the login cards above the
        // composer), so a pending choice would fail every send afterwards.
        if backend == AgentBackend::Pi {
            if let Err(refusal) = self.settle_pi_source(&target) {
                return retype(refusal);
            }
        }
        match self.model_login_context(&target) {
            Ok((_, false)) if self.unified_login(backend).is_none() => {
                // 13.35: say which step is missing. An edited provider (e.g. an
                // added Anthropic address) needs a new connection test before a
                // Run is created; the generic message hid that, and the send
                // looked like it silently did nothing.
                let (code, message) = login_gap_failure(crate::external_model::external_login_gap(&self.database, backend), backend);
                return failure(code, message, true);
            }
            Err(error) => return failure("model.connection_failed", error, true),
            _ => {}
        }
        // 13.38 B4-5: the model service address this Agent gets from 模型服务
        // must be reachable before a Run is created (refused / DNS / timeout).
        if let Some(login) = self.unified_login(backend) {
            if let Some((code, message)) = crate::model_reachability::send_refusal(&login.base_url) {
                return failure(code, message, true);
            }
        }
        // A storage error here is not "no earlier session": starting a new
        // native session would silently drop the conversation's history.
        let stored = match self.database.latest_native_session(&request.conversation_id) {
            Ok(stored) => stored,
            Err(error) => return failure("storage.operation_failed", format!("无法读取这个对话的外部会话记录：{error}"), true),
        };
        let started = match self.database.create_run_checked(&request.conversation_id, &request.text, request.model.as_deref()) {
            Ok(started) => started,
            Err(error) => return crate::commands::create_run_refusal(&self.data_dir, &error),
        };
        let turn = TurnRequest {
            run_id: started.run.id.clone(),
            prompt: request.runtime_text.clone().unwrap_or_else(|| request.text.clone()),
            stored_native_session: stored,
            launch: LaunchRequest {
                conversation_id: request.conversation_id.clone(),
                backend,
                project_uuid: Some(fox_agent_backend::external_home::home_project_uuid(context.project_id.as_deref()).to_owned()),
                project_root,
                session_key: started.run.id.replace(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'), "_"),
                // Fail-closed: a grant that cannot be read is no grant.
                full_access_granted: self.database.conversation_full_access_granted(&request.conversation_id).unwrap_or(false),
                unified_login: self.unified_login(backend),
                // The conversation's choice in its Agent's own permission
                // list; replaces Fox's 询问 / 自动 / 只读 for external backends.
                native_permission: crate::commands::agent_backend_routing::stored_conversation_native_mode(&self.database, &request.conversation_id),
            },
        };
        if let Err(error) = self.runtime.start_turn(turn) {
            let _ = self.database.mark_run_failed(&started.run.id, error.code(), &error.message());
            return failure(error.code(), error.message(), true);
        }
        ApiResponse::success(started)
    }

    /// `run_cancel` for an external run (already routed).
    pub fn cancel_run(&self, run_id: &str) -> ApiResponse<bool> {
        match self.database.mark_run_cancelling(run_id) {
            Ok(Some(_)) => {}
            Ok(None) => return ApiResponse::success(false),
            Err(error) => return failure("storage.operation_failed", error, true),
        }
        match self.runtime.cancel(run_id) {
            CancelOutcome::Requested { .. } => ApiResponse::success(true),
            CancelOutcome::NotActive => {
                let message = "外部后端没有正在执行的这一轮";
                let _ = self.database.mark_run_interrupted(run_id, "runtime.cancel_not_active", message);
                failure("runtime.cancel_not_active", message, true)
            }
        }
    }

    /// pi 卡片上的「本项目总是允许」：校验并写入 `run_command` 规则，再按允许一次结算。
    /// 非 pending 返回 success(false)，与 Fox 的 `allow_approval_with_project_rules` 一致。
    /// Codex / dsh 不提供该选项，调用到这里也拒绝。
    pub fn allow_with_project_rule(&self, approval_id: &str, pattern: &str) -> ApiResponse<bool> {
        let approval = match self.database.approval_record(approval_id) {
            Ok(approval) => approval,
            Err(error) => return failure("approval.resolve_failed", error, false),
        };
        if approval.status != "pending" {
            return ApiResponse::success(false);
        }
        let backend = match self.database.conversation_agent_backend(&approval.conversation_id) {
            Ok(Some(backend)) => backend,
            Ok(None) => return permission_failure("permission.rule_not_offered: 这次审批不能保存为项目规则".into()),
            Err(error) => return failure("approval.resolve_failed", error, false),
        };
        if AgentBackend::parse(&backend) != Some(AgentBackend::Pi) {
            return permission_failure("permission.rule_not_offered: 这次审批不能保存为项目规则".into());
        }
        let offered = approval
            .request
            .get("availableDecisions")
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("allow_project_rule")));
        if !offered {
            return permission_failure("permission.rule_not_offered: 这次审批不能保存为项目规则".into());
        }
        if let Err(error) = crate::external_policy::allow_pi_with_project_rule(&self.database, &approval, pattern) {
            return permission_failure(error);
        }
        self.resolve_approval(approval_id, ApprovalDecision::AllowOnce)
    }

    /// `approval_resolve` for an external approval (already routed): record
    /// the decision, write the native one-shot option, settle the card.
    pub fn resolve_approval(&self, approval_id: &str, decision: ApprovalDecision) -> ApiResponse<bool> {
        if decision == ApprovalDecision::AllowConversation {
            return failure("approval.resolve_failed", "外部 Agent 的审批只支持“允许一次”或“拒绝”", false);
        }
        let resolved = match self.database.resolve_approval(approval_id, decision) {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return ApiResponse::success(false),
            Err(error) => return failure("approval.resolve_failed", error, true),
        };
        let approved = resolved.status == "approved";
        let outcome = self.runtime.answer(approval_id, approved);
        let written = matches!(outcome, AnswerOutcome::Written);
        let result = json!({
            "decision": if approved && decision == ApprovalDecision::AllowConversation { "allow_conversation" } else if approved { "allow_once" } else if resolved.status == "expired" { "expired" } else { "deny" },
            "nativeAnswer": match outcome {
                AnswerOutcome::Written => "response_written",
                AnswerOutcome::Unconfirmed => "outcome_unknown",
                AnswerOutcome::NotPending => "not_pending",
                AnswerOutcome::NoAllowOnce => "refused_no_allow_once",
            },
        });
        let key = self
            .database
            .with_connection(|connection| {
                connection.query_row("SELECT runtime_tool_call_id FROM tool_calls WHERE id = ?1", [&resolved.tool_call_id], |row| row.get::<_, String>(0))
            })
            .ok();
        if let Some(key) = key {
            if approved && written {
                if self.database.claim_approved_tool_call(&resolved.run_id, &key).unwrap_or(false) {
                    let _ = self.database.complete_host_tool_call(&resolved.run_id, &key, Some(&result), None);
                }
            } else {
                let message = if approved && outcome == AnswerOutcome::NoAllowOnce {
                    "这个 Agent 没有提供「只允许这一次」选项，已按拒绝处理"
                } else if approved {
                    "已批准，但外部后端已不在等待这次审批（结果未知）"
                } else {
                    approval_outcome_label(&resolved.status)
                };
                let _ = self.database.complete_host_tool_call(&resolved.run_id, &key, Some(&result), Some(message));
            }
        }
        let _ = self.app.emit("fox://approval-resolved", &resolved);
        ApiResponse::success(true)
    }

    /// Stop a conversation's backend in the background (deleted / archived).
    pub fn close_conversation_detached(&self, conversation_id: &str, reason: &'static str) {
        let runtime = self.runtime.clone();
        let conversation_id = conversation_id.to_owned();
        tauri::async_runtime::spawn(async move {
            let _ = runtime.close_conversation(&conversation_id, reason).await;
        });
    }

    /// Application exit: stop every backend, bounded.
    pub fn shutdown_blocking(&self) {
        let runtime = self.runtime.clone();
        let _ = tauri::async_runtime::block_on(async move {
            tokio::time::timeout(std::time::Duration::from_secs(12), runtime.shutdown()).await
        });
    }

    /// The native permission mode the conversation's live process runs in
    /// (None when no process is running).
    pub(crate) fn running_native_mode(&self, conversation_id: &str) -> Option<String> {
        self.runtime.session_info(conversation_id).filter(|info| info.live).and_then(|info| info.native_mode)
    }

    /// The four capability dimensions for a conversation's backend: live
    /// when a session ran in this Fox process, planned otherwise.
    pub fn capabilities(&self, conversation_id: Option<&str>, backend: AgentBackend) -> CapabilityView {
        if let Some(info) = conversation_id.and_then(|id| self.runtime.session_info(id)) {
            if let (Some(init), Some(declaration)) = (info.initialize_response.as_ref(), info.declaration.as_ref()) {
                return capability_view(info.backend, Some(init), declaration, info.pi_launch);
            }
        }
        let launcher = self.runtime.launcher();
        let mode = launcher.config.permission_mode(&LaunchRequest {
            conversation_id: conversation_id.unwrap_or("-").to_owned(),
            backend,
            project_uuid: None,
            project_root: std::path::PathBuf::new(),
            session_key: "planned".into(),
            full_access_granted: conversation_id.is_some_and(|id| self.database.conversation_full_access_granted(id).unwrap_or(false)),
            unified_login: None,
            // Plan C2: the recorded mode follows the conversation's native mode.
            native_permission: conversation_id.and_then(|id| crate::commands::agent_backend_routing::stored_conversation_native_mode(&self.database, id)),
        });
        let pi_launch = launcher.config.pi_launch();
        capability_view(backend, None, &LaunchDeclaration::declare(mode), pi_launch)
    }

    /// Settings batch, item 6: Fox「模型服务」's default provider (base URL,
    /// API type, default model, key from the OS keyring) for the unified
    /// login; `None` when it is missing, disabled or has no key (the login
    /// card stays the fallback). The key never leaves this value except into
    /// the backend's own config / environment.
    fn unified_login(&self, backend: AgentBackend) -> Option<fox_agent_backend::unified_login::UnifiedLogin> {
        crate::external_model::external_login(&self.database, backend)
    }

    /// e2e-1101: settle Pi's pending 「配置来源」 choice before a send (see
    /// [`pi_source_step`]). Defaulting to 模型服务 only scopes this project's
    /// private HOME (`choose_fox(.., remember = false)`) and is audited.
    fn settle_pi_source(&self, target: &AgentBackendTarget) -> Result<(), ApiResponse<()>> {
        let request = self.target_request(target)?;
        let launcher = self.runtime.launcher();
        let config = &launcher.config;
        let home = config
            .private_home_path(fox_agent_backend::external_home::home_project_uuid(request.project_uuid.as_deref()), AgentBackend::Pi)
            .map_err(|error| failure(PI_SOURCE_CHOICE_REQUIRED, format!("无法确定 Pi 的私有目录：{}", error.message), true))?;
        let source = fox_agent_backend::login_source::status(&config.state_root, &home, Self::real_home().as_ref())
            .map_err(|error| failure(PI_SOURCE_CHOICE_REQUIRED, format!("无法读取 Pi 的配置来源：{error}"), true))?;
        let fox_login = self.unified_login(AgentBackend::Pi);
        let implicit_fox = !source.choice_required && source.current == "fox" && !source.fox_chosen;
        match pi_source_step(source.choice_required, source.source_changed, fox_login.is_some(), implicit_fox) {
            PiSourceStep::Proceed => Ok(()),
            PiSourceStep::Refuse => Err(failure(PI_SOURCE_CHOICE_REQUIRED, PI_SOURCE_CHOICE_MESSAGE, true)),
            PiSourceStep::DefaultToFox => {
                fox_agent_backend::login_source::choose_fox(&config.state_root, &home, false)
                    .map_err(|error| failure(PI_SOURCE_CHOICE_REQUIRED, format!("无法记录 Pi 的配置来源：{error}"), true))?;
                let login = fox_login.as_ref();
                let details = json!({
                    "provider": login.map(|login| login.provider_name.clone()),
                    "model": login.map(|login| login.model.clone()),
                    "nativeAvailable": source.native_available,
                    // choice_pending：本机有 Pi 原生配置但没选；no_source_chosen：什么都没配，默认用模型服务。
                    "reason": if source.choice_required { "choice_pending" } else { "no_source_chosen" },
                });
                let _ = self.database.append_agent_backend_audit(target.conversation_id.as_deref(), AgentBackend::Pi.as_str(), "agent_backend.pi_source_defaulted_to_fox", &details);
                Ok(())
            }
        }
    }

    pub(crate) fn model_login_context(&self, target: &AgentBackendTarget) -> Result<(AgentBackend, bool), String> {
        let mut request = self.target_request(target).map_err(|r| r.error.map(|e| e.message).unwrap_or_else(|| "无法读取会话配置".into()))?;
        request.unified_login = None;
        let launcher = self.runtime.launcher();
        let config = &launcher.config;
        let home = config.private_home_path(fox_agent_backend::external_home::home_project_uuid(request.project_uuid.as_deref()), request.backend).map_err(|e| e.message)?;
        if request.backend == AgentBackend::Pi {
            let source = fox_agent_backend::login_source::status(&config.state_root, &home, Self::real_home().as_ref())?;
            if source.choice_required { return Ok((request.backend, true)); }
            if source.reusable { config.apply_unified_login(&request).map_err(|e| e.message)?; }
        }
        let probe = config.credential_probe(&request).map_err(|e| e.message)?;
        let host_login = probe.present_keys.iter().any(|key| key.ends_with("（Host 环境）"));
        Ok((request.backend, fox_agent_backend::unified_login::user_login_present(request.backend, &home, host_login)))
    }

    /// The user changed the conversation's native permission mode: the live
    /// session follows as far as its Agent allows (Codex at once, dsh at the
    /// next turn, pi at the next request).
    pub(crate) async fn apply_native_mode(&self, conversation_id: &str, mode: &str) -> fox_agent_backend::external::NativeModeOutcome {
        self.runtime.apply_native_mode(conversation_id, mode).await
    }

    /// Record a Host-level warning (no conversation) in the external-backend
    /// audit log. Names and counts only.
    pub(crate) fn audit_warning(&self, event: &str, details: &Value) {
        self.audit_host(event, details);
    }

    /// Record a Host-level event (no conversation) that is not a warning,
    /// e.g. a provider deleted together with its Keys (sandbox.md 13.22).
    /// Same log and shape as [`Self::audit_warning`]; names and counts only.
    pub(crate) fn audit_host(&self, event: &str, details: &Value) {
        let _ = self.database.append_agent_backend_audit(None, "-", event, details);
    }

    /// Remove the Fox-managed login files of every project x backend (the
    /// default provider changed or lost its key). Never stops at the first
    /// problem: projects without a canonical UUID (e.g. `legacy-<hex>` ids
    /// of old conversations) have no private HOME and are skipped; every
    /// other failure is collected per project / backend and returned, so the
    /// caller can report it as a warning after its own work is done.
    pub(crate) fn clear_managed_model_files(&self) -> Vec<String> {
        let projects = match self.database.known_projects() {
            Ok(projects) => projects,
            Err(error) => return vec![format!("无法列出项目：{error}")],
        };
        let mut errors = Vec::new();
        for (id, _) in projects {
            if fox_agent_backend::external_home::canonical_project_uuid(&id).is_none() {
                continue;
            }
            for backend in [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek] {
                let result = self
                    .runtime
                    .launcher()
                    .config
                    .private_home_path(&id, backend)
                    .map_err(|e| e.message)
                    .and_then(|home| fox_agent_backend::unified_login::remove_managed(backend, &home).map(|_| ()));
                if let Err(error) = result {
                    errors.push(format!("项目 {id} / {}：{error}", backend.as_str()));
                }
            }
        }
        // 无项目外部会话共用的 HOME 不是 projects 行，换供应商时同样清掉 Fox 代写的登录。
        for backend in [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek] {
            let result = self
                .runtime
                .launcher()
                .config
                .private_home_path(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID, backend)
                .map_err(|e| e.message)
                .and_then(|home| fox_agent_backend::unified_login::remove_managed(backend, &home).map(|_| ()));
            if let Err(error) = result {
                errors.push(format!("未选项目的对话 / {}：{error}", backend.as_str()));
            }
        }
        errors
    }

    pub(crate) fn prepare_model_login(&self, target: &AgentBackendTarget) -> Result<(), String> {
        let request = self.target_request(target).map_err(|r| r.error.map(|e| e.message).unwrap_or_else(|| "无法读取会话配置".into()))?;
        self.runtime.launcher().config.apply_unified_login(&request).map_err(|e| e.message)?;
        Ok(())
    }

    /// The launch request a turn of `conversation_id` would use (backend,
    /// project, grant), for the probe / import / revoke entries.
    fn planned_request(&self, conversation_id: &str) -> Result<LaunchRequest, ApiResponse<()>> {
        let backend = match self.database.conversation_agent_backend(conversation_id) {
            Ok(Some(stored)) => match AgentBackend::parse(&stored) {
                Some(AgentBackend::Fox) | None => return Err(failure("agent_backend_invalid", "这不是外部 Agent 后端的会话", false)),
                Some(backend) => backend,
            },
            Ok(None) => return Err(failure("conversation.not_found", "会话不存在", false)),
            Err(error) => return Err(failure("agent_backend_lookup_failed", error, true)),
        };
        let context = match self.database.external_conversation_context(conversation_id) {
            Ok(Some(context)) => context,
            Ok(None) => return Err(failure("conversation.not_found", "会话不存在", false)),
            Err(error) => return Err(failure("storage.operation_failed", error, true)),
        };
        let project_root = match authorized_root(&self.data_dir, conversation_id, context.project_id.as_deref(), context.project_root.as_deref()) {
            Ok(root) => root,
            Err(refusal) => return Err(refusal),
        };
        Ok(LaunchRequest {
            conversation_id: conversation_id.to_owned(),
            backend,
            project_uuid: Some(fox_agent_backend::external_home::home_project_uuid(context.project_id.as_deref()).to_owned()),
            project_root,
            session_key: "planned".into(),
            full_access_granted: self.database.conversation_full_access_granted(conversation_id).unwrap_or(false),
            unified_login: self.unified_login(backend),
            native_permission: crate::commands::agent_backend_routing::stored_conversation_native_mode(&self.database, conversation_id),
        })
    }

    /// The launch request of a NEW conversation of `backend` in a project
    /// (no conversation row yet): what its first turn would use.
    fn draft_request(&self, target: &AgentBackendTarget) -> Result<LaunchRequest, ApiResponse<()>> {
        let backend = match target.backend.as_deref().and_then(AgentBackend::parse) {
            Some(AgentBackend::Fox) | None => return Err(failure("agent_backend_invalid", "只有外部 Agent 后端需要登录", false)),
            Some(backend) => backend,
        };
        let project = match self.database.project_by_id_or_root(target.project_id.as_deref(), target.project_root.as_deref()) {
            Ok(Some(project)) => project,
            Ok(None) if project_is_bound(target.project_id.as_deref(), target.project_root.as_deref()) => {
                // 点名了一个项目但库里没有：不能改落到默认目录。
                return Err(failure(
                    "agent_backend_project_required",
                    project_inaccessible_message(target.project_id.as_deref(), target.project_root.as_deref()),
                    false,
                ));
            }
            Ok(None) => {
                // 草稿还没有会话 id，不建具体会话目录。父目录必须存在：
                // model_endpoint::resolve 对不存在的文件返回 NotFound，不会报错；
                // 但若这条路径本身是文件，读取会失败。这里保证它是目录。
                let root = crate::data_paths::conversation_workspaces_root(&self.data_dir);
                if let Err(reason) = crate::data_paths::ensure_directory(&root, "默认对话目录") {
                    return Err(failure("agent_backend_default_workspace_unavailable", format!("默认对话目录不可用：{reason}"), false));
                }
                return Ok(LaunchRequest {
                    conversation_id: "draft".to_owned(),
                    backend,
                    project_uuid: Some(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID.to_owned()),
                    project_root: root,
                    session_key: "planned".into(),
                    full_access_granted: target.full_access,
                    unified_login: self.unified_login(backend),
                    native_permission: crate::commands::agent_backend_routing::stored_last_native_mode(&self.database, backend),
                });
            }
            Err(error) => return Err(failure("storage.operation_failed", error, true)),
        };
        Ok(LaunchRequest {
            conversation_id: "draft".to_owned(),
            backend,
            project_uuid: Some(project.0),
            project_root: std::path::PathBuf::from(project.1),
            session_key: "planned".into(),
            full_access_granted: target.full_access,
            unified_login: self.unified_login(backend),
            native_permission: crate::commands::agent_backend_routing::stored_last_native_mode(&self.database, backend),
        })
    }

    /// An existing conversation's request, else a draft's. AP-4: only for a
    /// backend that may run ([`fox_agent_backend::host_routing::credential_entry_gate`]):
    /// a retired, hidden or switched-off backend gets no login copied into
    /// its private HOME, no model key written, no probe and no folder.
    fn target_request(&self, target: &AgentBackendTarget) -> Result<LaunchRequest, ApiResponse<()>> {
        let existing = target.conversation_id.as_deref().is_some_and(|id| !id.is_empty());
        let request = match target.conversation_id.as_deref().filter(|id| !id.is_empty()) {
            Some(id) => self.planned_request(id),
            None => self.draft_request(target),
        }?;
        credential_gate(request.backend, existing)?;
        Ok(request)
    }

    /// Login probe (UX batch ①): names only, never a value. Works before
    /// the first message of a new conversation (draft target).
    pub fn credential_probe(&self, target: &AgentBackendTarget) -> ApiResponse<fox_agent_backend::onboarding::CredentialProbe> {
        let request = match self.target_request(target) {
            Ok(request) => request,
            Err(refusal) => return retype(refusal),
        };
        match self.runtime.launcher().config.credential_probe(&request) {
            Ok(probe) => ApiResponse::success(probe),
            Err(error) => failure(&error.code, error.message, false),
        }
    }

    fn real_home() -> Option<fox_agent_backend::onboarding::RealHome> {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).filter(|value| !value.is_empty())?;
        let codex_home = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()).map(std::path::PathBuf::from);
        Some(fox_agent_backend::onboarding::RealHome { home: std::path::PathBuf::from(home), codex_home })
    }

    /// What the real HOME has for this backend (file / key names only).
    pub fn import_candidates(&self, target: &AgentBackendTarget) -> ApiResponse<Vec<fox_agent_backend::onboarding::ImportItem>> {
        let request = match self.target_request(target) {
            Ok(request) => request,
            Err(refusal) => return retype(refusal),
        };
        let Some(real) = Self::real_home() else {
            return failure("agent_backend_import_failed", "找不到当前用户的 HOME 目录", false);
        };
        ApiResponse::success(fox_agent_backend::onboarding::import_candidates(request.backend, &real))
    }

    /// Copy the confirmed items into this project's private HOME, or every
    /// known project's ("套用到我所有项目"); one audit event, names only. A
    /// login already there (this project included) is kept unless
    /// `overwrite` ("覆盖已有登录"), which backs it up first.
    pub fn import_credentials(&self, target: &AgentBackendTarget, selected: &[String], all_projects: bool, overwrite: bool, reuse_for_new_projects: bool) -> ApiResponse<fox_agent_backend::onboarding::ImportReport> {
        let request = match self.target_request(target) {
            Ok(request) => request,
            Err(refusal) => return retype(refusal),
        };
        let Some(real) = Self::real_home() else {
            return failure("agent_backend_import_failed", "找不到当前用户的 HOME 目录", false);
        };
        let known = match self.database.known_projects() {
            Ok(known) => known,
            Err(error) => return failure("storage.operation_failed", error, true),
        };
        let projects = import_home_targets(all_projects, request.project_uuid.as_deref(), &known);
        let snapshot = if reuse_for_new_projects && request.backend == AgentBackend::Pi {
            match fox_agent_backend::login_source::Snapshot::capture(&real, selected) {
                Ok(snapshot) => Some(snapshot),
                Err(error) => return failure("agent_backend_source_failed", error, false),
            }
        } else { None };
        match self.runtime.launcher().config.import_credentials(request.backend, &projects, &real, selected, overwrite) {
            Ok(report) => {
                if let Some(snapshot) = snapshot {
                    if !snapshot.matches(&real.home) { return failure("agent_backend_source_changed", "配置已导入，但原生来源已变化；新项目复用未启用，请重新确认", true); }
                    let current_home = self.runtime.launcher().config.private_home_path(fox_agent_backend::external_home::home_project_uuid(request.project_uuid.as_deref()), request.backend);
                    if !current_home.is_ok_and(|home| snapshot.matches(&home)) {
                        return failure("agent_backend_source_not_copied", "已有文件被保留，新项目复用尚未启用。要切换到原生配置，请勾选覆盖已有登录后重新导入；覆盖前会备份。", true);
                    }
                    if let Err(error) = snapshot.remember(&self.runtime.launcher().config.state_root) {
                        return failure("agent_backend_source_failed", error, true);
                    }
                }
                let mut details = report.audit_event();
                details["allProjects"] = json!(all_projects);
                let conversation = target.conversation_id.as_deref().filter(|id| !id.is_empty());
                let _ = self.database.append_agent_backend_audit(conversation, request.backend.as_str(), "agent_backend.credentials_imported", &details);
                ApiResponse::success(report)
            }
            Err(error) => failure(&error.code, error.message, false),
        }
    }

    pub fn choose_fox_source(&self, target: &AgentBackendTarget, remember: bool) -> ApiResponse<()> {
        let request = match self.target_request(target) { Ok(request) => request, Err(refusal) => return refusal };
        if request.backend != AgentBackend::Pi { return failure("agent_backend_invalid", "此选择仅用于 Pi", false); }
        let config = &self.runtime.launcher().config;
        let result = config.private_home_path(fox_agent_backend::external_home::home_project_uuid(request.project_uuid.as_deref()), request.backend)
            .map_err(|e| e.message)
            .and_then(|home| fox_agent_backend::login_source::choose_fox(&config.state_root, &home, remember));
        match result { Ok(()) => ApiResponse::success(()), Err(error) => failure("agent_backend_source_failed", error, true) }
    }

    /// Open the private HOME in the system file manager (created when
    /// missing with the backend's own home inside, e.g. `.codex`; Windows
    /// through the same private-root preparation as a launch).
    pub fn open_private_home(&self, target: &AgentBackendTarget) -> ApiResponse<String> {
        let request = match self.target_request(target) {
            Ok(request) => request,
            Err(refusal) => return retype(refusal),
        };
        let private = match self.runtime.launcher().config.private_dirs(&request) {
            Ok(private) => private,
            Err(error) => return failure(&error.code, error.message, false),
        };
        if let Err(error) = fox_agent_backend::external_home::ensure_backend_home_dirs(request.backend, &private.private_home) {
            return failure("agent_backend_open_failed", format!("无法创建私有 HOME：{error}"), false);
        }
        match crate::app_capabilities::open_folder(&private.private_home) {
            Ok(()) => ApiResponse::success(private.private_home.to_string_lossy().into_owned()),
            Err(error) => failure("agent_backend_open_failed", error, false),
        }
    }

    /// Permission mode of a conversation and whether full access can be
    /// chosen on this machine (new-conversation picker, lock chip).
    pub fn permission_status(&self, conversation_id: Option<&str>) -> Value {
        let config = &self.runtime.launcher().config;
        let forbidden = config.full_access_forbidden;
        let mut status = json!({
            "fullAccessForbidden": forbidden,
            "forbiddenReason": if forbidden { Some(fox_agent_backend::external_launch::FULL_ACCESS_FORBIDDEN_REASON) } else { None },
            "engineeringSwitch": config.full_access != fox_agent_backend::external_launch::FullAccessSwitch::Off,
        });
        if let Some(id) = conversation_id {
            if let Ok(request) = self.planned_request(id) {
                status["granted"] = json!(request.full_access_granted);
                status["mode"] = json!(config.permission_mode(&request).as_str());
                status["locked"] = json!(true);
            }
        }
        status
    }

    /// Revoke full access mid-session (plan C2: the Agent's own full-access
    /// mode). The conversation's native mode is set back to the Agent's last
    /// restricted choice (else its default) first, and any legacy grant row
    /// is removed, so a relaunch can only be without full access. Then the
    /// live session follows the Agent's way: Codex at once (`session/set_mode`),
    /// pi from its next request, dsh is stopped
    /// (`recheck_permission_mode` -> RestartWithoutFullAccess) and restarts at
    /// the next turn.
    pub async fn revoke_full_access(&self, conversation_id: &str) -> ApiResponse<Value> {
        use fox_agent_backend::native_permission as native;
        let request = match self.planned_request(conversation_id) {
            Ok(request) => request,
            Err(refusal) => return retype(refusal),
        };
        let native_full = native::is_full_access(request.backend, request.native_mode());
        let legacy_grant = match self.database.revoke_full_access_grant(conversation_id) {
            Ok(removed) => removed,
            Err(error) => return failure("storage.operation_failed", error, true),
        };
        if !native_full && !legacy_grant {
            return failure("agent_backend_no_full_access", "本会话没有使用完全访问", false);
        }
        let context = request.native_context();
        let restricted = crate::commands::agent_backend_routing::stored_last_native_mode(&self.database, request.backend)
            .filter(|mode| !native::is_full_access(request.backend, mode))
            .map(|mode| native::effective_mode(request.backend, Some(&mode), false, &context))
            .unwrap_or_else(|| native::catalog(request.backend, &context).default_mode);
        if native_full {
            if let Err(error) = self.database.set_app_setting(&format!("{}{conversation_id}", crate::commands::agent_backend_routing::NATIVE_MODE_CONVERSATION_PREFIX), restricted) {
                return failure("storage.operation_failed", error, true);
            }
        }
        let request = LaunchRequest { full_access_granted: false, native_permission: Some(restricted.to_owned()), ..request };
        let outcome = self.runtime.revoke_full_access(&request).await;
        let (label, confirmed, error) = match outcome {
            RevokeOutcome::NotRunning => ("not_running", true, None),
            RevokeOutcome::Unchanged => ("unchanged", true, None),
            RevokeOutcome::RestartWithoutFullAccess { confirmed } => ("restart_without_full_access", confirmed, None),
            RevokeOutcome::SwitchedLive => ("switched_live", true, None),
            RevokeOutcome::SwitchFailed(message) => ("switch_failed", true, Some(message)),
            // L-1: revoked while the process was starting; a full-access
            // process is stopped before any prompt reaches it.
            RevokeOutcome::LaunchAborted => ("launch_aborted", true, None),
        };
        let backend = self.database.conversation_agent_backend(conversation_id).ok().flatten().unwrap_or_default();
        let details = json!({ "outcome": label, "stopConfirmed": confirmed, "to": restricted, "error": error });
        let _ = self.database.append_agent_backend_audit(Some(conversation_id), &backend, "agent_backend.full_access_revoked", &details);
        ApiResponse::success(details)
    }
}

/// AP-4: refuse a credential / private-HOME entry for a backend that may not
/// run, with the routing refusal's own code (`agent_backend_disabled`, ...).
pub(crate) fn credential_gate(backend: AgentBackend, existing_conversation: bool) -> Result<(), ApiResponse<()>> {
    fox_agent_backend::host_routing::credential_entry_gate(backend, &fox_agent_backend::host_routing::BackendSwitch::from_env(), existing_conversation)
        .map_err(|refusal| failure(refusal.code(), refusal.message(), refusal.retryable()))
}

#[cfg(test)]
mod tests {
    use super::{approval_outcome_label, authorized_root, external_permission_request, import_home_targets};
    use fox_agent_backend::projection::{PermissionCard, PermissionOption};
    use serde_json::json;

    /// e2e-1101: the composer's backend bar / login cards are gone (UI 1103);
    /// every model-connection refusal of an external Agent points at
    /// 「设置 → 模型服务」 and names the Agent where the address is chosen.
    #[test]
    fn login_gap_refusals_point_at_model_settings_never_at_removed_ui() {
        use crate::external_model::LoginGap;
        use fox_agent_backend::port::AgentBackend;
        for backend in [AgentBackend::Pi, AgentBackend::Codex, AgentBackend::Deepseek] {
            for gap in [None, Some(LoginGap::Missing), Some(LoginGap::Unverified), Some(LoginGap::ChooseAddress)] {
                let (code, message) = super::login_gap_failure(gap, backend);
                assert!(code.starts_with("model.connection_"), "{code}");
                for removed in ["后端栏", "Agent 栏", "登录卡片"] {
                    assert!(!message.contains(removed), "{message}");
                }
                assert!(message.contains("设置 → 模型服务"), "{message}");
            }
        }
        let (code, message) = super::login_gap_failure(Some(LoginGap::ChooseAddress), AgentBackend::Deepseek);
        assert_eq!(code, "model.connection_choose_address");
        assert!(message.contains("用作 DeepSeek 默认协议"), "{message}");
        assert_eq!(super::login_gap_failure(Some(LoginGap::Unverified), AgentBackend::Pi).0, "model.connection_unverified");
        assert_eq!(super::login_gap_failure(Some(LoginGap::Missing), AgentBackend::Pi).0, "model.connection_required");
        assert!(!super::PI_SOURCE_CHOICE_MESSAGE.contains("Agent 栏"));
    }

    /// e2e-1101: a Pi source choice never made defaults to 模型服务 when it can
    /// serve Pi; a changed confirmed source or no usable 模型服务 refuses the
    /// send before a Run exists; nothing pending proceeds.
    #[test]
    fn pi_source_defaults_to_fox_only_when_never_chosen_and_fox_is_ready() {
        use super::{pi_source_step, PiSourceStep};
        assert_eq!(pi_source_step(false, false, false, false), PiSourceStep::Proceed);
        assert_eq!(pi_source_step(false, true, true, false), PiSourceStep::Proceed);
        assert_eq!(pi_source_step(true, false, true, false), PiSourceStep::DefaultToFox);
        assert_eq!(pi_source_step(true, false, false, false), PiSourceStep::Refuse);
        assert_eq!(pi_source_step(true, true, true, false), PiSourceStep::Refuse, "a changed confirmed import is never overridden");
        // 13.38（B3-4）：什么来源都没选、本机也没有 Pi 配置，模型服务可用：默认到模型服务并写审计。
        assert_eq!(pi_source_step(false, false, true, true), PiSourceStep::DefaultToFox);
        assert_eq!(pi_source_step(false, false, false, true), PiSourceStep::Proceed, "模型服务不可用时照旧（之后由缺凭据拒绝）");
    }

    #[test]
    fn expired_approvals_read_expired_not_user_denied() {
        assert_eq!(approval_outcome_label("expired"), "已过期");
        assert_eq!(approval_outcome_label("denied"), "用户拒绝");
        assert_eq!(approval_outcome_label("cancelled"), "已取消");
        assert_ne!(approval_outcome_label("expired"), approval_outcome_label("denied"));
    }

    fn card(decisions: Vec<&'static str>, host_policy: Option<serde_json::Value>) -> PermissionCard {
        PermissionCard {
            title: "Fox approval: bash".into(),
            native_tool_call_id: Some("pi-ui-1".into()),
            tool: "agent_tool",
            input: json!({ "method": "confirm", "message": "not-the-command" }),
            options: vec![PermissionOption { option_id: "yes".into(), name: "Yes".into(), kind: "allow_once".into() }],
            available_decisions: decisions,
            host_policy,
        }
    }

    #[test]
    fn an_eligible_pi_card_inserts_allow_project_rule_between_the_native_choices() {
        let policy = json!({
            "mode": "ask",
            "class": "command",
            "risk": "low",
            "tool": "run_command",
            "verdict": "ask",
            "projectRule": { "eligible": true, "tool": "run_command", "suggestedPrefix": "npm test" },
            "command": "npm test -- foo",
            "cwd": "/work/proj",
        });
        let request = external_permission_request("pi", "pi", &card(vec!["allow_once", "deny"], Some(policy)));
        assert_eq!(request["availableDecisions"], json!(["allow_once", "allow_project_rule", "deny"]));
        assert_eq!(request["projectRule"]["eligible"], true);
        assert_eq!(request["projectRule"]["suggestedPrefix"], "npm test");
        assert_eq!(request["command"], "npm test -- foo");
        assert_eq!(request["cwd"], "/work/proj");
        assert_eq!(request["hostExecution"], serde_json::Value::Null);
        assert!(request["policy"].get("projectRule").is_none());
        assert!(request["policy"].get("command").is_none());
        assert_eq!(request["policy"]["risk"], "low");
        assert_eq!(request["input"]["message"], "not-the-command");
    }

    #[test]
    fn a_card_without_an_eligible_project_rule_is_unchanged() {
        let plain = external_permission_request("Codex", "codex", &card(vec!["allow_once", "deny"], None));
        assert_eq!(plain["availableDecisions"], json!(["allow_once", "deny"]));
        assert!(plain.get("projectRule").is_none());
        assert!(plain.get("policy").is_none());
        assert!(plain.get("cwd").is_none());
        assert!(plain.get("hostExecution").is_none());
        assert!(plain["command"].is_null());

        let ineligible = json!({
            "projectRule": { "eligible": false, "tool": "run_command" },
            "command": "npm test",
            "cwd": "/work/proj",
        });
        let request = external_permission_request("pi", "pi", &card(vec!["allow_once", "deny"], Some(ineligible)));
        assert_eq!(request["availableDecisions"], json!(["allow_once", "deny"]));
        assert!(request.get("projectRule").is_none());
        assert!(request.get("hostExecution").is_none());
        assert!(request["command"].is_null(), "不 eligible 时仍用卡片入参里的 command");
    }

    /// 插入「本项目总是允许」时必须留下「只允许这一次」，不能把 allow_once 挤掉。
    #[test]
    fn inserting_allow_project_rule_keeps_allow_once() {
        let policy = json!({
            "projectRule": { "eligible": true, "tool": "run_command", "suggestedPrefix": "dir" },
            "command": "cd \"D:/fox-test/测试项目\" && dir 2>&1; echo \"EXIT_CODE: $?\"",
            "cwd": "D:/fox-test/测试项目",
        });
        let request = external_permission_request("pi", "pi", &card(vec!["allow_once", "deny"], Some(policy)));
        let decisions = request["availableDecisions"].as_array().expect("decisions");
        assert_eq!(decisions.first().and_then(|item| item.as_str()), Some("allow_once"));
        assert!(decisions.iter().any(|item| item == "allow_once"));
        assert!(decisions.iter().any(|item| item == "allow_project_rule"));
        assert_eq!(request["availableDecisions"], json!(["allow_once", "allow_project_rule", "deny"]));
    }

    #[test]
    fn agent_backend_target_deserializes_without_a_project() {
        let target: super::AgentBackendTarget = serde_json::from_str(r#"{"backend":"codex","fullAccess":true}"#).unwrap();
        assert_eq!(target.backend.as_deref(), Some("codex"));
        assert!(target.conversation_id.is_none());
        assert!(target.project_id.is_none());
        assert!(target.project_root.is_none());
        assert!(target.full_access);
    }

    #[test]
    fn import_all_projects_includes_the_default_home_label() {
        let known = vec![("8f14e45f-ceea-467a-9a36-dedd4bea2543".to_owned(), "主项目".to_owned())];
        let all = import_home_targets(true, None, &known);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0], ("8f14e45f-ceea-467a-9a36-dedd4bea2543".to_owned(), "主项目".to_owned()));
        assert_eq!(
            all[1],
            (fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID.to_owned(), "未选项目的对话".to_owned())
        );
        let one = import_home_targets(false, Some(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID), &known);
        assert_eq!(one, vec![(fox_agent_backend::external_home::DEFAULT_CONVERSATIONS_HOME_ID.to_owned(), "未选项目的对话".to_owned())]);
        let named = import_home_targets(false, Some("8f14e45f-ceea-467a-9a36-dedd4bea2543"), &known);
        assert_eq!(named, vec![("8f14e45f-ceea-467a-9a36-dedd4bea2543".to_owned(), "主项目".to_owned())]);
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("fox-ux1101-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// 无项目会话的授权目录是默认对话目录且已创建；有项目仍是项目根；项目文件夹被删掉仍拒绝。
    #[test]
    fn authorized_root_is_the_default_workspace_without_a_project_and_the_project_folder_with_one() {
        let database = crate::database::Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let data = temp_dir("data");
        let bare = database
            .create_conversation_with_backend(database.default_agent_id(), Some("no-project"), None, None, "pi")
            .expect("bare");
        let context = database.external_conversation_context(&bare.id).expect("context").expect("row");
        assert!(context.project_id.is_none());
        assert!(context.project_root.is_none());
        let root = authorized_root(&data, &bare.id, context.project_id.as_deref(), context.project_root.as_deref()).expect("default workspace");
        let expected = crate::data_paths::default_conversation_workspace(&data, &bare.id).canonicalize().expect("created");
        assert_eq!(root, expected);
        assert!(root.is_dir());
        let other = database
            .create_conversation_with_backend(database.default_agent_id(), Some("other"), None, None, "codex")
            .expect("other");
        let other_root = authorized_root(&data, &other.id, None, None).expect("other workspace");
        assert_ne!(root, other_root);
        assert!(other_root.ends_with(&other.id));

        let project = temp_dir("project");
        let project_text = project.to_str().expect("utf-8 project").to_owned();
        let with_project = database
            .create_conversation_with_backend(database.default_agent_id(), Some("with-project"), Some(&project_text), Some("ask"), "pi")
            .expect("project conversation");
        let bound = database.external_conversation_context(&with_project.id).expect("bound context").expect("bound row");
        let project_root = authorized_root(&data, &with_project.id, bound.project_id.as_deref(), bound.project_root.as_deref()).expect("project root");
        assert_eq!(project_root, project.canonicalize().expect("project canonical"));

        std::fs::remove_dir_all(&project).expect("delete project");
        let missing = authorized_root(&data, &with_project.id, bound.project_id.as_deref(), bound.project_root.as_deref()).expect_err("deleted project");
        let error = missing.error.expect("api error");
        assert_eq!(error.code, "agent_backend_project_required");
        assert!(error.message.starts_with("项目文件夹不可访问："), "{}", error.message);
        assert!(error.message.contains(&project_text), "{}", error.message);

        let unsafe_id = authorized_root(&data, "..", None, None).expect_err("path escape");
        assert_eq!(unsafe_id.error.expect("escape error").code, "agent_backend_default_workspace_unavailable");

        let blocked = temp_dir("blocked");
        std::fs::write(blocked.join("conversation-workspaces"), b"x").unwrap();
        let unavailable = authorized_root(&blocked, &bare.id, None, None).expect_err("file in the way");
        let unavailable = unavailable.error.expect("unavailable");
        assert_eq!(unavailable.code, "agent_backend_default_workspace_unavailable");
        assert!(unavailable.message.starts_with("默认对话目录不可用："), "{}", unavailable.message);

        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&blocked);
    }
}
