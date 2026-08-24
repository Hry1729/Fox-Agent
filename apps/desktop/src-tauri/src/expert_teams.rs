use crate::{
    app_state::AppState,
    database::{
        package_snapshot_hash, ApiResponse, ChildRunBudget, Database, ExpertTeamSnapshot,
        StartExpertTeamInput,
    },
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use tauri::State;

const MAX_TEAM_BYTES: usize = 128 * 1024;
const MAX_MEMBERS: usize = 8;

pub const TEAM_TOOLS: [&str; 5] = [
    "team_snapshot_get",
    "team_start",
    "team_member_start",
    "team_collect",
    "team_cancel",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TeamDefinition {
    pub(crate) schema_version: u32,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
    pub(crate) strategy: TeamStrategy,
    pub(crate) members: Vec<TeamMemberDefinition>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TeamStrategy {
    Supervisor,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TeamMemberDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) agent_id: String,
    pub(crate) role: String,
    pub(crate) instructions: String,
    pub(crate) allowed_tools: Vec<String>,
    #[serde(default = "default_budget")]
    pub(crate) budget: ChildRunBudget,
}

fn default_budget() -> ChildRunBudget {
    ChildRunBudget {
        max_duration_ms: 300_000,
        max_total_tokens: 32_000,
        max_output_tokens: 4_096,
        max_tool_calls: 16,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertTeamConversationRequest {
    conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertTeamCancelRequest {
    team_run_id: String,
    #[serde(default)]
    reason: String,
}

pub fn is_team_tool(tool: &str) -> bool {
    TEAM_TOOLS.contains(&tool)
}

pub(crate) fn validate_team_definition(value: &Value) -> Result<(), String> {
    parse_team(value.clone()).map(|_| ())
}

pub(crate) fn parse_team(value: Value) -> Result<TeamDefinition, String> {
    let encoded = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_TEAM_BYTES {
        return Err(format!("team exceeds the {MAX_TEAM_BYTES} byte limit"));
    }
    let team: TeamDefinition = serde_json::from_value(value)
        .map_err(|error| format!("team does not match schema v1: {error}"))?;
    if team.schema_version != 1 {
        return Err("team schemaVersion must be 1".to_owned());
    }
    validate_id("team.id", &team.id)?;
    Version::parse(&team.version)
        .map_err(|error| format!("team.version is not SemVer: {error}"))?;
    validate_text("team.title", &team.title, 1, 160)?;
    validate_text("team.description", &team.description, 0, 2_000)?;
    if team.strategy != TeamStrategy::Supervisor {
        return Err("team.strategy must be supervisor".to_owned());
    }
    if team.members.is_empty() || team.members.len() > MAX_MEMBERS {
        return Err(format!("team.members must contain 1-{MAX_MEMBERS} members"));
    }
    let mut ids = HashSet::new();
    for member in &team.members {
        validate_id("team member id", &member.id)?;
        if !ids.insert(member.id.as_str()) {
            return Err(format!("duplicate team member id '{}'", member.id));
        }
        validate_text("team member name", &member.name, 1, 120)?;
        validate_id("team member agentId", &member.agent_id)?;
        validate_text("team member role", &member.role, 1, 500)?;
        validate_text("team member instructions", &member.instructions, 1, 8_000)?;
        if member.allowed_tools.len() > 64 {
            return Err("team member allowedTools may contain at most 64 entries".to_owned());
        }
        let mut tools = HashSet::new();
        for tool in &member.allowed_tools {
            if !crate::expert_packages::is_known_tool(tool)
                || !tools.insert(tool.as_str())
                || is_team_tool(tool)
                || matches!(
                    tool.as_str(),
                    "child_agent_list"
                        | "child_run_start"
                        | "child_run_collect"
                        | "child_run_cancel"
                )
            {
                return Err(format!(
                    "team member '{}' has an invalid, duplicate, or delegating tool '{}'",
                    member.id, tool
                ));
            }
        }
        validate_budget(&member.budget)?;
    }
    Ok(team)
}

fn validate_budget(budget: &ChildRunBudget) -> Result<(), String> {
    if !(1_000..=900_000).contains(&budget.max_duration_ms)
        || !(256..=200_000).contains(&budget.max_total_tokens)
        || !(64..=32_768).contains(&budget.max_output_tokens)
        || budget.max_output_tokens > budget.max_total_tokens
        || !(0..=100).contains(&budget.max_tool_calls)
    {
        return Err("team member budget exceeds Host Child Run limits".to_owned());
    }
    Ok(())
}

fn validate_id(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-' | b'_')
        })
    {
        return Err(format!(
            "{field} must use lowercase letters, digits, dot, dash, or underscore"
        ));
    }
    Ok(())
}

fn validate_text(field: &str, value: &str, minimum: usize, maximum: usize) -> Result<(), String> {
    let length = value.trim().chars().count();
    if length < minimum || length > maximum || value.chars().any(char::is_control) {
        return Err(format!(
            "{field} must contain {minimum}-{maximum} visible characters"
        ));
    }
    Ok(())
}

pub(crate) fn team_for_conversation(
    database: &Database,
    conversation_id: &str,
) -> Result<(crate::database::ConversationExpertBinding, TeamDefinition), String> {
    let binding = database
        .current_conversation_expert_binding(conversation_id)?
        .ok_or_else(|| "team.expert_binding_required".to_owned())?;
    if package_snapshot_hash(&binding.package_snapshot) != binding.package_hash {
        return Err("team.expert_snapshot_tampered".to_owned());
    }
    let value = binding
        .package_snapshot
        .get("packageManifest")
        .and_then(|manifest| manifest.get("team"))
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or_else(|| "team.not_available".to_owned())?;
    Ok((binding, parse_team(value)?))
}

pub(crate) fn start_team(
    database: &Database,
    conversation_id: &str,
    parent_run_id: &str,
    objective: &str,
    context: &str,
) -> Result<ExpertTeamSnapshot, String> {
    let (binding, team) = team_for_conversation(database, conversation_id)?;
    database.start_expert_team(&StartExpertTeamInput {
        conversation_id: conversation_id.to_owned(),
        expert_binding_id: binding.id,
        expert_id: binding.expert_id,
        package_hash: binding.package_hash,
        team_id: team.id.clone(),
        team_version: team.version.clone(),
        team: serde_json::to_value(team).map_err(|error| error.to_string())?,
        parent_run_id: parent_run_id.to_owned(),
        objective: objective.to_owned(),
        context: context.to_owned(),
    })
}

fn response<T: Serialize>(result: Result<T, String>) -> ApiResponse<T> {
    result.map(ApiResponse::success).unwrap_or_else(|message| {
        let code = if message.starts_with("team.") {
            message.as_str()
        } else {
            "team.failed"
        };
        ApiResponse::failure(code, message.clone(), false)
    })
}

#[tauri::command]
pub fn expert_team_get(
    state: State<'_, AppState>,
    request: ExpertTeamConversationRequest,
) -> ApiResponse<Option<ExpertTeamSnapshot>> {
    response(state.database.latest_expert_team(&request.conversation_id))
}

#[tauri::command]
pub fn expert_team_cancel(
    state: State<'_, AppState>,
    request: ExpertTeamCancelRequest,
) -> ApiResponse<ExpertTeamSnapshot> {
    response(
        state.runtime_host.cancel_expert_team(
            &request.team_run_id,
            request
                .reason
                .trim()
                .is_empty()
                .then_some("cancelled by user")
                .unwrap_or(request.reason.as_str()),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn valid_team() -> Value {
        json!({
            "schemaVersion": 1,
            "id": "review.team",
            "version": "1.0.0",
            "title": "Review team",
            "description": "Inspect and verify a delivery",
            "strategy": "supervisor",
            "members": [{
                "id": "reviewer",
                "name": "Reviewer",
                "agentId": "fox-general",
                "role": "Independent reviewer",
                "instructions": "Inspect only the delegated objective and return bounded evidence.",
                "allowedTools": ["read", "grep", "git_read"],
                "budget": {
                    "maxDurationMs": 60000,
                    "maxTotalTokens": 8000,
                    "maxOutputTokens": 2000,
                    "maxToolCalls": 10
                }
            }]
        })
    }

    #[test]
    fn validates_supervisor_team_with_bounded_members() {
        let team = parse_team(valid_team()).expect("valid team");
        assert_eq!(team.members.len(), 1);
        assert_eq!(team.members[0].allowed_tools, ["read", "grep", "git_read"]);
    }

    #[test]
    fn rejects_nested_delegation_and_unbounded_budget() {
        let mut nested = valid_team();
        nested["members"][0]["allowedTools"] = json!(["child_run_start"]);
        assert!(parse_team(nested).is_err());
        let mut unbounded = valid_team();
        unbounded["members"][0]["budget"]["maxDurationMs"] = json!(900001);
        assert!(parse_team(unbounded).is_err());
    }
}
