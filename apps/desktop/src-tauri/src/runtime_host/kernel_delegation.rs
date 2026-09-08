//! Delegation operations create durable records only. Starting or cancelling
//! owned child executors is a post-result action of the parent Kernel loop.
use crate::{
    database::{CreateChildRunInput, Database},
    kernel::CancellationToken,
};
use fox_engine_protocol::RunControlBinding;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

pub(super) const TOOLS: &[&str] = &[
    "child_agent_list",
    "child_run_start",
    "child_run_collect",
    "child_run_cancel",
    "team_snapshot_get",
    "team_start",
    "team_member_start",
    "team_collect",
    "team_cancel",
];

#[derive(Serialize, Deserialize)]
#[serde(tag = "kernelCancel", rename_all = "snake_case", deny_unknown_fields)]
pub(super) enum CancelAction {
    Child { child_run_id: String },
    Team { team_run_id: String, reason: String },
}

fn stage(
    database: &Database,
    run_id: &str,
    tool_id: &str,
    action: &impl Serialize,
) -> Result<(), String> {
    let body = serde_json::to_string(action).map_err(|_| "invalid Kernel delegation action")?;
    database.stage_kernel_host_action(run_id, tool_id, &body)
}

fn create_child(
    database: &Database,
    binding: &RunControlBinding,
    tool_id: &str,
    input: CreateChildRunInput<'_>,
) -> Result<Value, String> {
    let (started, child_run, created) = database.create_child_run(input)?;
    if created {
        stage(
            database,
            &binding.run_id,
            tool_id,
            &super::work_tools::WorkToolPostFinalizeDirective::StartChild(
                super::work_tools::WorkToolChildDispatch {
                    started,
                    child_run: child_run.clone(),
                },
            ),
        )?;
    }
    Ok(json!({"childRun":child_run,"created":created}))
}

pub(super) fn execute(
    database: &Database,
    binding: &RunControlBinding,
    tool_id: &str,
    tool: &str,
    input: &Value,
    token: &CancellationToken,
) -> Result<Value, String> {
    let run_id = binding.run_id.as_str();
    let conversation_id = binding.conversation_id.as_str();
    let text = |field, max| super::required_bounded_text(input, field, max);
    let optional = |field, max| super::optional_bounded_text(input, field, max);
    let remaining = binding
        .budgets
        .run_execution_ms
        .saturating_sub(
            database
                .kernel_build_full_snapshot(run_id)?
                .running_elapsed_ms,
        )
        .min(binding.budgets.tool_execution_ms)
        .max(0) as u64;
    if remaining == 0 {
        return Err("Kernel delegation budget exhausted".into());
    }
    let wait = input
        .get("waitMs")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .min(60_000)
        .min(remaining);
    let deadline = Instant::now() + Duration::from_millis(wait);
    let max_output = || {
        database.kernel_model_config(run_id)?.model_service["maxOutputTokens"]
            .as_i64()
            .filter(|value| *value >= 64)
            .ok_or_else(|| "missing frozen child output limit".to_owned())
    };
    let result = match tool {
        "child_agent_list" => {
            json!({"agents":database.list_child_agents()?,"experts":database.list_consultable_experts()?})
        }
        "child_run_start" => {
            let mode = input
                .get("mode")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("worker");
            let agent_id = match mode {
                "worker" => {
                    if input.get("expertId").is_some() {
                        return Err("worker mode does not accept expertId".into());
                    }
                    let id = input
                        .get("agentId")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("fox-general");
                    if !database
                        .list_child_agents()?
                        .iter()
                        .any(|agent| agent.id == id)
                    {
                        return Err("child worker identity is unavailable".into());
                    }
                    id
                }
                "expert_consultation" => {
                    if input.get("agentId").is_some() {
                        return Err("expert consultation does not accept agentId".into());
                    }
                    let id = text("expertId", 160)?;
                    if !database
                        .list_consultable_experts()?
                        .iter()
                        .any(|agent| agent.id == id)
                    {
                        return Err("consultation expert identity is unavailable".into());
                    }
                    id
                }
                _ => return Err("invalid child delegation mode".into()),
            };
            let mut budget = super::normalized_child_budget(input.get("budget"), max_output()?)?;
            budget.max_duration_ms = budget.max_duration_ms.min(binding.budgets.run_execution_ms);
            create_child(
                database,
                binding,
                tool_id,
                CreateChildRunInput {
                    parent_run_id: run_id,
                    tool_call_id: tool_id,
                    worker_agent_id: agent_id,
                    objective: text("objective", 8_000)?,
                    context: optional("context", 12_000)?,
                    budget: &budget,
                    team_run_id: None,
                    team_member_id: None,
                    allowed_tools: None,
                },
            )?
        }
        "child_run_collect" => {
            let ids = input["childRunIds"]
                .as_array()
                .filter(|ids| !ids.is_empty() && ids.len() <= 8)
                .ok_or("childRunIds must contain 1 to 8 IDs")?;
            let ids = ids
                .iter()
                .map(|id| {
                    id.as_str()
                        .filter(|id| !id.trim().is_empty() && id.len() <= 160)
                        .ok_or("invalid childRunId")
                })
                .collect::<Result<BTreeSet<_>, _>>()?;
            loop {
                token.check()?;
                let children = database
                    .child_runs_for_parent(run_id)?
                    .into_iter()
                    .filter(|child| ids.contains(child.child_run_id.as_str()))
                    .collect::<Vec<_>>();
                if children.len() != ids.len() {
                    return Err("child does not belong to the frozen parent Run".into());
                }
                let all_terminal = children
                    .iter()
                    .all(|child| super::run_status_is_terminal(&child.status));
                if all_terminal || Instant::now() >= deadline {
                    break json!({"childRuns":children,"allTerminal":all_terminal});
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "child_run_cancel" => {
            let id = text("childRunId", 160)?;
            let child = database.child_run(id)?.ok_or("child was not found")?;
            if child.parent_run_id != run_id {
                return Err("child does not belong to the frozen parent Run".into());
            }
            super::ensure_child_cancel_not_graph_bound(database, id)?;
            let requested = !super::run_status_is_terminal(&child.status);
            if requested {
                stage(
                    database,
                    run_id,
                    tool_id,
                    &CancelAction::Child {
                        child_run_id: id.into(),
                    },
                )?;
            }
            json!({"cancelRequested":requested,"childRun":child})
        }
        "team_snapshot_get" => json!({"team":database.latest_expert_team(conversation_id)?}),
        "team_start" => {
            json!({"team":crate::expert_teams::start_team(database,conversation_id,run_id,
            text("objective",8_000)?,optional("context",12_000)?)?})
        }
        "team_member_start" => {
            let snapshot = database
                .active_expert_team(conversation_id)?
                .ok_or("team is not active")?;
            if snapshot.run.parent_run_id != run_id {
                return Err("team parent Run mismatch".into());
            }
            let member_id = text("memberId", 128)?;
            if snapshot
                .members
                .iter()
                .any(|member| member.team_member_id.as_deref() == Some(member_id))
            {
                return Err("team member already started".into());
            }
            let team = crate::expert_teams::parse_team(snapshot.run.team.clone())?;
            let member = team
                .members
                .iter()
                .find(|member| member.id == member_id)
                .ok_or("team member was not found")?;
            if member.budget.max_output_tokens > max_output()? {
                return Err("team budget exceeds the frozen model output limit".into());
            }
            let context = format!("# Expert Team\n{} ({})\n\n# Member identity\n{} — {}\n\n# Frozen member instructions\n{}\n\n# Lead objective\n{}\n\n# Shared bounded context\n{}\n\n# Member-specific bounded context\n{}",
                team.title,team.id,member.name,member.role,member.instructions,snapshot.run.objective,snapshot.run.context,optional("context",12_000)?);
            let mut result = create_child(
                database,
                binding,
                tool_id,
                CreateChildRunInput {
                    parent_run_id: run_id,
                    tool_call_id: tool_id,
                    worker_agent_id: &member.agent_id,
                    objective: text("task", 8_000)?,
                    context: &context,
                    budget: &member.budget,
                    team_run_id: Some(&snapshot.run.id),
                    team_member_id: Some(member_id),
                    allowed_tools: Some(&member.allowed_tools),
                },
            )?;
            result["teamRunId"] = json!(snapshot.run.id);
            result
        }
        "team_collect" => {
            let snapshot = database
                .active_expert_team(conversation_id)?
                .or(database.latest_expert_team(conversation_id)?)
                .ok_or("team was not found")?;
            if snapshot.run.parent_run_id != run_id {
                return Err("team parent Run mismatch".into());
            }
            loop {
                token.check()?;
                let current = database
                    .get_expert_team(&snapshot.run.id)?
                    .ok_or("team was not found")?;
                if current
                    .members
                    .iter()
                    .all(|member| super::run_status_is_terminal(&member.status))
                {
                    break json!({"team":database.finalize_expert_team(&snapshot.run.id)?});
                }
                if Instant::now() >= deadline {
                    break json!({"team":current});
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        "team_cancel" => {
            let snapshot = database
                .active_expert_team(conversation_id)?
                .ok_or("team is not active")?;
            if snapshot.run.parent_run_id != run_id {
                return Err("team parent Run mismatch".into());
            }
            let reason = optional("reason", 2_000)?;
            stage(
                database,
                run_id,
                tool_id,
                &CancelAction::Team {
                    team_run_id: snapshot.run.id.clone(),
                    reason: if reason.is_empty() {
                        "cancelled by Team Lead".into()
                    } else {
                        reason.into()
                    },
                },
            )?;
            json!({"cancelRequested":true,"team":snapshot})
        }
        _ => return Err("unsupported Kernel delegation operation".into()),
    };
    token.check()?;
    Ok(json!({"content":[{"type":"text","text":result.to_string()}],"details":result}))
}
