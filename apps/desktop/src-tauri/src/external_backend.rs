//! Thin Host glue for the external Agent backends (pi / Codex / DeepSeek over
//! ACP; external backend wiring batch). Every decision lives in
//! `fox_agent_backend::external` (runtime: start, stream, approvals, cancel,
//! crash, close) and `fox_agent_backend::external_launch` (managed runtime,
//! permission mode, srt). This file only:
//!
//! * persists + publishes runtime events exactly like the yuxi runtime
//!   (`record_external_event` -> `fox://runtime-event`), so the existing
//!   projection, thread and tool cards are reused;
//! * turns a native permission request into the existing approval card (a
//!   Host tool call `agent_permission` pending approval + `approvals` row +
//!   `fox://approval-requested`) and settles that row after the answer;
//! * offers the commands' entry points (start / cancel / approve / close /
//!   capabilities) and the startup / exit hooks.
use crate::database::{ApiResponse, ApprovalDecision, Database, StartRunRequest, StartRunResult};
use crate::runtime_host::RuntimeEventNotification;
use fox_agent_backend::capability_view::{capability_view, CapabilityView};
use fox_agent_backend::external::{AnswerOutcome, CancelOutcome, ExternalRuntime, ExternalSink, LaunchRequest, TurnRequest};
use fox_agent_backend::external_launch::{HostLaunchConfig, HostLauncher};
use fox_agent_backend::port::AgentBackend;
use fox_agent_backend::projection::PermissionCard;
use fox_agent_backend::sandbox::{HostOs, SandboxDeclaration};
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
}

fn failure<T: serde::Serialize>(code: &str, message: impl Into<String>, retryable: bool) -> ApiResponse<T> {
    ApiResponse::failure(code, message.into(), retryable)
}

impl ExternalBackendHost {
    pub fn new(app: AppHandle, database: Database, fox_data: &std::path::Path) -> Self {
        let config = HostLaunchConfig::from_env(fox_data);
        if let Err(error) = fox_agent_backend::external_launch::recover_at_startup(&config) {
            eprintln!("fox.external startup_recovery_failed={error:?}");
        }
        let sink = DesktopSink { app: app.clone(), database: database.clone(), event_lock: Arc::new(Mutex::new(())) };
        let runtime = ExternalRuntime::new(sink, HostLauncher::new(config), tauri::async_runtime::handle().inner().clone());
        let host = Self { runtime, database, app };
        host.interrupt_runs_left_active();
        host
    }

    /// Runs of external conversations a previous Fox process left
    /// non-terminal: their backend died with that process.
    fn interrupt_runs_left_active(&self) {
        match self.database.external_runs_left_active() {
            Ok(runs) => {
                for run_id in runs {
                    let _ = self.database.mark_run_interrupted(
                        &run_id,
                        "agent_backend_host_restarted",
                        "Fox 已重启，外部后端的这一轮随上次进程结束，结果未知",
                    );
                }
            }
            Err(error) => eprintln!("fox.external left_active_lookup_failed={error:?}"),
        }
    }

    /// `run_start` for an external conversation (already routed).
    pub fn start_run(&self, request: &StartRunRequest, backend: AgentBackend) -> ApiResponse<StartRunResult> {
        if !request.attachment_ids.is_empty() {
            return failure("agent_backend_attachments_unsupported", "外部 Agent 后端暂不支持附件，请只发送文字", false);
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
        let started = match self.database.create_run(&request.conversation_id, &request.text, request.model.as_deref()) {
            Ok(started) => started,
            Err(error) => return failure("run.create_failed", error, false),
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
            "decision": if approved { "allow_once" } else if resolved.status == "expired" { "expired" } else { "deny" },
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
        });
        let pi_launch = launcher.config.pi_launch();
        capability_view(backend, None, &SandboxDeclaration::declare(mode, None, HostOs::current()), pi_launch)
    }
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
