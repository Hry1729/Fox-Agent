//! Canonical model-view form of mid-run steering messages.
//!
//! The exact wrapper text is mirrored by the engine worker
//! (`steeringNoticeText` in pi-kernel-loop.mjs): the Host builds the same
//! wording when it records durable history, the worker when it splices a
//! directive into its live transcript. Steering is ordinary user text — it
//! never carries tools, grants or replayable writes.

use fox_engine_protocol::KernelSteeringNotice;

use crate::database::SteeringMessage;

pub(super) fn steering_notice_text(content: &str) -> String {
    format!(
        "用户在运行过程中补充要求（不改变已有授权，按既有工具策略执行）：\n{content}"
    )
}

pub(super) fn steering_user_message(row: &SteeringMessage) -> serde_json::Value {
    super::bound_steering_user_message(&row.content,row.received_at)
}

pub(super) fn steering_notices(rows: &[SteeringMessage]) -> Vec<KernelSteeringNotice> {
    rows.iter()
        .map(|row| KernelSteeringNotice {
            message_id: row.message_id.clone(),
            content: row.content.clone(),
            received_at: Some(row.received_at),
        })
        .collect()
}

/// Bounded number of follow-up rounds the Host dedicates to additional user
/// input accepted mid-run. A separate budget from the stop-review: new user
/// input must neither consume nor be starved by the review of the model's own
/// stop. The receipt boundary refuses new text once this is spent, so the two
/// sides can never disagree about whether a request can still be answered.
pub(super) const STEERING_FOLLOWUP_LIMIT: i64 = crate::database::MAX_STEERING_FOLLOWUPS;

/// Prompt for a round that answers additional user input received while the Run
/// was in flight. It is new user work, not a review of the model's own stop.
pub(super) const STEERING_PROMPT: &str = "Fox 补充要求处理：用户在本次任务运行期间提交了新的补充要求（见上方）。请在既有授权与工具策略范围内处理这些补充要求，然后给出最终答复；不要重放已经完成的写入操作。";

/// Whether the Host must answer additional user input before it may complete
/// the Run, and the frozen input that round uses.
///
/// `history_before_reply` is the model view of the round that just stopped
/// **without its final assistant reply**: everything the round saw — the durable
/// history, the assistant tool call and the settled tool results. This function
/// appends the refused reply exactly once, then the prompt. Callers must not
/// pre-append the reply, or the same assistant turn lands in the history twice
/// (inflating context, distorting the round correspondence, and feeding the
/// compactor duplicate turns).
///
/// Rebuilding from the Run's initial input would be worse still: it would drop
/// every tool round and analysis in between, so the next request would ask the
/// model to continue from a state it never produced (and invite it to repeat
/// work that already ran).
///
/// Both model transports (the live round loop and the per-round transport) ask
/// this same question from the same durable facts, so the guarantee cannot
/// depend on which transport happened to be running: input the user was told Fox
/// had received is either answered by its own `steering` lane round or the Run
/// does not complete.
pub(super) fn steering_followup_input(
    database: &crate::database::Database,
    binding: &fox_engine_protocol::RunControlBinding,
    history_before_reply: Vec<serde_json::Value>,
    refused_assistant: serde_json::Value,
) -> Result<Option<fox_engine_protocol::KernelInitialModelInput>, String> {
    if database.pending_run_steering(&binding.run_id)?.is_empty() {
        return Ok(None);
    }
    // The ceiling is checked by the caller *before* accepting new input, so a
    // queue that is already at its limit never reaches this point. It is still
    // re-checked here so a follow-up round is never planned beyond the bound.
    if database.kernel_count_steering_followups(&binding.run_id)? >= STEERING_FOLLOWUP_LIMIT {
        return Ok(None);
    }
    let mut input = database.kernel_initial_input(&binding.run_id)?;
    input.messages = history_before_reply;
    input.messages.push(refused_assistant);
    input
        .messages
        .push(serde_json::json!({"role":"user","content":[{"type":"text","text":STEERING_PROMPT}],"timestamp":0}));
    input.validate()?;
    Ok(Some(input))
}

/// Tool-result messages of one settled batch, in source order.
///
/// Single definition shared by the tool-proposal resume and the steering
/// follow-up, so both rounds see byte-identical results.
pub(super) fn settled_tool_result_messages(
    run_id: &str,
    tools: &[fox_engine_protocol::KernelSettledToolResult],
    assistant: &serde_json::Value,
) -> Vec<serde_json::Value> {
    super::KernelCoordinator::tool_result_messages(run_id, assistant, tools)
}
