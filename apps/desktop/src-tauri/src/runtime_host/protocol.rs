use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const PROTOCOL_NAME: &str = "fox-runtime-jsonl";
pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITY_MANIFEST_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeToolCapability {
    pub name: String,
    pub category: String,
    pub execution: String,
    pub approval: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
            if tool.category == "work" && !self.work_loop_enabled() {
                return Err("runtime work tools require the workLoop capability".to_owned());
            }
            if tool.execution == "host" && !host_tool_is_supported(&tool.name) {
                return Err(format!(
                    "runtime capability tool {} declares host execution but Fox has no handler",
                    tool.name
                ));
            }
        }
        Ok(())
    }

    pub fn work_loop_enabled(&self) -> bool {
        self.manifest_version >= 2 && self.work_loop
    }
}

fn host_tool_is_supported(tool: &str) -> bool {
    matches!(
        tool,
        "read_attachment"
            | "write_file"
            | "edit_file"
            | "run_command"
            | "list_knowledge_bases"
            | "search_knowledge"
            | "read_knowledge_document"
            | "query_knowledge_graph"
            | "list_mcp_tools"
            | "call_mcp_tool"
    ) || super::work_tools::is_work_tool(tool)
}

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Serialize)]
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

#[cfg(test)]
mod tests {
    use super::{host_tool_is_supported, RuntimeCapabilityManifest, CAPABILITY_MANIFEST_VERSION};
    use serde_json::json;

    #[test]
    fn validates_the_runtime_capability_manifest_contract() {
        let manifest: RuntimeCapabilityManifest = serde_json::from_value(json!({
            "manifestVersion": CAPABILITY_MANIFEST_VERSION,
            "streamingText": true,
            "cancellation": true,
            "reasoning": true,
            "sessionResume": true,
            "toolApproval": true,
            "imageInput": false,
            "steering": false,
            "contextCompaction": true,
            "dynamicModelSwitch": false,
            "workLoop": true,
            "tools": [{
                "name": "read",
                "category": "project-read",
                "execution": "runtime",
                "approval": "preflight"
            }]
        }))
        .expect("deserialize capability manifest");
        manifest.validate().expect("valid manifest");
        assert!(manifest.work_loop_enabled());
    }

    #[test]
    fn every_registered_work_tool_has_a_host_handler() {
        for tool in super::super::work_tools::WORK_TOOLS {
            assert!(
                host_tool_is_supported(tool),
                "registered work tool {tool} must be accepted by capability validation"
            );
        }
    }

    #[test]
    fn rejects_duplicate_runtime_tools() {
        let manifest: RuntimeCapabilityManifest = serde_json::from_value(json!({
            "manifestVersion": CAPABILITY_MANIFEST_VERSION,
            "streamingText": true,
            "cancellation": true,
            "reasoning": true,
            "sessionResume": true,
            "toolApproval": true,
            "imageInput": false,
            "steering": false,
            "contextCompaction": true,
            "dynamicModelSwitch": false,
            "workLoop": true,
            "tools": [
                { "name": "read", "category": "project-read", "execution": "runtime", "approval": "preflight" },
                { "name": "read", "category": "project-read", "execution": "runtime", "approval": "preflight" }
            ]
        }))
        .expect("deserialize capability manifest");
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn version_one_runtime_degrades_without_work_tools() {
        let manifest: RuntimeCapabilityManifest = serde_json::from_value(json!({
            "manifestVersion": 1,
            "streamingText": true,
            "cancellation": true,
            "reasoning": true,
            "sessionResume": true,
            "toolApproval": true,
            "imageInput": false,
            "steering": false,
            "contextCompaction": true,
            "dynamicModelSwitch": false,
            "tools": []
        }))
        .expect("deserialize v1 manifest");
        manifest.validate().expect("v1 remains compatible");
        assert!(!manifest.work_loop_enabled());
    }

    #[test]
    fn rejects_a_raw_database_host_surface() {
        let manifest: RuntimeCapabilityManifest = serde_json::from_value(json!({
            "manifestVersion": CAPABILITY_MANIFEST_VERSION,
            "streamingText": true,
            "cancellation": true,
            "reasoning": true,
            "sessionResume": true,
            "toolApproval": true,
            "imageInput": false,
            "steering": false,
            "contextCompaction": true,
            "dynamicModelSwitch": false,
            "workLoop": true,
            "tools": [{
                "name": "sqlite_execute",
                "category": "work",
                "execution": "host",
                "approval": "none"
            }]
        }))
        .expect("deserialize manifest");
        assert!(manifest.validate().is_err());
    }
}
