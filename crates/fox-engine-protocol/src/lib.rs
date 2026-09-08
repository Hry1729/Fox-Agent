//! Engine-neutral JSONL DTOs and canonical tool contracts.
//! Generated schema and TypeScript bindings come from this crate.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

mod control;
pub use control::*;
mod resume;
pub use resume::*;
mod snapshot;
pub use snapshot::*;

pub const PROTOCOL_NAME: &str = "fox-runtime-jsonl";
pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITY_MANIFEST_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeToolCapability {
    pub name: String,
    pub category: String,
    pub execution: String,
    pub approval: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilityManifest {
    pub manifest_version: u32,
    pub streaming_text: bool,
    pub cancellation: bool,
    pub reasoning: bool,
    pub session_resume: bool,
    pub tool_approval: bool,
    pub image_input: bool,
    pub steering: bool,
    pub context_compaction: bool,
    pub dynamic_model_switch: bool,
    #[serde(default)]
    pub work_loop: bool,
    pub tools: Vec<RuntimeToolCapability>,
}

impl RuntimeCapabilityManifest {
    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.manifest_version, 1 | CAPABILITY_MANIFEST_VERSION) {
            return Err(format!(
                "runtime capability manifest version mismatch: expected {}, got {}",
                CAPABILITY_MANIFEST_VERSION, self.manifest_version
            ));
        }
        let mut names = std::collections::HashSet::new();
        for tool in &self.tools {
            if tool.name.trim().is_empty() {
                return Err("runtime capability tool name is required".to_owned());
            }
            if !matches!(
                tool.category.as_str(),
                "project-read"
                    | "project-write"
                    | "attachment"
                    | "process"
                    | "knowledge"
                    | "skill"
                    | "mcp"
                    | "work"
                    | "memory"
                    | "delegation"
            ) {
                return Err(format!(
                    "runtime capability tool {} has unsupported category {}",
                    tool.name, tool.category
                ));
            }
            if !matches!(tool.execution.as_str(), "runtime" | "host" | "remote") {
                return Err(format!(
                    "runtime capability tool {} has unsupported execution {}",
                    tool.name, tool.execution
                ));
            }
            if !matches!(
                tool.approval.as_str(),
                "none" | "preflight" | "policy" | "always"
            ) {
                return Err(format!(
                    "runtime capability tool {} has unsupported approval {}",
                    tool.name, tool.approval
                ));
            }
            if !names.insert(tool.name.as_str()) {
                return Err(format!(
                    "runtime capability tool {} is declared more than once",
                    tool.name
                ));
            }
            let Some((category, execution, approval)) = canonical_runtime_tool_contract(&tool.name)
            else {
                return Err(format!(
                    "runtime capability tool {} is not in the Host canonical catalog",
                    tool.name
                ));
            };
            if tool.category != category || tool.execution != execution || tool.approval != approval
            {
                return Err(format!(
                    "runtime capability tool {} contract mismatch: expected {category}/{execution}/{approval}, got {}/{}/{}",
                    tool.name, tool.category, tool.execution, tool.approval
                ));
            }
            if tool.category == "work" && !self.work_loop_enabled() {
                return Err("runtime work tools require the workLoop capability".to_owned());
            }
        }
        Ok(())
    }

    pub fn work_loop_enabled(&self) -> bool {
        self.manifest_version >= 2 && self.work_loop
    }
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRequest {
    pub protocol: &'static str,
    pub version: u32,
    pub kind: &'static str,
    pub id: String,
    pub timestamp: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

impl RuntimeRequest {
    pub fn new(request_type: &str) -> Self {
        Self {
            protocol: PROTOCOL_NAME,
            version: PROTOCOL_VERSION,
            kind: "request",
            id: Uuid::new_v4().to_string(),
            timestamp: timestamp(),
            r#type: request_type.to_owned(),
            conversation_id: None,
            runtime_session_id: None,
            run_id: None,
            payload: None,
        }
    }

    pub fn with_conversation(mut self, id: &str) -> Self {
        self.conversation_id = Some(id.to_owned());
        self
    }

    pub fn with_runtime_session(mut self, id: &str) -> Self {
        self.runtime_session_id = Some(id.to_owned());
        self
    }

    pub fn with_run(mut self, id: &str) -> Self {
        self.run_id = Some(id.to_owned());
        self
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = Some(payload);
        self
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEnvelope {
    pub protocol: String,
    pub version: u32,
    pub kind: String,
    pub id: String,
    pub r#type: String,
    pub request_id: Option<String>,
    pub conversation_id: Option<String>,
    pub runtime_session_id: Option<String>,
    pub run_id: Option<String>,
    pub seq: Option<i64>,
    pub timestamp: String,
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostResponse {
    pub protocol: &'static str,
    pub version: u32,
    pub kind: &'static str,
    pub id: String,
    pub request_id: String,
    pub timestamp: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub payload: Value,
}

impl HostResponse {
    pub fn for_request(request: &RuntimeEnvelope, response_type: &str, payload: Value) -> Self {
        Self {
            protocol: PROTOCOL_NAME,
            version: PROTOCOL_VERSION,
            kind: "response",
            id: Uuid::new_v4().to_string(),
            request_id: request.id.clone(),
            timestamp: timestamp(),
            r#type: response_type.to_owned(),
            conversation_id: request.conversation_id.clone(),
            runtime_session_id: request.runtime_session_id.clone(),
            run_id: request.run_id.clone(),
            payload,
        }
    }
}

pub fn timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}


/// Stable declaration order is part of the runtime tool-registration contract.
pub const TOOL_CONTRACTS: &[(&str, &str, &str, &str)] = &[
    ("read", "project-read", "runtime", "preflight"),
    ("ls", "project-read", "runtime", "preflight"),
    ("find", "project-read", "runtime", "preflight"),
    ("grep", "project-read", "runtime", "preflight"),
    ("graph_readonly_run", "project-read", "runtime", "none"),
    ("read_attachment", "attachment", "host", "none"),
    ("write_file", "project-write", "host", "policy"),
    ("edit_file", "project-write", "host", "policy"),
    ("run_command", "process", "host", "always"),
    ("web_search", "skill", "host", "always"),
    ("web_read", "skill", "host", "always"),
    ("http_request", "skill", "host", "always"),
    ("system_info", "skill", "host", "always"),
    ("sqlite_read", "project-read", "host", "always"),
    ("structured_data", "skill", "host", "none"),
    ("git_read", "project-read", "host", "none"),
    ("test_run", "process", "host", "always"),
    ("code_check", "process", "host", "always"),
    ("format_code", "project-write", "host", "always"),
    ("tabular_data", "project-read", "host", "none"),
    ("work_snapshot_get", "work", "host", "none"),
    ("graph_readonly_activate", "work", "host", "none"),
    ("graph_readonly_snapshot_get", "work", "host", "none"),
    ("graph_readonly_node_start", "work", "host", "none"),
    ("graph_readonly_node_review", "work", "host", "none"),
    ("graph_readonly_node_finish", "work", "host", "none"),
    ("graph_readonly_node_cancel", "work", "host", "none"),
    ("graph_readonly_accept", "work", "host", "none"),
    ("workflow_snapshot_get", "work", "host", "none"),
    ("workflow_start", "work", "host", "none"),
    ("workflow_stage_start", "work", "host", "none"),
    ("workflow_stage_complete", "work", "host", "none"),
    ("workflow_stage_fail", "work", "host", "none"),
    ("workflow_cancel", "work", "host", "none"),
    ("team_snapshot_get", "delegation", "host", "none"),
    ("team_start", "delegation", "host", "none"),
    ("team_member_start", "delegation", "host", "none"),
    ("team_collect", "delegation", "host", "none"),
    ("team_cancel", "delegation", "host", "none"),
    ("child_agent_list", "delegation", "host", "none"),
    ("child_run_start", "delegation", "host", "none"),
    ("child_run_collect", "delegation", "host", "none"),
    ("child_run_cancel", "delegation", "host", "none"),
    ("goal_propose", "work", "host", "none"),
    ("goal_complete", "work", "host", "none"),
    ("task_create_many", "work", "host", "none"),
    ("task_update", "work", "host", "none"),
    ("task_attempt_start", "work", "host", "none"),
    ("task_repair_start", "work", "host", "none"),
    ("task_repair_escalate_start", "work", "host", "always"),
    ("task_attempt_finish", "work", "host", "none"),
    ("task_evidence_add", "work", "host", "none"),
    ("task_evidence_validate", "work", "host", "none"),
    ("plan_revision_create", "work", "host", "none"),
    ("review_finding_add", "work", "host", "none"),
    ("review_finding_resolve", "work", "host", "none"),
    ("acceptance_submit", "work", "host", "none"),
    ("memory_search", "memory", "host", "none"),
    ("memory_propose", "memory", "host", "none"),
    ("list_knowledge_bases", "knowledge", "host", "none"),
    ("search_knowledge", "knowledge", "host", "none"),
    ("read_knowledge_document", "knowledge", "host", "none"),
    ("query_knowledge_graph", "knowledge", "host", "none"),
    ("list_mcp_tools", "mcp", "host", "none"),
    ("call_mcp_tool", "mcp", "host", "always"),
];

pub fn canonical_runtime_tool_contract(tool: &str) -> Option<(&'static str, &'static str, &'static str)> {
    TOOL_CONTRACTS.iter().find(|entry| entry.0 == tool).map(|entry| (entry.1, entry.2, entry.3))
}

pub fn schema_bundle() -> Value {
    serde_json::json!({
        "schemaVersion": 1,
        "protocolName": PROTOCOL_NAME,
        "protocolVersion": PROTOCOL_VERSION,
        "capabilityManifestVersion": CAPABILITY_MANIFEST_VERSION,
        "schemas": {
            "RuntimeRequest": schemars::schema_for!(RuntimeRequest),
            "RuntimeEnvelope": schemars::schema_for!(RuntimeEnvelope),
            "HostResponse": schemars::schema_for!(HostResponse),
            "RuntimeCapabilityManifest": schemars::schema_for!(RuntimeCapabilityManifest),
            "RunControlBinding": schemars::schema_for!(RunControlBinding),
            "KernelBatchResumeFrame": schemars::schema_for!(KernelBatchResumeFrame),
            "KernelEngineBatchCheckpoint": schemars::schema_for!(KernelEngineBatchCheckpoint),
            "KernelModelResponse": schemars::schema_for!(KernelModelResponse),
            "KernelRunSnapshot": schemars::schema_for!(KernelRunSnapshot),
            "KernelStateInvalidation": schemars::schema_for!(KernelStateInvalidation)
        },
        "toolContracts": TOOL_CONTRACTS.iter().map(|t| serde_json::json!({
            "name": t.0, "category": t.1, "execution": t.2, "approval": t.3
        })).collect::<Vec<_>>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_schema_carries_every_wire_dto_and_contract() {
        let bundle = schema_bundle();
        assert_eq!(bundle["protocolVersion"], PROTOCOL_VERSION);
        assert!(bundle["schemas"]["RuntimeEnvelope"]["properties"]["runId"].is_object());
        assert_eq!(bundle["toolContracts"].as_array().unwrap().len(), TOOL_CONTRACTS.len());
        let names: std::collections::BTreeSet<_> = TOOL_CONTRACTS.iter().map(|t| t.0).collect();
        assert_eq!(names.len(), TOOL_CONTRACTS.len());
    }

    #[test]
    fn envelopes_preserve_the_version_one_wire_shape() {
        let request = RuntimeRequest::new("run.start").with_run("r").with_payload(serde_json::json!({"text": "hello"}));
        let json = serde_json::to_value(&request).unwrap();
        let decoded: RuntimeEnvelope = serde_json::from_value(json).unwrap();
        assert_eq!(decoded.protocol, PROTOCOL_NAME);
        assert_eq!(decoded.version, PROTOCOL_VERSION);
        assert_eq!(decoded.run_id.as_deref(), Some("r"));
        assert_eq!(HostResponse::for_request(&decoded, "run.accepted", Value::Null).request_id, request.id);
    }
}
