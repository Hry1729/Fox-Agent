//! Host adapter around the engine-neutral generated protocol.
#[allow(unused_imports)]
pub use fox_engine_protocol::{
    canonical_runtime_tool_contract, timestamp, HostResponse, RuntimeCapabilityManifest,
    RuntimeEnvelope, RuntimeRequest, RuntimeToolCapability, CAPABILITY_MANIFEST_VERSION,
    PROTOCOL_NAME, PROTOCOL_VERSION,
};

/// Protocol validity and actual Host handler availability are separate checks.
/// This adapter retains the latter after the wire DTOs move out of Tauri.
pub fn validate_host_manifest(manifest: &RuntimeCapabilityManifest) -> Result<(), String> {
    manifest.validate()?;
    for tool in &manifest.tools {
        if tool.execution == "host" && !host_tool_is_supported(&tool.name) {
            return Err(format!("runtime capability tool {} declares host execution but Fox has no handler", tool.name));
        }
    }
    Ok(())
}

fn host_tool_is_supported(tool: &str) -> bool {
    matches!(
        tool,
        "read_attachment"
            | "read_tool_result"
            | "write_file"
            | "edit_file"
            | "run_command"
            | "list_knowledge_bases"
            | "search_knowledge"
            | "read_knowledge_document"
            | "query_knowledge_graph"
            | "list_mcp_tools"
            | "call_mcp_tool"
            | "memory_search"
            | "memory_propose"
            | "child_agent_list"
            | "child_run_start"
            | "child_run_collect"
            | "child_run_cancel"
            | "team_snapshot_get"
            | "team_start"
            | "team_member_start"
            | "team_collect"
            | "team_cancel"
    ) || super::capability_tools::is_capability_tool(tool)
        || super::work_tools::is_work_tool(tool)
}

#[cfg(test)]
mod tests {
    use super::{
        canonical_runtime_tool_contract, host_tool_is_supported, RuntimeCapabilityManifest,
        CAPABILITY_MANIFEST_VERSION,
    };
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
            "tools": [
                {
                    "name": "read",
                    "category": "project-read",
                    "execution": "runtime",
                    "approval": "preflight"
                },
                {
                    "name": "child_run_start",
                    "category": "delegation",
                    "execution": "host",
                    "approval": "none"
                }
            ]
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
    fn persistent_read_only_graph_tools_have_exact_host_work_contracts() {
        for tool in [
            "graph_readonly_activate",
            "graph_readonly_snapshot_get",
            "graph_readonly_node_start",
            "graph_readonly_node_finish",
            "graph_readonly_node_cancel",
            "graph_readonly_node_review",
            "graph_readonly_accept",
        ] {
            assert_eq!(
                canonical_runtime_tool_contract(tool),
                Some(("work", "host", "none"))
            );
            assert!(host_tool_is_supported(tool));
        }
    }

    #[test]
    fn governed_memory_tools_have_host_handlers() {
        for tool in ["memory_search", "memory_propose"] {
            assert!(
                host_tool_is_supported(tool),
                "registered memory tool {tool} must be accepted by capability validation"
            );
        }
    }

    #[test]
    fn child_run_tools_have_host_handlers() {
        for tool in [
            "team_snapshot_get",
            "team_start",
            "team_member_start",
            "team_collect",
            "team_cancel",
            "child_agent_list",
            "child_run_start",
            "child_run_collect",
            "child_run_cancel",
        ] {
            assert!(
                host_tool_is_supported(tool),
                "registered Child Run tool {tool} must be accepted by capability validation"
            );
        }
    }

    #[test]
    fn every_registered_capability_tool_has_a_host_handler() {
        for tool in super::super::capability_tools::CAPABILITY_TOOLS {
            assert!(
                host_tool_is_supported(tool),
                "registered capability tool {tool} must be accepted by capability validation"
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
    fn rejects_unknown_tools_and_canonical_contract_spoofing() {
        for tool in [
            json!({
                "name": "invented_runtime_tool",
                "category": "project-read",
                "execution": "runtime",
                "approval": "preflight"
            }),
            json!({
                "name": "write_file",
                "category": "project-write",
                "execution": "runtime",
                "approval": "preflight"
            }),
            json!({
                "name": "run_command",
                "category": "process",
                "execution": "host",
                "approval": "none"
            }),
        ] {
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
                "tools": [tool]
            }))
            .expect("deserialize manifest");
            assert!(manifest.validate().is_err());
        }
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
