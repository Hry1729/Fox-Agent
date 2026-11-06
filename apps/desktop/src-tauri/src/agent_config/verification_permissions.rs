use super::now_ms;
use crate::{
    app_state::AppState,
    database::{ApiResponse, ApprovalRecord},
    external_policy,
};
use fox_agent_backend::{
    external::permission_decision,
    native_permission::display_name,
    port::AgentBackend,
    projection::{allow_option, backend_permission_card, reject_option, AutoPermission},
    verify_session::VerificationApprovals,
};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, path::Path, sync::Mutex};
use tauri::{ipc::Channel, State};
use tokio::sync::oneshot;

pub(super) type PendingApprovals = Mutex<HashMap<AgentBackend, (String, oneshot::Sender<bool>)>>;

pub(super) struct SettingsApprovals<'a> {
    pub backend: AgentBackend,
    pub mode: &'static str,
    pub channel: Channel<ApprovalRecord>,
    pub pending: &'a PendingApprovals,
}

impl VerificationApprovals for SettingsApprovals<'_> {
    async fn answer(&self, project: &Path, request: &Value) -> Result<Option<String>, String> {
        let mut card = backend_permission_card(self.backend, request);
        let home = crate::runtime_host::approval_policy::host_home();
        // A verification workspace is disposable and has no saved project rules.
        let host = if self.backend == AgentBackend::Pi {
            let (tool, decision) =
                external_policy::decide_pi(self.mode, request, Some(project), home.as_deref(), &[]);
            Some(external_policy::answer_from(
                &tool,
                &decision,
                &card.options,
                false,
                None,
                None,
            ))
        } else {
            external_policy::native_block_decision(
                self.backend,
                self.mode,
                request,
                Some(project),
                home.as_deref(),
                &[],
            )
            .map(|decision| {
                external_policy::answer_from(
                    "run_command",
                    &decision,
                    &card.options,
                    false,
                    None,
                    None,
                )
            })
        };
        let (decision, policy) =
            permission_decision(self.backend, self.mode, request, &card.options, host);
        match decision {
            AutoPermission::Allow { option_id } => Ok(Some(option_id)),
            AutoPermission::Reject { option_id } => Ok(option_id),
            AutoPermission::Ask => {
                card.host_policy = policy;
                let id = uuid::Uuid::new_v4().to_string();
                let scope = project
                    .parent()
                    .and_then(Path::file_name)
                    .and_then(|s| s.to_str())
                    .unwrap_or("verification");
                let approval = ApprovalRecord {
                    id: id.clone(),
                    tool_call_id: card
                        .native_tool_call_id
                        .clone()
                        .unwrap_or_else(|| id.clone()),
                    conversation_id: format!("verify-{scope}"),
                    run_id: scope.into(),
                    tool_name: crate::external_backend::PERMISSION_TOOL_NAME.into(),
                    status: "pending".into(),
                    requested_action: card.title.clone(),
                    request: crate::external_backend::external_permission_request(
                        display_name(self.backend),
                        self.backend.as_str(),
                        &card,
                    ),
                    decision: None,
                    requested_at: now_ms(),
                    resolved_at: None,
                    category: "tool".into(),
                    claimed_at: None,
                    claimed_by_run_id: None,
                };
                let (sender, receiver) = oneshot::channel();
                self.pending
                    .lock()
                    .unwrap()
                    .insert(self.backend, (id, sender));
                self.channel.send(approval).map_err(|e| e.to_string())?;
                let approved = receiver.await.map_err(|_| "验证审批已结束".to_owned())?;
                let option = if approved {
                    allow_option(&card.options)
                } else {
                    reject_option(&card.options)
                };
                Ok(option.map(|item| item.option_id.clone()))
            }
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct AnswerRequest {
    backend: AgentBackend,
    id: String,
    approved: bool,
}

#[tauri::command]
pub(crate) fn agent_config_verify_answer(
    state: State<'_, AppState>,
    request: AnswerRequest,
) -> ApiResponse<bool> {
    let mut pending = state.agent_configuration.pending_approvals.lock().unwrap();
    if pending
        .get(&request.backend)
        .is_none_or(|(id, _)| id != &request.id)
    {
        return ApiResponse::success(false);
    }
    let (_, sender) = pending.remove(&request.backend).unwrap();
    ApiResponse::success(sender.send(request.approved).is_ok())
}
