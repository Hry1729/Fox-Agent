use crate::{
    app_state::AppState,
    database::{
        AgentIdRequest, AgentRecord, ApiResponse, ExpertPackageVersionRecord,
        InstallExpertPackageVersionRequest, KnowledgeReference,
    },
};
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use tauri::State;

const PACKAGE_SCHEMA_VERSION: u32 = 3;
const MAX_PACKAGE_BYTES: usize = 2 * 1024 * 1024;
const MAX_FILES: usize = 32;
const MAX_FILE_BYTES: usize = 256 * 1024;
const MAX_TOTAL_FILE_BYTES: usize = 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 64 * 1024;

const KNOWN_TOOLS: &[&str] = &[
    "read",
    "ls",
    "find",
    "grep",
    "read_attachment",
    "write_file",
    "edit_file",
    "run_command",
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
    "child_agent_list",
    "child_run_start",
    "child_run_collect",
    "child_run_cancel",
    "memory_search",
    "memory_propose",
    "list_knowledge_bases",
    "search_knowledge",
    "read_knowledge_document",
    "query_knowledge_graph",
    "list_mcp_tools",
    "call_mcp_tool",
    "work_snapshot_get",
    "goal_propose",
    "goal_complete",
    "task_create_many",
    "task_update",
    "task_evidence_add",
    "task_evidence_validate",
    "plan_revision_create",
    "review_finding_add",
    "review_finding_resolve",
    "acceptance_submit",
    "workflow_snapshot_get",
    "workflow_start",
    "workflow_stage_start",
    "workflow_stage_complete",
    "workflow_stage_fail",
    "workflow_cancel",
    "team_snapshot_get",
    "team_start",
    "team_member_start",
    "team_collect",
    "team_cancel",
];

pub(crate) fn is_known_tool(tool: &str) -> bool {
    KNOWN_TOOLS.contains(&tool)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpertPackage {
    schema_version: u32,
    id: String,
    version: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    category: String,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    default_model: String,
    #[serde(default)]
    opening_suggestions: Vec<String>,
    compatibility: PackageCompatibility,
    agent: PackageAgent,
    #[serde(default)]
    resources: PackageResources,
    #[serde(default)]
    permissions: PackagePermissions,
    #[serde(default)]
    workflow: Option<Value>,
    #[serde(default)]
    team: Option<Value>,
    files: BTreeMap<String, PackageFile>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageCompatibility {
    fox_version: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageAgent {
    system_prompt: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageResources {
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    tools: Vec<String>,
    #[serde(default)]
    mcp_servers: Vec<String>,
    #[serde(default)]
    knowledge_references: Vec<KnowledgeReference>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackagePermissions {
    #[serde(default)]
    project: ProjectPermission,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ProjectPermission {
    #[default]
    None,
    ReadOnly,
    Ask,
    Allow,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PackageFile {
    sha256: String,
    content: String,
}

#[derive(Debug, Clone)]
struct ValidatedPackage {
    package: ExpertPackage,
    raw: Value,
    package_hash: String,
    prompt: String,
    version: Version,
    compatibility: VersionReq,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MissingPackageResources {
    agents: Vec<String>,
    skills: Vec<String>,
    tools: Vec<String>,
    mcp_servers: Vec<String>,
    knowledge_references: Vec<KnowledgeReference>,
}

impl MissingPackageResources {
    fn is_empty(&self) -> bool {
        self.skills.is_empty()
            && self.agents.is_empty()
            && self.tools.is_empty()
            && self.mcp_servers.is_empty()
            && self.knowledge_references.is_empty()
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertPackagePreview {
    package_id: String,
    name: String,
    version: String,
    package_hash: String,
    action: String,
    current_version: Option<String>,
    current_hash: Option<String>,
    compatible: bool,
    fox_version: String,
    requested_project_permission: String,
    missing_resources: MissingPackageResources,
    warnings: Vec<String>,
    can_install: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertPackageInspectRequest {
    package: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertPackageInstallRequest {
    package: Value,
    expected_current_hash: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpertPackageRollbackRequest {
    expert_id: String,
    version: String,
    expected_current_hash: Option<String>,
}

#[derive(Debug)]
struct PackageError {
    code: &'static str,
    message: String,
}

impl PackageError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "expert.package_invalid",
            message: message.into(),
        }
    }
}

fn package_failure<T: Serialize>(error: PackageError) -> ApiResponse<T> {
    ApiResponse::failure(error.code, error.message, false)
}

fn project_permission_label(permission: &ProjectPermission) -> &'static str {
    match permission {
        ProjectPermission::None => "none",
        ProjectPermission::ReadOnly => "read_only",
        ProjectPermission::Ask => "ask",
        ProjectPermission::Allow => "allow",
    }
}

fn parse_package(raw: Value) -> Result<ValidatedPackage, PackageError> {
    let encoded = serde_json::to_vec(&raw)
        .map_err(|error| PackageError::invalid(format!("package cannot be encoded: {error}")))?;
    if encoded.len() > MAX_PACKAGE_BYTES {
        return Err(PackageError::invalid(format!(
            "package exceeds the {MAX_PACKAGE_BYTES} byte limit"
        )));
    }
    let package: ExpertPackage = serde_json::from_value(raw.clone()).map_err(|error| {
        PackageError::invalid(format!(
            "package manifest does not match schema v1: {error}"
        ))
    })?;
    if !(1..=PACKAGE_SCHEMA_VERSION).contains(&package.schema_version) {
        return Err(PackageError::invalid(format!(
            "unsupported schemaVersion {}; expected 1-{PACKAGE_SCHEMA_VERSION}",
            package.schema_version
        )));
    }
    validate_package_id(&package.id)?;
    validate_text("name", &package.name, 1, 120)?;
    validate_text("description", &package.description, 0, 1_000)?;
    validate_text("category", &package.category, 0, 80)?;
    validate_text("defaultModel", &package.default_model, 0, 160)?;
    if package.opening_suggestions.len() > 6 {
        return Err(PackageError::invalid(
            "openingSuggestions may contain at most 6 entries",
        ));
    }
    for suggestion in &package.opening_suggestions {
        validate_text("openingSuggestion", suggestion, 1, 240)?;
    }
    let version = Version::parse(&package.version)
        .map_err(|error| PackageError::invalid(format!("version is not SemVer: {error}")))?;
    let compatibility = VersionReq::parse(&package.compatibility.fox_version).map_err(|error| {
        PackageError::invalid(format!("compatibility.foxVersion is invalid: {error}"))
    })?;
    if let Some(workflow) = package.workflow.as_ref().filter(|value| !value.is_null()) {
        if package.schema_version < 2 {
            return Err(PackageError::invalid(
                "workflow requires package schemaVersion 2",
            ));
        }
        crate::expert_workflows::validate_workflow_definition(workflow)
            .map_err(PackageError::invalid)?;
    }
    if let Some(team) = package.team.as_ref().filter(|value| !value.is_null()) {
        if package.schema_version < 3 {
            return Err(PackageError::invalid(
                "team requires package schemaVersion 3",
            ));
        }
        let team = crate::expert_teams::parse_team(team.clone()).map_err(PackageError::invalid)?;
        let declared_tools = package
            .resources
            .tools
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        for required in crate::expert_teams::TEAM_TOOLS {
            if !declared_tools.contains(required) {
                return Err(PackageError::invalid(format!(
                    "team package resources.tools must declare '{required}'"
                )));
            }
        }
        for member in &team.members {
            if let Some(tool) = member
                .allowed_tools
                .iter()
                .find(|tool| !declared_tools.contains(tool.as_str()))
            {
                return Err(PackageError::invalid(format!(
                    "team member '{}' tool '{}' is not declared by package resources.tools",
                    member.id, tool
                )));
            }
        }
    }
    validate_distinct_ids("resources.skills", &package.resources.skills)?;
    validate_distinct_ids("resources.tools", &package.resources.tools)?;
    validate_distinct_ids("resources.mcpServers", &package.resources.mcp_servers)?;
    let mut knowledge = HashSet::new();
    for reference in &package.resources.knowledge_references {
        reference.validate().map_err(PackageError::invalid)?;
        let key = format!(
            "{}:{}:{}:{}",
            reference.source,
            reference.provider_key,
            reference.connection_id.as_deref().unwrap_or_default(),
            reference.id
        );
        if !knowledge.insert(key) {
            return Err(PackageError::invalid(
                "resources.knowledgeReferences contains duplicates",
            ));
        }
    }
    if package.files.is_empty() || package.files.len() > MAX_FILES {
        return Err(PackageError::invalid(format!(
            "files must contain between 1 and {MAX_FILES} entries"
        )));
    }
    let mut total_bytes = 0usize;
    for (path, file) in &package.files {
        validate_package_path(path)?;
        let bytes = file.content.as_bytes();
        if bytes.len() > MAX_FILE_BYTES {
            return Err(PackageError::invalid(format!(
                "file {path} exceeds the {MAX_FILE_BYTES} byte limit"
            )));
        }
        total_bytes = total_bytes.saturating_add(bytes.len());
        validate_sha256(&file.sha256, path)?;
        let actual = hex::encode(Sha256::digest(bytes));
        if actual != file.sha256 {
            return Err(PackageError::invalid(format!(
                "file hash mismatch for {path}"
            )));
        }
    }
    if total_bytes > MAX_TOTAL_FILE_BYTES {
        return Err(PackageError::invalid(format!(
            "embedded files exceed the {MAX_TOTAL_FILE_BYTES} byte limit"
        )));
    }
    validate_package_path(&package.agent.system_prompt)?;
    let prompt = package
        .files
        .get(&package.agent.system_prompt)
        .ok_or_else(|| PackageError::invalid("agent.systemPrompt does not exist in files"))?
        .content
        .trim()
        .to_owned();
    if prompt.is_empty() || prompt.len() > MAX_PROMPT_BYTES {
        return Err(PackageError::invalid(format!(
            "system prompt must contain 1 to {MAX_PROMPT_BYTES} bytes"
        )));
    }
    let package_hash = hex::encode(Sha256::digest(canonical_json(&raw).as_bytes()));
    Ok(ValidatedPackage {
        package,
        raw,
        package_hash,
        prompt,
        version,
        compatibility,
    })
}

fn validate_package_id(value: &str) -> Result<(), PackageError> {
    if value.len() < 3
        || value.len() > 128
        || value.starts_with('.')
        || value.ends_with('.')
        || value.contains("..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b".-".contains(&byte))
    {
        return Err(PackageError::invalid(
            "id must be 3-128 lowercase letters, digits, dots, or hyphens",
        ));
    }
    Ok(())
}

fn validate_text(
    field: &str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), PackageError> {
    let length = value.trim().chars().count();
    if length < minimum || length > maximum || value.chars().any(char::is_control) {
        return Err(PackageError::invalid(format!(
            "{field} must contain {minimum}-{maximum} visible characters"
        )));
    }
    Ok(())
}

fn validate_distinct_ids(field: &str, values: &[String]) -> Result<(), PackageError> {
    if values.len() > 128 {
        return Err(PackageError::invalid(format!(
            "{field} contains too many entries"
        )));
    }
    let mut seen = HashSet::new();
    for value in values {
        if value.trim().is_empty()
            || value.len() > 160
            || value.chars().any(|character| character.is_control())
            || !seen.insert(value)
        {
            return Err(PackageError::invalid(format!(
                "{field} contains an invalid or duplicate id"
            )));
        }
    }
    Ok(())
}

fn validate_package_path(value: &str) -> Result<(), PackageError> {
    if !value.starts_with("./")
        || value.len() > 240
        || value.contains('\\')
        || value.contains(':')
        || value.chars().any(char::is_control)
    {
        return Err(PackageError::invalid(format!(
            "unsafe package path: {value}"
        )));
    }
    let relative = &value[2..];
    let reserved = [
        "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
        "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];
    if relative.is_empty()
        || relative.split('/').any(|segment| {
            let device = segment
                .split('.')
                .next()
                .unwrap_or_default()
                .to_ascii_lowercase();
            segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.ends_with(['.', ' '])
                || reserved.contains(&device.as_str())
        })
    {
        return Err(PackageError::invalid(format!(
            "unsafe package path: {value}"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, path: &str) -> Result<(), PackageError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PackageError::invalid(format!(
            "file {path} has an invalid lowercase SHA-256"
        )));
    }
    Ok(())
}

fn canonical_json(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => serde_json::to_string(value).expect("serialize JSON string"),
        Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Object(values) => {
            let sorted = values.iter().collect::<BTreeMap<_, _>>();
            format!(
                "{{{}}}",
                sorted
                    .into_iter()
                    .map(|(key, value)| format!(
                        "{}:{}",
                        serde_json::to_string(key).expect("serialize JSON key"),
                        canonical_json(value)
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

fn missing_resources(
    state: &AppState,
    package: &ValidatedPackage,
) -> Result<MissingPackageResources, PackageError> {
    let available_skills = crate::skills::scan_skills(&state.skills_dir, &[])
        .map_err(|error| PackageError {
            code: "expert.package_resource_scan_failed",
            message: error,
        })?
        .into_iter()
        .filter(|skill| skill.record.valid)
        .map(|skill| skill.record.id)
        .collect::<HashSet<_>>();
    let available_tools = KNOWN_TOOLS.iter().copied().collect::<HashSet<_>>();
    let available_agents = state
        .database
        .list_child_agents()
        .map_err(|message| PackageError {
            code: "expert.package_storage",
            message,
        })?
        .into_iter()
        .map(|agent| agent.id)
        .collect::<HashSet<_>>();
    let available_mcps = state
        .database
        .list_mcp_servers()
        .map_err(|message| PackageError {
            code: "expert.package_storage",
            message,
        })?
        .into_iter()
        .map(|server| server.id)
        .collect::<HashSet<_>>();
    let available_local_knowledge = state
        .local_knowledge
        .list_bases()
        .map_err(|error| PackageError {
            code: "expert.package_resource_scan_failed",
            message: error.to_string(),
        })?
        .into_iter()
        .map(|base| base.id)
        .collect::<HashSet<_>>();
    let available_remote_knowledge = state
        .database
        .known_remote_knowledge_references()
        .map_err(|message| PackageError {
            code: "expert.package_storage",
            message,
        })?
        .into_iter()
        .map(|reference| {
            (
                reference.provider_key,
                reference.connection_id.unwrap_or_default(),
                reference.id,
            )
        })
        .collect::<HashSet<_>>();
    Ok(MissingPackageResources {
        agents: package
            .package
            .team
            .as_ref()
            .filter(|value| !value.is_null())
            .and_then(|value| crate::expert_teams::parse_team(value.clone()).ok())
            .map(|team| {
                team.members
                    .into_iter()
                    .map(|member| member.agent_id)
                    .filter(|id| !available_agents.contains(id))
                    .collect()
            })
            .unwrap_or_default(),
        skills: package
            .package
            .resources
            .skills
            .iter()
            .filter(|id| !available_skills.contains(*id))
            .cloned()
            .collect(),
        tools: package
            .package
            .resources
            .tools
            .iter()
            .filter(|id| !available_tools.contains(id.as_str()))
            .cloned()
            .collect(),
        mcp_servers: package
            .package
            .resources
            .mcp_servers
            .iter()
            .filter(|id| !available_mcps.contains(*id))
            .cloned()
            .collect(),
        knowledge_references: package
            .package
            .resources
            .knowledge_references
            .iter()
            .filter(|reference| match reference.source.as_str() {
                "local" => !available_local_knowledge.contains(&reference.id),
                "remote" => !available_remote_knowledge.contains(&(
                    reference.provider_key.clone(),
                    reference.connection_id.clone().unwrap_or_default(),
                    reference.id.clone(),
                )),
                _ => true,
            })
            .cloned()
            .collect(),
    })
}

fn inspect(
    state: &AppState,
    package: &ValidatedPackage,
) -> Result<ExpertPackagePreview, PackageError> {
    let current = state
        .database
        .get_agent_by_package_id(&package.package.id)
        .map_err(|message| PackageError {
            code: "expert.package_storage",
            message,
        })?;
    let fox_version = Version::parse(env!("CARGO_PKG_VERSION")).expect("valid Fox SemVer");
    let compatible = package.compatibility.matches(&fox_version);
    let missing_resources = missing_resources(state, package)?;
    let (action, version_allowed) = match &current {
        None => ("install", true),
        Some(current) => {
            let current_version = Version::parse(&current.package_version).map_err(|error| {
                PackageError::invalid(format!("installed package version is invalid: {error}"))
            })?;
            if package.version > current_version {
                ("upgrade", true)
            } else if package.version < current_version {
                ("downgrade", false)
            } else if current.package_hash.as_deref() == Some(package.package_hash.as_str()) {
                ("no_change", true)
            } else {
                ("conflict", false)
            }
        }
    };
    let mut warnings = Vec::new();
    if matches!(
        package.package.permissions.project,
        ProjectPermission::Allow
    ) {
        warnings.push(
            "package requests direct project write access; installation does not grant it"
                .to_owned(),
        );
    } else if !matches!(
        package.package.permissions.project,
        ProjectPermission::None | ProjectPermission::ReadOnly
    ) {
        warnings.push(
            "package requests project access; the Host will still require runtime authorization"
                .to_owned(),
        );
    }
    if !compatible {
        warnings.push(format!(
            "package requires Fox {}, current version is {}",
            package.package.compatibility.fox_version, fox_version
        ));
    }
    if !missing_resources.is_empty() {
        warnings.push("one or more required resources are unavailable".to_owned());
    }
    Ok(ExpertPackagePreview {
        package_id: package.package.id.clone(),
        name: package.package.name.clone(),
        version: package.package.version.clone(),
        package_hash: package.package_hash.clone(),
        action: action.to_owned(),
        current_version: current.as_ref().map(|agent| agent.package_version.clone()),
        current_hash: current.and_then(|agent| agent.package_hash),
        compatible,
        fox_version: fox_version.to_string(),
        requested_project_permission: project_permission_label(
            &package.package.permissions.project,
        )
        .to_owned(),
        can_install: compatible && missing_resources.is_empty() && version_allowed,
        missing_resources,
        warnings,
    })
}

fn expert_id(package_id: &str) -> String {
    let digest = hex::encode(Sha256::digest(package_id.as_bytes()));
    format!("fox-package-{}", &digest[..20])
}

fn compiled_manifest(package: &ValidatedPackage) -> Value {
    let local_knowledge = package
        .package
        .resources
        .knowledge_references
        .iter()
        .filter(|reference| reference.source == "local")
        .map(|reference| reference.id.clone())
        .collect::<Vec<_>>();
    json!({
        "version": package.package.version,
        "manifestSchemaVersion": package.package.schema_version,
        "packageId": package.package.id,
        "packageHash": package.package_hash,
        "prompt": package.prompt,
        "skills": package.package.resources.skills,
        "knowledge": local_knowledge,
        "knowledgeReferences": package.package.resources.knowledge_references,
        "mcpServers": package.package.resources.mcp_servers,
        "allowedTools": package.package.resources.tools,
        "requestedPermissions": {
            "project": project_permission_label(&package.package.permissions.project)
        },
        "workflow": package.package.workflow,
        "team": package.package.team,
    })
}

fn install_request(
    package: &ValidatedPackage,
    expected_current_hash: Option<String>,
) -> InstallExpertPackageVersionRequest {
    InstallExpertPackageVersionRequest {
        expert_id: expert_id(&package.package.id),
        package_id: package.package.id.clone(),
        version: package.package.version.clone(),
        package_hash: package.package_hash.clone(),
        package: package.raw.clone(),
        package_manifest: compiled_manifest(package),
        name: package.package.name.trim().to_owned(),
        description: package.package.description.trim().to_owned(),
        icon: package
            .package
            .icon
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        category: package.package.category.trim().to_owned(),
        system_prompt: package.prompt.clone(),
        default_model: package.package.default_model.trim().to_owned(),
        opening_suggestions: package
            .package
            .opening_suggestions
            .iter()
            .map(|value| value.trim().to_owned())
            .collect(),
        enabled_skills: package.package.resources.skills.clone(),
        expected_current_hash,
    }
}

#[tauri::command]
pub fn expert_package_inspect(
    state: State<'_, AppState>,
    request: ExpertPackageInspectRequest,
) -> ApiResponse<ExpertPackagePreview> {
    parse_package(request.package)
        .and_then(|package| inspect(&state, &package))
        .map(ApiResponse::success)
        .unwrap_or_else(package_failure)
}

#[tauri::command]
pub fn expert_package_install(
    state: State<'_, AppState>,
    request: ExpertPackageInstallRequest,
) -> ApiResponse<AgentRecord> {
    let package = match parse_package(request.package) {
        Ok(package) => package,
        Err(error) => return package_failure(error),
    };
    let preview = match inspect(&state, &package) {
        Ok(preview) => preview,
        Err(error) => return package_failure(error),
    };
    if request.expected_current_hash != preview.current_hash {
        return ApiResponse::failure(
            "expert.package_state_changed",
            "installed package state changed after preview; inspect it again",
            true,
        );
    }
    if !preview.can_install {
        let code = match preview.action.as_str() {
            "downgrade" => "expert.package_downgrade_forbidden",
            "conflict" => "expert.package_version_conflict",
            _ if !preview.compatible => "expert.package_incompatible",
            _ => "expert.package_resources_missing",
        };
        return ApiResponse::failure(code, preview.warnings.join("; "), false);
    }
    if preview.action == "no_change" {
        return state
            .database
            .get_agent_by_package_id(&package.package.id)
            .and_then(|agent| agent.ok_or_else(|| "expert package was not found".to_owned()))
            .map(ApiResponse::success)
            .unwrap_or_else(|message| {
                ApiResponse::failure("expert.package_storage", message, true)
            });
    }
    state
        .database
        .install_expert_package_version(&install_request(&package, request.expected_current_hash))
        .map(ApiResponse::success)
        .unwrap_or_else(|message| ApiResponse::failure(&message, message.clone(), true))
}

#[tauri::command]
pub fn expert_package_versions(
    state: State<'_, AppState>,
    request: AgentIdRequest,
) -> ApiResponse<Vec<ExpertPackageVersionRecord>> {
    state
        .database
        .list_expert_package_versions(&request.agent_id)
        .map(ApiResponse::success)
        .unwrap_or_else(|message| ApiResponse::failure("expert.package_storage", message, true))
}

#[tauri::command]
pub fn expert_package_rollback(
    state: State<'_, AppState>,
    request: ExpertPackageRollbackRequest,
) -> ApiResponse<AgentRecord> {
    let current = match state.database.get_agent(&request.expert_id) {
        Ok(Some(agent)) if agent.package_source == "imported" => agent,
        Ok(_) => {
            return ApiResponse::failure(
                "expert.package_not_found",
                "installed expert package was not found",
                false,
            )
        }
        Err(message) => return ApiResponse::failure("expert.package_storage", message, true),
    };
    if current.package_hash != request.expected_current_hash {
        return ApiResponse::failure(
            "expert.package_state_changed",
            "installed package state changed; refresh version history",
            true,
        );
    }
    let record = match state
        .database
        .get_expert_package_version(&request.expert_id, &request.version)
    {
        Ok(Some(record)) => record,
        Ok(None) => {
            return ApiResponse::failure(
                "expert.package_version_not_found",
                "requested expert package version was not found",
                false,
            )
        }
        Err(message) => return ApiResponse::failure("expert.package_storage", message, true),
    };
    let package = match parse_package(record.package) {
        Ok(package) => package,
        Err(error) => return package_failure(error),
    };
    let missing = match missing_resources(&state, &package) {
        Ok(missing) => missing,
        Err(error) => return package_failure(error),
    };
    let fox_version = Version::parse(env!("CARGO_PKG_VERSION")).expect("valid Fox SemVer");
    if !package.compatibility.matches(&fox_version) {
        return ApiResponse::failure(
            "expert.package_incompatible",
            "historical package is not compatible with this Fox version",
            false,
        );
    }
    if !missing.is_empty() {
        return ApiResponse::failure(
            "expert.package_resources_missing",
            "historical package requires resources that are no longer available",
            false,
        );
    }
    state
        .database
        .activate_expert_package_version(&install_request(&package, request.expected_current_hash))
        .map(ApiResponse::success)
        .unwrap_or_else(|message| ApiResponse::failure(&message, message.clone(), true))
}

#[tauri::command]
pub fn expert_package_export(
    state: State<'_, AppState>,
    request: AgentIdRequest,
) -> ApiResponse<Value> {
    let agent = match state.database.get_agent(&request.agent_id) {
        Ok(Some(agent)) => agent,
        Ok(None) => {
            return ApiResponse::failure("expert.package_not_found", "expert was not found", false)
        }
        Err(message) => return ApiResponse::failure("expert.package_storage", message, true),
    };
    if agent.package_source == "imported" {
        let version = match state
            .database
            .get_expert_package_version(&agent.id, &agent.package_version)
        {
            Ok(Some(version)) => version,
            Ok(None) => {
                return ApiResponse::failure(
                    "expert.package_version_not_found",
                    "active expert package version was not found",
                    false,
                )
            }
            Err(message) => return ApiResponse::failure("expert.package_storage", message, true),
        };
        return ApiResponse::success(version.package);
    }
    if agent.runtime_type != "pi" {
        return ApiResponse::failure(
            "expert.package_export_unsupported",
            "only local Fox experts can be exported",
            false,
        );
    }
    ApiResponse::success(export_local_agent(&agent))
}

fn export_local_agent(agent: &AgentRecord) -> Value {
    let path = "./prompts/system.md";
    let content_hash = hex::encode(Sha256::digest(agent.system_prompt.as_bytes()));
    let skills = agent
        .package_manifest
        .get("skills")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let tools = agent
        .package_manifest
        .get("allowedTools")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let mcps = agent
        .package_manifest
        .get("mcpServers")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let knowledge = agent
        .package_manifest
        .get("knowledgeReferences")
        .cloned()
        .unwrap_or_else(|| json!([]));
    let normalized_id = agent
        .id
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    json!({
        "schemaVersion": 1,
        "id": format!("fox.local.{normalized_id}"),
        "version": agent.package_version,
        "name": agent.name,
        "description": agent.description,
        "category": agent.category,
        "icon": agent.icon,
        "defaultModel": agent.default_model,
        "openingSuggestions": agent.opening_suggestions,
        "compatibility": { "foxVersion": format!("^{}", env!("CARGO_PKG_VERSION")) },
        "agent": { "systemPrompt": path },
        "resources": {
            "skills": skills,
            "tools": tools,
            "mcpServers": mcps,
            "knowledgeReferences": knowledge
        },
        "permissions": { "project": "ask" },
        "workflow": null,
        "team": null,
        "files": {
            path: { "sha256": content_hash, "content": agent.system_prompt }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(prompt_path: &str, prompt: &str) -> Value {
        let hash = hex::encode(Sha256::digest(prompt.as_bytes()));
        json!({
            "schemaVersion": 1,
            "id": "fox.test-reviewer",
            "version": "0.1.0",
            "name": "Test Reviewer",
            "compatibility": { "foxVersion": ">=0.1.0, <0.2.0" },
            "agent": { "systemPrompt": prompt_path },
            "resources": {},
            "permissions": { "project": "read_only" },
            "workflow": null,
            "files": { prompt_path: { "sha256": hash, "content": prompt } }
        })
    }

    #[test]
    fn validates_a_bounded_code_free_package() {
        let validated = parse_package(package("./prompts/system.md", "Review carefully."))
            .expect("valid package");
        assert_eq!(validated.package.id, "fox.test-reviewer");
        assert_eq!(validated.prompt, "Review carefully.");
        assert_eq!(validated.package_hash.len(), 64);
    }

    #[test]
    fn rejects_path_escape_and_hash_tampering() {
        let error = parse_package(package("../prompt.md", "Review carefully."))
            .expect_err("path escape must fail");
        assert!(error.message.contains("unsafe package path"));

        let mut tampered = package("./prompts/system.md", "Review carefully.");
        tampered["files"]["./prompts/system.md"]["content"] = json!("Tampered");
        let error = parse_package(tampered).expect_err("hash mismatch must fail");
        assert!(error.message.contains("hash mismatch"));
    }

    #[test]
    fn canonical_hash_is_independent_of_object_key_order() {
        let left = json!({"b": 2, "a": {"d": 4, "c": 3}});
        let right: Value = serde_json::from_str(r#"{"a":{"c":3,"d":4},"b":2}"#).unwrap();
        assert_eq!(canonical_json(&left), canonical_json(&right));
    }

    #[test]
    fn schema_two_embeds_a_validated_workflow_while_schema_one_stays_frozen() {
        let workflow = json!({
            "schemaVersion": 1,
            "id": "review.delivery",
            "version": "1.0.0",
            "title": "Review delivery",
            "description": "Review and verify a change",
            "inputSchema": { "type": "object" },
            "outputSchema": {
                "type": "object",
                "required": ["verdict"],
                "properties": { "verdict": { "enum": ["pass", "fail"] } },
                "additionalProperties": false
            },
            "acceptanceCriteria": ["The verdict cites validated evidence"],
            "stages": [{
                "id": "review",
                "title": "Review",
                "instructions": "Inspect the implementation",
                "gate": "user_approval",
                "retry": { "maxAttempts": 2 },
                "outputSchema": { "type": "object" },
                "acceptanceCriteria": ["Findings are recorded"],
                "stopConditions": ["The package snapshot no longer matches"]
            }]
        });
        let mut legacy = package("./prompts/system.md", "Review carefully.");
        legacy["workflow"] = workflow.clone();
        assert!(parse_package(legacy)
            .expect_err("schema one workflow must fail")
            .message
            .contains("schemaVersion 2"));

        let mut current = package("./prompts/system.md", "Review carefully.");
        current["schemaVersion"] = json!(2);
        current["workflow"] = workflow;
        let validated = parse_package(current).expect("schema two workflow package");
        assert_eq!(validated.package.schema_version, 2);
        assert_eq!(
            validated.package.workflow.as_ref().unwrap()["id"],
            "review.delivery"
        );
    }

    #[test]
    fn schema_three_embeds_a_strict_serial_team() {
        let mut current = package("./prompts/system.md", "Coordinate carefully.");
        current["schemaVersion"] = json!(3);
        current["resources"]["tools"] = json!([
            "team_snapshot_get",
            "team_start",
            "team_member_start",
            "team_collect",
            "team_cancel",
            "read",
            "grep"
        ]);
        current["team"] = json!({
            "schemaVersion": 1,
            "id": "review.team",
            "version": "1.0.0",
            "title": "Review team",
            "strategy": "supervisor",
            "members": [{
                "id": "reviewer",
                "name": "Reviewer",
                "agentId": "fox-general",
                "role": "Reviewer",
                "instructions": "Inspect the delegated task and return evidence.",
                "allowedTools": ["read", "grep"]
            }]
        });
        let validated = parse_package(current).expect("schema three team package");
        assert_eq!(validated.package.schema_version, 3);
        assert_eq!(
            validated.package.team.as_ref().unwrap()["id"],
            "review.team"
        );
    }
}
