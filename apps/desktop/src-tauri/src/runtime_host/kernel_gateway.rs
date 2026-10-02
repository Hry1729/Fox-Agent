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

/// The identity a Host-established "the target is absent" observation carries.
///
/// It is keyed by the TARGET, not by the dispatch: several writes proposed in one
/// turn share a dispatch identity, so a dispatch-keyed record would make every
/// write after the first collide on the observation id and be dropped (the ledger
/// inserts are `ON CONFLICT DO NOTHING`). That collision is what left the second
/// and later new-file writes without a baseline and got them refused as
/// `observation_incomplete` (O11, 2026-10-01 fix verification).
fn absent_target_observation_id(target: &str) -> String {
    format!("host-precondition:{target}")
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
    fn current_mode(&self) -> Result<PermissionMode, String> {
        match &self.database {
            Some(db) => match db.execution_policy(&self.binding.conversation_id)?.mode.as_str() {
                "ask" => Ok(PermissionMode::Ask),
                "allow" => Ok(PermissionMode::Allow),
                "read_only" => Ok(PermissionMode::ReadOnly),
                _ => Err("invalid live permission mode".into()),
            },
            None => Ok(self.binding.permission.mode),
        }
    }

    /// The frozen grants that are still in force *right now*.
    ///
    /// REV-04 / M2: a grant frozen into an already-running Run must not outlive
    /// the approval it came from. `ApprovalReuse` grants are therefore
    /// re-checked against the live authorization rows on every use, so a user
    /// who tightens the permission mode (or revokes the approval) immediately
    /// stops a Run that was frozen before the change. `Resource` grants are
    /// facts about the Run itself and are never re-queried.
    pub(super) fn live_frozen_grants(&self) -> Vec<fox_engine_protocol::PermissionGrant> {
        let Some(database) = self.database.as_ref() else {
            return self.binding.permission.grants.clone();
        };
        // A reusable approval is bound to the generation that issued it.
        //
        // * `Some(frozen)` and the generation still matches: the approval may
        //   still be in force, subject to the per-grant liveness check below.
        // * `Some(frozen)` and the generation has moved on: a revocation
        //   happened, so this Run's approval-reuse grants are dead — even if the
        //   identical (tool, scope) was granted again afterwards.
        // * `None`: the binding predates the field, so the original approval
        //   identity CANNOT be proven. An approval-reuse entry must not
        //   authorize; a NEW explicit approval still works because it produces a
        //   fresh binding with a recorded generation.
        //
        // Only `ApprovalReuse` entries are affected. `Resource` entries are
        // facts about the Run itself and are never filtered.
        let epoch_matches = match self.binding.permission.approval_epoch {
            Some(frozen) => database
                .execution_policy(&self.binding.conversation_id)
                .ok()
                .map(|policy| policy.version)
                == Some(frozen),
            None => false,
        };
        self.binding
            .permission
            .grants
            .iter()
            .filter(|grant| match grant.kind {
                fox_engine_protocol::GrantKind::Resource => true,
                fox_engine_protocol::GrantKind::ApprovalReuse => {
                    epoch_matches
                        && database
                            .conversation_tool_permission_granted(
                                &self.binding.conversation_id,
                                &grant.tool,
                                &grant.scope,
                            )
                            .unwrap_or(false)
                }
            })
            .cloned()
            .collect()
    }

    /// Trusted Office authorization derived solely from the frozen Run binding:
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

    pub(super) fn remaining_budget(&self, database: &Database) -> Result<std::time::Duration, String> {
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
            return Err("[tool.invalid_input] tool input must be an object".into());
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
            if self.current_mode()? == PermissionMode::ReadOnly
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
                // Office argument preparation is a *contract* check: a malformed
                // call must reach the model as an actionable rejection with the
                // reason, not as an opaque policy denial — otherwise the model
                // retries the same invalid call and burns the task budget.
                let prepared = self
                    .prepare_office(remote_tool, &input["arguments"])
                    .map_err(|error| crate::tool_host::ToolErrorCode::InvalidInput.error(error))?;
                // A mutating Office call is a managed write like any other, so the
                // frozen task's own file roles bound it at the same boundary: a
                // path the task introduced as input material is refused here, and
                // an approval for one write cannot widen that. A read-only Office
                // operation is unaffected.
                if prepared.mutates {
                    if let (Some(database), Some(root), Some(target)) = (
                        self.database.as_ref(),
                        self.binding.permission.project_root.as_deref(),
                        prepared.target_path(),
                    ) {
                        let task_text = database.run_task_text(&self.binding.run_id)?;
                        crate::tool_host::ensure_writable_target(
                            task_text.as_deref(),
                            root,
                            target,
                        )?;
                    }
                }
            } else if self.current_mode()? == PermissionMode::ReadOnly {
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
            if self.current_mode()? == PermissionMode::ReadOnly
                && crate::database::kernel_execution_admission::operation_class(tool, &input.to_string()) != fox_engine_protocol::ActionClass::Manage {
                return Err("project permission is read-only".into());
            }
            let mut checked = input.clone();
            if matches!(tool, "write_file" | "edit_file") {
                if let Some(db) = &self.database {
                    let path_text = input["path"].as_str().ok_or("[tool.invalid_input] missing file path")?;
                    let target = crate::tool_host::canonical_file_identity(std::path::Path::new(root), path_text)?;
                    // The task's own file roles bound this Run's writes: a path it
                    // introduced as input material is refused at the same boundary
                    // an approval would otherwise widen.
                    let task_text = db.run_task_text(&self.binding.run_id)?;
                    crate::tool_host::ensure_writable_target(
                        task_text.as_deref(),
                        root,
                        std::path::Path::new(&target),
                    )?;
                    // REV-01: the model's declared `expectedVersion` is a
                    // *precondition claim*. It is verified against this Run's
                    // observations and never silently upgraded to the newest
                    // one, so a stale candidate cannot ride a later read.
                    let declared = input
                        .get("expectedVersion")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty());
                    let observed = db
                        .host_observation_for_version(&self.binding.run_id, &target, declared)?;
                    // A target that provably does not exist yet needs no
                    // observation; a new file is created, and its own version
                    // check still refuses to clobber a concurrent creation.
                    let baseline = crate::tool_host::write_baseline(
                        std::path::Path::new(&target),
                        declared,
                        observed.as_ref().map(|value| value.version.as_str()),
                    )?;
                    checked["expectedVersion"] = json!(baseline);
                    checked.as_object_mut().map(|o| o.remove("readVersion"));
                    // REV-05: a precise edit may rest on a partial read, but the
                    // fragment it replaces must itself have been delivered.
                    if tool == "edit_file" {
                        let baseline = observed.ok_or_else(|| {
                            crate::tool_host::ToolErrorCode::Conflict.error(
                                "edit_file requires an observation of the target",
                            )
                        })?;
                        self.require_observed_fragment(db, &baseline, &input)?;
                    }
                }
            }
            crate::tool_host::prepare(tool, &checked, root)?;
        }
        Ok(())
    }

    /// Record the Host's own "this target does not exist" observation for a
    /// first write, so the credential issued at admission has a real baseline.
    ///
    /// The check is the same independent filesystem probe the reader path uses
    /// (`resource_gateway::observe_missing_file_target`): only a positively
    /// absent target inside the frozen root produces an observation, and every
    /// other case leaves the state exactly as it was — an existing file still
    /// needs a read. Best effort by design: a recording failure cannot make a
    /// write *more* permitted, and admission refuses the write on its own terms.
    fn record_absent_target_observation(&self, database: &Database, input: &Value) {
        let (Some(root), Some(path)) = (
            self.binding.permission.project_root.as_deref(),
            input["path"].as_str(),
        ) else {
            return;
        };
        let Ok(target) = crate::tool_host::canonical_file_identity(std::path::Path::new(root), path)
        else {
            return;
        };
        if database
            .host_observation_for(&self.binding.run_id, &target)
            .ok()
            .flatten()
            .is_some()
        {
            return;
        }
        let Ok(missing) =
            crate::resource_gateway::observe_missing_file_target(&self.binding, &target)
        else {
            return;
        };
        let _ = database.record_host_observation(
            &self.binding.run_id,
            &self.binding.conversation_id,
            &missing.host_observation(&absent_target_observation_id(&target)),
        );
    }

    /// REV-05 fragment coverage: the `oldText` an `edit_file` replaces must lie
    /// inside a range this Run actually delivered for the bound version. The
    /// Host locates the fragment in that version's own text, so a model cannot
    /// claim coverage it never received.
    #[allow(clippy::too_many_arguments)]
    fn require_observed_fragment(
        &self,
        db: &Database,
        baseline: &fox_engine_protocol::HostObservation,
        input: &Value,
    ) -> Result<(), String> {
        let old_text = input
            .get("oldText")
            .and_then(Value::as_str)
            .ok_or("[tool.invalid_input] edit_file requires a string oldText")?;
        if old_text.is_empty() {
            return Err("[tool.invalid_input] oldText must not be empty".to_owned());
        }
        let root = self
            .binding
            .permission
            .project_root
            .as_deref()
            .ok_or("tool requires a frozen project root")?;
        let target = crate::tool_host::canonical_file_identity(std::path::Path::new(root), input["path"].as_str().unwrap_or_default())?;
        // The version's own text, read back by the Host and re-hashed, so the
        // fragment is located against exactly the bytes the observation named.
        let bytes = std::fs::read(&target).map_err(|error| format!("[tool.file_conflict] cannot re-read the target: {error}"))?;
        if crate::tool_host::file_version(&bytes) != baseline.version {
            return Err(crate::tool_host::ToolErrorCode::Conflict.error(
                "File changed since the bound observation; read it again and retry",
            ));
        }
        let text = String::from_utf8_lossy(&bytes);
        let units: Vec<u16> = text.encode_utf16().collect();
        let needle: Vec<u16> = old_text.encode_utf16().collect();
        if needle.is_empty() || needle.len() > units.len() {
            return Err(crate::tool_host::ToolErrorCode::Conflict.error(
                "the replaced fragment was not delivered by any read in this Run",
            ));
        }
        // Every occurrence the edit will touch. `replaceAll` replaces all of
        // them, so each one must have been delivered; a single-match edit only
        // needs its own occurrence covered.
        let replace_all = input
            .get("replaceAll")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut occurrences: Vec<(u64, u64)> = units
            .windows(needle.len())
            .enumerate()
            .filter(|(_, window)| *window == needle.as_slice())
            .map(|(index, _)| (index as u64, index as u64 + needle.len() as u64))
            .collect();
        if occurrences.is_empty() {
            return Err(crate::tool_host::ToolErrorCode::Conflict.error(
                "the replaced fragment is not present in the observed version",
            ));
        }
        if !replace_all {
            occurrences.truncate(1);
        }
        let observed = db.host_observations_for_target(&self.binding.run_id, &target)?;
        for (start, end) in occurrences {
            let covered = observed
                .iter()
                .filter(|observation| observation.version == baseline.version)
                .any(|observation| observation.covers_units(start, end));
            if !covered {
                return Err(crate::tool_host::ToolErrorCode::Conflict.error(
                    "the replaced fragment was not delivered by any read in this Run",
                ));
            }
        }
        Ok(())
    }

    /// REV-05 whole-file replacement: `write_file` replaces the entire content,
    /// so it needs either a whole-file observation whose content actually
    /// REACHED the model or an explicit, purpose-specific replacement
    /// authorization. Returns the reason a dedicated approval is required, or
    /// `None` when the read delivered the whole file *and* the model view kept
    /// every byte of it.
    fn whole_file_replacement_gap(&self, tool: &str, input: &Value) -> Result<Option<String>, String> {
        if tool != "write_file" {
            return Ok(None);
        }
        let Some(db) = &self.database else {
            return Ok(None);
        };
        let root = self
            .binding
            .permission
            .project_root
            .as_deref()
            .ok_or("tool requires a frozen project root")?;
        let Some(path_text) = input["path"].as_str() else {
            return Ok(None);
        };
        let target = match crate::tool_host::canonical_file_identity(std::path::Path::new(root), path_text) {
            Ok(target) => target,
            Err(_) => return Ok(None),
        };
        let declared = input
            .get("expectedVersion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let observation = db.host_observation_for_version(&self.binding.run_id, &target, declared)?;
        match observation {
            // The source fact AND the model delivery fact must both hold. A
            // whole-file read whose model projection was bounded (the durable
            // result is longer than one model view) delivered the bytes to the
            // tool boundary but not to the model, so it never authorizes a
            // replacement on its own.
            Some(observation)
                if db.model_delivery_covered_whole_file(&self.binding.run_id, &observation)? =>
            {
                Ok(None)
            }
            Some(_) => Ok(Some(
                "whole-file replacement needs a complete same-version read that actually \
                 reached the model, or an explicit replacement confirmation"
                    .to_owned(),
            )),
            // No observation at all: `validate` already refuses with a conflict.
            None => Ok(None),
        }
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
        self.execute_context_resource_with_terminal_notice(
            database, attachments_dir, sessions_dir, skills_dir, tool, input, token, None,
        )
    }

    pub(super) fn execute_context_resource_with_terminal_notice(
        &self,
        database: &Database,
        attachments_dir: &std::path::Path,
        sessions_dir: &std::path::Path,
        skills_dir: &std::path::Path,
        tool: &str,
        input: &Value,
        token: &CancellationToken,
        on_terminal: Option<std::sync::Arc<dyn Fn(&str) + Send + Sync>>,
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
            return super::background_jobs::execute(database, attachments_dir, sessions_dir, &self.binding.run_id, tool, input, Some(token.clone()), self.remaining_budget(database)?, on_terminal);
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
                &database.kernel_approval_identity(&self.binding.run_id, tool_id)?,
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

    pub(super) fn execute_reader(
        &self, database: &Database, tool: &str, input: &Value,
        tool_call_id: &str, token: &CancellationToken,
    ) -> Result<Value, String> {
        token.check()?;
        if !crate::resource_gateway::is_reader(tool) { return Err("not a reader tool".into()); }
        if database.run_control_binding(&self.binding.run_id)?.as_ref() != Some(&self.binding)
            || database.kernel_host_scope(&self.binding.run_id)? != self.scope {
            return Err("frozen Kernel resource identity changed".into());
        }
        self.validate(tool, input)?;
        database.kernel_validate_resource_acquisition_for(&self.binding.run_id, Some(tool))?;
        super::managed_files::execute_observed_reader(database, &self.binding, tool, input,
            tool_call_id, token, self.remaining_budget(database)?)
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
            // Callers with a durable tool identity use execute_reader directly.
            // Other Host-owned reader operations get their own observation id.
            self.execute_reader(database, tool, input,
                &format!("host-read:{}", uuid::Uuid::new_v4()), token)
        } else {
            let mut action = crate::tool_host::prepare(
                tool,
                input,
                self.binding.permission.project_root.as_deref().unwrap(),
            )?;
            if let crate::tool_host::PreparedToolAction::CommandJob {root,input,..} = &action {
                return super::command_jobs::execute(database,&self.binding.run_id,input,root,token,self.remaining_budget(database)?);
            }
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
        if run_id != self.binding.run_id {
            return PolicyDecision::Deny {
                reason: "Run identity differs from the frozen resource policy".into(),
            };
        }
        let validation = input.as_ref()
            .map_err(|_| "[tool.invalid_input] Tool input must be valid JSON".to_owned())
            .and_then(|input| self.validate(tool, input));
        if let Err(error) = validation {
            let code = crate::tool_host::ToolErrorCode::classify(&error);
            if matches!(code,
                Some(crate::tool_host::ToolErrorCode::InvalidInput | crate::tool_host::ToolErrorCode::Conflict))
            {
                return PolicyDecision::Reject {
                    code: code.unwrap().as_str().into(),
                    message: super::redact_execution_diagnostic(&error, 2000),
                };
            }
            // A refusal the model can act on keeps its own reason: "the target is
            // the task's read-only input" or "the path is outside what this Run
            // may write" tells it to choose a different path, while the generic
            // sentence only invites the same call again.
            if matches!(code,
                Some(crate::tool_host::ToolErrorCode::ReadOnlyInput
                    | crate::tool_host::ToolErrorCode::PermissionDenied))
            {
                return PolicyDecision::Deny {
                    reason: super::redact_execution_diagnostic(&error, 2000),
                };
            }
            return PolicyDecision::Deny {
                reason: "Tool is not permitted by the frozen resource policy".into(),
            };
        }
        // The absent-target baseline is established HERE, while the write is
        // still a proposal: the execution credential is issued later, and the
        // claim-time host-observation requirement is satisfied by the record
        // this leaves behind. Without it, a first write of a new file would be
        // refused as `observation_incomplete` even though nothing is in the way.
        if matches!(tool, "write_file" | "edit_file") {
            if let (Some(database), Ok(input)) = (self.database.as_ref(), input.as_ref()) {
                self.record_absent_target_observation(database, input);
            }
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
        if tool == "run_command" && crate::database::kernel_execution_admission::operation_class(tool, input_json) == fox_engine_protocol::ActionClass::Manage {
            return PolicyDecision::Allow;
        }
        // REV-05: a whole-file replacement is a different, stronger operation
        // than a precise edit. It never rides on a partial read, and it never
        // rides on an ordinary one-shot approval or a reusable grant: it needs
        // the purpose-specific replacement authorization (see
        // `kernel_whole_file_replace_grants`).
        if let Ok(Some(_reason)) = self.whole_file_replacement_gap(tool, input.as_ref().unwrap_or(&Value::Null)) {
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
            .live_frozen_grants()
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
                "mode": self.current_mode().unwrap_or(PermissionMode::ReadOnly).as_str(), "projectRoot": self.binding.permission.project_root,
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

#[cfg(test)]
mod revision_tests {
    use super::*;

    /// Regression, 2026-10-01 fix verification (O11): two new files proposed in
    /// one turn must each get their own absent-target observation. A dispatch-
    /// keyed id collided on the second one, so its credential had no baseline
    /// and the write was refused as `observation_incomplete`.
    #[test]
    fn absent_target_observations_are_keyed_by_target_not_by_dispatch() {
        let first = absent_target_observation_id("\\\\?\\D:\\p\\out\\a.csv");
        let second = absent_target_observation_id("\\\\?\\D:\\p\\out\\b.csv");
        assert_ne!(first, second, "different targets must not share an id");
        assert_eq!(
            first,
            absent_target_observation_id("\\\\?\\D:\\p\\out\\a.csv"),
            "the same target keeps one stable id"
        );
    }

    #[test]
    fn r1_matching_errors_are_repairable_but_scope_denials_remain_denials() {
        use fox_engine_protocol::{FrozenPermission, TimeBudgets, ExecutionAuthority, ResourceExecutor};
        let root=std::env::temp_dir().join(format!("fox-r1-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("a.txt"),"hello hello").unwrap();
        let permission=FrozenPermission {mode:PermissionMode::Ask,project_root:Some(root.to_string_lossy().into_owned()),grants:vec![],
        approval_epoch: None,
    };
        let mut policy=GatewayPolicy {
            binding:RunControlBinding {schema_version:1,run_id:"r".into(),conversation_id:"c".into(),engine_id:"pi".into(),
                execution_profile_id:"legacy".into(),authority:ExecutionAuthority::Authoritative,read_only_executor:ResourceExecutor::Rust,
                permission_snapshot_id:Database::run_control_permission_hash(&permission).unwrap(),permission,budgets:TimeBudgets::default()},
            scope:KernelHostScope {schema_version:1,tool_names:["edit_file".into()].into_iter().collect(),
                mcp_server_hashes:Default::default(),knowledge_reference_hashes:Default::default(),
                knowledge_connection_hashes:Default::default(),office_tools:Default::default(),lifecycle_hooks:vec![]},
            database:None,sessions_dir:None,artifacts_dir:None,
        };
        for old in ["absent","hello"] {
            let input=json!({"path":"a.txt","oldText":old,"newText":"x"}).to_string();
            let PolicyDecision::Reject {code,message}=policy.decide("r","t","edit_file",&input) else {panic!("match error must not become permission denial")};
            assert_eq!(code,"tool.invalid_input");
            assert!(message.contains(if old=="absent" {"not found"} else {"occurs 2 times"}));
        }
        let input=json!({"path":"a.txt","oldText":"hello hello","newText":"fixed",
            "expectedVersion":crate::tool_host::file_version(b"hello hello")}).to_string();
        assert!(matches!(policy.decide("r","t","edit_file",&input),PolicyDecision::RequireApproval));
        assert!(matches!(policy.decide("wrong","t","edit_file",&input),PolicyDecision::Deny {..}));
        policy.binding.permission.mode = PermissionMode::ReadOnly;
        policy.binding.permission_snapshot_id = Database::run_control_permission_hash(&policy.binding.permission).unwrap();
        policy.scope.tool_names.insert("run_command".into());
        for action in ["status", "output", "cancel"] {
            assert!(matches!(policy.decide("r", "manage", "run_command", &json!({"action":action,"jobId":"owned-job"}).to_string()), PolicyDecision::Allow));
        }
        assert!(matches!(policy.decide("r", "start", "run_command", &json!({"action":"start","command":"echo no"}).to_string()), PolicyDecision::Deny { .. }));
        policy.scope.tool_names.clear();
        assert!(matches!(policy.decide("r","t","edit_file",&input),PolicyDecision::Deny {..}));
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(),"hello hello");
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Regression, 2026-10-01 review (R01): the same read-only-input rule bounds
    /// a **mutating Office call** at the gateway, so the task's own input cannot
    /// be rewritten through `call_mcp_tool` either. A read-only Office operation
    /// on the same path stays allowed.
    #[test]
    fn an_office_write_cannot_target_the_tasks_read_only_input() {
        use fox_engine_protocol::{FrozenPermission, TimeBudgets, ExecutionAuthority, ResourceExecutor};
        let root = std::env::temp_dir().join(format!("fox-office-ro-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("source.xlsx"), b"placeholder").unwrap();
        let db = Database::open(root.join("facts.db")).unwrap();
        let conversation = db
            .create_conversation(db.default_agent_id(), None, Some(root.to_str().unwrap()), Some("ask"))
            .unwrap();
        let run = db
            .create_run(&conversation.id, "读取 source.xlsx，生成 报告.docx。", None)
            .unwrap()
            .run
            .id;
        let permission = FrozenPermission {
            mode: PermissionMode::Ask,
            project_root: Some(root.to_string_lossy().into_owned()),
            grants: vec![],
            approval_epoch: None,
        };
        let mut policy = GatewayPolicy {
            binding: RunControlBinding {
                schema_version: 1, run_id: run.clone(), conversation_id: conversation.id.clone(),
                engine_id: "pi".into(), execution_profile_id: "legacy".into(),
                authority: ExecutionAuthority::Authoritative,
                read_only_executor: ResourceExecutor::Rust,
                permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(),
                permission, budgets: TimeBudgets::default(),
            },
            scope: KernelHostScope {
                schema_version: 1,
                tool_names: ["call_mcp_tool".into()].into_iter().collect(),
                mcp_server_hashes: std::collections::BTreeMap::from([(
                    crate::office::SERVER_ID.to_owned(),
                    "office-server-hash".to_owned(),
                )]),
                knowledge_reference_hashes: Default::default(),
                knowledge_connection_hashes: Default::default(),
                office_tools: ["office_create".into(), "office_read".into()].into_iter().collect(),
                lifecycle_hooks: vec![],
            },
            database: Some(db.clone()),
            sessions_dir: None,
            artifacts_dir: None,
        };
        // Writing the task's own input back is refused before any side effect,
        // and the refusal names the reason so the model can pick another path.
        let write_input = json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "source.xlsx", "overwrite": true}});
        let decision = policy.decide(&run, "tc", "call_mcp_tool", &write_input.to_string());
        let PolicyDecision::Deny { reason } = decision else {
            panic!("an Office write to a read-only input must be denied: {decision:?}");
        };
        assert!(reason.contains("tool.read_only_input"), "{reason}");
        assert!(reason.contains("source.xlsx"), "{reason}");
        assert!(
            !reason.contains("not permitted by the frozen resource policy"),
            "the refusal must keep its own actionable reason: {reason}"
        );

        // A different output the task asked for is not affected.
        let allowed_input = json!({"serverId": crate::office::SERVER_ID, "tool": "office_create",
            "arguments": {"output": "报告.docx"}});
        assert!(
            !matches!(policy.decide(&run, "tc", "call_mcp_tool", &allowed_input.to_string()),
                PolicyDecision::Deny { reason } if reason.contains("tool.read_only_input")),
            "a declared output must stay writable"
        );

        // A read-only Office operation on the input is not a write at all.
        let read_input = json!({"serverId": crate::office::SERVER_ID, "tool": "office_read",
            "arguments": {"file": "source.xlsx", "mode": "text"}});
        assert!(
            !matches!(policy.decide(&run, "tc", "call_mcp_tool", &read_input.to_string()),
                PolicyDecision::Deny { reason } if reason.contains("tool.read_only_input")),
            "reading the input must stay allowed"
        );
        policy.database = None;
        let _ = std::fs::remove_dir_all(root);
    }
}
