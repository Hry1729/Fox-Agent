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
        let request = json!({
            "title": format!("允许 {} 执行此操作？", label.unwrap_or("外部 Agent")),
            "summary": card.title,
            "input": card.input,
            "command": card.input.get("command"),
            "availableDecisions": card.available_decisions,
            "agentBackend": backend,
            "nativeToolCallId": card.native_tool_call_id,
            "options": card.options,
            "answerAcknowledged": false,
        });
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

    /// Plan C2: pi is answered by the Host approval engine (same as
    /// PiNative); a failure to consult it is a fail-closed reject, never a
    /// fallback to another rule set. Codex / dsh: their own approval.
    fn host_permission_answer(&self, run_id: &str, conversation_id: &str, backend: AgentBackend, native_mode: &str, request: &Value, options: &[PermissionOption]) -> Option<HostPermissionAnswer> {
        if backend != AgentBackend::Pi {
            return None;
        }
        Some(match crate::external_policy::answer_pi_request(&self.database, run_id, conversation_id, native_mode, request, options) {
            Ok(answer) => answer,
            Err(error) => crate::external_policy::fail_closed(&error, options),
        })
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

/// Carry a refusal over to another payload type.
fn retype<T: serde::Serialize>(refusal: ApiResponse<()>) -> ApiResponse<T> {
    match refusal.error {
        Some(error) => ApiResponse::failure(&error.code, error.message, error.retryable),
        None => ApiResponse::failure("internal", "unexpected empty refusal", false),
    }
}

/// Which private HOME a login probe / import / "打开文件夹" is about: an
/// existing conversation (`conversationId`), or a NEW conversation that has
/// not been created yet (`backend` + `projectId` / `projectRoot`, plus the
/// draft's full-access choice), so the login card can show before the first
/// message (Windows acceptance of 80b1622).
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
        let sink = DesktopSink { app: app.clone(), database: database.clone(), event_lock: Arc::new(Mutex::new(())) };
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
        let project_root = match context.project_root.as_deref().map(std::path::PathBuf::from).map(|root| root.canonicalize()) {
            Some(Ok(root)) if root.is_dir() => root,
            _ => {
                return failure(
                    "agent_backend_project_required",
                    "外部 Agent 后端的会话需要绑定一个可访问的项目文件夹（作为授权目录）",
                    false,
                )
            }
        };
        let target = AgentBackendTarget { conversation_id: Some(request.conversation_id.clone()), backend: None, project_id: None, project_root: None, full_access: false };
        match self.model_login_context(&target) {
            Ok((_, false)) if self.unified_login(backend).is_none() => return failure("model.connection_required", "请先在后端栏补充或验证当前供应商的模型连接", true),
            Err(error) => return failure("model.connection_failed", error, true),
            _ => {}
        }
        let started = match self.database.create_run(&request.conversation_id, &request.text, request.model.as_deref()) {
            Ok(started) => started,
            Err(error) => return failure("run.create_failed", crate::data_paths::run_create_failure(&self.data_dir, &error), false),
        };
        let stored = self.database.latest_native_session(&request.conversation_id).ok().flatten();
        let turn = TurnRequest {
            run_id: started.run.id.clone(),
            prompt: request.runtime_text.clone().unwrap_or_else(|| request.text.clone()),
            stored_native_session: stored,
            launch: LaunchRequest {
                conversation_id: request.conversation_id.clone(),
                backend,
                project_uuid: context.project_id,
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
                let message = if approved {
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

    pub(crate) fn model_login_context(&self, target: &AgentBackendTarget) -> Result<(AgentBackend, bool), String> {
        let mut request = self.target_request(target).map_err(|r| r.error.map(|e| e.message).unwrap_or_else(|| "无法读取会话配置".into()))?;
        request.unified_login = None;
        let launcher = self.runtime.launcher();
        let config = &launcher.config;
        let home = config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend).map_err(|e| e.message)?;
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
        if context.project_id.is_none() {
            return Err(failure("agent_backend_project_required", "外部 Agent 后端的会话需要先绑定项目（私有 HOME 按项目区分）", false));
        }
        Ok(LaunchRequest {
            conversation_id: conversation_id.to_owned(),
            backend,
            project_uuid: context.project_id,
            project_root: context.project_root.map(std::path::PathBuf::from).unwrap_or_default(),
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
            Ok(None) => return Err(failure("agent_backend_project_required", "外部 Agent 后端的会话需要先绑定项目（私有 HOME 按项目区分）", false)),
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
        let projects: Vec<(String, String)> = if all_projects {
            known
        } else {
            request
                .project_uuid
                .clone()
                .into_iter()
                .map(|id| {
                    let label = known.iter().find(|(known_id, _)| known_id == &id).map(|(_, name)| name.clone()).unwrap_or_else(|| id.clone());
                    (id, label)
                })
                .collect()
        };
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
                    let current_home = self.runtime.launcher().config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend);
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
        let result = config.private_home_path(request.project_uuid.as_deref().unwrap_or_default(), request.backend)
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
    use super::approval_outcome_label;

    #[test]
    fn expired_approvals_read_expired_not_user_denied() {
        assert_eq!(approval_outcome_label("expired"), "已过期");
        assert_eq!(approval_outcome_label("denied"), "用户拒绝");
        assert_eq!(approval_outcome_label("cancelled"), "已取消");
        assert_ne!(approval_outcome_label("expired"), approval_outcome_label("denied"));
    }
}
