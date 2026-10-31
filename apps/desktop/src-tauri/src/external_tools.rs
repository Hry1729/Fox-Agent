//! Plan C3: the Host side of the Fox tool MCP bridge
//! (`fox_agent_backend::fox_mcp`). An external Agent (pi / Codex / dsh)
//! calls `present_files`, `list_knowledge_bases`, `search_knowledge`,
//! `read_knowledge_document` or `ask_user`; the bridge hands the call here
//! with the conversation its launch token is bound to. This file:
//!
//! * finds the conversation's active external turn (no turn: refused);
//! * asks the Host approval engine ([`crate::runtime_host::approval_policy`],
//!   the same engine as PiNative) in the tier of the conversation's native
//!   permission mode. These tools only read or declare (they write nothing),
//!   so the engine classes them as reads; a refusal or an "ask" is an error,
//!   never run unasked;
//! * runs the tool through the PiNative entry
//!   ([`crate::runtime_host::execute_external_mcp_tool`]: expert scope,
//!   lifecycle hooks, delivery card / knowledge retrieval), or for `ask_user`
//!   publishes the question card (`user.question.requested`) and marks the
//!   turn so it completes `awaiting_user`;
//! * returns the model-facing result (the bridge caps it at 64 KiB).
use crate::external_backend::DesktopRuntime;
use crate::runtime_host::approval_policy::{self, AccessMode, ToolSubject, Verdict};
use fox_agent_backend::external::ExternalSink;
use fox_agent_backend::fox_mcp::{FoxToolHost, ToolCallContext, ToolError};
use fox_agent_backend::native_permission::{self as native, FoxTier};
use fox_agent_backend::port::AgentBackend;
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Manager};

/// The engine mode of an external Agent's native permission mode (its Fox
/// tier); an unknown mode is 询问, never wider.
pub fn external_access_mode(backend: AgentBackend, native_mode: &str) -> AccessMode {
    match native::tier_of(backend, native_mode) {
        Some(FoxTier::ReadOnly) => AccessMode::ReadOnly,
        Some(FoxTier::Auto) => AccessMode::Auto,
        Some(FoxTier::FullAccess) => AccessMode::FullAccess,
        Some(FoxTier::Ask) | None => AccessMode::Ask,
    }
}

/// The engine subject of a bridge tool: every bridge tool reads (knowledge)
/// or declares (`present_files` records existing files, `ask_user` shows a
/// card); none writes or runs anything.
pub fn bridge_subject(tool: &str) -> Option<ToolSubject> {
    fox_agent_backend::fox_mcp::TOOL_NAMES.contains(&tool).then_some(ToolSubject::Read)
}

/// One engine decision for a bridge call (no I/O).
pub fn decide_bridge_call(backend: AgentBackend, native_mode: &str, tool: &str, project_root: Option<&std::path::Path>, rules: &[approval_policy::PolicyRule]) -> Result<(), ToolError> {
    let subject = bridge_subject(tool).ok_or_else(|| ToolError::new("tool.invalid_input", format!("{tool} is not a Fox bridge tool")))?;
    let home = approval_policy::host_home();
    let decision = approval_policy::decide(&approval_policy::PolicyRequest {
        tool,
        subject: &subject,
        mode: external_access_mode(backend, native_mode),
        project_root,
        home: home.as_deref(),
        rules,
    });
    match decision.verdict {
        Verdict::Allow => Ok(()),
        Verdict::Deny { code, message } => Err(ToolError::new(code, message)),
        // No card path exists for a bridge call: fail closed.
        Verdict::Ask => Err(ToolError::new("permission.approval_required", format!("{tool} 需要逐次审批，外部 Agent 的 Fox 工具调用不支持审批卡，已拒绝（未执行）"))),
    }
}

/// `ask_user` arguments -> the question card's `questions` (the shape the
/// desktop already renders for `user.question.requested`).
pub fn question_card(arguments: &Value) -> Result<Vec<Value>, ToolError> {
    let invalid = |message: &str| ToolError::new("tool.invalid_input", message.to_owned());
    let questions = arguments.get("questions").and_then(Value::as_array).ok_or_else(|| invalid("questions must be a non-empty array"))?;
    if questions.is_empty() || questions.len() > 4 {
        return Err(invalid("ask 1 to 4 questions"));
    }
    questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            let text = question.get("question").and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty()).ok_or_else(|| invalid("every question needs non-empty text"))?;
            if text.chars().count() > 300 {
                return Err(invalid("a question is longer than 300 characters"));
            }
            let options: Vec<Value> = match question.get("options") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(options)) if options.len() <= 8 => options
                    .iter()
                    .map(|option| option.as_str().map(str::trim).filter(|label| !label.is_empty()).map(|label| json!({ "label": label, "value": label })).ok_or_else(|| invalid("options must be non-empty strings")))
                    .collect::<Result<_, _>>()?,
                Some(_) => return Err(invalid("options must be at most 8 strings")),
            };
            let allow_other = question.get("allowOther").and_then(Value::as_bool).unwrap_or(true) || options.is_empty();
            Ok(json!({
                "question_id": format!("q-{}", index + 1),
                "question": text,
                "options": options,
                "multi_select": question.get("multiSelect").and_then(Value::as_bool).unwrap_or(false),
                "allow_other": allow_other,
            }))
        })
        .collect()
}

/// The text `ask_user` returns to the Agent.
pub const ASK_USER_RESULT: &str = "问题卡片已显示给用户。现在结束本轮回复，不要再调用工具或继续行动；用户的回答会作为下一条用户消息发给你。";

/// [`FoxToolHost`] of the desktop.
pub struct DesktopFoxTools {
    app: AppHandle,
    runtime: Arc<OnceLock<DesktopRuntime>>,
}

impl DesktopFoxTools {
    pub fn new(app: AppHandle, runtime: Arc<OnceLock<DesktopRuntime>>) -> Self {
        Self { app, runtime }
    }
}

impl FoxToolHost for DesktopFoxTools {
    fn call_tool(&self, context: ToolCallContext, tool: String, arguments: Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Value, ToolError>> + Send + 'static>> {
        let app = self.app.clone();
        let runtime = self.runtime.get().cloned();
        Box::pin(async move {
            let runtime = runtime.ok_or_else(|| ToolError::new("fox_mcp.host_unavailable", "Fox 外部后端运行时尚未就绪"))?;
            let state = app.try_state::<crate::app_state::AppState>().ok_or_else(|| ToolError::new("fox_mcp.host_unavailable", "Fox Host 尚未就绪"))?;
            let database = state.database.clone();
            let run_id = runtime
                .active_run_of(&context.conversation_id)
                .ok_or_else(|| ToolError::new("fox_mcp.no_active_run", "这个会话当前没有正在执行的一轮，Fox 工具调用已拒绝"))?;
            // The conversation's native mode NOW (a switch mid-turn applies to
            // the next call, as with PiNative).
            let stored = crate::commands::agent_backend_routing::stored_conversation_native_mode(&database, &context.conversation_id);
            let native_mode = native::effective_mode(context.backend, stored.as_deref(), false, &Default::default());
            let project_root = database
                .external_conversation_context(&context.conversation_id)
                .map_err(|error| ToolError::new("storage.operation_failed", error))?
                .and_then(|context| context.project_root)
                .map(|root| {
                    let root = std::path::PathBuf::from(root);
                    root.canonicalize().unwrap_or(root)
                });
            let rules = database.policy_rules_for_conversation(&context.conversation_id).map_err(|error| ToolError::new("storage.operation_failed", error))?;
            decide_bridge_call(context.backend, native_mode, &tool, project_root.as_deref(), &rules)?;
            if tool == "ask_user" {
                let questions = question_card(&arguments)?;
                let event = json!({ "type": "user.question.requested", "source": "ask_user", "agentBackend": context.backend.as_str(), "questions": questions });
                runtime.sink().emit(&run_id, &context.conversation_id, event).map_err(|error| ToolError::new("fox_mcp.question_failed", error))?;
                if !runtime.mark_awaiting_user(&context.conversation_id, &run_id) {
                    return Err(ToolError::new("fox_mcp.no_active_run", "提问时本轮已结束"));
                }
                return Ok(Value::String(ASK_USER_RESULT.to_owned()));
            }
            crate::runtime_host::execute_external_mcp_tool(
                &database,
                &state.yuxi_client,
                &state.local_knowledge,
                &context.conversation_id,
                &run_id,
                &context.tool_call_id,
                &tool,
                &arguments,
            )
            .await
            .map_err(split_code)
        })
    }
}

/// `[code] message` (the Host's tool-error convention) -> a [`ToolError`].
fn split_code(error: String) -> ToolError {
    if let Some(rest) = error.strip_prefix('[') {
        if let Some((code, message)) = rest.split_once(']') {
            if !code.is_empty() && !code.contains(char::is_whitespace) {
                return ToolError::new(code, message.trim());
            }
        }
    }
    ToolError::new("tool.failed", error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approval_policy::{PolicyRule, RuleEffect};

    #[test]
    fn native_modes_map_to_their_fox_tier() {
        assert_eq!(external_access_mode(AgentBackend::Codex, native::CODEX_READ_ONLY), AccessMode::ReadOnly);
        assert_eq!(external_access_mode(AgentBackend::Codex, native::CODEX_ASK), AccessMode::Ask);
        assert_eq!(external_access_mode(AgentBackend::Codex, native::CODEX_FULL), AccessMode::FullAccess);
        assert_eq!(external_access_mode(AgentBackend::Pi, native::PI_ALLOW), AccessMode::Auto);
        assert_eq!(external_access_mode(AgentBackend::Pi, "nonsense"), AccessMode::Ask, "unknown is never wider");
    }

    #[test]
    fn bridge_tools_are_reads_in_every_tier_and_unknown_tools_are_refused() {
        let rules: Vec<PolicyRule> = vec![PolicyRule { id: "b".into(), effect: RuleEffect::Block, tool: "run_command".into(), pattern: "rm".into() }];
        for mode in [native::PI_READ_ONLY, native::PI_ASK, native::PI_ALLOW, native::PI_FULL] {
            for tool in fox_agent_backend::fox_mcp::TOOL_NAMES {
                assert_eq!(decide_bridge_call(AgentBackend::Pi, mode, tool, None, &rules), Ok(()), "{tool} in {mode}");
            }
        }
        assert_eq!(decide_bridge_call(AgentBackend::Codex, native::CODEX_ASK, "run_command", None, &rules).unwrap_err().code, "tool.invalid_input");
    }

    #[test]
    fn ask_user_arguments_become_the_question_card() {
        let card = question_card(&json!({ "questions": [{ "question": " 选哪种格式？ ", "options": ["Word", "PDF"] }, { "question": "截止日期？" }] })).unwrap();
        assert_eq!(card[0]["question"], "选哪种格式？");
        assert_eq!(card[0]["options"][1], json!({ "label": "PDF", "value": "PDF" }));
        assert_eq!(card[0]["question_id"], "q-1");
        assert_eq!(card[1]["allow_other"], true, "a question without options is free text");
        assert!(question_card(&json!({ "questions": [] })).is_err());
        assert!(question_card(&json!({ "questions": [{ "question": "" }] })).is_err());
        assert!(question_card(&json!({ "questions": [{ "question": "q", "options": [1] }] })).is_err());
        assert!(question_card(&json!({})).is_err());
    }

    #[test]
    fn host_error_codes_are_kept() {
        let error = split_code("[local_knowledge.binding_invalid] not enabled".into());
        assert_eq!((error.code.as_str(), error.message.as_str()), ("local_knowledge.binding_invalid", "not enabled"));
        assert_eq!(split_code("plain failure".into()).code, "tool.failed");
    }
}
