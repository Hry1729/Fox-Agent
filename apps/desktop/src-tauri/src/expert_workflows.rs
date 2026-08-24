use crate::{
    app_state::AppState,
    database::{
        package_snapshot_hash, ApiResponse, Database, ExpertWorkflowSnapshot,
        StartExpertWorkflowInput, StartExpertWorkflowStageInput,
    },
};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use tauri::State;

const MAX_WORKFLOW_BYTES: usize = 128 * 1024;
const MAX_SCHEMA_BYTES: usize = 32 * 1024;
const MAX_STAGES: usize = 20;

pub const WORKFLOW_TOOLS: [&str; 6] = [
    "workflow_snapshot_get",
    "workflow_start",
    "workflow_stage_start",
    "workflow_stage_complete",
    "workflow_stage_fail",
    "workflow_cancel",
];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkflowDefinition {
    schema_version: u32,
    id: String,
    version: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default = "empty_schema")]
    input_schema: Value,
    output_schema: Value,
    acceptance_criteria: Vec<String>,
    stages: Vec<WorkflowStageDefinition>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkflowStageDefinition {
    id: String,
    title: String,
    instructions: String,
    #[serde(default)]
    gate: WorkflowGate,
    #[serde(default)]
    retry: WorkflowRetry,
    output_schema: Value,
    acceptance_criteria: Vec<String>,
    #[serde(default)]
    stop_conditions: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WorkflowGate {
    #[default]
    None,
    UserApproval,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkflowRetry {
    #[serde(default = "one_attempt")]
    max_attempts: u8,
}

impl Default for WorkflowRetry {
    fn default() -> Self {
        Self { max_attempts: 1 }
    }
}

fn one_attempt() -> u8 {
    1
}

fn empty_schema() -> Value {
    json!({})
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowConversationRequest {
    conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowStartRequest {
    conversation_id: String,
    #[serde(default)]
    input: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowGateResolveRequest {
    workflow_run_id: String,
    stage_id: String,
    decision: String,
    #[serde(default)]
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertWorkflowCancelRequest {
    workflow_run_id: String,
    #[serde(default)]
    reason: String,
}

pub fn is_workflow_tool(tool: &str) -> bool {
    WORKFLOW_TOOLS.contains(&tool)
}

pub(crate) fn validate_workflow_definition(value: &Value) -> Result<(), String> {
    parse_workflow(value.clone()).map(|_| ())
}

fn parse_workflow(value: Value) -> Result<WorkflowDefinition, String> {
    let encoded = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_WORKFLOW_BYTES {
        return Err(format!(
            "workflow exceeds the {MAX_WORKFLOW_BYTES} byte limit"
        ));
    }
    let workflow: WorkflowDefinition = serde_json::from_value(value)
        .map_err(|error| format!("workflow does not match schema v1: {error}"))?;
    if workflow.schema_version != 1 {
        return Err("workflow schemaVersion must be 1".to_owned());
    }
    validate_id("workflow.id", &workflow.id)?;
    Version::parse(&workflow.version)
        .map_err(|error| format!("workflow.version is not SemVer: {error}"))?;
    validate_text("workflow.title", &workflow.title, 1, 160)?;
    validate_text("workflow.description", &workflow.description, 0, 2_000)?;
    validate_criteria("workflow.acceptanceCriteria", &workflow.acceptance_criteria)?;
    validate_schema("workflow.inputSchema", &workflow.input_schema)?;
    validate_schema("workflow.outputSchema", &workflow.output_schema)?;
    if workflow.stages.is_empty() || workflow.stages.len() > MAX_STAGES {
        return Err(format!(
            "workflow.stages must contain 1-{MAX_STAGES} stages"
        ));
    }
    let mut stage_ids = HashSet::new();
    for stage in &workflow.stages {
        validate_id("workflow stage id", &stage.id)?;
        if !stage_ids.insert(stage.id.as_str()) {
            return Err(format!("duplicate workflow stage id '{}'", stage.id));
        }
        validate_text("workflow stage title", &stage.title, 1, 160)?;
        validate_text("workflow stage instructions", &stage.instructions, 1, 8_000)?;
        if !(1..=5).contains(&stage.retry.max_attempts) {
            return Err("workflow retry.maxAttempts must be between 1 and 5".to_owned());
        }
        validate_schema("workflow stage outputSchema", &stage.output_schema)?;
        validate_criteria(
            "workflow stage acceptanceCriteria",
            &stage.acceptance_criteria,
        )?;
        if stage.stop_conditions.len() > 16 {
            return Err("workflow stage stopConditions may contain at most 16 entries".to_owned());
        }
        for condition in &stage.stop_conditions {
            validate_text("workflow stop condition", condition, 1, 500)?;
        }
    }
    Ok(workflow)
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

fn validate_criteria(field: &str, values: &[String]) -> Result<(), String> {
    if values.is_empty() || values.len() > 32 {
        return Err(format!("{field} must contain 1-32 entries"));
    }
    for value in values {
        validate_text(field, value, 1, 500)?;
    }
    Ok(())
}

fn validate_schema(field: &str, schema: &Value) -> Result<(), String> {
    let encoded = serde_json::to_vec(schema).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_SCHEMA_BYTES {
        return Err(format!("{field} exceeds the {MAX_SCHEMA_BYTES} byte limit"));
    }
    reject_external_refs(schema)?;
    jsonschema::validator_for(schema)
        .map_err(|error| format!("{field} is not a valid JSON Schema: {error}"))?;
    Ok(())
}

fn reject_external_refs(value: &Value) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            if object
                .get("$ref")
                .and_then(Value::as_str)
                .is_some_and(|reference| !reference.starts_with('#'))
            {
                return Err("workflow JSON Schema external $ref values are forbidden".to_owned());
            }
            for value in object.values() {
                reject_external_refs(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_external_refs(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_instance(field: &str, schema: &Value, instance: &Value) -> Result<(), String> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("{field} schema is invalid: {error}"))?;
    validator
        .validate(instance)
        .map_err(|error| format!("{field} does not match its JSON Schema: {error}"))
}

fn workflow_for_conversation(
    database: &Database,
    conversation_id: &str,
) -> Result<
    (
        crate::database::ConversationExpertBinding,
        WorkflowDefinition,
    ),
    String,
> {
    let binding = database
        .current_conversation_expert_binding(conversation_id)?
        .ok_or_else(|| "workflow.expert_binding_required".to_owned())?;
    if package_snapshot_hash(&binding.package_snapshot) != binding.package_hash {
        return Err("workflow.expert_snapshot_tampered".to_owned());
    }
    let value = binding
        .package_snapshot
        .get("packageManifest")
        .and_then(|manifest| manifest.get("workflow"))
        .filter(|value| !value.is_null())
        .cloned()
        .ok_or_else(|| "workflow.not_available".to_owned())?;
    Ok((binding, parse_workflow(value)?))
}

fn start(
    database: &Database,
    conversation_id: &str,
    input: Value,
) -> Result<ExpertWorkflowSnapshot, String> {
    let (binding, workflow) = workflow_for_conversation(database, conversation_id)?;
    validate_instance("workflow input", &workflow.input_schema, &input)?;
    let stages = workflow
        .stages
        .iter()
        .map(|stage| StartExpertWorkflowStageInput {
            stage_id: stage.id.clone(),
            title: stage.title.clone(),
            detail: format!(
                "{}\n\nAcceptance criteria:\n- {}{}",
                stage.instructions,
                stage.acceptance_criteria.join("\n- "),
                if stage.stop_conditions.is_empty() {
                    String::new()
                } else {
                    format!(
                        "\n\nStop conditions:\n- {}",
                        stage.stop_conditions.join("\n- ")
                    )
                }
            ),
            max_attempts: i64::from(stage.retry.max_attempts),
            user_gate: stage.gate == WorkflowGate::UserApproval,
        })
        .collect();
    database.start_expert_workflow(&StartExpertWorkflowInput {
        conversation_id: conversation_id.to_owned(),
        expert_binding_id: binding.id,
        expert_id: binding.expert_id,
        package_hash: binding.package_hash,
        workflow_id: workflow.id.clone(),
        workflow_version: workflow.version.clone(),
        workflow: serde_json::to_value(&workflow).map_err(|error| error.to_string())?,
        title: workflow.title.clone(),
        objective: workflow.description.clone(),
        acceptance_summary: workflow.acceptance_criteria.join("\n"),
        input,
        stages,
    })
}

pub fn execute_tool(
    database: &Database,
    conversation_id: &str,
    run_id: &str,
    tool: &str,
    input: &Value,
) -> Result<Value, String> {
    let snapshot = match tool {
        "workflow_snapshot_get" => {
            return Ok(json!({
                "workflow": database.active_expert_workflow(conversation_id)?,
            }));
        }
        "workflow_start" => start(
            database,
            conversation_id,
            input.get("input").cloned().unwrap_or_else(|| json!({})),
        )?,
        "workflow_stage_start" => {
            let snapshot = active_required(database, conversation_id)?;
            let stage = current_stage(&snapshot)?;
            let requested = optional_string(input, "stageId");
            if requested.is_some_and(|requested| requested != stage.stage_id) {
                return Err("workflow.stage_not_current".to_owned());
            }
            database.start_expert_workflow_stage(&snapshot.run.id, &stage.stage_id, run_id)?
        }
        "workflow_stage_complete" => {
            let snapshot = active_required(database, conversation_id)?;
            let workflow = parse_workflow(snapshot.run.workflow.clone())?;
            let stage_run = current_stage(&snapshot)?;
            let stage = workflow
                .stages
                .get(snapshot.run.current_stage_index as usize)
                .ok_or_else(|| "workflow.stage_not_found".to_owned())?;
            if stage.id != stage_run.stage_id {
                return Err("workflow.stage_definition_mismatch".to_owned());
            }
            let stage_output = input
                .get("stageOutput")
                .cloned()
                .ok_or_else(|| "workflow.stage_output_required".to_owned())?;
            validate_instance("workflow stage output", &stage.output_schema, &stage_output)?;
            let last = snapshot.run.current_stage_index as usize + 1 == workflow.stages.len();
            let workflow_output = input.get("workflowOutput").cloned();
            if last {
                let output = workflow_output
                    .as_ref()
                    .ok_or_else(|| "workflow.output_required".to_owned())?;
                validate_instance("workflow output", &workflow.output_schema, output)?;
            } else if workflow_output.is_some() {
                return Err("workflow.output_only_allowed_on_final_stage".to_owned());
            }
            let next_user_gate = workflow
                .stages
                .get(snapshot.run.current_stage_index as usize + 1)
                .is_some_and(|stage| stage.gate == WorkflowGate::UserApproval);
            database.complete_expert_workflow_stage(
                &snapshot.run.id,
                &stage.id,
                &stage_output,
                workflow_output.as_ref(),
                next_user_gate,
            )?
        }
        "workflow_stage_fail" => {
            let snapshot = active_required(database, conversation_id)?;
            let stage = current_stage(&snapshot)?;
            let message = required_string(input, "error")?;
            database.fail_expert_workflow_stage(&snapshot.run.id, &stage.stage_id, message)?
        }
        "workflow_cancel" => {
            let snapshot = active_required(database, conversation_id)?;
            database.cancel_expert_workflow(
                &snapshot.run.id,
                optional_string(input, "reason").unwrap_or("cancelled by workflow agent"),
            )?
        }
        _ => return Err(format!("unsupported workflow tool: {tool}")),
    };
    serde_json::to_value(snapshot).map_err(|error| error.to_string())
}

fn active_required(
    database: &Database,
    conversation_id: &str,
) -> Result<ExpertWorkflowSnapshot, String> {
    database
        .active_expert_workflow(conversation_id)?
        .ok_or_else(|| "workflow.not_active".to_owned())
}

fn current_stage(
    snapshot: &ExpertWorkflowSnapshot,
) -> Result<&crate::database::ExpertWorkflowStageRunRecord, String> {
    snapshot
        .stages
        .iter()
        .find(|stage| stage.ordinal == snapshot.run.current_stage_index)
        .ok_or_else(|| "workflow.stage_not_found".to_owned())
}

fn required_string<'a>(input: &'a Value, key: &str) -> Result<&'a str, String> {
    optional_string(input, key).ok_or_else(|| format!("workflow.{key}_required"))
}

fn optional_string<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn response<T: Serialize>(result: Result<T, String>) -> ApiResponse<T> {
    result.map(ApiResponse::success).unwrap_or_else(|message| {
        let code = if message.starts_with("workflow.") {
            message.as_str()
        } else {
            "workflow.failed"
        };
        ApiResponse::failure(code, message.clone(), false)
    })
}

#[tauri::command]
pub fn expert_workflow_get(
    state: State<'_, AppState>,
    request: ExpertWorkflowConversationRequest,
) -> ApiResponse<Option<ExpertWorkflowSnapshot>> {
    response(
        state
            .database
            .active_expert_workflow(&request.conversation_id),
    )
}

#[tauri::command]
pub fn expert_workflow_start(
    state: State<'_, AppState>,
    request: ExpertWorkflowStartRequest,
) -> ApiResponse<ExpertWorkflowSnapshot> {
    response(start(
        &state.database,
        &request.conversation_id,
        request.input,
    ))
}

#[tauri::command]
pub fn expert_workflow_gate_resolve(
    state: State<'_, AppState>,
    request: ExpertWorkflowGateResolveRequest,
) -> ApiResponse<ExpertWorkflowSnapshot> {
    if !matches!(request.decision.as_str(), "approved" | "rejected") {
        return ApiResponse::failure(
            "workflow.gate_decision_invalid",
            "workflow gate decision must be approved or rejected",
            false,
        );
    }
    response(state.database.resolve_expert_workflow_gate(
        &request.workflow_run_id,
        &request.stage_id,
        request.decision == "approved",
        &request.reason,
        "user",
    ))
}

#[tauri::command]
pub fn expert_workflow_cancel(
    state: State<'_, AppState>,
    request: ExpertWorkflowCancelRequest,
) -> ApiResponse<ExpertWorkflowSnapshot> {
    response(
        state.database.cancel_expert_workflow(
            &request.workflow_run_id,
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

    fn valid_workflow() -> Value {
        json!({
            "schemaVersion": 1,
            "id": "docs.review",
            "version": "0.1.0",
            "title": "Review docs",
            "description": "Review and verify documentation",
            "inputSchema": {"type": "object", "required": ["path"], "properties": {"path": {"type": "string"}}},
            "outputSchema": {"type": "object", "required": ["approved"], "properties": {"approved": {"type": "boolean"}}},
            "acceptanceCriteria": ["All findings are resolved"],
            "stages": [{
                "id": "inspect",
                "title": "Inspect",
                "instructions": "Inspect the documents and collect evidence.",
                "gate": "user_approval",
                "retry": {"maxAttempts": 2},
                "outputSchema": {"type": "object", "required": ["findings"], "properties": {"findings": {"type": "array"}}},
                "acceptanceCriteria": ["Findings cite evidence"],
                "stopConditions": ["The target is unavailable"]
            }]
        })
    }

    #[test]
    fn validates_versioned_workflow_and_output_contracts() {
        let workflow = parse_workflow(valid_workflow()).expect("valid workflow");
        validate_instance("input", &workflow.input_schema, &json!({"path": "docs"}))
            .expect("valid input");
        assert!(validate_instance("input", &workflow.input_schema, &json!({})).is_err());
        assert!(validate_instance(
            "stage output",
            &workflow.stages[0].output_schema,
            &json!({"findings": []})
        )
        .is_ok());
    }

    #[test]
    fn rejects_external_schema_references_and_unbounded_retry() {
        let mut workflow = valid_workflow();
        workflow["inputSchema"] = json!({"$ref": "https://example.com/input.json"});
        assert!(parse_workflow(workflow)
            .unwrap_err()
            .contains("external $ref"));

        let mut workflow = valid_workflow();
        workflow["stages"][0]["retry"]["maxAttempts"] = json!(6);
        assert!(parse_workflow(workflow)
            .unwrap_err()
            .contains("between 1 and 5"));
    }
}
