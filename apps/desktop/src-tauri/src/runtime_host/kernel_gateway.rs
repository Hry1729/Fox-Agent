//! Frozen resource policy shared by proposal admission and final dispatch.
use crate::{
    database::{Database, KernelHostScope},
    kernel::{CancellationToken, PolicyDecision, PolicyDecisionPort},
};
use fox_engine_protocol::{PermissionMode, RunControlBinding};
use serde_json::{json, Value};

pub(super) const SUPPORTED_TOOLS: &[&str] = &[
    "read",
    "ls",
    "find",
    "grep",
    "write_file",
    "edit_file",
    "run_command",
    "list_mcp_tools",
    "call_mcp_tool",
    "list_knowledge_bases",
    "search_knowledge",
    "read_knowledge_document",
    "query_knowledge_graph",
    "web_search",
    "web_read",
    "http_request",
    "system_info",
    "sqlite_read",
    "structured_data",
    "git_read",
    "test_run",
    "code_check",
    "format_code",
    "tabular_data",
    "attachment_compute",
    "compute_job_start", "compute_job_status", "compute_job_cancel", "compute_job_result",
    "memory_search",
    "memory_propose",
    "read_attachment",
    // Reads bytes Host already stored for a settled call. No file is touched and
    // no tool is executed, so it belongs in the read-only resource catalogue.
    "read_tool_result",
    // Returns the full text of an enabled skill omitted from the initial prompt.
    // Instructions only; it never adds a tool to the frozen scope.
    "skill_load",
];

pub(super) fn supported_tools() -> Vec<&'static str> {
    SUPPORTED_TOOLS
        .iter()
        .copied()
        .chain(super::work_tools::WORK_TOOLS.iter().copied())
        .chain(super::kernel_delegation::TOOLS.iter().copied())
        .collect()
}

pub(super) fn is_knowledge(tool: &str) -> bool {
    matches!(
        tool,
        "list_knowledge_bases"
            | "search_knowledge"
            | "read_knowledge_document"
            | "query_knowledge_graph"
    )
}

pub(super) fn is_context_resource(tool: &str) -> bool {
    matches!(
        tool,
        "memory_search"
            | "memory_propose"
            | "read_attachment"
            | "attachment_compute"
            | "compute_job_start" | "compute_job_status" | "compute_job_cancel" | "compute_job_result"
            | "read_tool_result"
            | "skill_load"
    )
}

fn server_hash(server: &crate::database::McpServerRecord) -> String {
    super::runtime_shadow_hash(
        &json!({"id":server.id,"transport":server.transport,"command":server.command,
        "args":server.args,"endpoint":server.endpoint_url,"definition":server.definition})
        .to_string(),
    )
}

fn reference_hash(reference: &crate::database::KnowledgeReference) -> String {
    super::runtime_shadow_hash(
        &serde_json::to_string(reference).expect("serializable knowledge reference"),
    )
}

fn package_allows_reference(
    package: &Value,
    reference: &crate::database::KnowledgeReference,
) -> bool {
    if package.is_null() {
        return true;
    }
    let manifest = &package["packageManifest"];
    if let Some(references) = manifest["knowledgeReferences"].as_array() {
        return references
            .iter()
            .any(|item| super::package_knowledge_reference(item).ok().as_ref() == Some(reference));
    }
    if let Some(ids) = manifest["knowledge"].as_array() {
        return reference.source == "remote"
            && ids
                .iter()
                .any(|id| id.as_str() == Some(reference.id.as_str()));
    }
    true
}

pub(super) fn freeze_scope(
    database: &Database,
    binding: &RunControlBinding,
    prompt: &Value,
    config: &crate::kernel_model_config::KernelModelConfig,
) -> Result<KernelHostScope, String> {
    let packages = [&prompt["assistantPackage"], &prompt["expertPackage"]];
    let allowed_mcp = |server_id: &str| {
        packages.iter().all(|package| {
            package.is_null()
                || super::package_string_scope(&package["packageManifest"], "mcpServers")
                    .map(|scope| scope.contains(server_id))
                    .unwrap_or(package["isBuiltin"] == true)
        })
    };
    let mcp_server_hashes = database
        .list_mcp_servers()?
        .into_iter()
        .filter(|server| server.enabled && allowed_mcp(&server.id))
        .map(|server| (server.id.clone(), server_hash(&server)))
        .collect();
    let references = database
        .conversation_knowledge_reference_bindings(&binding.conversation_id)?
        .into_iter()
        .filter(|item| {
            item.enabled
                && packages
                    .iter()
                    .all(|package| package_allows_reference(package, &item.reference))
        })
        .map(|item| item.reference)
        .collect::<Vec<_>>();
    let mut knowledge_connection_hashes = std::collections::BTreeMap::new();
    if references
        .iter()
        .any(|reference| reference.source == "remote")
    {
        if let Some(service) = database
            .get_yuxi_service()?
            .filter(|service| service.enabled)
        {
            let hash = super::runtime_shadow_hash(
                &json!({"baseUrl":service.base_url,"connectionType":service.connection_type})
                    .to_string(),
            );
            for reference in &references {
                if let Some(id) = &reference.connection_id {
                    knowledge_connection_hashes.insert(id.clone(), hash.clone());
                }
            }
        }
    }
    let office_tools = crate::office::tool_definitions()
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .filter(|tool| {
            packages.iter().all(|package| {
                super::package_string_scope(&package["packageManifest"], "officeTools")
                    .is_none_or(|scope| scope.contains(tool))
            })
        })
        .collect();
    let mut scope = KernelHostScope {
        schema_version: 1,
        tool_names: config
            .proposal_tools
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
            .collect(),
        mcp_server_hashes,
        knowledge_reference_hashes: references.iter().map(reference_hash).collect(),
        knowledge_connection_hashes,
        office_tools,
        lifecycle_hooks: database
            .list_lifecycle_hooks()?
            .into_iter()
            .filter(|hook| hook.enabled)
            .collect(),
    };
    if let Some(child) = database.child_run(&binding.run_id)? {
        if database
            .run_control_binding(&child.parent_run_id)?
            .is_some_and(|parent| {
                parent.authority == fox_engine_protocol::ExecutionAuthority::Authoritative
            })
        {
            let parent = database.kernel_host_scope(&child.parent_run_id)?;
            scope
                .tool_names
                .retain(|name| parent.tool_names.contains(name));
            scope
                .mcp_server_hashes
                .retain(|id, digest| parent.mcp_server_hashes.get(id) == Some(digest));
            scope
                .knowledge_reference_hashes
                .retain(|digest| parent.knowledge_reference_hashes.contains(digest));
            scope
                .knowledge_connection_hashes
                .retain(|id, digest| parent.knowledge_connection_hashes.get(id) == Some(digest));
            scope
                .office_tools
                .retain(|name| parent.office_tools.contains(name));
            // A child cannot relax the parent's frozen hooks through a later
            // settings edit. Retain additional child-time hooks as well.
            let mut hooks = parent.lifecycle_hooks;
            for hook in scope.lifecycle_hooks {
                if !hooks.iter().any(|parent| parent.id == hook.id) {
                    hooks.push(hook);
                }
            }
            scope.lifecycle_hooks = hooks;
        }
    }
    Ok(scope)
}

pub(super) struct GatewayPolicy {
    pub binding: RunControlBinding,
    pub scope: KernelHostScope,
    /// Authority to read the run's *additional* authorization grants (#5).
    /// Absent in unit tests that only exercise the frozen snapshot; production
    /// always sets it so a conversation approval becomes reusable without ever
    /// rewriting the frozen binding.
    pub database: Option<Database>,
    /// The Host's runtime session store, used to resolve compute-artifact
    /// references (office_import_data's artifactId) against the frozen
    /// binding's own conversation. Set by production; optional in unit tests.
    pub sessions_dir: Option<std::path::PathBuf>,
    /// The Host's application data directory, used to place Host-private
    /// Office artifacts (rendered previews, working copies, commit staging).
    /// Deliberately **not** derived from a tool argument: the model can never
    /// point these areas at application data. Optional in unit tests, which
    /// then keep the project-only placement path.
    pub artifacts_dir: Option<std::path::PathBuf>,
}

/// Business eligibility is read before asking a human and checked again inside
/// the repair transaction after approval. It never grants resource permissions.
pub(super) struct GatewayProposalPolicy<'a> {
    pub gateway: &'a GatewayPolicy,
    pub database: &'a Database,
}

impl PolicyDecisionPort for GatewayProposalPolicy<'_> {
    fn decide(&self, run_id: &str, tool_id: &str, tool: &str, input_json: &str) -> PolicyDecision {
        let decision = self.gateway.decide(run_id, tool_id, tool, input_json);
        if tool == "task_repair_escalate_start" && !matches!(decision, PolicyDecision::Deny { .. })
        {
            let eligible = serde_json::from_str::<Value>(input_json)
                .ok()
                .is_some_and(|input| {
                    super::work_tools::preflight_task_repair_override(
                        self.database,
                        &self.gateway.binding.conversation_id,
                        run_id,
                        &input,
                    )
                    .is_ok()
                });
            if !eligible {
                return PolicyDecision::Deny {
                    reason: "Task repair override is not eligible for a one-time approval".into(),
                };
            }
        }
        decision
    }
}

impl GatewayPolicy {
    /// Trusted Office authorization derived solely from the frozen Run binding:
    /// the model contributes only the artifact id, never the conversation.
    /// Production sets both fields; policy unit tests that freeze only a scope
    /// get `None` and keep the inline-only preparation path.
    fn office_call_context(&self) -> Option<crate::office::OfficeCallContext<'_>> {
        match (self.sessions_dir.as_deref(), self.database.as_ref()) {
            (Some(sessions_dir), Some(database)) => Some(crate::office::OfficeCallContext {
                database,
                sessions_dir,
                conversation_id: &self.binding.conversation_id,
                artifacts_dir: self.artifacts_dir.as_deref(),
            }),
            _ => None,
        }
    }

    /// Office preparation through the same authorization the execution
    /// dispatch uses, so the pre-approval validate/decide gate accepts and
    /// integrity-checks an `artifactId` exactly as execution will. Never
    /// resolves an artifact against a model-supplied conversation.
    fn prepare_office(
        &self,
        remote_tool: &str,
        arguments: &Value,
    ) -> Result<crate::office::PreparedOffice, String> {
        let context = self.office_call_context();
        crate::office::prepare_with_context(
            remote_tool,
            arguments,
            self.binding.permission.project_root.as_deref(),
            self.binding.permission.mode.as_str(),
            context.as_ref(),
        )
    }

    fn remaining_budget(&self, database: &Database) -> Result<std::time::Duration, String> {
        let elapsed = database
            .kernel_build_full_snapshot(&self.binding.run_id)?
            .running_elapsed_ms;
        let remaining = self.binding.budgets.limit_operation_ms(
            self.binding.budgets.tool_execution_ms, elapsed,
        );
        if remaining <= 0 {
            return Err("Kernel resource execution budget exhausted".into());
        }
        Ok(std::time::Duration::from_millis(remaining as u64))
    }
    pub(super) fn validate(&self, tool: &str, input: &Value) -> Result<(), String> {
        self.binding.validate()?;
        if !self.scope.tool_names.contains(tool)
            || !(SUPPORTED_TOOLS.contains(&tool)
                || super::work_tools::is_work_tool(tool)
                || super::kernel_delegation::TOOLS.contains(&tool))
            || !super::continuation::ExecutionProfileSelection::resolve(
                &self.binding.execution_profile_id,
            )?
            .allows_host_tool(tool)
        {
            return Err("tool is outside the frozen Kernel resource catalog".into());
        }
        if !input.is_object() {
            return Err("tool input must be an object".into());
        }
        if self.scope.lifecycle_hooks.iter().any(|hook| {
            hook.event == "before_tool"
                && hook.action == "block"
                && crate::lifecycle_hooks::matcher_matches(&hook.matcher, tool)
        }) {
            return Err("tool was blocked by a frozen lifecycle hook".into());
        }
        if is_context_resource(tool) {
            return Ok(());
        }
        if super::kernel_delegation::TOOLS.contains(&tool) {
            return Ok(());
        }
        if tool == "task_repair_escalate_start" {
            super::work_tools::validate_task_repair_override_request(input)
                .map_err(|error| error.to_string())?;
        }
        if super::work_tools::is_work_tool(tool) {
            return Ok(());
        }
        if super::capability_tools::is_capability_tool(tool) {
            let action = super::capability_tools::prepare(
                tool,
                input,
                self.binding.permission.project_root.as_deref(),
            )?;
            if self.binding.permission.mode == PermissionMode::ReadOnly
                && (action.blocked_in_read_only()
                    || matches!(&action, super::capability_tools::PreparedCapabilityTool::HttpRequest { method, .. } if !matches!(method.as_str(),"GET"|"HEAD")))
            {
                return Err(
                    "capability would mutate resources under frozen read-only permission".into(),
                );
            }
            return Ok(());
        }
        if is_knowledge(tool) {
            if tool != "list_knowledge_bases" || super::has_explicit_knowledge_target(input) {
                let targets =
                    super::knowledge_tool_targets(input, tool != "read_knowledge_document")?;
                if targets.len() > 64 {
                    return Err("too many knowledge targets".into());
                }
            }
            return Ok(());
        }
        if tool == "list_mcp_tools" {
            return Ok(());
        }
        if tool == "call_mcp_tool" {
            let server = input["serverId"].as_str().ok_or("missing MCP connection")?;
            let remote_tool = input["tool"]
                .as_str()
                .filter(|name| !name.trim().is_empty())
                .ok_or("missing MCP tool")?;
            if !self.scope.mcp_server_hashes.contains_key(server) || !input["arguments"].is_object()
            {
                return Err("MCP connection or arguments are outside the frozen scope".into());
            }
            if server == crate::office::SERVER_ID {
                if !self.scope.office_tools.contains(remote_tool) {
                    return Err("Office operation is outside frozen scope".into());
                }
                // Same Host-resolved authorization as execution, so an
                // artifactId import is accepted and integrity-checked here.
                self.prepare_office(remote_tool, &input["arguments"])?;
            } else if self.binding.permission.mode == PermissionMode::ReadOnly {
                return Err("unclassified external MCP calls are unavailable under frozen read-only permission".into());
            }
            return Ok(());
        }
        let root = self
            .binding
            .permission
            .project_root
            .as_deref()
            .ok_or("tool requires a frozen project root")?;
        if !crate::resource_gateway::is_reader(tool) {
            if self.binding.permission.mode == PermissionMode::ReadOnly {
                return Err("frozen project permission is read-only".into());
            }
            crate::tool_host::prepare(tool, input, root)?;
        }
        Ok(())
    }

    pub(super) fn execute_context_resource(
        &self,
        database: &Database,
        attachments_dir: &std::path::Path,
        sessions_dir: &std::path::Path,
        skills_dir: &std::path::Path,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        self.validate(tool, input)?;
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope
        {
            return Err("frozen Kernel context resource identity changed".into());
        }
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        if super::background_jobs::TOOLS.contains(&tool) {
            return super::background_jobs::execute(database, attachments_dir, sessions_dir, &self.binding.run_id, tool, input, Some(token.clone()), self.remaining_budget(database)?);
        }
        let result = match tool {
            "memory_search" | "memory_propose" => super::execute_memory_operation(
                database,
                &self.binding.conversation_id,
                &self.binding.run_id,
                tool,
                input,
            )?,
            "read_attachment" => {
                let id = input["attachmentId"]
                    .as_str()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or("attachmentId is required")?;
                super::read_attachment_text(
                    database,
                    attachments_dir,
                    &self.binding.conversation_id,
                    id,
                    input,
                )?
            }
            "attachment_compute" => {
                let mut bounded=input.clone();
                let remaining=self.remaining_budget(database)?.as_millis().min(30_000) as u64;
                if input.get("timeoutMs").is_none() || input["timeoutMs"].is_u64() {
                    bounded["timeoutMs"]=json!(input["timeoutMs"].as_u64().unwrap_or(15_000).min(remaining));
                }
                super::attachment_compute::execute(
                database,
                attachments_dir,
                sessions_dir,
                &self.binding.conversation_id,
                &self.binding.run_id,
                &bounded,
                { let token = token.clone(); move || token.is_cancelled() },
            )?
            },
            "read_tool_result" => {
                let request = super::tool_result_read::prepare(input)?;
                // The scope is the Run's own frozen binding. A Kernel context
                // resource never asks the caller which conversation to read.
                super::tool_result_read::execute(
                    database,
                    &self.binding.conversation_id,
                    &request,
                )?
            }
            "skill_load" => {
                let request = super::skill_load::prepare(input)?;
                // Enablement is resolved from the conversation's frozen
                // assistant/expert bindings; availability is the frozen tool
                // scope, so loading text can never grow the Run's authority.
                let enabled =
                    super::conversation_enabled_skill_ids(database, &self.binding.conversation_id)?;
                let frozen_tools = self
                    .scope
                    .tool_names
                    .iter()
                    .cloned()
                    .collect::<std::collections::HashSet<_>>();
                super::skill_load::execute(
                    database,
                    skills_dir,
                    &self.binding.conversation_id,
                    &self.binding.run_id,
                    &enabled,
                    Some(&frozen_tools),
                    &request,
                    crate::database::now_ms(),
                )?
            }
            _ => return Err("unsupported Kernel context resource".into()),
        };
        token.check()?;
        // `read_tool_result` already answers with a content block of pure stored
        // bytes. Re-serialising that whole envelope into the text block would
        // make ranges impossible to concatenate back into the original result,
        // so it keeps its own shape and only the metadata becomes `details`.
        // `skill_load` likewise returns its own content/details envelope (the
        // audited skill text plus activation metadata).
        if tool == "read_tool_result" {
            let text = result["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let details = result.get("details").cloned().unwrap_or_else(|| json!({}));
            return Ok(json!({"content":[{"type":"text","text":text}],"details":details}));
        }
        if tool == "skill_load" {
            return Ok(result);
        }
        let content = result.to_string();
        let mut details = result;
        if tool == "read_attachment" {
            if let Some(details) = details.as_object_mut() { details.remove("text"); }
        }
        Ok(json!({"content":[{"type":"text","text":content}],"details":details}))
    }

    pub(super) fn execute_work(
        &self,
        database: &Database,
        tool_id: &str,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        self.validate(tool, input)?;
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope
        {
            return Err("frozen Kernel work identity changed".into());
        }
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        let projected_id = format!("kernel-tool:{}:{tool_id}", self.binding.run_id);
        let outcome = if tool == "task_repair_escalate_start" {
            super::work_tools::preflight_task_repair_override(
                database,
                &self.binding.conversation_id,
                &self.binding.run_id,
                input,
            )
            .map_err(|error| error.to_string())?;
            super::work_tools::execute_approved_task_repair_override(
                database,
                &self.binding.conversation_id,
                &self.binding.run_id,
                &projected_id,
                &format!("kernel-approval:{}:{tool_id}", self.binding.run_id),
                input,
            )
        } else {
            super::work_tools::execute_with_host_tool_call_id(
                database,
                &self.binding.conversation_id,
                &self.binding.run_id,
                &projected_id,
                tool,
                input,
            )
        }
        .map_err(|error| error.to_string())?;
        if let Some(directive) = outcome.post_finalize {
            let body =
                serde_json::to_string(&directive).map_err(|_| "invalid Kernel work action")?;
            database.stage_kernel_host_action(&self.binding.run_id, tool_id, &body)?;
        }
        token.check()?;
        Ok(outcome.result)
    }

    pub(super) fn execute_delegation(
        &self,
        database: &Database,
        tool_id: &str,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        self.validate(tool, input)?;
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope
        {
            return Err("frozen Kernel delegation identity changed".into());
        }
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        if let Some(result) =
            super::kernel_delegation::preflight_child(database, &self.binding, tool, input)?
        {
            token.check()?;
            let rejected = database
                .kernel_build_full_snapshot(&self.binding.run_id)?
                .tool_calls
                .iter()
                .filter(|call| {
                    matches!(
                        call.tool.as_str(),
                        "child_run_start" | "child_run_collect" | "child_run_cancel"
                    ) && call.state == "failed"
                        && call
                            .result_json
                            .as_deref()
                            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                            .is_some_and(|value| {
                                value["details"]["error"]["code"] == "child.invalid_arguments"
                                    && value["details"]["executionStarted"] == false
                            })
                })
                .count();
            if rejected >= 3 {
                return Ok(super::kernel_delegation::correction_limit_result());
            }
            return Ok(result);
        }
        let result = super::kernel_delegation::execute(
            database,
            &self.binding,
            tool_id,
            tool,
            input,
            token,
        )?;
        token.check()?;
        Ok(result)
    }

    pub(super) fn execute_knowledge(
        &self,
        database: &Database,
        client: &crate::yuxi::YuxiClient,
        local: &crate::local_knowledge::LocalKnowledgeStore,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope
        {
            return Err("frozen knowledge identity changed".into());
        }
        self.validate(tool, input)?;
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        let bindings = database
            .conversation_knowledge_reference_bindings(&self.binding.conversation_id)?
            .into_iter()
            .filter(|item| {
                item.enabled
                    && self
                        .scope
                        .knowledge_reference_hashes
                        .contains(&reference_hash(&item.reference))
            })
            .collect::<Vec<_>>();
        let service = database
            .get_yuxi_service()?
            .filter(|service| service.enabled);
        if bindings
            .iter()
            .any(|item| item.reference.source == "remote")
        {
            let service = service
                .as_ref()
                .ok_or("frozen knowledge connection is unavailable")?;
            let hash = super::runtime_shadow_hash(
                &json!({"baseUrl":service.base_url,"connectionType":service.connection_type})
                    .to_string(),
            );
            if bindings
                .iter()
                .filter(|item| item.reference.source == "remote")
                .any(|item| {
                    item.reference.connection_id.as_ref().is_none_or(|id| {
                        self.scope.knowledge_connection_hashes.get(id) != Some(&hash)
                    })
                })
            {
                return Err("knowledge connection changed since Run creation".into());
            }
        }
        let budget = self.remaining_budget(database)?;
        tauri::async_runtime::block_on(async {
            let deadline = std::time::Instant::now() + budget;
            let mut request = Box::pin(super::execute_bound_knowledge_tool(
                service.as_ref(),
                client,
                local,
                &bindings,
                tool,
                input,
            ));
            loop {
                token.check()?;
                let remaining = deadline
                    .checked_duration_since(std::time::Instant::now())
                    .ok_or("knowledge execution budget exceeded")?;
                match tokio::time::timeout(
                    remaining.min(std::time::Duration::from_millis(20)),
                    request.as_mut(),
                )
                .await
                {
                    Ok(result) => {
                        token.check()?;
                        return result;
                    }
                    Err(_) => continue,
                }
            }
        })
    }

    pub(super) fn execute(
        &self,
        database: &Database,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope
        {
            return Err("frozen Kernel resource identity changed".into());
        }
        self.validate(tool, input)?;
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        if super::capability_tools::is_capability_tool(tool) {
            let action = super::capability_tools::prepare(
                tool,
                input,
                self.binding.permission.project_root.as_deref(),
            )?;
            return super::capability_tools::execute_with_cancellation(
                action,
                token,
                self.remaining_budget(database)?,
            );
        }
        if matches!(tool, "list_mcp_tools" | "call_mcp_tool") {
            return self.execute_mcp(database, tool, input, token);
        }
        if crate::resource_gateway::is_reader(tool) {
            crate::resource_gateway::execute_with_budget(
                &self.binding,
                tool,
                input,
                token,
                self.remaining_budget(database)?,
            )
        } else {
            let mut action = crate::tool_host::prepare(
                tool,
                input,
                self.binding.permission.project_root.as_deref().unwrap(),
            )?;
            if let crate::tool_host::PreparedToolAction::RunCommand { timeout, .. } = &mut action {
                *timeout = (*timeout).min(self.remaining_budget(database)?);
            }
            crate::tool_host::execute_with_cancellation(action, Some(token))
        }
    }

    fn execute_mcp(
        &self,
        database: &Database,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
    ) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + self.remaining_budget(database)?;
        let remaining = || -> Result<std::time::Duration, String> {
            token.check()?;
            deadline
                .checked_duration_since(std::time::Instant::now())
                .ok_or_else(|| "MCP tool budget exceeded".into())
        };
        let load = |id: &str| -> Result<crate::database::McpServerRecord, String> {
            let server = database
                .get_mcp_server(id)?
                .filter(|server| server.enabled)
                .ok_or("frozen MCP connection is unavailable")?;
            if self.scope.mcp_server_hashes.get(id) != Some(&server_hash(&server)) {
                return Err("MCP definition changed since Run creation".into());
            }
            Ok(server)
        };
        if tool == "call_mcp_tool" {
            let server = load(input["serverId"].as_str().unwrap())?;
            let name = input["tool"].as_str().unwrap();
            if server.id == crate::office::SERVER_ID {
                // The artifact context derives the conversation from the frozen
                // binding only; the model's input contributes nothing but the
                // artifact id, so a cross-session or forged reference cannot be
                // resolved here. Same authorization validate/decide used.
                let context = self.office_call_context();
                return crate::office::execute_with_cancellation(
                    &server,
                    name,
                    &input["arguments"],
                    self.binding.permission.project_root.as_deref(),
                    self.binding.permission.mode.as_str(),
                    Some(token),
                    remaining()?,
                    context.as_ref(),
                );
            }
            return crate::mcp::execute_owned(
                &server,
                Some((name, &input["arguments"])),
                token,
                remaining()?,
            );
        }
        let query = input["query"].as_str().unwrap_or_default().to_lowercase();
        let mut servers = Vec::new();
        for id in self.scope.mcp_server_hashes.keys() {
            let server = load(id)?;
            let tools = if server.id == crate::office::SERVER_ID {
                remaining()?;
                crate::office::verify_binary(std::path::Path::new(&server.command))?;
                crate::office::tool_definitions()
                    .into_iter()
                    .filter(|tool| {
                        tool["name"]
                            .as_str()
                            .is_some_and(|name| self.scope.office_tools.contains(name))
                    })
                    .collect()
            } else {
                crate::mcp::execute_owned(&server, None, token, remaining()?)?["tools"]
                    .as_array()
                    .cloned()
                    .ok_or("missing MCP catalog")?
            };
            let tools = tools
                .into_iter()
                .filter(|tool: &Value| {
                    query.is_empty() || tool.to_string().to_lowercase().contains(&query)
                })
                .collect::<Vec<_>>();
            if !tools.is_empty() {
                servers.push(json!({"serverId":server.id,"serverName":server.name,"tools":tools}));
            }
        }
        token.check()?;
        Ok(json!({"content":[{"type":"text","text":json!({"servers":servers}).to_string()}]}))
    }
}

impl PolicyDecisionPort for GatewayPolicy {
    fn decide(&self, run_id: &str, _tool_id: &str, tool: &str, input_json: &str) -> PolicyDecision {
        let input = serde_json::from_str::<Value>(input_json);
        if run_id != self.binding.run_id
            || input
                .as_ref()
                .ok()
                .is_none_or(|input| self.validate(tool, input).is_err())
        {
            return PolicyDecision::Deny {
                reason: "tool parameters or scope were rejected by the frozen resource policy"
                    .into(),
            };
        }
        if tool == "task_repair_escalate_start" {
            return PolicyDecision::RequireApproval;
        }
        if self.scope.lifecycle_hooks.iter().any(|hook| {
            hook.event == "before_tool"
                && hook.action == "require_approval"
                && crate::lifecycle_hooks::matcher_matches(&hook.matcher, tool)
        }) {
            return PolicyDecision::RequireApproval;
        }
        if tool == "call_mcp_tool"
            && input.as_ref().unwrap()["serverId"] == crate::office::SERVER_ID
        {
            let input = input.as_ref().unwrap();
            if self
                .prepare_office(input["tool"].as_str().unwrap(), &input["arguments"])
                .is_ok_and(|action| !action.mutates)
            {
                return PolicyDecision::Allow;
            }
        }
        // The effective policy is the frozen snapshot UNION the additional
        // authorization facts this run accumulated from human approvals (#5).
        // The frozen list is never rewritten; an added grant can only ever name
        // an exact (tool, scope) pair it was approved for, so anything outside
        // that pair still falls through to the unchanged frozen decision.
        let additional = self
            .database
            .as_ref()
            .and_then(|database| {
                database
                    .kernel_effective_authorization_grants(
                        &self.binding.run_id,
                        crate::database::now_ms(),
                    )
                    .ok()
            })
            .unwrap_or_default();
        let mut grants: Vec<Value> = self
            .binding
            .permission
            .grants
            .iter()
            .map(|grant| json!([grant.tool, grant.scope]))
            .collect();
        for grant in &additional {
            let pair = json!([grant.tool, grant.scope_value]);
            if !grants.contains(&pair) {
                grants.push(pair);
            }
        }
        let decision = super::shadow_reconcile::frozen_kernel_tool_policy(
            &json!({
                "mode": self.binding.permission.mode.as_str(), "projectRoot": self.binding.permission.project_root,
                "grants": grants,
            }),
            tool,
            input_json,
        );
        // Audit the reuse: a grant that silently authorizes work is not
        // auditable. Only a grant that actually decided this call is recorded.
        if matches!(decision, PolicyDecision::Allow) {
            if let (Some(database), Ok(parsed)) = (
                self.database.as_ref(),
                serde_json::from_str::<Value>(input_json),
            ) {
                if let Some(scope) = self.scope_key_for(tool, &parsed) {
                    if additional.iter().any(|grant| grant.matches(tool, &scope)) {
                        let _ = database.kernel_note_authorization_grant_use(
                            &self.binding.run_id,
                            tool,
                            &scope,
                        );
                    }
                }
            }
        }
        decision
    }
}

impl GatewayPolicy {
    /// The exact scope key this call is measured against, computed by the same
    /// Host helpers the frozen policy uses. `None` means the call cannot be
    /// named exactly, so no grant may cover it.
    fn scope_key_for(&self, tool: &str, input: &Value) -> Option<String> {
        super::shadow_reconcile::tool_operation_scope(
            tool,
            input,
            self.binding.permission.project_root.as_deref(),
        )
    }
}
