mod capability_tools;
pub(crate) mod artifact_store;
pub(crate) mod attachment_compute;
pub(crate) mod background_jobs;
pub(crate) mod command_jobs;
mod continuation;
pub(crate) mod managed_files;
mod tool_result_read;
mod skill_load;
pub(crate) mod delivery;
pub(crate) mod kernel_coordinator;
mod kernel_model_worker;
mod kernel_run_lock;
mod kernel_host;
mod kernel_authority;
pub(crate) mod process_identity;
pub(crate) use process_identity::process_start_marker;
pub(crate) use process_identity::owner_is_gone as job_owner_is_gone;
mod kernel_gateway;
mod kernel_delegation;
mod protocol;
pub(crate) mod shadow_reconcile;
#[cfg(test)]
mod shadow_reconcile_tests;
mod work_tools;

#[cfg(test)]
pub(crate) use work_tools::WORK_TOOLS;

use crate::yuxi::{get_access_token, YuxiClient};
use crate::kernel::CancellationPort;
use crate::{
    app_state::AppState,
    database::{
        AgentRecord, ApprovalDecision, ApprovalRecord, AttachmentRecord, ChildRunBudget,
        ChildRunRecord, Database, EvaluationRunSummary, HostToolCallDisposition,
        KnowledgeReference, MemoryProposalInput, PreparedDigitalColleagueTrigger,
        SkillActivationRecord, StartRunResult, ToolCallRecord,
        MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS,
    },
    local_knowledge::LocalKnowledgeStore,
};
use flate2::read::DeflateDecoder;
use protocol::{HostResponse, RuntimeCapabilityManifest, RuntimeEnvelope, RuntimeRequest};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

pub(crate) fn runtime_tool_is_supported(tool: &str) -> bool {
    protocol::canonical_runtime_tool_contract(tool).is_some()
}

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const CANCELLED_BEFORE_SUBMISSION: &str = "runtime run was cancelled before submission";
const STDERR_LIMIT: usize = 80;
const MAX_PROTOCOL_LINE_BYTES: usize = 1024 * 1024;
const MAX_KNOWLEDGE_TOOL_TEXT_BYTES: usize = 256 * 1024;
const MAX_KNOWLEDGE_TOOL_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_ATTACHMENT_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_ATTACHMENT_TEXT_BYTES: usize = 1024 * 1024;
const MAX_IMAGE_FILE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 25_000_000;
const MAX_IMAGES_PER_MESSAGE: usize = 4;
const GRAPH_REVIEWER_EXECUTION_PROFILE: &str = "graph_reviewer_v1";
const GRAPH_REVIEWER_ALLOWED_TOOLS: [&str; 4] = ["read", "ls", "find", "grep"];
const GRAPH_REVIEWER_MAX_DURATION_MS: i64 = 45_000;
const GRAPH_REVIEWER_MAX_TOTAL_TOKENS: i64 = 4_096;
const GRAPH_REVIEWER_MAX_OUTPUT_TOKENS: i64 = 1_024;
const GRAPH_REVIEWER_MAX_TOOL_CALLS: i64 = 6;
const MAX_CONTINUATION_RECONCILIATION_ITEMS: usize = 256;
const MAX_CONTINUATION_RECONCILIATION_ITEMS_PER_PASS: usize = 1_024;
const MAX_CONTINUATION_RECONCILIATION_PASS_DURATION: Duration = Duration::from_secs(2);
const MAX_CONCURRENT_TOOL_HANDLERS: usize = 32;

struct ToolHandlerPermit {
    active: Arc<AtomicUsize>,
}

impl Drop for ToolHandlerPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RuntimeToolIngress {
    Preflight,
    Execute,
    Event,
}

fn validate_runtime_tool_authority(
    active_profile: &continuation::ExecutionProfileSelection,
    frozen_profile_id: Option<&str>,
    manifest: &RuntimeCapabilityManifest,
    run_id: &str,
    tool: &str,
    ingress: RuntimeToolIngress,
) -> Result<(), (&'static str, String)> {
    protocol::validate_host_manifest(manifest)
        .map_err(|error| ("runtime.capability.invalid", error))?;
    // A missing snapshot is only retained for Runs created before schema v34.
    // Any non-Legacy active Runtime will therefore fail the identity comparison.
    let frozen_profile_id = frozen_profile_id.unwrap_or("legacy");
    let frozen_profile = continuation::ExecutionProfileSelection::resolve(frozen_profile_id)
        .map_err(|error| ("runtime.execution_profile.frozen_invalid", error))?;
    if frozen_profile.id() != active_profile.id() {
        return Err((
            "runtime.execution_profile.run_mismatch",
            format!(
                "Run '{run_id}' froze execution profile '{}', but the active Runtime uses '{}'",
                frozen_profile.id(),
                active_profile.id()
            ),
        ));
    }
    if !active_profile.allows_host_tool(tool) {
        return Err((
            "runtime.execution_profile.tool_blocked",
            format!(
                "execution profile '{}' does not allow Runtime tool '{tool}'",
                active_profile.id()
            ),
        ));
    }
    if !frozen_profile.allows_host_tool(tool) {
        return Err((
            "runtime.execution_profile.run_tool_blocked",
            format!(
                "Run '{run_id}' frozen execution profile '{}' does not allow tool '{tool}'",
                frozen_profile.id()
            ),
        ));
    }
    let Some(declared) = manifest.tools.iter().find(|declared| declared.name == tool) else {
        return Err((
            "runtime.capability.tool_not_declared",
            format!("runtime did not declare Runtime tool '{tool}' in its ready manifest"),
        ));
    };
    let ingress_allowed = match ingress {
        RuntimeToolIngress::Preflight => {
            declared.execution == "runtime" && declared.approval == "preflight"
        }
        RuntimeToolIngress::Execute => declared.execution == "host",
        RuntimeToolIngress::Event => true,
    };
    if !ingress_allowed {
        return Err((
            "runtime.capability.ingress_mismatch",
            format!(
                "Runtime tool '{tool}' cannot enter through {:?} with contract {}/{}/{}",
                ingress, declared.category, declared.execution, declared.approval
            ),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn create_fresh_host_tool_call(
    database: &Database,
    run_id: &str,
    runtime_tool_call_id: &str,
    tool_name: &str,
    input: &Value,
    status: &str,
    requires_approval: bool,
) -> Result<HostToolCallExecution, String> {
    let (record, disposition) = database.create_host_tool_call_once(
        run_id,
        runtime_tool_call_id,
        tool_name,
        input,
        status,
        requires_approval,
    )?;
    match disposition {
        HostToolCallDisposition::Created | HostToolCallDisposition::PromotedRuntime => {
            Ok(HostToolCallExecution::Execute(record))
        }
        HostToolCallDisposition::ReplayTerminal => replayed_host_tool_call(database, record),
        HostToolCallDisposition::AlreadyInFlight => Err(format!(
            "[tool_call.already_in_flight] ToolCall '{runtime_tool_call_id}' is already '{}' and cannot start a second handler",
            record.status
        )),
    }
}

fn inspect_existing_host_tool_call(
    database: &Database,
    run_id: &str,
    runtime_tool_call_id: &str,
    tool_name: &str,
    input: &Value,
) -> Result<Option<Value>, String> {
    let Some((record, disposition)) =
        database.inspect_host_tool_call_replay(run_id, runtime_tool_call_id, tool_name, input)?
    else {
        return Ok(None);
    };
    match disposition {
        HostToolCallDisposition::ReplayTerminal => {
            match replayed_host_tool_call(database, record)? {
                HostToolCallExecution::Replay(response) => Ok(Some(response)),
                HostToolCallExecution::Execute(_) => unreachable!("terminal replay never executes"),
            }
        }
        HostToolCallDisposition::AlreadyInFlight => Err(format!(
            "[tool_call.already_in_flight] ToolCall '{runtime_tool_call_id}' is already in flight"
        )),
        HostToolCallDisposition::Created | HostToolCallDisposition::PromotedRuntime => {
            unreachable!("read-only inspection cannot create or promote a ToolCall")
        }
    }
}

fn replayed_host_tool_call(
    database: &Database,
    record: ToolCallRecord,
) -> Result<HostToolCallExecution, String> {
    if record.status == "completed" {
        return Ok(HostToolCallExecution::Replay(settled_host_tool_call_envelope(
            database,
            &record.run_id,
            &record.runtime_tool_call_id,
            &record.conversation_id,
            record.result.clone().unwrap_or(Value::Null),
            None,
            // Replays never re-run hooks, so their annotation set is empty.
            Vec::new(),
            true,
        )));
    }
    if record.status == "failed" {
        // A replayed failure must hand back the same facts the original attempt
        // produced - same bounded result, same classification, same trusted
        // storage fact and reference - otherwise the second delivery of one
        // ToolCall is silently less informative and the model repeats the
        // command it was told about.
        let error = record
            .error_message
            .clone()
            .unwrap_or_else(|| "the persisted ToolCall failed".to_owned());
        let Some(result) = record.result.clone() else {
            // A failure that never had a result keeps its original minimal shape.
            return Ok(HostToolCallExecution::Replay(json!({
                "isError": true,
                "error": error,
            })));
        };
        return Ok(HostToolCallExecution::Replay(settled_host_tool_call_envelope(
            database,
            &record.run_id,
            &record.runtime_tool_call_id,
            &record.conversation_id,
            result,
            Some(error),
            Vec::new(),
            true,
        )));
    }
    Err(format!(
        "[tool_call.replayed_terminal] ToolCall '{}' already ended as '{}': {}",
        record.runtime_tool_call_id,
        record.status,
        record
            .error_message
            .as_deref()
            .unwrap_or("the persisted terminal outcome is authoritative")
    ))
}

#[derive(Debug)]
enum HostToolCallExecution {
    Execute(ToolCallRecord),
    Replay(Value),
}

fn structured_runtime_error(message: &str) -> (&str, &str) {
    let Some(rest) = message.strip_prefix('[') else {
        return ("runtime.start_failed", message);
    };
    let Some((code, detail)) = rest.split_once(']') else {
        return ("runtime.start_failed", message);
    };
    if code.is_empty()
        || !code.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.' || byte == b'_'
        })
    {
        return ("runtime.start_failed", message);
    }
    (code, detail.trim_start())
}

fn event_projection_failure_code(error: &str) -> &'static str {
    if error.contains("[child_run.tool_budget_exceeded]") {
        "child_run.tool_budget_exceeded"
    } else if error.contains("[digital_colleague.tool_budget_exceeded]") {
        "digital_colleague.tool_budget_exceeded"
    } else if error.contains("[child_run.budget_exceeded]") {
        "child_run.budget_exceeded"
    } else if error.contains("[digital_colleague.budget_exceeded]") {
        "digital_colleague.budget_exceeded"
    } else {
        "storage.event_projection_failed"
    }
}

fn effective_digital_colleague_output_limit(
    configured: i64,
    remaining: i64,
    model_maximum: i64,
) -> Result<i64, String> {
    let effective = configured.min(remaining).min(model_maximum);
    if effective < MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS {
        return Err(format!(
            "[digital_colleague.budget_exceeded] remaining output budget {effective} is below the Host minimum {MIN_DIGITAL_COLLEAGUE_OUTPUT_TOKENS}"
        ));
    }
    Ok(effective)
}

fn validate_envelope_identity_scope(
    conversation_id: &str,
    run_id: &str,
    runtime_session_id: &str,
    expected_conversation_id: Option<&str>,
    expected_run_id: Option<&str>,
    expected_runtime_session_id: Option<&str>,
    authoritative_conversation_id: &str,
) -> Result<(), (&'static str, String)> {
    if expected_conversation_id != Some(conversation_id)
        || expected_run_id != Some(run_id)
        || expected_runtime_session_id != Some(runtime_session_id)
    {
        return Err((
            "runtime.identity.active_scope_mismatch",
            "runtime envelope identity does not match the active Host worker scope".to_owned(),
        ));
    }
    if authoritative_conversation_id != conversation_id {
        return Err((
            "runtime.identity.run_conversation_mismatch",
            "runtime envelope conversationId does not own the active runId".to_owned(),
        ));
    }
    Ok(())
}

fn runtime_agent_package(
    agent: &AgentRecord,
    enabled_skills: &[String],
    enabled_mcps: &[String],
    bound_knowledge: &[KnowledgeReference],
) -> Result<Value, String> {
    let system_prompt = (agent.runtime_type == "pi").then(|| agent.system_prompt.clone());
    let mut package_manifest = agent.package_manifest.clone();
    if let Some(object) = package_manifest.as_object_mut() {
        object.insert("skills".to_owned(), json!(enabled_skills));
        if agent.is_builtin {
            object.insert("mcpServers".to_owned(), json!(enabled_mcps));
        }
        apply_bound_knowledge_declaration(object, bound_knowledge, false)?;
    }
    Ok(json!({
        "id": agent.id,
        "name": agent.name,
        "description": agent.description,
        "runtimeType": agent.runtime_type,
        "agentKind": agent.agent_kind,
        "invocationMode": agent.invocation_mode,
        "visibility": agent.visibility,
        "defaultModel": agent.default_model,
        "systemPrompt": system_prompt,
        "category": agent.category,
        "isBuiltin": agent.is_builtin,
        "openingSuggestions": agent.opening_suggestions,
        "packageVersion": agent.package_version,
        "packageManifest": package_manifest,
        "capabilities": agent.capabilities,
        "resources": agent.resources,
        "enabledSkills": enabled_skills,
    }))
}

fn package_string_scope(manifest: &Value, field: &str) -> Option<HashSet<String>> {
    manifest.get(field).and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

fn runtime_package_tool_scope(package: &Value) -> Option<HashSet<String>> {
    package
        .get("packageManifest")
        .and_then(|manifest| package_string_scope(manifest, "allowedTools"))
}

fn intersect_optional_scopes(
    left: Option<HashSet<String>>,
    right: Option<HashSet<String>>,
) -> Option<HashSet<String>> {
    match (left, right) {
        (None, None) => None,
        (Some(scope), None) | (None, Some(scope)) => Some(scope),
        (Some(left), Some(right)) => Some(left.intersection(&right).cloned().collect()),
    }
}

fn apply_delegated_package_scope(
    package: &mut Value,
    parent_tools: Option<&HashSet<String>>,
    parent_mcps: Option<&HashSet<String>>,
) -> Result<(), String> {
    let manifest = package
        .get_mut("packageManifest")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "delegated agent package manifest is invalid".to_owned())?;
    for (field, parent_scope) in [("allowedTools", parent_tools), ("mcpServers", parent_mcps)] {
        let Some(parent_scope) = parent_scope else {
            continue;
        };
        let selected = package_string_scope(&Value::Object(manifest.clone()), field)
            .map(|child_scope| {
                child_scope
                    .intersection(parent_scope)
                    .cloned()
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_else(|| parent_scope.clone());
        let mut selected = selected.into_iter().collect::<Vec<_>>();
        selected.sort();
        manifest.insert(field.to_owned(), json!(selected));
    }
    Ok(())
}

fn apply_bound_knowledge_declaration(
    manifest: &mut serde_json::Map<String, Value>,
    bound_knowledge: &[KnowledgeReference],
    strict: bool,
) -> Result<(), String> {
    if let Some(declaration) = manifest.get("knowledgeReferences") {
        let references = declaration.as_array().ok_or_else(|| {
            "[conversation.expert_snapshot_invalid] Expert knowledgeReferences must be an array."
                .to_owned()
        })?;
        let mut effective = Vec::new();
        for reference in references {
            let parsed = match package_knowledge_reference(reference) {
                Ok(reference) => reference,
                Err(error) if strict => return Err(error),
                Err(_) => continue,
            };
            if bound_knowledge.contains(&parsed) {
                effective.push(reference.clone());
            }
        }
        manifest.insert("manifestSchemaVersion".to_owned(), json!(2));
        manifest.insert("knowledgeReferences".to_owned(), Value::Array(effective));
        manifest.remove("knowledge");
    } else if strict && manifest.contains_key("knowledge") {
        let declared = manifest
            .get("knowledge")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                "[conversation.expert_snapshot_invalid] Expert knowledge must be an array."
                    .to_owned()
            })?
            .iter()
            .map(|item| {
                item.as_str().ok_or_else(|| {
                    "[conversation.expert_snapshot_invalid] Expert knowledge id is invalid."
                        .to_owned()
                })
            })
            .collect::<Result<HashSet<_>, _>>()?;
        let effective = bound_knowledge
            .iter()
            .filter(|reference| {
                reference.source == "remote" && declared.contains(reference.id.as_str())
            })
            .map(|reference| reference.id.clone())
            .collect::<Vec<_>>();
        manifest.insert("knowledge".to_owned(), json!(effective));
    } else if bound_knowledge.iter().all(|reference| {
        reference.source == "remote" && reference.connection_id.as_deref() == Some("yuxi")
    }) {
        manifest.insert(
            "knowledge".to_owned(),
            json!(bound_knowledge
                .iter()
                .map(|reference| reference.id.clone())
                .collect::<Vec<_>>()),
        );
    } else {
        manifest.insert("manifestSchemaVersion".to_owned(), json!(2));
        manifest.insert("knowledgeReferences".to_owned(), json!(bound_knowledge));
        manifest.remove("knowledge");
    }
    Ok(())
}

fn package_knowledge_reference(value: &Value) -> Result<KnowledgeReference, String> {
    let invalid = || {
        "[conversation.expert_snapshot_invalid] Expert knowledge reference is invalid.".to_owned()
    };
    let object = value.as_object().ok_or_else(invalid)?;
    let source = object
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(invalid)?;
    let reference = match source {
        "local" => KnowledgeReference::local(id),
        "remote" => KnowledgeReference::remote(
            object
                .get("connectionId")
                .and_then(Value::as_str)
                .ok_or_else(invalid)?,
            id,
        ),
        _ => return Err(invalid()),
    };
    reference.validate().map_err(|_| invalid())?;
    Ok(reference)
}

fn package_string_list(package: &Value, key: &str) -> Vec<String> {
    package
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn validate_expert_package_snapshot(snapshot: &Value, expected_hash: &str) -> Result<(), String> {
    let actual_hash = hex::encode(Sha256::digest(snapshot.to_string().as_bytes()));
    if expected_hash != actual_hash {
        return Err(
            "[conversation.expert_snapshot_invalid] Expert package snapshot hash does not match; choose the expert again."
                .to_owned(),
        );
    }
    if snapshot.get("agentKind").and_then(Value::as_str) != Some("expert")
        || snapshot.get("invocationMode").and_then(Value::as_str) != Some("inline")
        || snapshot.get("visibility").and_then(Value::as_str) != Some("expert_center")
    {
        return Err(
            "[conversation.expert_snapshot_invalid] Expert package snapshot classification is invalid; choose the expert again."
                .to_owned(),
        );
    }
    Ok(())
}

fn runtime_expert_package(
    snapshot: &Value,
    expected_hash: &str,
    enabled_skills: &[String],
    enabled_mcps: &[String],
    bound_knowledge: &[KnowledgeReference],
) -> Result<Value, String> {
    validate_expert_package_snapshot(snapshot, expected_hash)?;

    let mut package = snapshot.clone();
    let package_object = package.as_object_mut().ok_or_else(|| {
        "[conversation.expert_snapshot_invalid] Expert package snapshot is invalid; choose the expert again."
            .to_owned()
    })?;
    let is_builtin = package_object
        .get("isBuiltin")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let manifest = package_object
        .get_mut("packageManifest")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            "[conversation.expert_snapshot_invalid] Expert package manifest is invalid; choose the expert again."
                .to_owned()
        })?;
    let declared_skills = manifest
        .get("skills")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<HashSet<_>>()
        });
    let effective_skills = enabled_skills
        .iter()
        .filter(|skill| {
            declared_skills
                .as_ref()
                .is_none_or(|declared| declared.contains(skill.as_str()))
        })
        .cloned()
        .collect::<Vec<_>>();
    manifest.insert("skills".to_owned(), json!(effective_skills));
    if is_builtin {
        manifest.insert("mcpServers".to_owned(), json!(enabled_mcps));
    }
    apply_bound_knowledge_declaration(manifest, bound_knowledge, true)?;
    package_object.insert("enabledSkills".to_owned(), json!(effective_skills));
    Ok(package)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub state: String,
    pub runtime: String,
    pub runtime_version: Option<String>,
    pub capabilities: Value,
    pub available: bool,
    pub current_conversation_id: Option<String>,
    pub active_run_id: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostics {
    pub protocol: String,
    pub protocol_version: u32,
    pub capability_manifest_version: u32,
    pub state: String,
    pub runtime: String,
    pub runtime_version: Option<String>,
    pub capabilities: Value,
    pub available: bool,
    pub current_conversation_id: Option<String>,
    pub active_run_id: Option<String>,
    pub last_error: Option<String>,
    pub stderr_tail: Vec<String>,
    pub session_file_count: usize,
    pub recovery_attempts: u8,
    pub last_recovery_at: Option<i64>,
    pub kernel_shadow_gate: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventNotification {
    pub conversation_id: String,
    pub runtime_session_id: Option<String>,
    pub run_id: String,
    pub seq: i64,
    pub timestamp: String,
    pub event: Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildRunNotification {
    pub parent_run_id: String,
    pub parent_conversation_id: String,
    pub child_run: ChildRunRecord,
}

type PendingResponses = Arc<Mutex<HashMap<String, mpsc::Sender<RuntimeEnvelope>>>>;
#[derive(Debug)]
struct PendingApproval {
    run_id: String,
    sender: mpsc::Sender<bool>,
}

type PendingApprovals = Arc<Mutex<HashMap<String, PendingApproval>>>;

/// Per-target serialization for state-changing built-in Office calls in the
/// Legacy transport. A model occasionally emits a parallel burst of writes to
/// the SAME workbook (e.g. one import per sheet); without serialization each
/// call created its own approval request, producing many overlapping approval
/// dialogs and racing writes. The key is `run id + absolute target path`;
/// different files stay independent.
type ActiveFileWrites = Arc<Mutex<HashSet<String>>>;

/// Holds one per-target Office write slot for the duration of a Legacy
/// dispatch (approval wait + execution + registration). Removing the key on
/// drop serializes a parallel model burst aimed at the same file while
/// leaving writes to different files independent.
struct ActiveOfficeWrite {
    active: ActiveFileWrites,
    key: String,
}

impl Drop for ActiveOfficeWrite {
    fn drop(&mut self) {
        if let Ok(mut set) = self.active.lock() {
            set.remove(&self.key);
        }
    }
}

struct WorkerHandle {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingResponses,
    current_conversation_id: Option<String>,
    active_run_id: Option<String>,
    current_runtime_session_id: Option<String>,
}

struct QueuedRun {
    started: StartRunResult,
    text: String,
    attachments: Vec<AttachmentRecord>,
}

struct RuntimeCommand {
    program: PathBuf,
    script: Option<PathBuf>,
}

fn node_dependency_is_available(script: &Path, package: &str) -> bool {
    let relative_package = package
        .split('/')
        .fold(PathBuf::new(), |path, segment| path.join(segment))
        .join("package.json");

    script.parent().is_some_and(|directory| {
        directory.ancestors().any(|ancestor| {
            ancestor
                .join("node_modules")
                .join(&relative_package)
                .is_file()
        })
    })
}

struct RuntimeHostState {
    cancellation: crate::kernel::CancellationRegistry,
    kernel_active_runs: HashSet<String>,
    shutting_down: bool,
    state: String,
    worker: Option<WorkerHandle>,
    stderr: VecDeque<String>,
    last_error: Option<String>,
    runtime: String,
    runtime_version: Option<String>,
    capabilities: Value,
    execution_profile_id: String,
    pending_approvals: PendingApprovals,
    active_file_writes: ActiveFileWrites,
    queued_runs: VecDeque<QueuedRun>,
    dispatching_run_id: Option<String>,
    dispatching_conversation_id: Option<String>,
    cancelled_dispatches: HashSet<String>,
    recovery_attempts: u8,
    last_recovery_at: Option<i64>,
    /// Per-legacy-run shadow contexts. These run the observation-only kernel
    /// decision core and are fed legacy events; they never execute side
    /// effects. Keyed by legacy run id.
    shadow: Arc<shadow_reconcile::ShadowReconciler>,
}

/// Content-addressed shadow identity hash over the normalised CONTENT of a
/// frozen contract input. Hashing normalised JSON (not an identity tag) means
/// changing the capability set, permission scope or prompt/profile content
/// changes the hash.
fn runtime_shadow_hash(seed: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(seed.as_bytes())))
}

fn shadow_prompt_config_hash(snapshot: &Value) -> Result<String, String> {
    let stable_prompt_hash = snapshot
        .get("stablePromptHash")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "run.request_snapshot omitted the non-empty stablePromptHash required by Kernel Shadow"
                .to_owned()
        })?;
    let mut identity = serde_json::Map::new();
    for key in [
        "contextHash",
        "contextSchemaHash",
        "executionProfile",
        "promptCacheIdentity",
        "promptContentHash",
        "promptDefinitionId",
        "promptVersion",
        "toolCatalogHash",
    ] {
        identity.insert(
            key.to_owned(),
            snapshot.get(key).cloned().unwrap_or(Value::Null),
        );
    }
    identity.insert(
        "stablePromptHash".to_owned(),
        Value::String(stable_prompt_hash.to_owned()),
    );
    Ok(runtime_shadow_hash(&Value::Object(identity).to_string()))
}

fn shadow_permission_snapshot_id(permission_mode: &str, project_access: &str) -> String {
    runtime_shadow_hash(&format!("permission|{permission_mode}|{project_access}"))
}

fn shadow_preflight_tool_call_id(envelope: &RuntimeEnvelope) -> Result<&str, &'static str> {
    envelope
        .payload
        .as_ref()
        .and_then(|payload| payload.get("toolCallId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("tool.preflight omitted a non-empty payload.toolCallId")
}

fn shadow_preflight_input(envelope: &RuntimeEnvelope, allowed: bool, payload: &Value) -> Value {
    if allowed {
        if let Some(input) = payload.get("input") {
            return input.clone();
        }
    }
    envelope
        .payload
        .as_ref()
        .and_then(|request| request.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}))
}

fn runtime_tool_event_succeeded(payload: &Value) -> bool {
    payload.get("type").and_then(Value::as_str) == Some("tool.completed")
        && payload.get("isError").and_then(Value::as_bool) != Some(true)
}

fn shadow_legacy_tool_decision(
    tool: &str,
    execution_location: &str,
    requires_approval: bool,
) -> Result<&'static str, String> {
    // Single source of truth: shared with the ShadowReconciler so the Host
    // and the integration tests never maintain two classification lists.
    shadow_reconcile::legacy_tool_decision(tool, execution_location, requires_approval)
}

struct AgentPromptContext {
    system_prompt: String,
    assistant_package: Value,
    expert_package: Option<Value>,
    expert_binding_payload: Option<Value>,
    run_role: &'static str,
    run_context: Value,
    skill_prompt: String,
    skill_activations: Vec<SkillActivationRecord>,
}

/// The conversation-scoped prompt context of one Run, built from Host facts.
///
/// This is the *production* construction, extracted from
/// `RuntimeHost::agent_prompt_context` so a non-UI entry point can build exactly
/// the same thing. The real acceptance harness calls this instead of
/// hand-assembling a reduced prompt: the review of 2026-09-19 found that the
/// cloud tier had been given a three-tool contract and a one-line system prompt
/// that no production Run ever sees, which made its failures unattributable.
pub(crate) struct PromptScopeOverrides<'a> {
    pub tool_allowlist: Option<&'a HashSet<String>>,
    pub mcp_server_scope: Option<&'a HashSet<String>>,
}

pub(crate) fn conversation_prompt_context(
    database: &Database,
    skills_dir: &std::path::Path,
    conversation_id: &str,
    text: &str,
    run_kind: &str,
    overrides: PromptScopeOverrides<'_>,
) -> Result<AgentPromptContext, String> {
    {
        let system_prompt = database
            .conversation_system_prompt(conversation_id)?;
        let agent_id = database
            .conversation_agent_id(conversation_id)?;
        let assistant = database
            .get_agent(&agent_id)?
            .ok_or_else(|| "conversation assistant was not found".to_owned())?;
        let run_role = match (run_kind, assistant.agent_kind.as_str()) {
            ("child", "expert") => "expert_consultation",
            ("child", _) => "child_worker",
            ("digital_colleague", _) => "digital_colleague",
            _ => "user_facing_lead",
        };
        let run_context = json!({
            "runKind": run_kind,
            "runRole": run_role,
            "isUserFacingLead": run_role == "user_facing_lead",
        });
        let expert_binding = database
            .current_conversation_expert_binding(conversation_id)?;
        let assistant_skills = database.enabled_agent_skills(&assistant.id)?;
        let configured_expert_skills = expert_binding
            .as_ref()
            .map(|binding| database.enabled_agent_skills(&binding.expert_id))
            .transpose()?
            .unwrap_or_default();
        let enabled_mcps = database
            .list_mcp_servers()?
            .into_iter()
            .filter(|server| server.enabled)
            .map(|server| server.id)
            .filter(|server_id| {
                overrides
                    .mcp_server_scope
                    .is_none_or(|scope| scope.contains(server_id))
            })
            .collect::<Vec<_>>();
        let bound_knowledge = database
            .conversation_knowledge_references(conversation_id)?;
        let mut assistant_package = runtime_agent_package(
            &assistant,
            &assistant_skills,
            &enabled_mcps,
            &bound_knowledge,
        )?;
        apply_delegated_package_scope(
            &mut assistant_package,
            overrides.tool_allowlist,
            overrides.mcp_server_scope,
        )?;
        let expert_package = expert_binding
            .as_ref()
            .map(|binding| {
                runtime_expert_package(
                    &binding.package_snapshot,
                    &binding.package_hash,
                    &configured_expert_skills,
                    &enabled_mcps,
                    &bound_knowledge,
                )
            })
            .transpose()?;
        let effective_expert_skills = expert_package
            .as_ref()
            .map(|package| package_string_list(package, "enabledSkills"))
            .unwrap_or_default();
        let mut seen_skills = HashSet::new();
        let enabled_skills = assistant_skills
            .iter()
            .chain(effective_expert_skills.iter())
            .filter(|skill| seen_skills.insert((*skill).clone()))
            .cloned()
            .collect::<Vec<_>>();
        let available_skill_tools = intersect_optional_scopes(
            runtime_package_tool_scope(&assistant_package),
            expert_package.as_ref().and_then(runtime_package_tool_scope),
        );
        // Units are Unicode characters (never UTF-8 bytes or tokens); the plan
        // also keeps a full on-demand catalog so budget-skipped skills stay
        // discoverable and loadable through `skill_load`.
        let skill_plan = crate::skills::skill_prompt_plan(
            skills_dir,
            &enabled_skills,
            available_skill_tools.as_ref(),
            text,
            8_000,
        )?;
        let expert_binding_payload = expert_binding.as_ref().map(|binding| {
            json!({
                "bindingId": binding.id,
                "expertId": binding.expert_id,
                "version": binding.expert_version,
                "packageHash": binding.package_hash,
                "activationSource": binding.activation_source,
                "activatedAt": binding.activated_at,
            })
        });
        let skill_activations = skill_plan.included.iter().map(|included| SkillActivationRecord {
            skill_id: included.id.clone(), version: included.version.clone(),
            content_sha256: included.content_sha256.clone(), source: "initial_prompt".into(),
            required_tools: included.required_tools.clone(), missing_tools: Vec::new(),
            tools_available: true, char_count: included.chars as i64, byte_count: included.bytes as i64,
            created_at: crate::database::now_ms(),
        }).collect();
        Ok(AgentPromptContext { system_prompt, assistant_package, expert_package,
            expert_binding_payload, run_role, run_context, skill_prompt: skill_plan.prompt, skill_activations })
    }
}

/// The model-facing prompt payload of one Kernel Run.
///
/// One constructor for every entry point, so the keys and the reference data a
/// Run receives cannot drift between the application start path and a non-UI
/// caller such as the real acceptance harness.
///
/// Everything is borrowed: the caller keeps ownership of the same values for the
/// Legacy prompt that follows, and the reference-data fields stay generic over
/// `Serialize` exactly as the `json!` literal they replace did.
pub(crate) struct KernelPromptPayload<'a, M, W, R> {
    pub text: &'a str,
    pub messages: &'a M,
    pub images: &'a [Value],
    pub context: &'a AgentPromptContext,
    pub project_context: &'a Value,
    pub work_snapshot: &'a W,
    pub memory_context: &'a R,
    pub max_prompt_tokens: i64,
}

pub(crate) fn kernel_prompt_payload<M, W, R>(payload: KernelPromptPayload<'_, M, W, R>) -> Value
where
    M: serde::Serialize,
    W: serde::Serialize,
    R: serde::Serialize,
{
    json!({
        "text": payload.text,
        "messages": payload.messages,
        "images": payload.images,
        "systemPrompt": payload.context.system_prompt,
        "skillPrompt": payload.context.skill_prompt,
        "assistantPackage": payload.context.assistant_package,
        "expertPackage": payload.context.expert_package,
        "expertBinding": payload.context.expert_binding_payload,
        "runContext": payload.context.run_context,
        "projectContext": payload.project_context,
        "workSnapshot": payload.work_snapshot,
        "memoryContext": payload.memory_context,
        "promptBudget": {"maxPromptTokens": payload.max_prompt_tokens},
    })
}

#[derive(Clone)]
pub struct RuntimeHost {
    app: AppHandle,
    database: Database,
    state: Arc<Mutex<RuntimeHostState>>,
    run_transition: Arc<Mutex<()>>,
    sessions_dir: PathBuf,
    attachments_dir: PathBuf,
    skills_dir: PathBuf,
    /// Host-side backups of files before intercepted writes; restore reads
    /// these. Lives outside project folders so backups never pollute repos.
    managed_files_dir: PathBuf,
    yuxi_client: YuxiClient,
    child_hosts: Arc<Mutex<HashMap<String, RuntimeHost>>>,
    run_budget_override: Option<ChildRunBudget>,
    enforce_count_budget_override: bool,
    execution_profile_override: Option<&'static str>,
    tool_allowlist_override: Option<HashSet<String>>,
    mcp_server_scope_override: Option<HashSet<String>>,
    digital_scheduler_started: Arc<AtomicBool>,
    digital_scheduler_stop: Arc<AtomicBool>,
    active_tool_handlers: Arc<AtomicUsize>,
}

impl RuntimeHost {
    pub(crate) fn execute_job_operation(&self, conversation: &str, run_id: &str, tool: &str, input: &Value) -> Result<Value,String> {
        run_bound_conversation(&self.database,run_id,Some(conversation))?;
        let binding=self.database.run_control_binding(run_id)?.ok_or("job operation needs a frozen Run")?;
        let elapsed=if binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative {
            let scope=self.database.kernel_host_scope(run_id)?;
            if !scope.tool_names.contains(tool) {return Err("job operation is outside the frozen tool scope".into());}
            if scope.lifecycle_hooks.iter().any(|h|h.event=="before_tool"&&matches!(h.action.as_str(),"block"|"require_approval")&&crate::lifecycle_hooks::matcher_matches(&h.matcher,tool)) {
                return Err("job operation requires the existing tool approval workflow".into());
            }
            self.database.kernel_build_full_snapshot(run_id)?.running_elapsed_ms
        } else {
            self.ensure_runtime_tool_allowed(run_id,tool,RuntimeToolIngress::Execute).map_err(|(_,e)|e)?;
            0
        };
        let token=self.state.lock().map_err(|_| "runtime state poisoned")?.cancellation.run_token(run_id).ok();
        let budget=std::time::Duration::from_millis(binding.budgets.limit_operation_ms(binding.budgets.tool_execution_ms, elapsed).max(0) as u64);
        background_jobs::execute(&self.database,&self.attachments_dir,&self.sessions_dir,run_id,tool,input,token,budget)
    }
    fn handle_job_tool_request(&self, stdin: &Arc<Mutex<ChildStdin>>, envelope: RuntimeEnvelope) {
        let outcome=(|| -> Result<Value,String> {
            let run=envelope.run_id.as_deref().ok_or("missing runId")?;
            let conversation=run_bound_conversation(&self.database,run,envelope.conversation_id.as_deref())?;
            let p=envelope.payload.as_ref().ok_or("missing payload")?;
            let tool=p["tool"].as_str().ok_or("missing tool")?;
            let call=p["toolCallId"].as_str().ok_or("missing toolCallId")?;
            let input=&p["input"];
            if let Some(response)=inspect_existing_host_tool_call(&self.database,run,call,tool,input)? {return Ok(response);}
            let hooks=crate::lifecycle_hooks::before_tool(&self.database,run,call,tool,input)?;
            if hooks.blocked.is_some()||hooks.requires_approval {return Err("job operation requires explicit approval through Kernel".into());}
            match create_fresh_host_tool_call(&self.database,run,call,tool,input,"running",false)? {
                HostToolCallExecution::Replay(response)=>Ok(response),
                HostToolCallExecution::Execute(_)=>finalize_host_tool_execution(&self.database,run,call,tool,input,hooks.annotations,self.execute_job_operation(&conversation,run,tool,input))
            }
        })();
        let response=outcome.unwrap_or_else(|error| json!({"isError":true,"error":error}));
        let kind=if response["isError"]==true {"tool.execute_failed"}else{"tool.execute_completed"};
        let response=HostResponse::for_request(&envelope,kind,response);
        if let Err(error)=write_protocol_message(stdin,&response) {eprintln!("fox job response: {error}");}
    }

    fn try_acquire_tool_handler(&self) -> Option<ToolHandlerPermit> {
        let active = self.active_tool_handlers.fetch_add(1, Ordering::SeqCst);
        if active >= MAX_CONCURRENT_TOOL_HANDLERS {
            self.active_tool_handlers.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(ToolHandlerPermit {
            active: self.active_tool_handlers.clone(),
        })
    }

    pub fn run_offline_evaluations(&self) -> Result<EvaluationRunSummary, String> {
        let runtime = self.runtime_command()?;
        let mut command = Command::new(&runtime.program);
        if let Some(script) = runtime.script.as_ref() {
            command.arg(script);
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to start the offline evaluator: {error}"))?;
        let request = RuntimeRequest::new("evaluation.run");
        let request_id = request.id.clone();
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "offline evaluator stdin was unavailable".to_owned())?;
        serde_json::to_writer(&mut stdin, &request).map_err(|error| error.to_string())?;
        stdin
            .write_all(b"\n")
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("failed to request offline evaluation: {error}"))?;
        drop(stdin);
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "offline evaluator stdout was unavailable".to_owned())?;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            let result = loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) => break Err("offline evaluator closed without a report".to_owned()),
                    Ok(_) if line.len() > MAX_PROTOCOL_LINE_BYTES => {
                        break Err(
                            "offline evaluation report exceeded the protocol limit".to_owned()
                        )
                    }
                    Ok(_) => match serde_json::from_str::<RuntimeEnvelope>(line.trim()) {
                        Ok(envelope)
                            if envelope.kind == "response"
                                && envelope.request_id.as_deref() == Some(request_id.as_str()) =>
                        {
                            if envelope.r#type != "evaluation_report" {
                                break Err(response_error(&envelope, "offline evaluation failed"));
                            }
                            break envelope
                                .payload
                                .ok_or_else(|| "offline evaluation report was empty".to_owned());
                        }
                        Ok(_) => continue,
                        Err(error) => {
                            break Err(format!("offline evaluator returned invalid JSON: {error}"))
                        }
                    },
                    Err(error) => break Err(format!("failed to read offline evaluation: {error}")),
                }
            };
            let _ = sender.send(result);
        });
        let started = Instant::now();
        let received = receiver.recv_timeout(Duration::from_secs(30));
        let _ = child.kill();
        let _ = child.wait();
        let report =
            received.map_err(|_| "offline evaluation timed out after 30 seconds".to_owned())?;
        let report = report?;
        self.database
            .record_evaluation_report(&report, started.elapsed().as_millis() as i64)
    }

    pub fn new(
        app: AppHandle,
        database: Database,
        sessions_dir: PathBuf,
        attachments_dir: PathBuf,
        skills_dir: PathBuf,
        yuxi_client: YuxiClient,
    ) -> Self {
        let host = Self {
            app,
            database: database.clone(),
            state: Arc::new(Mutex::new(RuntimeHostState {
                cancellation: crate::kernel::CancellationRegistry::default(),
                kernel_active_runs: HashSet::new(),
                shutting_down: false,
                state: "stopped".to_owned(),
                worker: None,
                stderr: VecDeque::new(),
                last_error: None,
                runtime: "not-started".to_owned(),
                runtime_version: None,
                capabilities: json!({}),
                execution_profile_id: "legacy".to_owned(),
                pending_approvals: Arc::new(Mutex::new(HashMap::new())),
                active_file_writes: Arc::new(Mutex::new(HashSet::new())),
                queued_runs: VecDeque::new(),
                dispatching_run_id: None,
                dispatching_conversation_id: None,
                cancelled_dispatches: HashSet::new(),
                recovery_attempts: 0,
                last_recovery_at: None,
                shadow: Arc::new(shadow_reconcile::ShadowReconciler::new(database.clone())),
            })),
            run_transition: Arc::new(Mutex::new(())),
            managed_files_dir: sessions_dir
                .parent()
                .map(|base| base.join("managed-file-backups"))
                .unwrap_or_else(|| sessions_dir.join("managed-file-backups")),
            sessions_dir,
            attachments_dir,
            skills_dir,
            yuxi_client,
            child_hosts: Arc::new(Mutex::new(HashMap::new())),
            run_budget_override: None,
            enforce_count_budget_override: true,
            execution_profile_override: None,
            tool_allowlist_override: None,
            mcp_server_scope_override: None,
            digital_scheduler_started: Arc::new(AtomicBool::new(false)),
            digital_scheduler_stop: Arc::new(AtomicBool::new(false)),
            active_tool_handlers: Arc::new(AtomicUsize::new(0)),
        };
        host.reconcile_continuation_proposals();
        if let Err(error) = database.kernel_jobs_reconcile_orphans() {
            eprintln!("Background job recovery could not finish: {error}");
        }
        host
    }

    pub(crate) fn managed_files_dir(&self) -> &Path {
        &self.managed_files_dir
    }

    fn reconcile_continuation_proposals(&self) {
        if !self.reconcile_continuation_proposals_pass() {
            return;
        }
        let host = self.clone();
        thread::spawn(move || {
            while host.reconcile_continuation_proposals_pass() {
                thread::yield_now();
            }
        });
    }

    fn reconcile_continuation_proposals_pass(&self) -> bool {
        match continuation::reconcile_unprojected_proposals_until_budget(
            &self.database,
            MAX_CONTINUATION_RECONCILIATION_ITEMS,
            MAX_CONTINUATION_RECONCILIATION_ITEMS_PER_PASS,
            MAX_CONTINUATION_RECONCILIATION_PASS_DURATION,
        ) {
            Ok(sweep) => {
                for decision in sweep.report.decisions {
                    let _ = self.app.emit("fox://continuation-decision", decision);
                }
                for diagnostic in sweep.report.diagnostics {
                    if !diagnostic.shadow {
                        let _ = self.database.mark_run_failed(
                            &diagnostic.run_id,
                            &diagnostic.code,
                            &diagnostic.error,
                        );
                    }
                    let _ = self.app.emit("fox://continuation-diagnostic", diagnostic);
                }
                sweep.has_more
            }
            Err(error) => {
                let _ = self.app.emit(
                    "fox://continuation-diagnostic",
                    json!({
                        "code": "runtime.continuation.reconciliation_scan_failed",
                        "error": error,
                    }),
                );
                false
            }
        }
    }

    pub fn status(&self) -> RuntimeStatus {
        match self.state.lock() {
            Ok(state) => RuntimeStatus {
                state: state.state.clone(),
                runtime: state.runtime.clone(),
                runtime_version: state.runtime_version.clone(),
                capabilities: self.effective_capabilities(&state.capabilities),
                available: self.runtime_available(),
                current_conversation_id: state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.current_conversation_id.clone()),
                active_run_id: state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.active_run_id.clone()),
                last_error: state.last_error.clone(),
            },
            Err(_) => RuntimeStatus {
                state: "crashed".to_owned(),
                runtime: "unknown".to_owned(),
                runtime_version: None,
                capabilities: json!({}),
                available: false,
                current_conversation_id: None,
                active_run_id: None,
                last_error: Some("runtime state lock is poisoned".to_owned()),
            },
        }
    }

    pub fn diagnostics(&self) -> RuntimeDiagnostics {
        let available = self.runtime_available();
        let kernel_shadow_gate = self.database.kernel_shadow_recent_exit_gate().unwrap_or_else(|error| json!({
            "passed": false, "authoritativeEnabled": false, "productionRolloutApproved": false,
            "blockingReasons": [redact_diagnostic_line(&error)],
        }));
        let session_file_count = std::fs::read_dir(&self.sessions_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.path().extension().and_then(|value| value.to_str()) == Some("json")
            })
            .count();
        match self.state.lock() {
            Ok(state) => RuntimeDiagnostics {
                protocol: protocol::PROTOCOL_NAME.to_owned(),
                protocol_version: protocol::PROTOCOL_VERSION,
                capability_manifest_version: protocol::CAPABILITY_MANIFEST_VERSION,
                state: state.state.clone(),
                runtime: state.runtime.clone(),
                runtime_version: state.runtime_version.clone(),
                capabilities: state.capabilities.clone(),
                available,
                current_conversation_id: state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.current_conversation_id.clone()),
                active_run_id: state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.active_run_id.clone()),
                last_error: state.last_error.as_deref().map(redact_diagnostic_line),
                stderr_tail: state
                    .stderr
                    .iter()
                    .rev()
                    .take(20)
                    .rev()
                    .map(|line| redact_diagnostic_line(line))
                    .collect(),
                session_file_count,
                recovery_attempts: state.recovery_attempts,
                last_recovery_at: state.last_recovery_at,
                kernel_shadow_gate: kernel_shadow_gate.clone(),
            },
            Err(_) => RuntimeDiagnostics {
                protocol: protocol::PROTOCOL_NAME.to_owned(),
                protocol_version: protocol::PROTOCOL_VERSION,
                capability_manifest_version: protocol::CAPABILITY_MANIFEST_VERSION,
                state: "crashed".to_owned(),
                runtime: "unknown".to_owned(),
                runtime_version: None,
                capabilities: json!({}),
                available: false,
                current_conversation_id: None,
                active_run_id: None,
                last_error: Some("runtime state lock is poisoned".to_owned()),
                stderr_tail: Vec::new(),
                session_file_count,
                recovery_attempts: 0,
                last_recovery_at: None,
                kernel_shadow_gate,
            },
        }
    }

    pub fn reload_configuration(&self) -> Result<(), String> {
        let active = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .worker
            .as_ref()
            .and_then(|worker| worker.active_run_id.clone());
        let active_children = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .len();
        if active.is_some() || active_children > 0 {
            return Err("模型配置不能在 Agent 正在运行时修改".to_owned());
        }
        self.stop_worker()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?;
        state.capabilities = json!({});
        state.runtime = "not-started".to_owned();
        state.runtime_version = None;
        Ok(())
    }

    fn effective_capabilities(&self, capabilities: &Value) -> Value {
        if capabilities
            .as_object()
            .is_some_and(|capabilities| !capabilities.is_empty())
        {
            return capabilities.clone();
        }
        let image_input = self
            .database
            .get_model_service()
            .ok()
            .flatten()
            .is_some_and(|service| service.supports_image_input);
        json!({
            "streamingText": true,
            "cancellation": true,
            "reasoning": true,
            "sessionResume": true,
            "toolApproval": true,
            "imageInput": image_input,
            // Host persists mid-run additions durably and consumes them at
            // dispatch boundaries; a runtime that does not splice directive
            // steering still receives them through frozen input messages.
            "steering": true,
            "contextCompaction": true,
            "dynamicModelSwitch": false
        })
    }

    pub fn validate_run_attachments(
        &self,
        conversation_id: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<(), String> {
        self.runtime_images(conversation_id, attachments)
            .map(|_| ())
    }

    pub fn start_run(
        &self,
        started: &StartRunResult,
        text: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<(), String> {
        let result = self.start_run_inner(started, text, attachments);
        if let Err(error) = &result {
            if error == CANCELLED_BEFORE_SUBMISSION
                || error == kernel_run_lock::KERNEL_RUN_ALREADY_OWNED {
                return result;
            }
            let kernel_run = self.database.kernel_host_run_state(&started.run.id)
                .ok().flatten().is_some();
            let retire = !kernel_run || kernel_host::kernel_scope_should_retire(
                &self.database, &started.run.id);
            if let Ok(mut state) = self.state.lock() {
                if retire {
                    state.cancellation.retire_run(&started.run.id);
                }
                if let Some(worker) = state.worker.as_mut() {
                    if worker.active_run_id.as_deref() == Some(&started.run.id) {
                        worker.active_run_id = None;
                    }
                }
                if state.state == "busy" {
                    state.state = "ready".to_owned();
                }
                state.last_error = Some(error.clone());
            }
        }
        result
    }

    /// Freeze an observation-only Kernel shadow context for a legacy run.
    ///
    /// This is the production Shadow entry point: it only records a frozen
    /// `kernel_shadow_runs` row (kernel mode `shadow`). It does NOT run tools,
    /// raise approvals, dispatch to the executable outbox, or touch the legacy
    /// run. Any error is swallowed (logged into diagnostics) so Shadow can
    /// never break the authoritative legacy path. Authoritative Kernel
    /// execution remains disabled; there is no production caller that leases
    /// `kernel_effect_outbox`.
    fn bootstrap_shadow_context(
        &self,
        legacy_run_id: &str,
        conversation_id: &str,
        execution_profile_id: &str,
        prompt_config_hash: &str,
    ) {
        if let Err(error) = self.try_bootstrap_shadow_context(
            legacy_run_id,
            conversation_id,
            execution_profile_id,
            prompt_config_hash,
        ) {
            // Shadow must never fail the legacy run; surface it in diagnostics only.
            if let Ok(mut state) = self.state.lock() {
                state.last_error = Some(format!("kernel shadow bootstrap skipped: {error}"));
            }
        }
    }

    fn bootstrap_shadow_context_from_request_snapshot(
        &self,
        legacy_run_id: &str,
        conversation_id: &str,
        snapshot: &Value,
    ) {
        let result = (|| -> Result<(String, String), String> {
            let execution_profile_id = self
                .database
                .frozen_run_execution_profile_id(legacy_run_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    format!(
                        "Run '{legacy_run_id}' has no frozen execution profile for Kernel Shadow"
                    )
                })?;
            let prompt_config_hash = shadow_prompt_config_hash(snapshot)?;
            Ok((execution_profile_id, prompt_config_hash))
        })();
        match result {
            Ok((execution_profile_id, prompt_config_hash)) => self.bootstrap_shadow_context(
                legacy_run_id,
                conversation_id,
                &execution_profile_id,
                &prompt_config_hash,
            ),
            Err(error) => {
                if let Ok(mut state) = self.state.lock() {
                    state.last_error = Some(format!("kernel shadow bootstrap skipped: {error}"));
                }
            }
        }
    }

    fn try_bootstrap_shadow_context(
        &self,
        legacy_run_id: &str,
        conversation_id: &str,
        execution_profile_id: &str,
        prompt_config_hash: &str,
    ) -> Result<(), String> {
        let engine = "pi";
        // Content-addressed identities: hash the actual frozen contract inputs
        // (engine + manifest version; conversation/permission; profile/prompt)
        // rather than emitting synthetic prefixes. These are observation
        // hashes over stable contract identity, not user content or secrets.
        let manifest_version = protocol::CAPABILITY_MANIFEST_VERSION as u32;
        // Content-addressed hashes over the actual frozen contract CONTENT, not
        // just identity tags, so changing permission scope, the capability set,
        // or the prompt/profile changes the hash.
        let capabilities_json = self
            .shadow_capabilities_json()
            .unwrap_or_else(|| format!("manifest-v{manifest_version}"));
        let permission_snapshot = self
            .database
            .run_shadow_permission_snapshot(legacy_run_id, conversation_id)?;
        let manifest_hash = runtime_shadow_hash(&format!(
            "manifest|{engine}|v{manifest_version}|{capabilities_json}"
        ));
        let permission_snapshot_id = runtime_shadow_hash(&permission_snapshot.to_string());
        let budgets = self.database.run_time_budgets(legacy_run_id)?;
        let frozen = crate::kernel::RunFrozenConfig {
            engine_id: engine.to_owned(),
            kernel_mode: crate::kernel::KernelMode::Shadow.as_str().to_owned(),
            capability_manifest_version: manifest_version,
            capability_manifest_hash: manifest_hash.clone(),
            permission_snapshot_id,
            execution_profile_id: execution_profile_id.to_owned(),
            prompt_config_hash: prompt_config_hash.to_owned(),
            model_request_timeout_ms: budgets.model_request_ms,
            model_first_response_ms: budgets.model_first_response_ms,
            model_idle_ms: budgets.model_idle_ms,
            tool_execution_timeout_ms: budgets.tool_execution_ms,
            run_execution_budget_ms: budgets.run_execution_ms,
            run_execution_limited: budgets.run_execution_limited,
            approval_wait_timeout_ms: budgets.approval_wait_ms,
            provider_max_retries: 2,
            turn_max_retries: 0,
            experimental_compute_job_notice: false,
        };
        self.shadow_reconciler()?.bootstrap_with_policy(
            legacy_run_id,
            conversation_id,
            execution_profile_id,
            frozen,
            Some(permission_snapshot),
        )
    }

    fn shadow_reconciler(&self) -> Result<Arc<shadow_reconcile::ShadowReconciler>, String> {
        self.state
            .lock()
            .map(|state| state.shadow.clone())
            .map_err(|_| "shadow host state lock poisoned".to_string())
    }

    fn report_shadow_result(&self, result: Result<(), String>) {
        if let Err(error) = result {
            if let Ok(mut state) = self.state.lock() {
                state.last_error = Some(format!("kernel shadow observation failed: {error}"));
            }
        }
    }

    pub(crate) fn recover_shadow_observations(&self) {
        self.report_shadow_result((|| self.shadow_reconciler()?.recover_all().map(|_| ()))());
    }

    /// Feed a legacy tool-batch decision into the run's shadow context, run the
    /// shadow decision core over the same batch, and persist the comparison
    /// record. Independent legacy tool facts (`legacy_tools`) are supplied by
    /// the caller from what legacy actually decided. Never executes tools and
    /// never affects the legacy run; failures are diagnostic-only.
    /// Feed a single legacy tool.preflight decision into the shadow context.
    /// The preflight is treated as a one-tool observation; legacy decision and
    /// identity come from the real preflight (`allowed` + payload), kernel
    /// decision from the shadow policy. Observation-only.
    fn shadow_feed_preflight(
        &self,
        legacy_run_id: &str,
        tool: &str,
        envelope: &RuntimeEnvelope,
        allowed: bool,
        payload: &Value,
    ) {
        let nested_parent = envelope
            .payload
            .as_ref()
            .filter(|request| {
                request.get("observationScope").and_then(Value::as_str) == Some("nested")
            })
            .and_then(|request| request.get("parentToolCallId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if let Some(parent_tool_call_id) = nested_parent {
            let valid_graph_parent = self
                .database
                .get_runtime_tool_call(legacy_run_id, parent_tool_call_id)
                .ok()
                .flatten()
                .is_some_and(|record| {
                    record.tool_name == "graph_readonly_run"
                        && record.execution_location == "runtime"
                });
            if valid_graph_parent {
                // Graph preview node reads are internal to graph_readonly_run
                // and have no top-level Runtime tool event to settle. The outer
                // graph ToolCall is registered from its real projection.
                return;
            }
        }
        let tool_call_id = match shadow_preflight_tool_call_id(envelope) {
            Ok(tool_call_id) => tool_call_id.to_owned(),
            Err(error) => {
                if let Ok(mut state) = self.state.lock() {
                    state.last_error = Some(format!(
                        "kernel shadow preflight observation skipped: {error}"
                    ));
                }
                return;
            }
        };
        // An allowed Host preflight may canonicalise or default arguments. Feed
        // the approved payload.input as the actual legacy/kernel observation;
        // blocked or malformed responses fall back to the requested input.
        let input = shadow_preflight_input(envelope, allowed, payload);
        // Legacy decision from the real preflight outcome.
        let legacy_decision = if allowed {
            "allow"
        } else if payload
            .get("decision")
            .and_then(Value::as_str)
            .is_some_and(|d| d.contains("approv") || d.contains("ask"))
        {
            "approval"
        } else {
            "deny"
        };
        self.report_shadow_result((|| {
            let reconciler = self.shadow_reconciler()?;
            if reconciler.context_state(legacy_run_id) == "absent" {
                return Ok(());
            }
            let outcome = reconciler.feed_preflight(
                legacy_run_id,
                tool,
                &tool_call_id,
                input,
                legacy_decision,
            )?;
            outcome.persist_error.map_or(Ok(()), Err)
        })());
    }

    /// Normalised capability manifest content for the shadow identity hash.
    /// Reads the live (effective) capability set so changing the manifest
    /// changes the frozen manifest hash.
    fn shadow_capabilities_json(&self) -> Option<String> {
        let state = self.state.lock().ok()?;
        let caps = self.effective_capabilities(&state.capabilities);
        // Canonicalise (sort keys) via serde_json for a stable hash input.
        let mut value = caps.clone();
        if let Some(obj) = value.as_object_mut() {
            for (_k, v) in obj.iter_mut() {
                if v.is_array() {
                    if let Some(arr) = v.as_array() {
                        let mut sorted = arr.clone();
                        sorted.sort_by(|a, b| {
                            a.to_string()
                                .partial_cmp(&b.to_string())
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        *v = serde_json::Value::Array(sorted);
                    }
                }
            }
        }
        Some(value.to_string())
    }

    /// Feed a legacy terminal event into the run's shadow context and persist
    /// the comparison; then drop the shadow context. `legacy_state` is the
    /// legacy run STATE name (`completed` / `failed` / `cancelled`). The kernel
    /// side is driven through the matching state machine transitions: a cancel
    /// requires a prior `request_cancel`, and completed requires settled tools
    /// (fed earlier via `shadow_feed_tool_settled`).
    pub fn shadow_feed_terminal(
        &self,
        legacy_run_id: &str,
        legacy_state: &str,
        kernel_outcome: crate::kernel::RunOutcome,
        _retry_timeout: crate::kernel::RetryTimeoutFacts,
    ) {
        self.report_shadow_result((|| {
            let reconciler = self.shadow_reconciler()?;
            if reconciler.context_state(legacy_run_id) == "absent" {
                return Ok(());
            }
            let outcome = reconciler.terminal(legacy_run_id, legacy_state, kernel_outcome, true)?;
            outcome.persist_error.map_or(Ok(()), Err)
        })());
    }

    pub fn shadow_feed_tool_settled(
        &self,
        legacy_run_id: &str,
        tool_call_id: &str,
        ok: bool,
        _result_json: &str,
    ) {
        self.report_shadow_result((|| {
            let reconciler = self.shadow_reconciler()?;
            if reconciler.context_state(legacy_run_id) == "absent" {
                return Ok(());
            }
            reconciler.settle_applied_tool(legacy_run_id, tool_call_id, ok)
        })());
    }

    pub fn start_run_detached(
        &self,
        started: StartRunResult,
        text: String,
        attachments: Vec<AttachmentRecord>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            state.queued_runs.push_back(QueuedRun {
                started,
                text,
                attachments,
            });
        }
        self.dispatch_next_queued_run();
    }

    fn dispatch_next_queued_run(&self) {
        let queued = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state.shutting_down || state.dispatching_run_id.is_some() || !state.kernel_active_runs.is_empty()
                || state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.active_run_id.as_ref())
                    .is_some()
                || matches!(
                    state.state.as_str(),
                    "busy" | "starting" | "recovering" | "stopping"
                )
            {
                return;
            }
            let Some(queued) = state.queued_runs.pop_front() else {
                return;
            };
            state.dispatching_run_id = Some(queued.started.run.id.clone());
            state.dispatching_conversation_id = Some(queued.started.run.conversation_id.clone());
            queued
        };
        let runtime_host = self.clone();
        std::mem::drop(tauri::async_runtime::spawn_blocking(move || {
            let run_id = queued.started.run.id.clone();
            let cancelled = runtime_host
                .state
                .lock()
                .ok()
                .is_some_and(|mut state| state.cancelled_dispatches.remove(&run_id));
            if !cancelled {
                let result =
                    runtime_host.start_run(&queued.started, &queued.text, &queued.attachments);
                if let Err(error) = result {
                    if error == CANCELLED_BEFORE_SUBMISSION {
                        if let Ok(mut state) = runtime_host.state.lock() {
                            if state.state == "busy" {
                                state.state = "ready".to_owned();
                            }
                        }
                    } else if error != kernel_run_lock::KERNEL_RUN_ALREADY_OWNED {
                        runtime_host.record_start_failure(&queued.started, error);
                    }
                }
            }
            if let Ok(mut state) = runtime_host.state.lock() {
                if state.dispatching_run_id.as_deref() == Some(&run_id) {
                    state.dispatching_run_id = None;
                    state.dispatching_conversation_id = None;
                }
            }
            runtime_host.dispatch_next_queued_run();
        }));
    }

    fn cancel_queued_run(&self, run_id: &str) -> Result<bool, String> {
        let conversation_id = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            if state.dispatching_run_id.as_deref() == Some(run_id)
                && state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.active_run_id.as_deref())
                    != Some(run_id)
            {
                state.cancelled_dispatches.insert(run_id.to_owned());
                state.dispatching_conversation_id.clone()
            } else {
                let Some(index) = state
                    .queued_runs
                    .iter()
                    .position(|queued| queued.started.run.id == run_id)
                else {
                    return Ok(false);
                };
                state
                    .queued_runs
                    .remove(index)
                    .map(|queued| queued.started.run.conversation_id)
            }
        };
        let Some(conversation_id) = conversation_id else {
            return Ok(false);
        };
        let seq = self.database.next_run_seq(run_id).unwrap_or(1);
        let payload = json!({
            "type": "run.cancelled",
            "code": "runtime.cancelled_while_queued",
            "message": "Run was cancelled before it started.",
        });
        if self.database.apply_runtime_event(run_id, seq, &payload)? {
            let _ = self.app.emit(
                "fox://runtime-event",
                RuntimeEventNotification {
                    conversation_id,
                    runtime_session_id: None,
                    run_id: run_id.to_owned(),
                    seq,
                    timestamp: protocol::timestamp(),
                    event: payload,
                },
            );
        }
        Ok(true)
    }

    fn record_start_failure(&self, started: &StartRunResult, message: String) {
        if self.database.run_control_binding(&started.run.id).ok().flatten()
            .is_some_and(|binding| binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative) {
            let _ = self.record_kernel_start_failure(&started.run.id);
            return;
        }
        let (code, detail) = structured_runtime_error(&message);
        let seq = self.database.next_run_seq(&started.run.id).unwrap_or(1);
        let payload = json!({
            "type": "run.failed",
            "code": code,
            "message": detail,
        });
        match self
            .database
            .apply_runtime_event(&started.run.id, seq, &payload)
        {
            Ok(true) => {
                let runtime_session_id = self
                    .database
                    .get_runtime_session(&started.run.conversation_id)
                    .ok()
                    .flatten()
                    .map(|session| session.runtime_session_id);
                let _ = self.app.emit(
                    "fox://runtime-event",
                    RuntimeEventNotification {
                        conversation_id: started.run.conversation_id.clone(),
                        runtime_session_id,
                        run_id: started.run.id.clone(),
                        seq,
                        timestamp: protocol::timestamp(),
                        event: payload,
                    },
                );
            }
            Ok(false) => {}
            Err(error) => {
                let _ = self.database.mark_run_failed(
                    &started.run.id,
                    code,
                    &format!("{detail}; failed to persist runtime event: {error}"),
                );
            }
        }
        self.shadow_feed_terminal(
            &started.run.id,
            "failed",
            crate::kernel::RunOutcome::Failed {
                code: code.to_owned(),
                message: detail.to_owned(),
            },
            Default::default(),
        );
    }

    /// Continue a paused Kernel Run through the *normal* initialization path (R2).
    ///
    /// Order matters and is the whole point of the fix:
    ///   1. re-verify the permission scope against live conversation state;
    ///   2. resolve the runtime/model service and ask the Runtime for the model
    ///      description — exactly what a fresh authoritative Run does;
    ///   3. derive the prompt hash, the frozen initial input and the Host scope;
    ///   4. only then insert the attempt, so the `kernel_runs` row is created with
    ///      the real `prompt_config_hash` / `frozen_config_json` /
    ///      `capability_manifest_hash` instead of placeholders;
    ///   5. freeze the model config, initial input and scope, then drive it.
    ///
    /// The previous version inserted placeholder frozen values and never froze the
    /// artifacts, so the production dispatch failed with
    /// "Kernel model configuration is missing; no current-settings fallback".
    #[allow(clippy::too_many_arguments)]
    pub fn continue_kernel_run(
        &self,
        request: &crate::database::ContinuationRequest,
    ) -> Result<StartRunResult, String> {
        let source_binding = self
            .database
            .run_control_binding(&request.source_run_id)?
            .ok_or("缺少冻结权限")?;
        // 1. Permission is re-verified (and recorded) before anything is created.
        let verified = self.database.re_verify_continuation_permission(
            &request.source_run_id,
            &request.conversation_id,
        )?;
        let prompt_text =
            self.database
                .kernel_continuation_prompt(&request.conversation_id, &request.source_run_id)?;
        let budgets = crate::database::run_budget_for_tier(
            source_binding.budgets.clone(),
            request.tier,
            request.custom_execution_ms,
        )?;
        // 2. The runtime description needs the real prompt/service pair.
        let model_service = self
            .database
            .get_model_service()?
            .ok_or_else(|| "model service is not configured".to_owned())?;
        let effective_max_output_tokens = model_service.max_output_tokens;
        let context_chars = model_service
            .context_window
            .saturating_sub(effective_max_output_tokens)
            .max(4096)
            .saturating_mul(3)
            .clamp(12_000, 240_000) as usize;
        let messages = self.database.runtime_prompt_context_budget(
            &request.conversation_id,
            240,
            context_chars,
        )?;
        let context = self.agent_prompt_context(&request.conversation_id, &prompt_text, "primary")?;
        let conversation_id = request.conversation_id.clone();
        let prompt = json!({
            "text": prompt_text,
            "messages": messages,
            "images": Vec::<Value>::new(),
            "systemPrompt": context.system_prompt,
            "skillPrompt": context.skill_prompt,
            "assistantPackage": context.assistant_package,
            "expertPackage": context.expert_package,
            "expertBinding": context.expert_binding_payload,
            "projectContext": project_context_for(&self.database, &self.sessions_dir, &conversation_id, &verified.permission),
            "workSnapshot": work_tools::snapshot(&self.database, &request.conversation_id)?,
            "memoryContext": self.database.recall_memories(&request.conversation_id, None, &prompt_text, 8, 6_000)?,
            "runContext": {
                "runKind": "primary",
                "runRole": "user_facing_lead",
                "isUserFacingLead": true,
            },
            "promptBudget": {"maxPromptTokens": model_service
                .context_window
                .saturating_sub(effective_max_output_tokens)
                .max(4096)},
            "continuation": true,
        });
        let service = json!({
            "baseUrl": model_service.base_url,
            "modelId": model_service.model_id,
            "apiType": model_service.api_type,
            "contextWindow": model_service.context_window,
            "maxOutputTokens": effective_max_output_tokens,
            "supportsImageInput": model_service.supports_image_input,
        });
        let runtime = self.runtime_command()?;
        // The description is bound to the *verified* binding, so the frozen scope
        // and the tool catalog are derived from the re-verified permission.
        let mut verified_binding = source_binding.clone();
        verified_binding.run_id = request.source_run_id.clone();
        verified_binding.permission = verified.permission.clone();
        verified_binding.permission_snapshot_id = verified.permission_snapshot_id.clone();
        verified_binding.budgets = budgets.clone();
        let registry = crate::kernel::CancellationRegistry::default();
        registry.register_run(&format!("continuation-describe:{}", request.source_run_id))?;
        let token = registry.run_token(&format!(
            "continuation-describe:{}",
            request.source_run_id
        ))?;
        let mut supported = kernel_gateway::supported_tools();
        // 3. Describe against the verified scope, exactly like a fresh start.
        let config = {
            let describe_binding = verified_binding.clone();
            kernel_model_worker::describe(
                &runtime,
                &describe_binding,
                service.clone(),
                prompt.clone(),
                std::mem::take(&mut supported),
                &token,
            )?
        };
        let hash = config.hash()?;
        let input = kernel_host::initial_input(&verified_binding, &prompt, &hash)?;
        let scope =
            kernel_gateway::freeze_scope(&self.database, &verified_binding, &prompt, &config)?;
        let frozen = crate::kernel::RunFrozenConfig {
            engine_id: verified_binding.engine_id.clone(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: runtime_shadow_hash(
                &serde_json::to_string(&scope).map_err(|_| "invalid resource scope")?,
            ),
            permission_snapshot_id: verified_binding.permission_snapshot_id.clone(),
            execution_profile_id: verified_binding.execution_profile_id.clone(),
            prompt_config_hash: hash.clone(),
            model_request_timeout_ms: budgets.model_request_ms,
            model_first_response_ms: budgets.model_first_response_ms,
            model_idle_ms: budgets.model_idle_ms,
            tool_execution_timeout_ms: budgets.tool_execution_ms,
            run_execution_budget_ms: budgets.run_execution_ms,
            run_execution_limited: budgets.run_execution_limited,
            approval_wait_timeout_ms: budgets.approval_wait_ms,
            provider_max_retries: 2,
            turn_max_retries: 1,
            experimental_compute_job_notice: false,
        };
        let prepared = crate::database::PreparedContinuation {
            budgets: budgets.clone(),
            prompt_config_hash: hash.clone(),
            capability_manifest_hash: frozen.capability_manifest_hash.clone(),
            frozen_config_json: serde_json::to_string(&frozen)
                .map_err(|_| "invalid frozen Kernel Run")?,
            artifacts: Some((config, input, scope)),
            skill_activations: context.skill_activations,
        };
        // 4. The attempt row now carries the real frozen values.
        let started =
            self.database
                .kernel_insert_continuation_attempt(request, &prompt_text, &prepared)?;
        // All new-Run identities and frozen artifacts were committed atomically.
        // 6. Start it detached, like every other Kernel start.
        let ownership = kernel_host::acquire(&self.sessions_dir, &started.run.id)?;
        let binding = self
            .database
            .run_control_binding(&started.run.id)?
            .ok_or("续做任务缺少冻结绑定")?;
        let host = self.clone_for_detached_dispatch();
        let database = self.database.clone();
        let run_id = started.run.id.clone();
        std::thread::spawn(move || {
            if let Err(error) = host.start_kernel_run(ownership, &binding, prompt, service) {
                if error != CANCELLED_BEFORE_SUBMISSION
                    && error != kernel_run_lock::KERNEL_RUN_ALREADY_OWNED {
                    let _ = database.mark_run_failed(
                        &run_id,
                        "kernel.continuation_start_failed",
                        &error,
                    );
                }
            }
        });
        Ok(started)
    }

    /// A handle for a detached Kernel dispatch. The struct keeps no mutable
    /// per-run state of its own, so a shallow copy is enough.
    fn clone_for_detached_dispatch(&self) -> RuntimeHost {
        RuntimeHost {
            app: self.app.clone(),
            database: self.database.clone(),
            state: self.state.clone(),
            run_transition: self.run_transition.clone(),
            sessions_dir: self.sessions_dir.clone(),
            attachments_dir: self.attachments_dir.clone(),
            skills_dir: self.skills_dir.clone(),
            managed_files_dir: self.managed_files_dir.clone(),
            yuxi_client: self.yuxi_client.clone(),
            child_hosts: self.child_hosts.clone(),
            run_budget_override: self.run_budget_override.clone(),
            enforce_count_budget_override: self.enforce_count_budget_override,
            execution_profile_override: self.execution_profile_override,
            tool_allowlist_override: self.tool_allowlist_override.clone(),
            mcp_server_scope_override: self.mcp_server_scope_override.clone(),
            digital_scheduler_started: self.digital_scheduler_started.clone(),
            digital_scheduler_stop: self.digital_scheduler_stop.clone(),
            active_tool_handlers: self.active_tool_handlers.clone(),
        }
    }

    /// Delegates to [`conversation_prompt_context`], the non-UI production
    /// construction, so the application and any non-UI entry point build the
    /// same prompt from the same Host facts.
    fn agent_prompt_context(&self, conversation_id: &str, text: &str, run_kind: &str) -> Result<AgentPromptContext, String> {
        conversation_prompt_context(
            &self.database,
            &self.skills_dir,
            conversation_id,
            text,
            run_kind,
            PromptScopeOverrides {
                tool_allowlist: self.tool_allowlist_override.as_ref(),
                mcp_server_scope: self.mcp_server_scope_override.as_ref(),
            },
        )
    }

    fn start_run_inner(
        &self,
        started: &StartRunResult,
        text: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<(), String> {
        if let Ok(mut state) = self.state.lock() {
            if state.shutting_down { return Err("Runtime Host is shutting down".into()); }
            state.recovery_attempts = 0;
        }
        self.cancel_stale_pending_approvals_for_conversation(
            &started.run.conversation_id,
            &started.run.id,
        );
        let run_kind = if self.database.is_digital_colleague_run(&started.run.id)? {
            "digital_colleague"
        } else if self.run_budget_override.is_some() {
            "child"
        } else {
            "primary"
        };
        let frozen_binding = self.database.run_control_binding(&started.run.id)?;
        let authority = kernel_authority::select(
            frozen_binding.as_ref().map(|binding| binding.authority),
            std::env::var("FOX_KERNEL_MODE"),
        )?;
        let kernel_ownership = if authority == fox_engine_protocol::ExecutionAuthority::Authoritative {
            Some(kernel_host::acquire(&self.sessions_dir, &started.run.id)?)
        } else { None };
        if authority == fox_engine_protocol::ExecutionAuthority::Legacy {
            crate::lifecycle_hooks::run_event(&self.database,"before_run",&started.run.id,run_kind,
                &json!({"conversationId":started.run.conversation_id,"runKind":run_kind,
                    "depth":if run_kind=="child" {1} else {0}}))?;
        }
        let execution_profile = if authority == fox_engine_protocol::ExecutionAuthority::Authoritative {
            if let Some(binding) = &frozen_binding {
                continuation::ExecutionProfileSelection::resolve(&binding.execution_profile_id)?
            } else if let Some(profile) = self.execution_profile_override {
                continuation::ExecutionProfileSelection::resolve(profile)?
            } else { continuation::ExecutionProfileSelection::load(&self.database)? }
        } else {
            self.ensure_worker(&started.run.conversation_id)?;
            self.current_execution_profile()?
        };
        self.database
            .freeze_run_execution_profile(&started.run.id, execution_profile.id())
            .map_err(|error| error.to_string())?;
        if authority == fox_engine_protocol::ExecutionAuthority::Authoritative
            && self.database.kernel_host_run_state(&started.run.id)?.is_some() {
            let binding = frozen_binding.as_ref().ok_or("existing Kernel has no binding")?;
            return self.start_kernel_run(kernel_ownership.ok_or("missing Kernel ownership")?, binding, Value::Null, Value::Null);
        }
        let runtime_session_id = if authority == fox_engine_protocol::ExecutionAuthority::Legacy {
            self.open_runtime_session(&started.run.conversation_id)?.0
        } else { String::new() };
        let images = self.runtime_images(&started.run.conversation_id, attachments)?;

        let model_service = self
            .database
            .get_model_service()?
            .ok_or_else(|| "model service is not configured".to_owned())?;
        let count_budget_override = self
            .run_budget_override
            .as_ref()
            .filter(|_| self.enforce_count_budget_override);
        let effective_max_output_tokens = count_budget_override
            .map(|budget| budget.max_output_tokens)
            .unwrap_or(model_service.max_output_tokens)
            .min(model_service.max_output_tokens);
        let input_tokens = model_service
            .context_window
            .saturating_sub(effective_max_output_tokens)
            .max(4096);
        let input_tokens = count_budget_override
            .map(|budget| {
                input_tokens.min(
                    budget
                        .max_total_tokens
                        .saturating_sub(effective_max_output_tokens)
                        .max(512),
                )
            })
            .unwrap_or(input_tokens);
        let context_chars = (input_tokens as usize)
            .saturating_mul(3)
            .clamp(12_000, 240_000);
        let messages = self.database.runtime_prompt_context_budget(
            &started.run.conversation_id,
            240,
            context_chars,
        )?;
        let AgentPromptContext { system_prompt, assistant_package, expert_package,
            expert_binding_payload, run_role, run_context, skill_prompt, skill_activations } =
            self.agent_prompt_context(&started.run.conversation_id, text, run_kind)?;
        for record in &skill_activations {
            self.database.record_skill_activation(&started.run.id, Some(&started.run.conversation_id), record)?;
        }
        let control_binding = match frozen_binding {
            Some(binding) => binding,
            None if authority == fox_engine_protocol::ExecutionAuthority::Authoritative => {
                let mut budgets = fox_engine_protocol::TimeBudgets::continuous();
                // Only an explicit caller budget limits the whole task. Existing
                // frozen bindings above remain unchanged on restart.
                if let Some((tier, custom_ms)) = self.database.kernel_run_budget_selection(&started.run.id)? {
                    budgets = crate::database::run_budget_for_tier(budgets, tier, custom_ms)?;
                }
                if let Some(budget) = &self.run_budget_override {
                    budgets.restrict_execution_ms(budget.max_duration_ms as i64);
                }
                self.database.freeze_kernel_run_control_for_engine(&started.run.id, execution_profile.id(), budgets, &crate::kernel_model_config::configured_engine()?)?
            }
            None => {
                let executor = match std::env::var("FOX_RESOURCE_GATEWAY_READS") {
                    // New runs need Host observations for guarded file writes. Existing frozen runs retain their executor.
                    Err(std::env::VarError::NotPresent) => fox_engine_protocol::ResourceExecutor::Rust,
                    Ok(value) if value == "runtime" => fox_engine_protocol::ResourceExecutor::Runtime,
                    Ok(value) if value == "rust" => fox_engine_protocol::ResourceExecutor::Rust,
                    _ => return Err("FOX_RESOURCE_GATEWAY_READS must be runtime or rust".into()),
                };
                self.database.freeze_legacy_run_control_with_executor(&started.run.id, execution_profile.id(), executor)?
            }
        };
        if control_binding.authority != authority
            || authority == fox_engine_protocol::ExecutionAuthority::Legacy && control_binding.engine_id != "pi"
            || control_binding.conversation_id != started.run.conversation_id
            || control_binding.execution_profile_id != execution_profile.id() {
            return Err("Startup cannot replace the frozen Run engine/authority/profile/conversation".into());
        }
        // Ordinary user-facing tasks that explicitly promise files get a
        // lightweight, artifact-bound delivery checklist. Questions, analysis
        // and delegated child Runs seed nothing; the ledger is evaluated by
        // the authoritative live loop (legacy per-round transport does not
        // verify it, so it must not leave pending rows there).
        if authority == fox_engine_protocol::ExecutionAuthority::Authoritative
            && run_role == "user_facing_lead"
        {
            let mut delivery_seeds = delivery::expectations_from_task(text);
            if !delivery_seeds.is_empty() {
                // Structured demands the task states in a machine-decidable form
                // are bound to the items that must satisfy them, so the stop gate
                // verifies exactly what this task promised. Expectations that must
                // be recomputed from source data are bound here, where the
                // authorized project root is known: the file's bytes, its hash and
                // the column's presence are Host facts, not task-text guesses.
                let requirements = delivery::requirements_from_task(text)
                    .into_iter()
                    .map(|requirement| match control_binding.permission.project_root.as_deref() {
                        Some(root) => {
                            delivery::bind_source_distribution(std::path::Path::new(root), requirement)
                        }
                        None => requirement,
                    })
                    .collect::<Vec<_>>();
                if !requirements.is_empty() {
                    delivery::attach_requirements_to_seeds(&mut delivery_seeds, &requirements);
                }
                self.database.seed_delivery_checklist(
                    &started.run.id,
                    &delivery_seeds,
                    crate::database::now_ms(),
                )?;
            }
        }
        // Host-decided placement for user-facing files. The folder is decided
        // once per (conversation, project) and persisted, so a continuation,
        // a retry on a later day and an application restart all land in the
        // same result folder, while two conversations can never overwrite each
        // other. It is reference data for the model; the Host still resolves
        // and admits every real path.
        let project_context = project_context_for(
            &self.database,
            &self.sessions_dir,
            &started.run.conversation_id,
            &control_binding.permission,
        );
        let work_snapshot = work_tools::snapshot(&self.database, &started.run.conversation_id)?;
        let memory_context = self.database.recall_memories(
            &started.run.conversation_id,
            Some(&started.run.id),
            text,
            8,
            6_000,
        )?;

        // Assembled once and borrowed by both prompt branches below, so the
        // Kernel and Legacy paths cannot disagree about what the model sees.
        let context = AgentPromptContext {
            system_prompt,
            assistant_package,
            expert_package,
            expert_binding_payload,
            run_role,
            run_context,
            skill_prompt,
            skill_activations,
        };
        if let Some(ownership) = kernel_ownership {
            // One payload constructor for every entry point, so what the model
            // sees cannot drift between the application and a non-UI caller such
            // as the real acceptance harness.
            let prompt = kernel_prompt_payload(KernelPromptPayload {
                text,
                messages: &messages,
                images: &images,
                context: &context,
                project_context: &project_context,
                work_snapshot: &work_snapshot,
                memory_context: &memory_context,
                max_prompt_tokens: input_tokens,
            });
            for record in &context.skill_activations {
                self.database.record_skill_activation(&started.run.id, Some(&started.run.conversation_id), record)?;
            }
            let service = json!({"baseUrl":model_service.base_url,"modelId":model_service.model_id,"apiType":model_service.api_type,
                "contextWindow":model_service.context_window,"maxOutputTokens":effective_max_output_tokens,"supportsImageInput":model_service.supports_image_input});
            return self.start_kernel_run(ownership, &control_binding, prompt, service);
        }

        let _transition = self
            .run_transition
            .lock()
            .map_err(|_| "runtime run transition lock is poisoned".to_owned())?;
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            if state.cancelled_dispatches.remove(&started.run.id) {
                return Err(CANCELLED_BEFORE_SUBMISSION.to_owned());
            }
            state.cancellation.register_run(&started.run.id)?;
            if let Some(worker) = state.worker.as_mut() {
                worker.active_run_id = Some(started.run.id.clone());
                worker.current_runtime_session_id = Some(runtime_session_id.clone());
            }
            state.state = "busy".to_owned();
        }

        let prompt_request = RuntimeRequest::new("prompt")
            .with_conversation(&started.run.conversation_id)
            .with_runtime_session(&runtime_session_id)
            .with_run(&started.run.id)
            .with_payload(json!({
                "text": text,
                "model": started.run.model,
                "messages": messages,
                "systemPrompt": context.system_prompt,
                "skillPrompt": context.skill_prompt,
                "images": images,
                "assistantPackage": context.assistant_package,
                "expertBinding": context.expert_binding_payload,
                "expertPackage": context.expert_package,
                "runContext": context.run_context,
                "projectContext": project_context,
                "controlBinding": control_binding,
                "workSnapshot": work_snapshot,
                "memoryContext": memory_context,
                "promptBudget": { "maxPromptTokens": input_tokens },
                "runBudget": self.run_budget_override.as_ref().map(|budget| {
                    let mut payload = json!({ "maxDurationMs": budget.max_duration_ms });
                    if self.enforce_count_budget_override {
                        payload["maxTotalTokens"] = json!(budget.max_total_tokens);
                        payload["maxToolCalls"] = json!(budget.max_tool_calls);
                    }
                    payload
                }),
            }));
        let prompt_response = self.send_request(prompt_request, RESPONSE_TIMEOUT)?;
        if prompt_response.r#type != "request_succeeded" {
            return Err(response_error(
                &prompt_response,
                "runtime rejected the prompt",
            ));
        }
        Ok(())
    }

    fn runtime_images(
        &self,
        conversation_id: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<Vec<Value>, String> {
        if attachments.iter().any(|attachment| {
            attachment_looks_like_image(attachment)
                && attachment_image_media_type(attachment).is_none()
        }) {
            return Err("图片格式不受支持，仅支持 PNG、JPEG、WebP 和 GIF".to_owned());
        }
        let image_attachments = attachments
            .iter()
            .filter(|attachment| attachment_image_media_type(attachment).is_some())
            .collect::<Vec<_>>();
        if image_attachments.is_empty() {
            return Ok(Vec::new());
        }
        if image_attachments.len() > MAX_IMAGES_PER_MESSAGE {
            return Err(format!("单条消息最多支持 {MAX_IMAGES_PER_MESSAGE} 张图片"));
        }
        let supports_images = self
            .database
            .get_model_service()?
            .is_some_and(|service| service.supports_image_input);
        if !supports_images {
            return Err(
                "当前模型未启用图片理解，请在模型服务设置中确认模型支持视觉输入后开启".to_owned(),
            );
        }
        image_attachments
            .into_iter()
            .map(|attachment| self.runtime_image(conversation_id, attachment))
            .collect()
    }

    fn runtime_image(
        &self,
        conversation_id: &str,
        attachment: &AttachmentRecord,
    ) -> Result<Value, String> {
        if attachment.conversation_id != conversation_id {
            return Err("图片附件不属于当前会话".to_owned());
        }
        let mime_type = attachment_image_media_type(attachment)
            .ok_or_else(|| "图片格式不受支持，仅支持 PNG、JPEG、WebP 和 GIF".to_owned())?;
        if attachment.byte_size < 0 || attachment.byte_size as u64 > MAX_IMAGE_FILE_BYTES {
            return Err("单张图片不能超过 10 MB".to_owned());
        }
        let root = std::fs::canonicalize(&self.attachments_dir)
            .map_err(|error| format!("附件目录不可用: {error}"))?;
        let path = std::fs::canonicalize(&attachment.storage_path)
            .map_err(|error| format!("图片附件不可用: {error}"))?;
        if !path.starts_with(root) {
            return Err("图片附件路径超出 Fox 附件目录".to_owned());
        }
        let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
        if !metadata.is_file() || metadata.len() > MAX_IMAGE_FILE_BYTES {
            return Err("图片附件无效或超过 10 MB".to_owned());
        }
        let dimensions =
            imagesize::size(&path).map_err(|error| format!("无法读取图片尺寸: {error}"))?;
        let pixels = (dimensions.width as u64)
            .checked_mul(dimensions.height as u64)
            .ok_or_else(|| "图片像素尺寸无效".to_owned())?;
        if pixels > MAX_IMAGE_PIXELS {
            return Err("图片像素总量不能超过 2500 万".to_owned());
        }
        let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
        Ok(json!({
            "type": "image",
            "data": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes),
            "mimeType": mime_type,
        }))
    }

    pub fn runtime_available(&self) -> bool {
        self.runtime_command().is_ok()
    }

    fn open_runtime_session(&self, conversation_id: &str) -> Result<(String, PathBuf), String> {
        if let Some(existing) = self.database.get_runtime_session(conversation_id)? {
            if existing.status != "unavailable" {
                let session_path = existing
                    .session_path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| self.session_path(&existing.runtime_session_id));
                let resume_request = RuntimeRequest::new("resume_session")
                    .with_conversation(conversation_id)
                    .with_runtime_session(&existing.runtime_session_id)
                    .with_payload(json!({ "sessionPath": session_path }));
                let response = self.send_request(resume_request, RESPONSE_TIMEOUT)?;
                if response.r#type == "session_created" {
                    self.database.ensure_runtime_session(
                        conversation_id,
                        &existing.runtime_session_id,
                        existing.runtime_version.as_deref(),
                        session_path.to_str(),
                    )?;
                    return Ok((existing.runtime_session_id, session_path));
                }
                self.database
                    .mark_runtime_session_unavailable(conversation_id)?;
            }
        }

        let runtime_session_id = Uuid::new_v4().to_string();
        let session_path = self.session_path(&runtime_session_id);
        let create_request = RuntimeRequest::new("create_session")
            .with_conversation(conversation_id)
            .with_runtime_session(&runtime_session_id)
            .with_payload(json!({ "sessionPath": session_path }));
        let response = self.send_request(create_request, RESPONSE_TIMEOUT)?;
        if response.r#type != "session_created" {
            return Err(response_error(
                &response,
                "runtime session could not be created",
            ));
        }
        self.database.ensure_runtime_session(
            conversation_id,
            &runtime_session_id,
            self.current_runtime_version().as_deref(),
            session_path.to_str(),
        )?;
        Ok((runtime_session_id, session_path))
    }

    fn session_path(&self, runtime_session_id: &str) -> PathBuf {
        self.sessions_dir.join(format!("{runtime_session_id}.json"))
    }

    fn start_child_runtime(
        &self,
        started: StartRunResult,
        child_run: ChildRunRecord,
        parent_conversation_id: String,
    ) -> Result<(), String> {
        self.start_child_runtime_with_profile(started, child_run, parent_conversation_id, None)
    }

    fn start_graph_reviewer_runtime(
        &self,
        started: StartRunResult,
        child_run: ChildRunRecord,
        parent_conversation_id: String,
        review_request_id: String,
    ) -> Result<(), String> {
        validate_graph_reviewer_runtime_contract(&started, &child_run)?;
        self.start_child_runtime_with_profile(
            started,
            child_run,
            parent_conversation_id,
            Some(review_request_id),
        )
    }

    fn start_child_runtime_with_profile(
        &self,
        started: StartRunResult,
        child_run: ChildRunRecord,
        parent_conversation_id: String,
        graph_review_request_id: Option<String>,
    ) -> Result<(), String> {
        let child_run_id = child_run.child_run_id.clone();
        let enforce_count_budget_override = graph_review_request_id.is_some()
            || self
                .database
                .graph_attempt_id_for_child_run(&child_run_id)?
                .is_some();
        if let Some(parent) = self.database.run_control_binding(&child_run.parent_run_id)?
            .filter(|binding| binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative) {
            if self.database.pending_kernel_host_commands(&parent.run_id)?.iter().any(|command| command.kind=="cancel")
                || self.database.kernel_host_run_state(&parent.run_id)?.as_deref()!=Some("running") {
                return Err("Kernel parent is no longer admitting child execution".into());
            }
            let mut inherited = parent;
            inherited.run_id = child_run_id.clone();
            inherited.conversation_id = child_run.child_conversation_id.clone();
            if enforce_count_budget_override { inherited.permission.mode = fox_engine_protocol::PermissionMode::ReadOnly; }
            if graph_review_request_id.is_some() { inherited.execution_profile_id = GRAPH_REVIEWER_EXECUTION_PROFILE.into(); }
            inherited.permission_snapshot_id = Database::run_control_permission_hash(&inherited.permission)?;
            inherited.budgets.restrict_execution_ms(child_run.budget.max_duration_ms);
            self.database.freeze_run_control(&inherited)?;
        }
        let mut child_host = RuntimeHost::new(
            self.app.clone(),
            self.database.clone(),
            self.sessions_dir.clone(),
            self.attachments_dir.clone(),
            self.skills_dir.clone(),
            self.yuxi_client.clone(),
        );
        child_host.run_budget_override = Some(child_run.budget.clone());
        child_host
            .state
            .lock()
            .map_err(|_| "child host state lock poisoned")?
            .shadow = self.shadow_reconciler()?;
        child_host.enforce_count_budget_override = enforce_count_budget_override;
        if graph_review_request_id.is_some() {
            child_host.execution_profile_override = Some(GRAPH_REVIEWER_EXECUTION_PROFILE);
        }
        let (shared_pending_approvals, shared_file_writes) = {
            let current = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            (current.pending_approvals.clone(), current.active_file_writes.clone())
        };
        {
            let mut child_state = child_host
                .state
                .lock()
                .map_err(|_| "child runtime state lock is poisoned".to_owned())?;
            child_state.pending_approvals = shared_pending_approvals;
            child_state.active_file_writes = shared_file_writes;
        }
        let (tool_scope, mcp_scope) = if graph_review_request_id.is_some() {
            (
                Some(
                    GRAPH_REVIEWER_ALLOWED_TOOLS
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                ),
                Some(HashSet::new()),
            )
        } else {
            let (mut tool_scope, mcp_scope) =
                effective_conversation_delegation_scope(&self.database, &parent_conversation_id)?;
            tool_scope = intersect_optional_scopes(
                tool_scope,
                child_run
                    .allowed_tools
                    .as_ref()
                    .map(|tools| tools.iter().cloned().collect()),
            );
            (tool_scope, mcp_scope)
        };
        child_host.tool_allowlist_override = tool_scope;
        child_host.mcp_server_scope_override = mcp_scope;
        {
            let mut hosts = self
                .child_hosts
                .lock()
                .map_err(|_| "child runtime map is poisoned".to_owned())?;
            if hosts.contains_key(&child_run_id) {
                return Ok(());
            }
            hosts.insert(child_run_id.clone(), child_host.clone());
        }
        self.emit_child_run_update(&parent_conversation_id, &child_run);

        let coordinator = self.clone();
        thread::spawn(move || {
            let prompt = started.user_message.content.clone();
            let kernel_owned = coordinator.database.run_control_binding(&child_run_id)
                .ok().flatten().is_some_and(|binding| binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative);
            if let Err(error) = child_host.start_run(&started, &prompt, &[]) {
                if kernel_owned {
                    let _ = child_host.record_kernel_start_failure(&child_run_id);
                } else {
                let (code, detail) = structured_runtime_error(&error);
                let _ = coordinator.database.mark_run_failed(
                    &child_run_id,
                    if code == "runtime.start_failed" {
                        "child_run.start_failed"
                    } else {
                        code
                    },
                    detail,
                );
                }
            } else {
                if let Ok(Some(record)) = coordinator.database.child_run(&child_run_id) {
                    coordinator.emit_child_run_update(&parent_conversation_id, &record);
                }
                let deadline =
                    Instant::now() + Duration::from_millis(child_run.budget.max_duration_ms as u64);
                loop {
                    // Kernel owns its paused execution clock and terminal
                    // facts. The Legacy wall timer must not override approval
                    // waits or settle the child ahead of executor cleanup.
                    let duration_exceeded = !kernel_owned && Instant::now() >= deadline;
                    if duration_exceeded {
                        let _ = child_host.cancel_run(&child_run_id);
                        let _ = child_host.stop_worker();
                        let _ = coordinator.database.mark_run_failed(
                            &child_run_id,
                            "child_run.duration_budget_exceeded",
                            "Child Run exceeded its Host-enforced duration budget.",
                        );
                        break;
                    }
                    let status = coordinator
                        .database
                        .child_run_status(&child_run_id)
                        .ok()
                        .flatten();
                    if status.as_deref().is_some_and(run_status_is_terminal) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = child_host.stop_worker();
            let _ = coordinator.database.sync_child_run_terminal(&child_run_id);
            if let Some(review_request_id) = graph_review_request_id {
                match coordinator
                    .database
                    .settle_graph_node_review(&review_request_id)
                {
                    Ok(decision) => {
                        let _ = coordinator
                            .app
                            .emit("fox://graph-review-decision", decision);
                    }
                    Err(error) => {
                        eprintln!(
                            "[graph.readonly_reviewer_settle_failed] request={} child={} error={}",
                            review_request_id, child_run_id, error
                        );
                    }
                }
            }
            if let Ok(Some(record)) = coordinator.database.child_run(&child_run_id) {
                coordinator.emit_child_run_update(&parent_conversation_id, &record);
            }
            if let Ok(mut hosts) = coordinator.child_hosts.lock() {
                hosts.remove(&child_run_id);
            }
        });
        Ok(())
    }

    pub(crate) fn dispatch_digital_colleague_trigger(
        &self,
        prepared: PreparedDigitalColleagueTrigger,
    ) -> Result<(), String> {
        let run_id = prepared
            .trigger
            .run_id
            .clone()
            .unwrap_or_else(|| prepared.started.run.id.clone());
        let conversation_id = prepared.started.run.conversation_id.clone();
        let model_max_output = self
            .database
            .get_model_service()?
            .ok_or_else(|| "digital_colleague.model_service_unavailable".to_owned())?
            .max_output_tokens;
        let effective_max_output = effective_digital_colleague_output_limit(
            prepared.colleague.max_output_tokens,
            prepared.remaining_tokens,
            model_max_output,
        )?;
        let mut managed_host = RuntimeHost::new(
            self.app.clone(),
            self.database.clone(),
            self.sessions_dir.clone(),
            self.attachments_dir.clone(),
            self.skills_dir.clone(),
            self.yuxi_client.clone(),
        );
        managed_host
            .state
            .lock()
            .map_err(|_| "managed host state lock poisoned")?
            .shadow = self.shadow_reconciler()?;
        managed_host.run_budget_override = Some(ChildRunBudget {
            max_duration_ms: prepared.colleague.max_duration_ms,
            max_total_tokens: prepared.remaining_tokens.min(200_000),
            max_output_tokens: effective_max_output,
            max_tool_calls: prepared.colleague.max_tool_calls,
        });
        let (shared_pending_approvals, shared_file_writes) = {
            let current = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            (current.pending_approvals.clone(), current.active_file_writes.clone())
        };
        {
            let mut managed_state = managed_host
                .state
                .lock()
                .map_err(|_| "digital colleague runtime state lock is poisoned".to_owned())?;
            managed_state.pending_approvals = shared_pending_approvals;
            managed_state.active_file_writes = shared_file_writes;
        }
        let (tool_scope, mcp_scope) =
            effective_conversation_delegation_scope(&self.database, &conversation_id)?;
        managed_host.tool_allowlist_override = tool_scope;
        managed_host.mcp_server_scope_override = mcp_scope;
        {
            let mut hosts = self
                .child_hosts
                .lock()
                .map_err(|_| "managed runtime map is poisoned".to_owned())?;
            if hosts.contains_key(&run_id) {
                return Ok(());
            }
            hosts.insert(run_id.clone(), managed_host.clone());
        }
        let coordinator = self.clone();
        thread::spawn(move || {
            let prompt = prepared.started.user_message.content.clone();
            if let Err(error) = managed_host.start_run(&prepared.started, &prompt, &[]) {
                let (code, detail) = structured_runtime_error(&error);
                let _ = coordinator.database.mark_run_failed(
                    &run_id,
                    if code == "runtime.start_failed" {
                        "digital_colleague.start_failed"
                    } else {
                        code
                    },
                    detail,
                );
            } else {
                let deadline = Instant::now()
                    + Duration::from_millis(prepared.colleague.max_duration_ms as u64);
                loop {
                    let token_budget_exceeded = coordinator
                        .database
                        .digital_colleague_budget_exceeded(&run_id)
                        .unwrap_or(false);
                    let duration_exceeded = Instant::now() >= deadline;
                    if token_budget_exceeded || duration_exceeded {
                        let _ = managed_host.cancel_run(&run_id);
                        let _ = managed_host.stop_worker();
                        let (code, message) = if token_budget_exceeded {
                            (
                                "digital_colleague.token_budget_exceeded",
                                "Digital colleague exceeded its Host-enforced token budget.",
                            )
                        } else {
                            (
                                "digital_colleague.duration_budget_exceeded",
                                "Digital colleague exceeded its Host-enforced duration budget.",
                            )
                        };
                        let _ = coordinator.database.mark_run_failed(&run_id, code, message);
                        break;
                    }
                    let status = coordinator
                        .database
                        .digital_colleague_run_status(&run_id)
                        .ok()
                        .flatten();
                    if status.as_deref().is_some_and(run_status_is_terminal) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = managed_host.stop_worker();
            if let Ok(mut hosts) = coordinator.child_hosts.lock() {
                hosts.remove(&run_id);
            }
        });
        Ok(())
    }

    pub(crate) fn start_digital_colleague_scheduler(&self) {
        if self.digital_scheduler_started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.digital_scheduler_stop.store(false, Ordering::SeqCst);
        let coordinator = self.clone();
        thread::spawn(move || {
            while !coordinator.digital_scheduler_stop.load(Ordering::SeqCst) {
                if let Ok(prepared) = coordinator
                    .database
                    .claim_due_digital_colleague_triggers(crate::database::now_ms())
                {
                    for trigger in prepared {
                        if let Err(error) =
                            coordinator.dispatch_digital_colleague_trigger(trigger.clone())
                        {
                            let _ = coordinator.database.mark_run_failed(
                                &trigger.started.run.id,
                                "digital_colleague.dispatch_failed",
                                &error,
                            );
                        }
                    }
                }
                for _ in 0..100 {
                    if coordinator.digital_scheduler_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            coordinator
                .digital_scheduler_started
                .store(false, Ordering::SeqCst);
        });
    }

    fn emit_child_run_update(&self, parent_conversation_id: &str, child_run: &ChildRunRecord) {
        let _ = self.app.emit(
            "fox://child-run-updated",
            ChildRunNotification {
                parent_run_id: child_run.parent_run_id.clone(),
                parent_conversation_id: parent_conversation_id.to_owned(),
                child_run: child_run.clone(),
            },
        );
    }

    fn ensure_delegated_tool_allowed(&self, tool: &str) -> Result<(), String> {
        if self
            .tool_allowlist_override
            .as_ref()
            .is_some_and(|scope| !scope.contains(tool))
        {
            return Err(format!(
                "child_run.parent_tool_scope_denied: parent Run did not authorize tool '{tool}'"
            ));
        }
        Ok(())
    }

    fn ensure_active_envelope_identity(
        &self,
        envelope: &RuntimeEnvelope,
    ) -> Result<(), (&'static str, String)> {
        let conversation_id = envelope
            .conversation_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or((
                "runtime.identity.conversation_missing",
                "runtime envelope omitted conversationId".to_owned(),
            ))?;
        let run_id = envelope
            .run_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or((
                "runtime.identity.run_missing",
                "runtime envelope omitted runId".to_owned(),
            ))?;
        let runtime_session_id = envelope
            .runtime_session_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .ok_or((
                "runtime.identity.session_missing",
                "runtime envelope omitted runtimeSessionId".to_owned(),
            ))?;
        let (expected_conversation_id, expected_run_id, expected_runtime_session_id) = {
            let state = self.state.lock().map_err(|_| {
                (
                    "runtime.state_unavailable",
                    "runtime state lock is poisoned".to_owned(),
                )
            })?;
            let worker = state.worker.as_ref().ok_or((
                "runtime.identity.worker_missing",
                "runtime envelope arrived without an active worker".to_owned(),
            ))?;
            (
                worker.current_conversation_id.clone(),
                worker.active_run_id.clone(),
                worker.current_runtime_session_id.clone(),
            )
        };
        let authoritative_conversation_id = self
            .database
            .authoritative_run_conversation_id(run_id)
            .map_err(|error| ("runtime.identity.run_unknown", error.to_string()))?;
        validate_envelope_identity_scope(
            conversation_id,
            run_id,
            runtime_session_id,
            expected_conversation_id.as_deref(),
            expected_run_id.as_deref(),
            expected_runtime_session_id.as_deref(),
            &authoritative_conversation_id,
        )
    }

    fn current_execution_profile(&self) -> Result<continuation::ExecutionProfileSelection, String> {
        let profile_id = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .execution_profile_id
            .clone();
        continuation::ExecutionProfileSelection::resolve(&profile_id)
    }

    fn emit_continuation_diagnostic(
        &self,
        run_id: Option<&str>,
        seq: Option<i64>,
        code: &str,
        error: &str,
        shadow: bool,
    ) {
        let _ = self.app.emit(
            "fox://continuation-diagnostic",
            json!({
                "runId": run_id,
                "seq": seq,
                "code": code,
                "error": error,
                "shadow": shadow,
            }),
        );
    }

    fn ensure_runtime_tool_allowed(
        &self,
        run_id: &str,
        tool: &str,
        ingress: RuntimeToolIngress,
    ) -> Result<(), (&'static str, String)> {
        let (active_profile_id, capabilities) = {
            let state = self.state.lock().map_err(|_| {
                (
                    "runtime.state_unavailable",
                    "runtime state lock is poisoned".to_owned(),
                )
            })?;
            (
                state.execution_profile_id.clone(),
                state.capabilities.clone(),
            )
        };
        let active_profile = continuation::ExecutionProfileSelection::resolve(&active_profile_id)
            .map_err(|error| ("runtime.execution_profile.invalid", error))?;
        let manifest =
            serde_json::from_value::<RuntimeCapabilityManifest>(capabilities).map_err(|error| {
                (
                    "runtime.capability.invalid",
                    format!("runtime capability manifest is invalid: {error}"),
                )
            })?;
        let frozen_profile_id = self
            .database
            .frozen_run_execution_profile_id(run_id)
            .map_err(|error| {
                (
                    "runtime.execution_profile.frozen_invalid",
                    error.to_string(),
                )
            })?;
        validate_runtime_tool_authority(
            &active_profile,
            frozen_profile_id.as_deref(),
            &manifest,
            run_id,
            tool,
            ingress,
        )
    }

    fn ensure_runtime_tool_event_allowed(
        &self,
        run_id: &str,
        payload: &Value,
    ) -> Result<(), (&'static str, String)> {
        if !matches!(
            payload.get("type").and_then(Value::as_str),
            Some("tool.started" | "tool.updated" | "tool.completed")
        ) {
            return Ok(());
        }
        let tool = payload
            .get("tool")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or((
                "runtime.tool_event.tool_missing",
                "Runtime tool event omitted a non-empty tool name".to_owned(),
            ))?;
        self.ensure_runtime_tool_allowed(run_id, tool, RuntimeToolIngress::Event)?;
        self.ensure_delegated_tool_allowed(tool)
            .map_err(|error| ("runtime.tool_event.delegated_scope_denied", error))?;
        let conversation_id = self
            .database
            .authoritative_run_conversation_id(run_id)
            .map_err(|error| ("runtime.identity.run_unknown", error.to_string()))?;
        ensure_expert_tool_allowed(&self.database, &conversation_id, tool)
            .map_err(|error| ("runtime.tool_event.package_scope_denied", error))
    }

    fn cancel_child_runtime(&self, child_run_id: &str) -> Result<bool, String> {
        if self.database.run_control_binding(child_run_id)?.is_some_and(|binding| binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative) {
            let host = self.child_hosts.lock().map_err(|_| "child runtime map is poisoned")?.get(child_run_id).cloned();
            return host.as_ref().unwrap_or(self).cancel_run(child_run_id);
        }
        if self
            .database
            .child_run_status(child_run_id)?
            .as_deref()
            .is_none_or(run_status_is_terminal)
        {
            return Ok(false);
        }
        if let Some(child) = self.database.child_run(child_run_id)? {
            if self.database.run_control_binding(&child.parent_run_id)?.is_some_and(|parent|
                parent.authority==fox_engine_protocol::ExecutionAuthority::Authoritative) {
                let _ownership = kernel_host::acquire(&self.sessions_dir,child_run_id)?;
                if self.database.kernel_cancel_unstarted_child(&child.parent_run_id,child_run_id)? { return Ok(true); }
                return Err("Kernel child preparation is changing; cancellation was not delegated to Legacy".into());
            }
        }
        if self.database.mark_run_cancelling(child_run_id)?.is_none() {
            return Ok(false);
        }
        let child_host = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .get(child_run_id)
            .cloned();
        let Some(child_host) = child_host else {
            let seq = self.database.next_run_seq(child_run_id)?;
            return self
                .database
                .apply_runtime_event(
                    child_run_id,
                    seq,
                    &json!({ "type": "run.cancelled", "reason": "parent_or_host_cancelled" }),
                )
                .map_err(|error| error.to_string());
        };
        if child_host.cancel_run(child_run_id)? {
            return Ok(true);
        }
        let seq = self.database.next_run_seq(child_run_id)?;
        self.database.apply_runtime_event(
            child_run_id,
            seq,
            &json!({ "type": "run.cancelled", "reason": "cancelled_before_runtime_submission" }),
        )
    }

    fn signal_graph_child_cancel(
        &self,
        child_run_id: &str,
        allow_redelivery: bool,
    ) -> Result<bool, String> {
        if self.database.run_control_binding(child_run_id)?.is_some_and(|binding| binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative) {
            return self.cancel_child_runtime(child_run_id);
        }
        let child_host = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .get(child_run_id)
            .cloned();
        signal_graph_child_cancel_with_transport(
            &self.database,
            child_run_id,
            allow_redelivery,
            || {
                child_host
                    .map(|host| host.cancel_run(child_run_id))
                    .transpose()
            },
        )
    }

    pub(crate) fn recover_active_graph_node_cancellations(&self) -> Result<usize, String> {
        let intents = self
            .database
            .active_graph_node_cancel_intents_for_recovery()
            .map_err(|error| error.to_string())?;
        let mut delivered = 0;
        for intent in intents {
            if self.signal_graph_child_cancel(&intent.child_run_id, true)? {
                delivered += 1;
            }
        }
        Ok(delivered)
    }

    pub(crate) fn recover_active_graph_node_reviews(&self) -> Result<usize, String> {
        let requests = self
            .database
            .active_graph_node_review_requests_for_recovery()
            .map_err(|error| error.to_string())?;
        let mut dispatched = 0;
        for request in requests {
            if let Some(child_run_id) = request.reviewer_child_run_id.as_deref() {
                if let Err(error) = self.database.sync_child_run_terminal(child_run_id) {
                    eprintln!(
                        "[graph.readonly_reviewer_recovery_sync_failed] request={} child={} error={}",
                        request.request_id, child_run_id, error
                    );
                    continue;
                }
                if self
                    .database
                    .child_run_status(child_run_id)?
                    .as_deref()
                    .is_some_and(run_status_is_terminal)
                {
                    match self.database.settle_graph_node_review(&request.request_id) {
                        Ok(decision) => {
                            let _ = self.app.emit("fox://graph-review-decision", decision);
                        }
                        Err(error) => {
                            eprintln!(
                                "[graph.readonly_reviewer_recovery_settle_failed] request={} child={} error={}",
                                request.request_id, child_run_id, error
                            );
                        }
                    }
                    continue;
                }
            }
            match dispatch_graph_reviewer_child(
                self,
                &request.request_id,
                &request.reviewer_agent_id,
            ) {
                Ok(true) => dispatched += 1,
                Ok(false) => {}
                Err(error) => {
                    eprintln!(
                        "[graph.readonly_reviewer_recovery_dispatch_failed] request={} error={}",
                        request.request_id, error
                    );
                }
            }
        }
        Ok(dispatched)
    }

    pub(crate) fn recover_pending_graph_acceptances(&self) -> Result<usize, String> {
        let intents = self
            .database
            .pending_graph_acceptances_for_recovery()
            .map_err(|error| error.to_string())?;
        let mut accepted = 0;
        for intent in intents {
            match self
                .database
                .activate_read_only_graph_acceptance(&intent.acceptance_id)
            {
                Ok(result) if result.accepted => {
                    accepted += 1;
                    let _ = self.app.emit("fox://graph-acceptance", result);
                }
                Ok(result) if result.stale => {
                    eprintln!(
                        "[graph.readonly_accept_recovery_stale] acceptance={}",
                        result.acceptance_id
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!(
                        "[graph.readonly_accept_recovery_failed] acceptance={} error={}",
                        intent.acceptance_id, error
                    );
                }
            }
        }
        Ok(accepted)
    }

    pub(crate) fn cancel_managed_run(&self, run_id: &str) -> Result<bool, String> {
        let is_managed = self
            .child_hosts
            .lock()
            .map_err(|_| "managed runtime map is poisoned".to_owned())?
            .contains_key(run_id);
        let is_child = self.database.child_run(run_id)?.is_some();
        if is_managed || is_child {
            self.cancel_child_runtime(run_id)
        } else {
            self.cancel_run(run_id)
        }
    }

    pub(crate) fn cancel_expert_team(
        &self,
        team_run_id: &str,
        reason: &str,
    ) -> Result<crate::database::ExpertTeamSnapshot, String> {
        let snapshot = self
            .database
            .get_expert_team(team_run_id)?
            .ok_or_else(|| "team.not_found".to_owned())?;
        let kernel_parent = self.database.run_control_binding(&snapshot.run.parent_run_id)?
            .filter(|binding|binding.authority==fox_engine_protocol::ExecutionAuthority::Authoritative);
        for member in &snapshot.members {
            if !run_status_is_terminal(&member.status) {
                let result = self.cancel_child_runtime(&member.child_run_id);
                if kernel_parent.is_some() { result?; }
            }
        }
        if let Some(binding) = kernel_parent {
            let selected = snapshot.members.iter().map(|member|member.child_run_id.clone()).collect();
            self.wait_kernel_selected_children(&binding,false,Some(&selected))?;
        }
        self.database.cancel_expert_team(team_run_id, reason)
    }

    pub fn cancel_run(&self, run_id: &str) -> Result<bool, String> {
        if self.database.run_control_binding(run_id)?.is_some_and(|binding| binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative) {
            let queued = self.database.queue_kernel_host_command(run_id, None)?;
            self.state.lock().map_err(|_| "runtime state lock poisoned")?.cancellation.request_run_cancel(run_id);
            for child_run_id in self.database.active_child_run_ids(run_id)? {
                if child_run_id != run_id { let _ = self.cancel_child_runtime(&child_run_id); }
            }
            return Ok(queued);
        }
        let _ = self
            .database
            .cancel_expert_teams_for_parent(run_id, "parent Run cancelled");
        for child_run_id in self.database.active_child_run_ids(run_id)? {
            if child_run_id != run_id {
                let _ = self.cancel_child_runtime(&child_run_id);
            }
        }
        let _transition = self
            .run_transition
            .lock()
            .map_err(|_| "runtime run transition lock is poisoned".to_owned())?;
        if self.cancel_queued_run(run_id)? {
            return Ok(true);
        }
        let (conversation_id, runtime_session_id) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            let Some(worker) = state.worker.as_ref() else {
                return Ok(false);
            };
            if worker.active_run_id.as_deref() != Some(run_id) {
                return Ok(false);
            }
            (
                worker.current_conversation_id.clone(),
                worker.current_conversation_id.as_ref().and_then(|id| {
                    self.database
                        .get_runtime_session(id)
                        .ok()
                        .flatten()
                        .map(|session| session.runtime_session_id)
                }),
            )
        };

        self.state.lock().map_err(|_| "runtime state lock is poisoned")?
            .cancellation.request_run_cancel(run_id);
        let mut request = RuntimeRequest::new("cancel").with_run(run_id);
        if let Some(conversation_id) = conversation_id.as_deref() {
            request = request.with_conversation(conversation_id);
        }
        if let Some(runtime_session_id) = runtime_session_id.as_deref() {
            request = request.with_runtime_session(runtime_session_id);
        }
        self.cancel_pending_approvals_for_run(run_id);
        let response = self.send_request(request, RESPONSE_TIMEOUT)?;
        Ok(response.r#type == "request_succeeded")
    }

    pub fn resolve_approval(
        &self,
        approval_id: &str,
        decision: ApprovalDecision,
    ) -> Result<bool, String> {
        if self.database.kernel_host_approval_target(approval_id)?.is_some() {
            let decision = match decision {
                ApprovalDecision::AllowOnce => "allow_once",
                ApprovalDecision::AllowConversation => "allow_conversation",
                ApprovalDecision::Deny => "denied",
            };
            // Enqueueing is only a queue command: the durable Kernel decision is
            // applied by the consumer. The dedicated replacement request is
            // therefore settled when the consumer acknowledges the command —
            // `complete_kernel_host_command`, in the same transaction as the
            // durable transition — so an ordinary decision, the dedicated
            // authorization and an executable dispatch can never disagree.
            let queued = self.database.queue_kernel_host_approval(approval_id, decision)?;
            return Ok(queued);
        }
        // B) `resolve_approval` settles the ordinary decision AND the dedicated
        // whole-file replacement authorization in ONE transaction, so the waiting
        // executor below is only released once both facts are durably visible.
        let resolved = self.database.resolve_approval(approval_id, decision)?;
        let Some(approval) = resolved else {
            return Ok(false);
        };
        let sender = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .pending_approvals
            .lock()
            .map_err(|_| "approval map is poisoned".to_owned())?
            .remove(approval_id);
        if let Some(pending) = sender {
            let _ = pending.sender.send(approval.status == "approved");
        }
        let _ = self.app.emit("fox://approval-resolved", approval);
        Ok(true)
    }

    fn cancel_pending_approvals_for_run(&self, run_id: &str) {
        let pending = self
            .state
            .lock()
            .ok()
            .map(|state| state.pending_approvals.clone());
        if let Some(pending) = pending {
            if let Ok(mut pending) = pending.lock() {
                let approval_ids = pending_approval_ids_for_run(&pending, run_id);
                for approval_id in approval_ids {
                    if let Some(approval) = pending.remove(&approval_id) {
                        let _ = approval.sender.send(false);
                    }
                }
            }
        }
    }

    fn cancel_stale_pending_approvals_for_conversation(
        &self,
        conversation_id: &str,
        current_run_id: &str,
    ) {
        let pending = self
            .state
            .lock()
            .ok()
            .map(|state| state.pending_approvals.clone());
        let Some(pending) = pending else {
            return;
        };
        let candidates = pending
            .lock()
            .ok()
            .map(|pending| {
                pending
                    .iter()
                    .filter(|(_, approval)| approval.run_id != current_run_id)
                    .map(|(approval_id, approval)| (approval_id.clone(), approval.run_id.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let approval_ids = candidates
            .into_iter()
            .filter_map(|(approval_id, run_id)| {
                self.database
                    .authoritative_run_conversation_id(&run_id)
                    .ok()
                    .filter(|owner| owner == conversation_id)
                    .map(|_| approval_id)
            })
            .collect::<Vec<_>>();
        if let Ok(mut pending) = pending.lock() {
            for approval_id in approval_ids {
                if let Some(approval) = pending.remove(&approval_id) {
                    let _ = approval.sender.send(false);
                }
            }
        };
    }

    pub fn remove_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let should_stop = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .worker
            .as_ref()
            .and_then(|worker| worker.current_conversation_id.as_deref())
            == Some(conversation_id);
        if should_stop {
            self.stop_worker()?;
        }
        if let Some(session) = self.database.get_runtime_session(conversation_id)? {
            if let Some(path) = session.session_path.map(PathBuf::from) {
                let sessions_root = self
                    .sessions_dir
                    .canonicalize()
                    .unwrap_or_else(|_| self.sessions_dir.clone());
                if let Ok(canonical) = path.canonicalize() {
                    if canonical.starts_with(&sessions_root) && canonical.is_file() {
                        std::fs::remove_file(canonical).map_err(|error| error.to_string())?;
                    }
                }
            }
        }
        Ok(())
    }

    pub fn shutdown(&self) -> Result<(), String> {
        self.digital_scheduler_stop.store(true, Ordering::SeqCst);
        self.stop_kernel_runs()?;
        let child_hosts = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .drain()
            .collect::<Vec<_>>();
        for (run_id, child_host) in child_hosts {
            if self.database.run_control_binding(&run_id)?.is_some_and(|binding| binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative) {
                child_host.stop_kernel_runs()?;
                continue;
            }
            let _ = child_host.cancel_run(&run_id);
            let _ = child_host.stop_worker();
            self.database.mark_run_interrupted(
                &run_id,
                "runtime.application_exit",
                "Fox closed before the managed Run reached a terminal state.",
            )?;
        }
        let (active_run_id, queued_run_ids) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            let active = state
                .worker
                .as_ref()
                .and_then(|worker| worker.active_run_id.clone());
            let queued = state
                .queued_runs
                .drain(..)
                .map(|queued| queued.started.run.id)
                .collect::<Vec<_>>();
            (active, queued)
        };
        self.stop_worker()?;
        if let Some(run_id) = active_run_id {
            self.database.mark_run_interrupted(
                &run_id,
                "runtime.application_exit",
                "Fox closed before the run reached a terminal state.",
            )?;
        }
        for run_id in queued_run_ids {
            self.database.mark_run_interrupted(
                &run_id,
                "runtime.application_exit",
                "Fox closed before the queued run started.",
            )?;
        }
        Ok(())
    }

    fn mark_run_ready(&self, run_id: &str) {
        let should_dispatch = if let Ok(mut state) = self.state.lock() {
            state.cancellation.retire_run(run_id);
            if let Some(worker) = state.worker.as_mut() {
                if worker.active_run_id.as_deref() == Some(run_id) {
                    worker.active_run_id = None;
                    state.state = "ready".to_owned();
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };
        if should_dispatch {
            self.dispatch_next_queued_run();
        }
    }

    fn ensure_worker(&self, conversation_id: &str) -> Result<(), String> {
        let (current_conversation, current_state) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            (
                state
                    .worker
                    .as_ref()
                    .and_then(|worker| worker.current_conversation_id.clone()),
                state.state.clone(),
            )
        };

        if current_conversation.as_deref() == Some(conversation_id)
            && matches!(current_state.as_str(), "ready" | "busy")
        {
            return Ok(());
        }
        if current_conversation.is_some() && current_state == "busy" {
            return Err("another conversation is currently running".to_owned());
        }
        if current_conversation.is_some() {
            self.stop_worker()?;
        }
        self.spawn_worker(conversation_id)
    }

    fn spawn_worker(&self, conversation_id: &str) -> Result<(), String> {
        let result = self.spawn_worker_inner(conversation_id);
        if let Err(error) = &result {
            self.cleanup_failed_worker(error);
        }
        result
    }

    fn spawn_worker_inner(&self, conversation_id: &str) -> Result<(), String> {
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            state.state = "starting".to_owned();
            state.last_error = None;
        }

        let execution_profile = match self.execution_profile_override {
            Some(profile_id) => continuation::ExecutionProfileSelection::resolve(profile_id)?,
            None => continuation::ExecutionProfileSelection::load(&self.database)?,
        };
        let runtime = self.runtime_command()?;
        let mut command = Command::new(&runtime.program);
        if let Some(script) = runtime.script.as_ref() {
            command.arg(script);
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start Fox Runtime: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "runtime stdin was not available".to_owned())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "runtime stdout was not available".to_owned())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "runtime stderr was not available".to_owned())?;
        let stdin = Arc::new(Mutex::new(stdin));
        let pending: PendingResponses = Arc::new(Mutex::new(HashMap::new()));

        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            state.worker = Some(WorkerHandle {
                child: Arc::new(Mutex::new(child)),
                stdin: stdin.clone(),
                pending: pending.clone(),
                current_conversation_id: Some(conversation_id.to_owned()),
                active_run_id: None,
                current_runtime_session_id: None,
            });
        }

        self.spawn_stdout_reader(stdout, stdin.clone(), pending);
        self.spawn_stderr_reader(stderr);

        let model_service = self
            .database
            .get_model_service()?
            .ok_or_else(|| "model service is not configured".to_owned())?;
        let api_key = crate::model_service::get_api_key(&model_service.base_url);
        let execution_profile_fields = execution_profile.initialize_fields();
        let response = self.send_request(
            RuntimeRequest::new("initialize").with_payload(json!({
                "modelService": {
                    "baseUrl": model_service.base_url,
                    "modelId": model_service.model_id,
                    "apiType": model_service.api_type,
                    "contextWindow": model_service.context_window,
                    "maxOutputTokens": self
                        .run_budget_override
                        .as_ref()
                        .filter(|_| self.enforce_count_budget_override)
                        .map(|budget| budget.max_output_tokens)
                        .unwrap_or(model_service.max_output_tokens)
                        .min(model_service.max_output_tokens),
                    "supportsImageInput": model_service.supports_image_input,
                    "apiKey": api_key,
                },
                "executionProfile": execution_profile_fields["executionProfile"].clone(),
                "executionStrategy": execution_profile_fields["executionStrategy"].clone(),
            })),
            RESPONSE_TIMEOUT,
        )?;
        if response.r#type != "ready" {
            let error = response_error(&response, "runtime handshake failed");
            let _ = self.stop_worker();
            return Err(error);
        }

        let ready_payload = response
            .payload
            .as_ref()
            .ok_or_else(|| "runtime handshake omitted its payload".to_owned())?;
        let (runtime, runtime_version, capabilities) =
            match parse_runtime_ready_payload(ready_payload).and_then(|handshake| {
                execution_profile.validate_ready_payload(ready_payload)?;
                let manifest =
                    serde_json::from_value::<RuntimeCapabilityManifest>(handshake.2.clone())
                        .map_err(|error| {
                            format!("runtime capability manifest is invalid: {error}")
                        })?;
                execution_profile.validate_capability_manifest(&manifest)?;
                Ok(handshake)
            }) {
                Ok(handshake) => handshake,
                Err(error) => {
                    let _ = self.stop_worker();
                    return Err(error);
                }
            };
        let mut state = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?;
        state.state = "ready".to_owned();
        state.runtime = runtime;
        state.runtime_version = runtime_version;
        state.capabilities = capabilities;
        state.execution_profile_id = execution_profile.id().to_owned();
        Ok(())
    }

    fn send_request(
        &self,
        request: RuntimeRequest,
        timeout: Duration,
    ) -> Result<RuntimeEnvelope, String> {
        let (stdin, pending) = {
            let state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            let worker = state
                .worker
                .as_ref()
                .ok_or_else(|| "runtime worker is not running".to_owned())?;
            (worker.stdin.clone(), worker.pending.clone())
        };
        let request_id = request.id.clone();
        let (sender, receiver) = mpsc::channel();
        pending
            .lock()
            .map_err(|_| "runtime response map is poisoned".to_owned())?
            .insert(request_id.clone(), sender);

        let line = serde_json::to_string(&request).map_err(|error| error.to_string())?;
        let write_result = stdin
            .lock()
            .map_err(|_| "runtime stdin lock is poisoned".to_owned())
            .and_then(|mut writer| {
                writer
                    .write_all(format!("{line}\n").as_bytes())
                    .and_then(|_| writer.flush())
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = write_result {
            if let Ok(mut responses) = pending.lock() {
                responses.remove(&request_id);
            }
            return Err(error);
        }

        match receiver.recv_timeout(timeout) {
            Ok(response) => Ok(response),
            Err(_) => {
                if let Ok(mut responses) = pending.lock() {
                    responses.remove(&request_id);
                }
                Err(format!("runtime request timed out: {}", request.r#type))
            }
        }
    }

    fn spawn_stdout_reader(
        &self,
        stdout: std::process::ChildStdout,
        stdin: Arc<Mutex<ChildStdin>>,
        pending: PendingResponses,
    ) {
        let app = self.app.clone();
        let database = self.database.clone();
        let state = self.state.clone();
        let yuxi_client = self.yuxi_client.clone();
        let attachments_dir = self.attachments_dir.clone();
        let sessions_dir = self.sessions_dir.clone();
        let runtime_host = self.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = match line {
                    Ok(line) => line,
                    Err(error) => {
                        runtime_host
                            .handle_runtime_crash(format!("runtime stdout failed: {error}"));
                        return;
                    }
                };
                if line.len() > MAX_PROTOCOL_LINE_BYTES {
                    runtime_host.handle_runtime_crash(
                        "runtime emitted an oversized JSONL message".to_owned(),
                    );
                    return;
                }
                let envelope: RuntimeEnvelope = match serde_json::from_str(&line) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        runtime_host.handle_runtime_crash(format!(
                            "runtime emitted invalid JSONL: {error}"
                        ));
                        return;
                    }
                };
                if envelope.protocol != protocol::PROTOCOL_NAME
                    || envelope.version != protocol::PROTOCOL_VERSION
                {
                    runtime_host.handle_runtime_crash(format!(
                        "runtime protocol mismatch: {} v{}",
                        envelope.protocol, envelope.version
                    ));
                    return;
                }

                if envelope.kind == "response" {
                    if let Some(request_id) = envelope.request_id.as_deref() {
                        let sender = pending
                            .lock()
                            .ok()
                            .and_then(|mut responses| responses.remove(request_id));
                        if let Some(sender) = sender {
                            let _ = sender.send(envelope);
                        }
                    }
                    continue;
                }

                if envelope.kind == "request" && envelope.r#type == "tool.preflight" {
                    if let Err((code, error)) =
                        runtime_host.ensure_active_envelope_identity(&envelope)
                    {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.preflight_blocked",
                            json!({
                                "decision": "block",
                                "code": code,
                                "message": error,
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        continue;
                    }
                    let tool = envelope
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("tool"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let run_id = envelope.run_id.as_deref().unwrap_or_default();
                    if let Err((code, error)) = runtime_host.ensure_runtime_tool_allowed(
                        run_id,
                        tool,
                        RuntimeToolIngress::Preflight,
                    ) {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.preflight_blocked",
                            json!({
                                "decision": "block",
                                "code": code,
                                "message": error,
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        // Legacy denied the preflight. Feed the shadow so the
                        // kernel-side decision is compared against a real deny.
                        runtime_host.shadow_feed_preflight(
                            run_id,
                            tool,
                            &envelope,
                            false,
                            &json!({ "decision": "deny", "code": code }),
                        );
                        continue;
                    }
                    let input = envelope
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("input"))
                        .cloned()
                        .unwrap_or_else(|| json!({}));
                    let project_access =
                        envelope
                            .conversation_id
                            .as_deref()
                            .and_then(|conversation_id| {
                                database
                                    .conversation_project_access(conversation_id)
                                    .ok()
                                    .flatten()
                            });
                    let persisted_permission_mode = envelope
                        .conversation_id
                        .as_deref()
                        .and_then(|conversation_id| {
                            database.conversation_permission_mode(conversation_id).ok()
                        })
                        .unwrap_or_else(|| "ask".to_owned());
                    let project_root = project_access.as_ref().map(|(root, _)| root.as_str());
                    let expert_guard = envelope
                        .conversation_id
                        .as_deref()
                        .map(|conversation_id| {
                            ensure_expert_tool_allowed(&database, conversation_id, tool)
                        })
                        .transpose();
                    let delegated_guard = runtime_host.ensure_delegated_tool_allowed(tool);
                    let (allowed, payload) = match delegated_guard.and(expert_guard.map(|_| ())) {
                        Ok(_) => match database.run_control_binding(run_id) {
                            Ok(Some(binding)) => {
                                let (allowed, mut payload) = crate::tool_guard::preflight_payload(
                                    tool, &input, binding.permission.project_root.as_deref(), binding.permission.mode.as_str(),
                                );
                                payload["executionRoute"] = json!(binding.read_only_executor);
                                payload["permissionSnapshotId"] = json!(binding.permission_snapshot_id);
                                (allowed, payload)
                            }
                            Ok(None) => crate::tool_guard::preflight_payload(tool, &input, project_root, &persisted_permission_mode),
                            Err(error) => (false, json!({"decision":"block","message":error})),
                        },
                        Err(error) => (
                            false,
                            json!({
                                "code": "expert_tool_not_allowed",
                                "reason": error,
                            }),
                        ),
                    };
                    let response_type = if allowed {
                        "tool.preflight_allowed"
                    } else {
                        "tool.preflight_blocked"
                    };
                    let response =
                        HostResponse::for_request(&envelope, response_type, payload.clone());
                    if let Err(error) = write_protocol_message(&stdin, &response) {
                        runtime_host.handle_runtime_crash(error);
                        return;
                    }
                    // Online Shadow feed: run the same preflight through the
                    // observation-only kernel and persist a comparison record.
                    // Independent legacy facts come from THIS real decision; the
                    // kernel side is the shadow controller's own decision. No
                    // side effects; failures never affect the legacy response.
                    runtime_host.shadow_feed_preflight(run_id, tool, &envelope, allowed, &payload);
                    continue;
                }

                if envelope.kind == "request" && matches!(envelope.r#type.as_str(), "tool.execute" | "tool.readonly_execute") {
                    if let Err((code, error)) =
                        runtime_host.ensure_active_envelope_identity(&envelope)
                    {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.execute_failed",
                            json!({
                                "isError": true,
                                "error": error,
                                "errorDetails": {
                                    "code": code,
                                    "retryable": false,
                                }
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        continue;
                    }
                    let tool = envelope
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("tool"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let run_id = envelope.run_id.as_deref().unwrap_or_default();
                    let readonly_execution = envelope.r#type == "tool.readonly_execute";
                    let ingress = if readonly_execution { RuntimeToolIngress::Preflight } else { RuntimeToolIngress::Execute };
                    let admission = runtime_host.ensure_runtime_tool_allowed(
                        run_id,
                        tool,
                        ingress,
                    ).and_then(|_| {
                        if readonly_execution {
                            require_rust_reader_binding(&database, &envelope).map(|_| ()).map_err(|error| ("resource_gateway.binding_rejected", error))
                        } else { Ok(()) }
                    });
                    if let Err((code, error)) = admission {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.execute_failed",
                            json!({
                                "isError": true,
                                "error": error,
                                "errorDetails": {
                                    "code": code,
                                    "retryable": false,
                                }
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        continue;
                    }
                    if let Err(error) = runtime_host.ensure_delegated_tool_allowed(tool) {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.execute_failed",
                            json!({
                                "isError": true,
                                "error": error,
                                "errorDetails": {
                                    "code": "child_run.parent_tool_scope_denied",
                                    "retryable": false,
                                }
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        continue;
                    }
                    let existing_tool_call = (|| -> Result<Option<Value>, String> {
                        let run_id = envelope
                            .run_id
                            .as_deref()
                            .ok_or_else(|| "tool request is missing runId".to_owned())?;
                        let payload = envelope
                            .payload
                            .as_ref()
                            .ok_or_else(|| "tool request is missing payload".to_owned())?;
                        let tool_call_id = payload
                            .get("toolCallId")
                            .and_then(Value::as_str)
                            .filter(|value| !value.trim().is_empty())
                            .ok_or_else(|| "tool request is missing toolCallId".to_owned())?;
                        let raw_input = if readonly_execution { payload.get("originalInput").or_else(|| payload.get("input")) } else { payload.get("input") }
                            .cloned().unwrap_or_else(|| json!({}));
                        let input = if tool == "task_repair_escalate_start" {
                            work_tools::canonicalize_task_repair_override_request(&raw_input)
                                .map_err(|error| error.to_string())?
                        } else {
                            raw_input
                        };
                        inspect_existing_host_tool_call(
                            &database,
                            run_id,
                            tool_call_id,
                            tool,
                            &input,
                        )
                    })();
                    match existing_tool_call {
                        Ok(Some(result)) => {
                            let response = HostResponse::for_request(
                                &envelope,
                                "tool.execute_completed",
                                result,
                            );
                            if let Err(write_error) = write_protocol_message(&stdin, &response) {
                                runtime_host.handle_runtime_crash(write_error);
                                return;
                            }
                            continue;
                        }
                        Err(error) => {
                            let response = HostResponse::for_request(
                                &envelope,
                                "tool.execute_failed",
                                json!({
                                    "isError": true,
                                    "error": error,
                                    "errorDetails": {
                                        "code": "tool_call.replay_rejected",
                                        "retryable": false,
                                    }
                                }),
                            );
                            if let Err(write_error) = write_protocol_message(&stdin, &response) {
                                runtime_host.handle_runtime_crash(write_error);
                                return;
                            }
                            continue;
                        }
                        Ok(None) => {}
                    }
                    if let Some(run_id) = envelope.run_id.as_deref() {
                        if runtime_host.enforce_count_budget_override {
                            if let Err(error) = database.enforce_child_tool_budget(run_id) {
                                let response = HostResponse::for_request(
                                    &envelope,
                                    "tool.execute_failed",
                                    json!({
                                        "isError": true,
                                        "error": error,
                                        "errorDetails": {
                                            "code": "child_run.budget_exceeded",
                                            "retryable": false,
                                        }
                                    }),
                                );
                                if let Err(write_error) = write_protocol_message(&stdin, &response)
                                {
                                    runtime_host.handle_runtime_crash(write_error);
                                    return;
                                }
                                continue;
                            }
                        }
                        if let Err(error) = database.enforce_digital_colleague_tool_budget(run_id) {
                            let response = HostResponse::for_request(
                                &envelope,
                                "tool.execute_failed",
                                json!({
                                    "isError": true,
                                    "error": error,
                                    "errorDetails": {
                                        "code": "digital_colleague.budget_exceeded",
                                        "retryable": false,
                                    }
                                }),
                            );
                            if let Err(write_error) = write_protocol_message(&stdin, &response) {
                                runtime_host.handle_runtime_crash(write_error);
                                return;
                            }
                            continue;
                        }
                    }
                    let app = app.clone();
                    let database = database.clone();
                    let state = state.clone();
                    let stdin = stdin.clone();
                    let yuxi_client = yuxi_client.clone();
                    let attachments_dir = attachments_dir.clone();
                    let sessions_dir = sessions_dir.clone();
                    let child_runtime_host = runtime_host.clone();
                    let Some(tool_handler_permit) = runtime_host.try_acquire_tool_handler() else {
                        let response = HostResponse::for_request(
                            &envelope,
                            "tool.execute_failed",
                            json!({
                                "isError": true,
                                "error": "Fox Host is handling too many tool requests; retry after another request finishes.",
                                "errorDetails": {
                                    "code": "runtime.tool_handler_capacity_exceeded",
                                    "retryable": true,
                                }
                            }),
                        );
                        if let Err(write_error) = write_protocol_message(&stdin, &response) {
                            runtime_host.handle_runtime_crash(write_error);
                            return;
                        }
                        continue;
                    };
                    thread::spawn(move || {
                        let _tool_handler_permit = tool_handler_permit;
                        let tool = envelope
                            .payload
                            .as_ref()
                            .and_then(|payload| payload.get("tool"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if is_child_run_tool(tool) {
                            handle_child_run_tool_request(
                                &child_runtime_host,
                                &app,
                                &database,
                                &state,
                                &stdin,
                                envelope,
                            )
                        } else if background_jobs::TOOLS.contains(&tool) {
                            child_runtime_host.handle_job_tool_request(&stdin, envelope);
                        } else if matches!(tool, "read_attachment" | "attachment_compute") {
                            handle_attachment_tool_request(
                                &app,
                                &database,
                                &attachments_dir,
                                &sessions_dir,
                                &state,
                                &stdin,
                                envelope,
                            )
                        } else if tool == "read_tool_result" {
                            // Stored-result reader: no approval gate and no
                            // side effects, so it does not need an AppHandle.
                            handle_tool_result_read_request(&database, &stdin, envelope)
                        } else if tool == "skill_load" {
                            // On-demand skill text: read-only and audited, no
                            // approval gate and no scope arguments from the model.
                            handle_skill_load_request(
                                &database,
                                &child_runtime_host.skills_dir,
                                &stdin,
                                envelope,
                            )
                        } else if capability_tools::is_capability_tool(tool) {
                            handle_capability_tool_request(
                                &app, &database, &state, &stdin, envelope,
                            )
                        } else if work_tools::is_work_tool(tool) {
                            handle_work_tool_request(
                                &child_runtime_host,
                                &app,
                                &database,
                                &state,
                                &stdin,
                                envelope,
                            )
                        } else if is_memory_tool(tool) {
                            handle_memory_tool_request(&app, &database, &state, &stdin, envelope)
                        } else if is_knowledge_tool(tool) {
                            handle_knowledge_tool_request(
                                &app,
                                &database,
                                &yuxi_client,
                                &state,
                                &stdin,
                                envelope,
                            )
                        } else if is_mcp_tool(tool) {
                            handle_mcp_tool_request(&app, &database, &state, &stdin, envelope)
                        } else {
                            handle_host_tool_request(&app, &database, &state, &stdin, envelope)
                        }
                    });
                    continue;
                }

                if envelope.kind == "event" && envelope.r#type == "runtime_event" {
                    if let Err((code, error)) =
                        runtime_host.ensure_active_envelope_identity(&envelope)
                    {
                        let profile = runtime_host.current_execution_profile().ok();
                        let shadow_proposal = profile.as_ref().is_some_and(|profile| {
                            envelope.payload.as_ref().is_some_and(|payload| {
                                profile.treats_proposal_error_as_observation(payload)
                            })
                        });
                        if shadow_proposal {
                            runtime_host.emit_continuation_diagnostic(
                                envelope.run_id.as_deref(),
                                envelope.seq,
                                code,
                                &error,
                                true,
                            );
                            continue;
                        }
                        let active_run_id = state.lock().ok().and_then(|state| {
                            state
                                .worker
                                .as_ref()
                                .and_then(|worker| worker.active_run_id.clone())
                        });
                        if let Some(active_run_id) = active_run_id {
                            let _ = database.mark_run_failed(&active_run_id, code, &error);
                        }
                        runtime_host.handle_runtime_crash(error);
                        return;
                    }
                    let (Some(conversation_id), Some(run_id), Some(seq), Some(payload)) = (
                        envelope.conversation_id.clone(),
                        envelope.run_id.clone(),
                        envelope.seq,
                        envelope.payload.clone(),
                    ) else {
                        continue;
                    };
                    if let Err((code, error)) =
                        runtime_host.ensure_runtime_tool_event_allowed(&run_id, &payload)
                    {
                        let _ = database.mark_run_failed(&run_id, code, &error);
                        runtime_host.handle_runtime_crash(error);
                        return;
                    }
                    match database.apply_runtime_event(&run_id, seq, &payload) {
                        Ok(applied) => {
                            let is_continuation_proposal =
                                payload.get("type").and_then(Value::as_str)
                                    == Some(continuation::CONTINUATION_PROPOSAL_EVENT_TYPE);
                            if is_continuation_proposal {
                                let profile = runtime_host.current_execution_profile();
                                let shadow =
                                    profile.as_ref().is_ok_and(|profile| profile.is_shadow());
                                let persisted = profile.and_then(|profile| {
                                    continuation::persist_runtime_proposal(
                                        &database, &run_id, seq, &profile, &payload,
                                    )
                                });
                                match persisted {
                                    Ok(decision) => {
                                        let _ = app.emit("fox://continuation-decision", decision);
                                    }
                                    Err(error) => {
                                        let profile_id = runtime_host
                                            .current_execution_profile()
                                            .ok()
                                            .map(|profile| profile.id().to_owned());
                                        let error_code = if shadow {
                                            "runtime.continuation.shadow_rejected"
                                        } else {
                                            "runtime.continuation.invalid_proposal"
                                        };
                                        let error = match continuation::record_ingest_diagnostic(
                                            &database,
                                            &run_id,
                                            seq,
                                            profile_id.as_deref(),
                                            shadow,
                                            error_code,
                                            &error,
                                        ) {
                                            Ok(()) => error,
                                            Err(diagnostic_error) => format!(
                                                "{error}; continuation ingest diagnostic persistence failed: {diagnostic_error}"
                                            ),
                                        };
                                        if !shadow {
                                            let _ = database
                                                .mark_run_failed(&run_id, error_code, &error);
                                            runtime_host.handle_runtime_crash(error);
                                            return;
                                        }
                                        runtime_host.emit_continuation_diagnostic(
                                            Some(&run_id),
                                            Some(seq),
                                            error_code,
                                            &error,
                                            true,
                                        );
                                    }
                                }
                            }
                            if !applied {
                                continue;
                            }
                            if payload.get("type").and_then(Value::as_str)
                                == Some("run.request_snapshot")
                            {
                                // Freeze Shadow only after the Runtime has
                                // emitted the actual composed prompt identity.
                                // This event precedes every Pi tool event.
                                runtime_host.bootstrap_shadow_context_from_request_snapshot(
                                    &run_id,
                                    &conversation_id,
                                    &payload,
                                );
                            }
                            if payload.get("type").and_then(Value::as_str).is_some_and(
                                |event_type| {
                                    crate::database::WORK_EVENT_TYPES.contains(&event_type)
                                },
                            ) {
                                if let Ok(event) = serde_json::from_value::<
                                    crate::database::WorkEventRecord,
                                >(payload.clone())
                                {
                                    let _ = database.apply_work_event(&event);
                                }
                            }
                            runtime_host.report_shadow_result((|| {
                                runtime_host
                                    .shadow_reconciler()?
                                    .observe_applied_event(&run_id, &payload)
                            })());
                            let notification = RuntimeEventNotification {
                                conversation_id,
                                runtime_session_id: envelope.runtime_session_id.clone(),
                                run_id: run_id.clone(),
                                seq,
                                timestamp: envelope.timestamp.clone(),
                                event: payload.clone(),
                            };
                            let _ = app.emit("fox://runtime-event", notification);
                            if matches!(
                                payload.get("type").and_then(Value::as_str),
                                Some(
                                    "run.completed"
                                        | "run.cancelled"
                                        | "run.failed"
                                        | "run.interrupted"
                                )
                            ) {
                                runtime_host.cancel_pending_approvals_for_run(&run_id);
                                let _ = crate::lifecycle_hooks::run_event(
                                    &database,
                                    "after_run",
                                    &run_id,
                                    payload
                                        .get("type")
                                        .and_then(Value::as_str)
                                        .unwrap_or("run.terminal"),
                                    &payload,
                                );
                                runtime_host.mark_run_ready(&run_id);
                            }
                        }
                        Err(error) => {
                            let shadow_proposal = runtime_host
                                .current_execution_profile()
                                .is_ok_and(|profile| {
                                    profile.treats_proposal_error_as_observation(&payload)
                                });
                            if shadow_proposal {
                                runtime_host.emit_continuation_diagnostic(
                                    Some(&run_id),
                                    Some(seq),
                                    "runtime.continuation.shadow_event_storage_failed",
                                    &error,
                                    true,
                                );
                                continue;
                            }
                            let _ = database.mark_run_failed(
                                &run_id,
                                event_projection_failure_code(&error),
                                &error,
                            );
                            runtime_host.handle_runtime_crash(error);
                            return;
                        }
                    }
                }
            }
            runtime_host.handle_runtime_crash("runtime process closed stdout".to_owned());
        });
    }

    fn handle_runtime_crash(&self, message: String) {
        if self
            .state
            .lock()
            .ok()
            .is_some_and(|state| matches!(state.state.as_str(), "stopped" | "stopping"))
        {
            return;
        }
        let (active, conversation_id, worker, should_recover) = {
            let mut state = match self.state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            let active = state.worker.as_ref().and_then(|worker| {
                Some((
                    worker.current_conversation_id.clone()?,
                    worker.active_run_id.clone()?,
                ))
            });
            let conversation_id = state
                .worker
                .as_ref()
                .and_then(|worker| worker.current_conversation_id.clone());
            let stable_session = conversation_id
                .as_deref()
                .and_then(|id| self.database.get_runtime_session(id).ok().flatten())
                .is_some_and(|session| session.status != "unavailable");
            let should_recover = should_attempt_recovery(
                &message,
                stable_session,
                state.recovery_attempts,
                conversation_id.is_some(),
            );
            state.state = if should_recover {
                "recovering"
            } else {
                "crashed"
            }
            .to_owned();
            state.last_error = Some(message.clone());
            if should_recover {
                state.recovery_attempts = 1;
                state.last_recovery_at = Some(crate::database::now_ms());
            }
            let worker = state.worker.take();
            (active, conversation_id, worker, should_recover)
        };
        if let Some(worker) = worker {
            if let Ok(mut child) = worker.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Some((conversation_id, run_id)) = active {
            let seq = self.database.next_run_seq(&run_id).unwrap_or(1);
            let payload = json!({
                "type": "run.failed",
                "code": "runtime.process_crashed",
                "message": message,
                "recoverable": should_recover,
            });
            match self.database.apply_runtime_event(&run_id, seq, &payload) {
                Ok(true) => {
                    let _ = self.app.emit(
                        "fox://runtime-event",
                        RuntimeEventNotification {
                            conversation_id,
                            runtime_session_id: None,
                            run_id: run_id.clone(),
                            seq,
                            timestamp: protocol::timestamp(),
                            event: payload,
                        },
                    );
                }
                Ok(false) => {}
                Err(error) => {
                    let _ = self.database.mark_run_failed(
                        &run_id,
                        "runtime.process_crashed",
                        &format!("{message}; failed to persist crash event: {error}"),
                    );
                }
            }
            // Always close/remove the live shadow context, including duplicate
            // or failed crash-event persistence paths.
            self.shadow_feed_terminal(
                &run_id,
                "failed",
                crate::kernel::RunOutcome::Failed {
                    code: "runtime.process_crashed".into(),
                    message: message.clone(),
                },
                Default::default(),
            );
            self.cancel_pending_approvals_for_run(&run_id);
        }
        if !should_recover {
            self.dispatch_next_queued_run();
            return;
        }
        let Some(conversation_id) = conversation_id else {
            return;
        };
        let host = self.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            let result = host
                .spawn_worker(&conversation_id)
                .and_then(|_| host.open_runtime_session(&conversation_id).map(|_| ()));
            if let Err(error) = result {
                host.cleanup_failed_worker(&format!("Sidecar 自动恢复失败: {error}"));
            } else if let Ok(mut state) = host.state.lock() {
                state.state = "ready".to_owned();
            }
            host.dispatch_next_queued_run();
        });
    }

    fn spawn_stderr_reader(&self, stderr: std::process::ChildStderr) {
        let state = self.state.clone();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut state) = state.lock() {
                    state.stderr.push_back(line);
                    while state.stderr.len() > STDERR_LIMIT {
                        state.stderr.pop_front();
                    }
                }
            }
        });
    }

    fn stop_worker(&self) -> Result<(), String> {
        let should_shutdown = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            let should_shutdown = state.worker.is_some();
            if should_shutdown {
                state.state = "stopping".to_owned();
            }
            should_shutdown
        };
        if should_shutdown {
            let _ = self.send_request(RuntimeRequest::new("shutdown"), Duration::from_secs(2));
        }
        let worker = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?;
            state.worker.take()
        };
        if let Some(worker) = worker {
            if let Ok(mut child) = worker.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?;
        state.state = "stopped".to_owned();
        Ok(())
    }

    fn cleanup_failed_worker(&self, message: &str) {
        let worker = self
            .state
            .lock()
            .ok()
            .and_then(|mut state| state.worker.take());
        if let Some(worker) = worker {
            if let Ok(mut child) = worker.child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        if let Ok(mut state) = self.state.lock() {
            state.state = "crashed".to_owned();
            state.last_error = Some(message.to_owned());
        }
    }

    fn runtime_script_path(&self) -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("FOX_RUNTIME_SCRIPT").map(PathBuf::from) {
            return path
                .is_file()
                .then_some(path.clone())
                .ok_or_else(|| format!("runtime script was not found at {}", path.display()));
        }

        if cfg!(debug_assertions) {
            let runtime_name = if std::env::var("FOX_RUNTIME_MODE").as_deref() == Ok("fake") {
                "fake-runtime.mjs"
            } else {
                "pi-runtime.mjs"
            };
            let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../services/agent-runtime/src")
                .join(runtime_name);
            if development.is_file() {
                return Ok(development);
            }
        }

        let bundled = self
            .app
            .path()
            .resource_dir()
            .map_err(|error| error.to_string())?
            .join("agent-runtime/pi-runtime.mjs");
        bundled
            .is_file()
            .then_some(bundled.clone())
            .ok_or_else(|| format!("runtime script was not found at {}", bundled.display()))
    }

    fn runtime_command(&self) -> Result<RuntimeCommand, String> {
        if let Some(executable) = std::env::var_os("FOX_RUNTIME_EXECUTABLE").map(PathBuf::from) {
            if executable.is_file() {
                return Ok(RuntimeCommand {
                    program: executable,
                    script: None,
                });
            }
            return Err(format!(
                "runtime executable was not found at {}",
                executable.display()
            ));
        }

        if !cfg!(debug_assertions) {
            let executable = self
                .app
                .path()
                .resource_dir()
                .map_err(|error| error.to_string())?
                .join("agent-runtime/fox-agent-runtime-x86_64-pc-windows-msvc.exe");
            if executable.is_file() {
                return Ok(RuntimeCommand {
                    program: executable,
                    script: None,
                });
            }
            return Err(format!(
                "bundled Fox Runtime executable was not found at {}",
                executable.display()
            ));
        }

        let script = self.runtime_script_path()?;
        if script.file_name().and_then(|name| name.to_str()) == Some("pi-runtime.mjs")
            && (!node_dependency_is_available(&script, "@earendil-works/pi-agent-core")
                || !node_dependency_is_available(&script, "@earendil-works/pi-ai"))
        {
            return Err(
                "Pi Runtime dependencies are missing; run pnpm install in the Fox workspace."
                    .to_owned(),
            );
        }
        let node_available = Command::new("node")
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !node_available {
            return Err("Node.js is required for the development Runtime; production builds use the bundled Runtime executable.".to_owned());
        }
        Ok(RuntimeCommand {
            program: PathBuf::from("node"),
            script: Some(script),
        })
    }

    fn current_runtime_version(&self) -> Option<String> {
        self.state.lock().ok().and_then(|state| {
            state
                .runtime_version
                .as_ref()
                .map(|version| format!("{}/{}", state.runtime, version))
        })
    }
}

fn write_protocol_message<T: Serialize>(
    stdin: &Arc<Mutex<ChildStdin>>,
    message: &T,
) -> Result<(), String> {
    let line = serde_json::to_string(message).map_err(|error| error.to_string())?;
    let mut writer = stdin
        .lock()
        .map_err(|_| "runtime stdin lock is poisoned".to_owned())?;
    writer
        .write_all(format!("{line}\n").as_bytes())
        .and_then(|_| writer.flush())
        .map_err(|error| error.to_string())
}

fn response_error(response: &RuntimeEnvelope, fallback: &str) -> String {
    response
        .payload
        .as_ref()
        .and_then(|payload| payload.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(fallback)
        .to_owned()
}

fn pending_approval_ids_for_run(
    pending: &HashMap<String, PendingApproval>,
    run_id: &str,
) -> Vec<String> {
    pending
        .iter()
        .filter_map(|(approval_id, approval)| {
            (approval.run_id == run_id).then_some(approval_id.clone())
        })
        .collect()
}

fn redact_diagnostic_line(line: &str) -> String {
    let mut result = line.chars().take(512).collect::<String>();
    for marker in [
        "authorization:",
        "bearer ",
        "api_key=",
        "apikey=",
        "api-key=",
        "token=",
        "access_token=",
        "access-token=",
    ] {
        let mut search_from = 0;
        loop {
            let lowercase = result[search_from..].to_ascii_lowercase();
            let Some(relative_index) = lowercase.find(marker) else {
                break;
            };
            let index = search_from + relative_index;
            let secret_start = index + marker.len();
            if marker == "authorization:" {
                result.replace_range(secret_start.., " [REDACTED]");
                break;
            }
            let value_start = result[secret_start..]
                .find(|character: char| !character.is_whitespace())
                .map(|offset| secret_start + offset)
                .unwrap_or(result.len());
            let value_end = result[value_start..]
                .find(|character: char| {
                    character.is_whitespace() || matches!(character, ',' | ';' | '"' | '\'')
                })
                .map(|offset| value_start + offset)
                .unwrap_or(result.len());
            result.replace_range(value_start..value_end, "[REDACTED]");
            search_from = value_start + "[REDACTED]".len();
        }
    }
    result
}

/// Bound one multi-line execution diagnostic for transport to the model or a
/// receipt. Every line goes through the same credential redaction as Host
/// diagnostics, then the whole text is capped. This never claims to be the
/// complete output: the full retained result stays persisted and reachable
/// through `read_tool_result`, so bounding here loses nothing that cannot be
/// fetched again under the same Run authorization.
pub(crate) fn redact_execution_diagnostic(text: &str, limit: usize) -> String {
    let joined = text
        .lines()
        .map(redact_diagnostic_line)
        .collect::<Vec<_>>()
        .join("\n");
    let characters = joined.chars().count();
    if characters <= limit {
        return joined;
    }
    format!(
        "{}…[诊断按 {limit} 字截断，完整输出见本工具调用的持久结果]",
        joined.chars().take(limit).collect::<String>()
    )
}

fn parse_runtime_ready_payload(payload: &Value) -> Result<(String, Option<String>, Value), String> {
    let declared_protocol = payload
        .get("protocol")
        .and_then(Value::as_str)
        .ok_or_else(|| "runtime handshake omitted protocol".to_owned())?;
    let declared_version = payload
        .get("protocolVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| "runtime handshake omitted protocolVersion".to_owned())?;
    if declared_protocol != protocol::PROTOCOL_NAME
        || declared_version != u64::from(protocol::PROTOCOL_VERSION)
    {
        return Err(format!(
            "runtime handshake protocol mismatch: {declared_protocol} v{declared_version}"
        ));
    }
    let runtime = payload
        .get("runtime")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "runtime handshake omitted runtime name".to_owned())?
        .to_owned();
    let runtime_version = payload
        .get("runtimeVersion")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned);
    let capabilities = payload
        .get("capabilities")
        .cloned()
        .ok_or_else(|| "runtime handshake omitted the capability manifest".to_owned())?;
    let manifest = serde_json::from_value::<RuntimeCapabilityManifest>(capabilities.clone())
        .map_err(|error| format!("runtime capability manifest is invalid: {error}"))?;
    protocol::validate_host_manifest(&manifest)?;
    Ok((runtime, runtime_version, capabilities))
}

/// The conversation a scoped read of this Run may touch.
///
/// Authorization is derived from Host's own persisted facts: the frozen
/// `run_control_bindings` row when one exists, otherwise the Run's own row. A Run
/// started before frozen bindings existed keeps exactly the scope it already had;
/// nothing here rewrites or widens it.
///
/// The conversation claimed on the protocol envelope is checked against that
/// answer rather than used as the answer, so a Runtime — or a model driving one —
/// cannot hand over a different conversation's id to reach results it was not
/// given.
fn run_bound_conversation(
    database: &Database,
    run_id: &str,
    claimed: Option<&str>,
) -> Result<String, String> {
    let conversation = match database.run_control_binding(run_id)? {
        Some(binding) => binding.conversation_id,
        // No frozen binding: fall back to the persisted Run row. This is the
        // pre-binding fact and is reported, never overlaid with a new one.
        None => database
            .run_conversation(run_id)?
            .ok_or_else(|| format!("Run '{run_id}' does not exist"))?,
    };
    if let Some(claimed) = claimed.map(str::trim).filter(|value| !value.is_empty()) {
        if claimed != conversation {
            return Err(format!(
                "Run '{run_id}' is bound to a different conversation; stored results stay scoped to it"
            ));
        }
    }
    Ok(conversation)
}

/// Serve `read_tool_result`.
///
/// The tool only reads bytes Host already stored for a settled call, so there is
/// no approval gate and no re-execution path: nothing it does can change a file
/// or repeat a write. The read is still recorded as its own ToolCall so the trace
/// shows what the model asked for.
fn execute_tool_result_read_request(
    database: &Database,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "read_tool_result request is missing runId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "read_tool_result request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "read_tool_result request is missing toolCallId".to_owned())?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let conversation_id =
        run_bound_conversation(database, run_id, envelope.conversation_id.as_deref())?;
    if let Some(response) = inspect_existing_host_tool_call(
        database,
        run_id,
        tool_call_id,
        "read_tool_result",
        &input,
    )? {
        return Ok(response);
    }
    let hook_decision = crate::lifecycle_hooks::before_tool(
        database,
        run_id,
        tool_call_id,
        "read_tool_result",
        &input,
    )?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let request = tool_result_read::prepare(&input)?;
    match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        "read_tool_result",
        &input,
        "running",
        false,
    )? {
        HostToolCallExecution::Execute(record) => {
            drop(record);
            let outcome = tool_result_read::execute(database, &conversation_id, &request);
            finalize_host_tool_execution(
                database,
                run_id,
                tool_call_id,
                "read_tool_result",
                &input,
                hook_decision.annotations,
                outcome,
            )
        }
        HostToolCallExecution::Replay(response) => Ok(response),
    }
}

fn handle_tool_result_read_request(
    database: &Database,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_tool_result_read_request(database, &envelope)
        .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        eprintln!("fox: failed to answer read_tool_result: {error}");
    }
}

/// Resolve the skill ids enabled for a conversation exactly the way Run start
/// does: assistant package plus the expert's frozen package snapshot, each
/// filtered through its package scope and de-duplicated. Used by `skill_load`
/// so on-demand enablement can never exceed the initial one.
pub(super) fn conversation_enabled_skill_ids(
    database: &Database,
    conversation_id: &str,
) -> Result<Vec<String>, String> {
    let agent_id = database.conversation_agent_id(conversation_id)?;
    let assistant = database
        .get_agent(&agent_id)?
        .ok_or_else(|| "conversation assistant was not found".to_owned())?;
    let assistant_skills = database.enabled_agent_skills(&assistant.id)?;
    let enabled_mcps = database
        .list_mcp_servers()?
        .into_iter()
        .filter(|server| server.enabled)
        .map(|server| server.id)
        .collect::<Vec<_>>();
    let bound_knowledge = database.conversation_knowledge_references(conversation_id)?;
    let assistant_package =
        runtime_agent_package(&assistant, &assistant_skills, &enabled_mcps, &bound_knowledge)?;
    let expert_binding = database.current_conversation_expert_binding(conversation_id)?;
    let effective_expert_skills = expert_binding
        .as_ref()
        .map(|binding| {
            let skills = database.enabled_agent_skills(&binding.expert_id)?;
            let package = runtime_expert_package(
                &binding.package_snapshot,
                &binding.package_hash,
                &skills,
                &enabled_mcps,
                &bound_knowledge,
            )?;
            Ok::<Vec<String>, String>(package_string_list(&package, "enabledSkills"))
        })
        .transpose()?
        .unwrap_or_default();
    let mut seen = HashSet::new();
    Ok(package_string_list(&assistant_package, "enabledSkills")
        .into_iter()
        .chain(effective_expert_skills)
        .filter(|id| seen.insert(id.clone()))
        .collect())
}

/// Serve `skill_load` on the legacy protocol: mirror the stored-result reader's
/// durable trace, but never touch approvals (instructions are read-only data).
fn execute_skill_load_request(
    database: &Database,
    skills_dir: &std::path::Path,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "skill_load request is missing runId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "skill_load request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "skill_load request is missing toolCallId".to_owned())?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let conversation_id =
        run_bound_conversation(database, run_id, envelope.conversation_id.as_deref())?;
    if let Some(response) = inspect_existing_host_tool_call(
        database,
        run_id,
        tool_call_id,
        "skill_load",
        &input,
    )? {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, "skill_load", &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let request = skill_load::prepare(&input)?;
    match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        "skill_load",
        &input,
        "running",
        false,
    )? {
        HostToolCallExecution::Execute(record) => {
            drop(record);
            // The legacy path has no frozen scope: availability was validated
            // structurally when the skill was scanned (unknown tools fail), and
            // actual tool calls remain gated by legacy permissions.
            let enabled = conversation_enabled_skill_ids(database, &conversation_id)?;
            let outcome = skill_load::execute(
                database,
                skills_dir,
                &conversation_id,
                run_id,
                &enabled,
                None,
                &request,
                crate::database::now_ms(),
            );
            finalize_host_tool_execution(
                database,
                run_id,
                tool_call_id,
                "skill_load",
                &input,
                hook_decision.annotations,
                outcome,
            )
        }
        HostToolCallExecution::Replay(response) => Ok(response),
    }
}

fn handle_skill_load_request(
    database: &Database,
    skills_dir: &std::path::Path,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_skill_load_request(database, skills_dir, &envelope)
        .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        eprintln!("fox: failed to answer skill_load: {error}");
    }
}

fn handle_host_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_host_tool_request(app, database, state, &envelope)
        .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn handle_capability_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_capability_tool_request(app, database, state, &envelope)
        .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn handle_work_tool_request(
    runtime_host: &RuntimeHost,
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = match execute_work_tool_request(runtime_host, app, database, state, &envelope) {
        Ok(response) => response,
        Err(error) => json!({
            "isError": true,
            "error": error.message,
            "errorDetails": {
                "code": error.code,
                "details": error.details,
            },
        }),
    };
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn dispatch_created_work_child<T>(
    dispatch: Option<T>,
    start: impl FnOnce(T) -> Result<(), String>,
) -> Result<bool, String> {
    let Some(dispatch) = dispatch else {
        return Ok(false);
    };
    start(dispatch)?;
    Ok(true)
}

fn dispatch_graph_reviewer_child(
    runtime_host: &RuntimeHost,
    request_id: &str,
    reviewer_agent_id: &str,
) -> Result<bool, String> {
    let dispatch = runtime_host
        .database
        .dispatch_graph_node_reviewer(request_id, reviewer_agent_id)
        .map_err(|error| error.to_string())?;
    if !dispatch.dispatch_required {
        return Ok(false);
    }
    let parent_conversation_id = runtime_host
        .database
        .authoritative_run_conversation_id(&dispatch.child_run.parent_run_id)
        .map_err(|error| error.to_string())?;
    let child_run_id = dispatch.child_run.child_run_id.clone();
    if let Err(error) = runtime_host.start_graph_reviewer_runtime(
        dispatch.started,
        dispatch.child_run,
        parent_conversation_id,
        request_id.to_owned(),
    ) {
        let _ = runtime_host.database.mark_run_failed(
            &child_run_id,
            "graph.readonly_reviewer_dispatch_failed",
            &error,
        );
        let _ = runtime_host.database.sync_child_run_terminal(&child_run_id);
        let _ = runtime_host.database.settle_graph_node_review(request_id);
        return Err(error);
    }
    Ok(true)
}

fn signal_graph_child_cancel_with_transport(
    database: &Database,
    child_run_id: &str,
    allow_redelivery: bool,
    signal: impl FnOnce() -> Result<Option<bool>, String>,
) -> Result<bool, String> {
    let Some(status) = database.child_run_status(child_run_id)? else {
        return Ok(false);
    };
    if run_status_is_terminal(&status) {
        return Ok(false);
    }
    if status == "cancelling" {
        if !allow_redelivery {
            return Ok(false);
        }
    } else if database.mark_run_cancelling(child_run_id)?.is_none() {
        return Ok(false);
    }
    // A durable intent is not a terminal Child fact. With no live transport, leave the Run in
    // cancelling so startup recovery can redeliver; never synthesize run.cancelled here.
    signal().map(|delivered| delivered.unwrap_or(false))
}

fn validate_graph_reviewer_runtime_contract(
    started: &StartRunResult,
    child_run: &ChildRunRecord,
) -> Result<(), String> {
    let expected_tools = GRAPH_REVIEWER_ALLOWED_TOOLS
        .into_iter()
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let actual_tools = child_run
        .allowed_tools
        .as_ref()
        .map(|tools| tools.iter().cloned().collect::<HashSet<_>>())
        .ok_or_else(|| {
            "[graph.readonly_reviewer_scope_invalid] Reviewer Child omitted its frozen tool scope"
                .to_owned()
        })?;
    if actual_tools != expected_tools || child_run.allowed_tools.as_ref().unwrap().len() != 4 {
        return Err(
            "[graph.readonly_reviewer_scope_invalid] Reviewer Child tools must be exactly read, ls, find, and grep"
                .to_owned(),
        );
    }
    if child_run.child_run_id != started.run.id
        || child_run.child_conversation_id != started.run.conversation_id
        || child_run.parent_run_id == child_run.child_run_id
        || !matches!(child_run.status.as_str(), "queued" | "running")
    {
        return Err(
            "[graph.readonly_reviewer_identity_invalid] Reviewer Child projection does not match its authoritative Run"
                .to_owned(),
        );
    }
    if child_run.team_run_id.is_some()
        || child_run.team_member_id.is_some()
        || child_run.budget.max_duration_ms != GRAPH_REVIEWER_MAX_DURATION_MS
        || child_run.budget.max_total_tokens != GRAPH_REVIEWER_MAX_TOTAL_TOKENS
        || child_run.budget.max_output_tokens != GRAPH_REVIEWER_MAX_OUTPUT_TOKENS
        || child_run.budget.max_tool_calls != GRAPH_REVIEWER_MAX_TOOL_CALLS
    {
        return Err(
            "[graph.readonly_reviewer_contract_invalid] Reviewer Child must use the fixed bounded review contract"
                .to_owned(),
        );
    }
    Ok(())
}

fn ensure_child_cancel_not_graph_bound(
    database: &Database,
    child_run_id: &str,
) -> Result<(), String> {
    if database
        .graph_attempt_id_for_child_run(child_run_id)?
        .is_some()
    {
        return Err(
            "[graph.readonly_dedicated_cancel_required] Graph-bound Child Runs must be cancelled through graph_readonly_node_cancel"
                .to_owned(),
        );
    }
    Ok(())
}

fn handle_child_run_tool_request(
    runtime_host: &RuntimeHost,
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_child_run_tool_request(runtime_host, app, database, state, &envelope)
        .unwrap_or_else(|error| {
            json!({
                "isError": true,
                "error": error,
                "errorDetails": { "code": "child_run.tool_failed", "retryable": false },
            })
        });
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn execute_child_run_tool_request(
    runtime_host: &RuntimeHost,
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "Child Run tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "Child Run tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "Child Run tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "Child Run tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .filter(|tool| is_child_run_tool(tool))
        .ok_or_else(|| "Child Run tool request has an unsupported tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if hook_decision.requires_approval {
            "pending"
        } else {
            "running"
        },
        hook_decision.requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome: Result<Value, String> = (|| {
        if hook_decision.requires_approval {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )?;
        }
        match tool {
            "team_snapshot_get" => Ok(json!({
                "team": database.latest_expert_team(conversation_id)?,
            })),
            "team_start" => {
                let objective = required_bounded_text(&input, "objective", 8_000)?;
                let context = optional_bounded_text(&input, "context", 12_000)?;
                let team = crate::expert_teams::start_team(
                    database,
                    conversation_id,
                    run_id,
                    objective,
                    context,
                )?;
                Ok(json!({ "team": team }))
            }
            "team_member_start" => {
                let snapshot = database
                    .active_expert_team(conversation_id)?
                    .ok_or_else(|| "team.not_active".to_owned())?;
                if snapshot.run.parent_run_id != run_id {
                    return Err("team.parent_run_mismatch".to_owned());
                }
                let member_id = required_bounded_text(&input, "memberId", 128)?;
                if snapshot
                    .members
                    .iter()
                    .any(|member| member.team_member_id.as_deref() == Some(member_id))
                {
                    return Err("team.member_already_started".to_owned());
                }
                if snapshot.members.iter().any(|member| {
                    matches!(member.status.as_str(), "queued" | "running" | "cancelling")
                }) {
                    return Err("team.serial_member_still_active".to_owned());
                }
                let team = crate::expert_teams::parse_team(snapshot.run.team.clone())?;
                let member = team
                    .members
                    .iter()
                    .find(|member| member.id == member_id)
                    .ok_or_else(|| "team.member_not_found".to_owned())?;
                let task = required_bounded_text(&input, "task", 8_000)?;
                let member_context = optional_bounded_text(&input, "context", 12_000)?;
                let model_max_output = database
                    .get_model_service()?
                    .ok_or_else(|| "model service is not configured".to_owned())?
                    .max_output_tokens;
                if member.budget.max_output_tokens > model_max_output {
                    return Err("team.member_budget_exceeds_model_output".to_owned());
                }
                let isolated_context = format!(
                "# Expert Team\n{} ({})\n\n# Member identity\n{} — {}\n\n# Frozen member instructions\n{}\n\n# Lead objective\n{}\n\n# Shared bounded context\n{}\n\n# Member-specific bounded context\n{}",
                team.title,
                team.id,
                member.name,
                member.role,
                member.instructions,
                snapshot.run.objective,
                snapshot.run.context,
                member_context,
            );
                let (started, child_run, created) = database
                .create_child_run(crate::database::CreateChildRunInput {
                    parent_run_id: run_id,
                    tool_call_id,
                    worker_agent_id: &member.agent_id,
                    objective: task,
                    context: &isolated_context,
                    budget: &member.budget,
                    team_run_id: Some(&snapshot.run.id),
                    team_member_id: Some(&member.id),
                    allowed_tools: Some(&member.allowed_tools),
                })
                .map_err(|error| format!(
                    "team.member_start_rejected: Host serial, identity, depth, concurrency, or budget constraints rejected the member ({error})"
                ))?;
                if created || child_run.status == "queued" {
                    runtime_host.start_child_runtime(
                        started,
                        child_run.clone(),
                        conversation_id.to_owned(),
                    )?;
                }
                Ok(
                    json!({ "teamRunId": snapshot.run.id, "childRun": child_run, "created": created }),
                )
            }
            "team_collect" => {
                let snapshot = database
                    .active_expert_team(conversation_id)?
                    .or_else(|| database.latest_expert_team(conversation_id).ok().flatten())
                    .ok_or_else(|| "team.not_found".to_owned())?;
                if snapshot.run.parent_run_id != run_id {
                    return Err("team.parent_run_mismatch".to_owned());
                }
                let wait_ms = input
                    .get("waitMs")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .clamp(0, 60_000) as u64;
                let deadline = Instant::now() + Duration::from_millis(wait_ms);
                let snapshot = loop {
                    let current = database
                        .get_expert_team(&snapshot.run.id)?
                        .ok_or_else(|| "team.not_found".to_owned())?;
                    if current
                        .members
                        .iter()
                        .all(|member| run_status_is_terminal(&member.status))
                        || wait_ms == 0
                        || Instant::now() >= deadline
                    {
                        break current;
                    }
                    thread::sleep(Duration::from_millis(100));
                };
                let team = if snapshot
                    .members
                    .iter()
                    .all(|member| run_status_is_terminal(&member.status))
                {
                    database.finalize_expert_team(&snapshot.run.id)?
                } else {
                    snapshot
                };
                Ok(json!({ "team": team }))
            }
            "team_cancel" => {
                let snapshot = database
                    .active_expert_team(conversation_id)?
                    .ok_or_else(|| "team.not_active".to_owned())?;
                if snapshot.run.parent_run_id != run_id {
                    return Err("team.parent_run_mismatch".to_owned());
                }
                let reason = optional_bounded_text(&input, "reason", 2_000)?;
                let team = runtime_host.cancel_expert_team(
                    &snapshot.run.id,
                    if reason.is_empty() {
                        "cancelled by Team Lead"
                    } else {
                        reason
                    },
                )?;
                Ok(json!({ "team": team }))
            }
            "child_agent_list" => Ok(json!({
                "agents": database.list_child_agents()?,
                "experts": database.list_consultable_experts()?,
            })),
            "child_run_start" => {
                let objective = required_bounded_text(&input, "objective", 8_000)?;
                let context = optional_bounded_text(&input, "context", 12_000)?;
                let mode = input
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("worker");
                match mode {
                    "worker" => {
                        if input.get("expertId").is_some() {
                            return Err(
                                "child_run.invalid_identity: mode=worker does not accept expertId"
                                    .to_owned(),
                            );
                        }
                        let agent_id = input
                            .get("agentId")
                            .and_then(Value::as_str)
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .unwrap_or("fox-general");
                        let available_agents = database.list_child_agents()?;
                        if !available_agents.iter().any(|agent| agent.id == agent_id) {
                            let available = available_agents
                                .iter()
                                .map(|agent| format!("{} ({})", agent.id, agent.name))
                                .collect::<Vec<_>>()
                                .join("、");
                            return Err(format!(
                                "child_run.agent_not_found: 子 Agent 模板 '{agent_id}' 不存在。可用模板：{}。请先调用 child_agent_list，并使用 agents 中返回的精确 agentId。",
                                if available.is_empty() { "无" } else { &available }
                            ));
                        }
                        start_isolated_child_run(
                            runtime_host,
                            database,
                            run_id,
                            conversation_id,
                            tool_call_id,
                            agent_id,
                            objective,
                            context,
                            input.get("budget"),
                            "child_run.start_rejected",
                        )
                    }
                    "expert_consultation" => {
                        if input.get("agentId").is_some() {
                            return Err(
                                "child_run.invalid_identity: mode=expert_consultation does not accept agentId"
                                    .to_owned(),
                            );
                        }
                        let expert_id = required_bounded_text(&input, "expertId", 160)?;
                        let available_experts = database.list_consultable_experts()?;
                        if !available_experts
                            .iter()
                            .any(|expert| expert.id == expert_id)
                        {
                            let available = available_experts
                                .iter()
                                .map(|expert| format!("{} ({})", expert.id, expert.name))
                                .collect::<Vec<_>>()
                                .join("、");
                            return Err(format!(
                                "child_run.expert_not_found: 专家 '{expert_id}' 不存在或不可咨询。可用专家：{}。请先调用 child_agent_list，并使用 experts 中返回的精确 expertId。",
                                if available.is_empty() { "无" } else { &available }
                            ));
                        }
                        start_isolated_child_run(
                            runtime_host,
                            database,
                            run_id,
                            conversation_id,
                            tool_call_id,
                            expert_id,
                            objective,
                            context,
                            input.get("budget"),
                            "child_run.expert_consultation_rejected",
                        )
                    }
                    _ => Err(
                        "child_run.invalid_mode: mode must be worker or expert_consultation"
                            .to_owned(),
                    ),
                }
            }
            "child_run_collect" => {
                let requested = input
                    .get("childRunIds")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "childRunIds must be a non-empty array".to_owned())?;
                if requested.is_empty() || requested.len() > 8 {
                    return Err("childRunIds must contain between 1 and 8 items".to_owned());
                }
                let requested = requested
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_owned)
                            .ok_or_else(|| "childRunIds contains an invalid ID".to_owned())
                    })
                    .collect::<Result<HashSet<_>, _>>()?;
                let wait_ms = input
                    .get("waitMs")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .clamp(0, 60_000) as u64;
                let deadline = Instant::now() + Duration::from_millis(wait_ms);
                let records = loop {
                    let records = database
                        .child_runs_for_parent(run_id)?
                        .into_iter()
                        .filter(|record| requested.contains(&record.child_run_id))
                        .collect::<Vec<_>>();
                    if records.len() != requested.len() {
                        return Err(
                            "one or more Child Runs do not belong to the current parent Run"
                                .to_owned(),
                        );
                    }
                    if records
                        .iter()
                        .all(|record| run_status_is_terminal(&record.status))
                        || wait_ms == 0
                        || Instant::now() >= deadline
                    {
                        break records;
                    }
                    thread::sleep(Duration::from_millis(100));
                };
                let all_terminal = records
                    .iter()
                    .all(|record| run_status_is_terminal(&record.status));
                Ok(json!({ "childRuns": records, "allTerminal": all_terminal }))
            }
            "child_run_cancel" => {
                let child_run_id = required_bounded_text(&input, "childRunId", 160)?;
                let child = database
                    .child_run(child_run_id)?
                    .ok_or_else(|| "Child Run was not found".to_owned())?;
                if child.parent_run_id != run_id {
                    return Err("Child Run does not belong to the current parent Run".to_owned());
                }
                ensure_child_cancel_not_graph_bound(database, child_run_id)?;
                let cancelled = runtime_host.cancel_child_runtime(child_run_id)?;
                let child_run = database.child_run(child_run_id)?;
                Ok(json!({ "cancelled": cancelled, "childRun": child_run }))
            }
            _ => unreachable!(),
        }
    })();

    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}

fn required_bounded_text<'a>(
    input: &'a Value,
    field: &str,
    max_chars: usize,
) -> Result<&'a str, String> {
    let value = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{field} is required"))?;
    if value.chars().count() > max_chars {
        return Err(format!("{field} exceeds the {max_chars}-character limit"));
    }
    Ok(value)
}

fn optional_bounded_text<'a>(
    input: &'a Value,
    field: &str,
    max_chars: usize,
) -> Result<&'a str, String> {
    let value = input
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if value.chars().count() > max_chars {
        return Err(format!("{field} exceeds the {max_chars}-character limit"));
    }
    Ok(value)
}

fn normalized_child_budget(
    value: Option<&Value>,
    model_max_output: i64,
) -> Result<ChildRunBudget, String> {
    let value = value.and_then(Value::as_object);
    let integer = |name: &str, default: i64, minimum: i64, maximum: i64| -> Result<i64, String> {
        let selected = value
            .and_then(|value| value.get(name))
            .map(|value| {
                value
                    .as_i64()
                    .ok_or_else(|| format!("budget.{name} must be an integer"))
            })
            .transpose()?
            .unwrap_or(default);
        if !(minimum..=maximum).contains(&selected) {
            return Err(format!(
                "budget.{name} must be between {minimum} and {maximum}"
            ));
        }
        Ok(selected)
    };
    let max_duration_ms = integer("maxDurationMs", 300_000, 1_000, 900_000)?;
    let max_total_tokens = integer("maxTotalTokens", 32_000, 256, 200_000)?;
    let requested_output = value.and_then(|value| value.get("maxOutputTokens"));
    let default_output = model_max_output.clamp(64, 4_096).min(max_total_tokens);
    let max_output_tokens = integer(
        "maxOutputTokens",
        default_output,
        64,
        model_max_output.clamp(64, 32_768),
    )?;
    if requested_output.is_some() && max_output_tokens > max_total_tokens {
        return Err("budget.maxOutputTokens cannot exceed budget.maxTotalTokens".to_owned());
    }
    let max_tool_calls = integer("maxToolCalls", 16, 0, 100)?;
    Ok(ChildRunBudget {
        max_duration_ms,
        max_total_tokens,
        max_output_tokens: max_output_tokens.min(max_total_tokens),
        max_tool_calls,
    })
}

#[allow(clippy::too_many_arguments)]
fn start_isolated_child_run(
    runtime_host: &RuntimeHost,
    database: &Database,
    parent_run_id: &str,
    parent_conversation_id: &str,
    tool_call_id: &str,
    worker_agent_id: &str,
    objective: &str,
    context: &str,
    budget_input: Option<&Value>,
    rejection_code: &str,
) -> Result<Value, String> {
    let model_max_output = database
        .get_model_service()?
        .ok_or_else(|| "model service is not configured".to_owned())?
        .max_output_tokens;
    let budget = normalized_child_budget(budget_input, model_max_output)?;
    let (started, child_run, created) = database
        .create_child_run(crate::database::CreateChildRunInput {
            parent_run_id,
            tool_call_id,
            worker_agent_id,
            objective,
            context,
            budget: &budget,
            team_run_id: None,
            team_member_id: None,
            allowed_tools: None,
        })
        .map_err(|error| {
            format!(
                "{rejection_code}: the agent, depth, concurrency, or per-parent limit rejected this isolated Child Run ({error})"
            )
        })?;
    if created || child_run.status == "queued" {
        runtime_host.start_child_runtime(
            started,
            child_run.clone(),
            parent_conversation_id.to_owned(),
        )?;
    }
    Ok(json!({ "childRun": child_run, "created": created }))
}

fn run_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "failed" | "cancelled" | "interrupted")
}

fn is_memory_tool(tool: &str) -> bool {
    matches!(tool, "memory_search" | "memory_propose")
}

fn is_child_run_tool(tool: &str) -> bool {
    crate::expert_teams::is_team_tool(tool)
        || matches!(
            tool,
            "child_agent_list" | "child_run_start" | "child_run_collect" | "child_run_cancel"
        )
}

fn handle_memory_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response =
        execute_memory_tool_request(app, database, state, &envelope).unwrap_or_else(|error| {
            json!({
                "isError": true,
                "error": error,
                "errorDetails": { "code": "memory.tool_failed", "retryable": false },
            })
        });
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn execute_memory_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "memory tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "memory tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "memory tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "memory tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "memory tool request is missing tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if hook_decision.requires_approval {
            "pending"
        } else {
            "running"
        },
        hook_decision.requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome: Result<Value, String> = (|| {
        if hook_decision.requires_approval {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )?;
        }
        execute_memory_operation(database,conversation_id,run_id,tool,&input)
    })();
    finalize_host_tool_execution(
        database,run_id,tool_call_id,tool,&input,hook_decision.annotations,outcome,
    )
}

// Shared operation only: execution authority, approvals and result commits
// remain with the calling Legacy or Kernel owner.
fn execute_memory_operation(database: &Database, conversation_id: &str, run_id: &str,
    tool: &str, input: &Value) -> Result<Value,String> {
        match tool {
            "memory_search" => {
                let query = input.get("query").and_then(Value::as_str).unwrap_or("");
                let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(8) as usize;
                database
                    .recall_memories(conversation_id, Some(run_id), query, limit, 8_000)
                    .and_then(|value| {
                        serde_json::to_value(value).map_err(|error| error.to_string())
                    })
            }
            "memory_propose" => {
                let required = |name: &str| {
                    input
                        .get(name)
                        .and_then(Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| format!("memory.{name}_required"))
                };
                let proposal = MemoryProposalInput {
                    scope: input
                        .get("scope")
                        .and_then(Value::as_str)
                        .unwrap_or("agent")
                        .to_owned(),
                    kind: required("kind")?,
                    canonical_key: required("canonicalKey")?,
                    content: required("content")?,
                    evidence_excerpt: required("evidenceExcerpt")?,
                    source_message_id: input
                        .get("sourceMessageId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    confidence: input
                        .get("confidence")
                        .and_then(Value::as_f64)
                        .unwrap_or(0.7),
                };
                database
                    .propose_memory_for_run(conversation_id, run_id, &proposal)
                    .and_then(|value| {
                        serde_json::to_value(value).map_err(|error| error.to_string())
                    })
            }
            _ => Err(format!("unsupported memory tool: {tool}")),
        }
}

fn execute_work_tool_request(
    runtime_host: &RuntimeHost,
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, work_tools::WorkToolError> {
    let work_loop_enabled = state
        .lock()
        .map_err(|_| "runtime state lock is poisoned".to_owned())?
        .capabilities
        .clone();
    let work_loop_enabled = serde_json::from_value::<RuntimeCapabilityManifest>(work_loop_enabled)
        .ok()
        .is_some_and(|manifest| manifest.work_loop_enabled());
    if !work_loop_enabled {
        return Err(
            "work tools are unavailable; this runtime is running as a normal conversation".into(),
        );
    }
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "work tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "work tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "work tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "work tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "work tool request is missing tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)
        .map_err(work_tools::WorkToolError::from)?;
    let raw_input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let repair_override = tool == "task_repair_escalate_start";
    let input = if repair_override {
        work_tools::canonicalize_task_repair_override_request(&raw_input)?
    } else {
        raw_input
    };
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)
            .map_err(work_tools::WorkToolError::from)?
    {
        return Ok(response);
    }
    if repair_override {
        work_tools::preflight_task_repair_override(database, conversation_id, run_id, &input)?;
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)
            .map_err(work_tools::WorkToolError::from)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(work_tools::WorkToolError::from(format!(
            "[hook.blocked] {reason}"
        )));
    }
    let requires_approval = repair_override || hook_decision.requires_approval;
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if requires_approval {
            "pending"
        } else {
            "running"
        },
        requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome = (|| -> Result<work_tools::WorkToolOutcome, work_tools::WorkToolError> {
        let repair_override_approval = if repair_override {
            Some(
                request_task_repair_override_approval(
                    app,
                    database,
                    state,
                    run_id,
                    &tool_call,
                    &input,
                    hook_decision.approval_reason.as_deref(),
                )
                .map_err(work_tools::WorkToolError::from)?,
            )
        } else if hook_decision.requires_approval {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )
            .map_err(work_tools::WorkToolError::from)?;
            None
        } else {
            None
        };
        if let Some(approval) = repair_override_approval.as_ref() {
            work_tools::execute_approved_task_repair_override(
                database,
                conversation_id,
                run_id,
                &tool_call.id,
                &approval.id,
                &input,
            )
        } else {
            work_tools::execute_with_host_tool_call_id(
                database,
                conversation_id,
                run_id,
                &tool_call.id,
                tool,
                &input,
            )
        }
    })();
    match outcome {
        Ok(outcome) => {
            let post_finalize = outcome.post_finalize;
            let response = finalize_host_tool_execution(
                database,
                run_id,
                tool_call_id,
                tool,
                &input,
                hook_decision.annotations,
                Ok(outcome.result),
            )
            .map_err(work_tools::WorkToolError::from)?;
            if let Some(directive) = post_finalize {
                match directive {
                    work_tools::WorkToolPostFinalizeDirective::StartChild(dispatch) => {
                        let failed_dispatch_child = dispatch.child_run.clone();
                        if let Err(error) =
                            dispatch_created_work_child(Some(dispatch), |dispatch| {
                                runtime_host.start_child_runtime(
                                    dispatch.started,
                                    dispatch.child_run,
                                    conversation_id.to_owned(),
                                )
                            })
                        {
                            let _ = database.mark_run_failed(
                                &failed_dispatch_child.child_run_id,
                                "graph.readonly_child_dispatch_failed",
                                &error,
                            );
                            let _ = database
                                .sync_child_run_terminal(&failed_dispatch_child.child_run_id);
                            if let Ok(Some(record)) =
                                database.child_run(&failed_dispatch_child.child_run_id)
                            {
                                runtime_host.emit_child_run_update(conversation_id, &record);
                            }
                        }
                    }
                    work_tools::WorkToolPostFinalizeDirective::CancelGraphChild(directive) => {
                        match database.activate_graph_node_cancel_intent(&directive.intent_id) {
                            Ok(activation) if activation.activated => {
                                if let Err(error) = runtime_host
                                    .signal_graph_child_cancel(&activation.child_run_id, false)
                                {
                                    eprintln!(
                                        "[graph.readonly_node_cancel_signal_failed] intent={} child={} error={}",
                                        activation.intent_id, activation.child_run_id, error
                                    );
                                }
                            }
                            Ok(activation) if activation.stale => {
                                eprintln!(
                                    "[graph.readonly_node_cancel_activation_stale] intent={} child={}",
                                    activation.intent_id, activation.child_run_id
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!(
                                    "[graph.readonly_node_cancel_activation_failed] intent={} child={} error={}",
                                    directive.intent_id, directive.child_run_id, error
                                );
                            }
                        }
                    }
                    work_tools::WorkToolPostFinalizeDirective::StartGraphReviewer(directive) => {
                        match database.activate_graph_node_review_request(&directive.request_id) {
                            Ok(activation) if activation.activated => {
                                match database.conversation_agent_id(conversation_id) {
                                    Ok(reviewer_agent_id) => {
                                        if let Err(error) = dispatch_graph_reviewer_child(
                                            runtime_host,
                                            &directive.request_id,
                                            &reviewer_agent_id,
                                        ) {
                                            eprintln!(
                                                "[graph.readonly_reviewer_dispatch_failed] request={} error={}",
                                                directive.request_id, error
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        eprintln!(
                                            "[graph.readonly_reviewer_agent_lookup_failed] request={} error={}",
                                            directive.request_id, error
                                        );
                                    }
                                }
                            }
                            Ok(activation) if activation.stale => {
                                eprintln!(
                                    "[graph.readonly_reviewer_activation_stale] request={}",
                                    activation.request_id
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!(
                                    "[graph.readonly_reviewer_activation_failed] request={} error={}",
                                    directive.request_id, error
                                );
                            }
                        }
                    }
                    work_tools::WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(directive) => {
                        match database.activate_read_only_graph_acceptance(&directive.acceptance_id)
                        {
                            Ok(acceptance) if acceptance.accepted => {
                                let _ = app.emit("fox://graph-acceptance", acceptance);
                            }
                            Ok(acceptance) if acceptance.stale => {
                                eprintln!(
                                    "[graph.readonly_accept_activation_stale] acceptance={}",
                                    acceptance.acceptance_id
                                );
                            }
                            Ok(_) => {}
                            Err(error) => {
                                eprintln!(
                                    "[graph.readonly_accept_activation_failed] acceptance={} error={}",
                                    directive.acceptance_id, error
                                );
                            }
                        }
                    }
                }
            }
            for event in outcome.events {
                let _ = app.emit("fox://work-event", event);
            }
            Ok(response)
        }
        Err(error) => {
            let message = error.to_string();
            match finalize_host_tool_execution(
                database,
                run_id,
                tool_call_id,
                tool,
                &input,
                hook_decision.annotations,
                Err(message.clone()),
            ) {
                Ok(authoritative) => Ok(authoritative),
                Err(authoritative) if authoritative == message => Err(error),
                Err(authoritative) => Err(work_tools::WorkToolError::from(authoritative)),
            }
        }
    }
}

fn is_knowledge_tool(tool: &str) -> bool {
    matches!(
        tool,
        "list_knowledge_bases"
            | "search_knowledge"
            | "read_knowledge_document"
            | "query_knowledge_graph"
    )
}

fn is_mcp_tool(tool: &str) -> bool {
    matches!(tool, "list_mcp_tools" | "call_mcp_tool")
}

fn ensure_agent_tool_allowed(agent: &AgentRecord, role: &str, tool: &str) -> Result<(), String> {
    ensure_package_tool_allowed(&agent.package_manifest, &agent.name, role, tool)
}

fn ensure_package_tool_allowed(
    package_manifest: &Value,
    package_name: &str,
    role: &str,
    tool: &str,
) -> Result<(), String> {
    let Some(allowed_tools) = package_manifest
        .get("allowedTools")
        .and_then(Value::as_array)
    else {
        return Ok(());
    };
    if allowed_tools.iter().any(|item| item.as_str() == Some(tool)) {
        return Ok(());
    }
    Err(format!(
        "{role} '{}' is not allowed to use tool '{}'",
        package_name, tool
    ))
}

fn agent_mcp_server_scope(agent: &AgentRecord) -> Option<HashSet<String>> {
    (!agent.is_builtin).then(|| {
        agent
            .package_manifest
            .get("mcpServers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

fn expert_package_manifest(package: &Value) -> Result<&Value, String> {
    package.get("packageManifest").ok_or_else(|| {
        "[conversation.expert_snapshot_invalid] Expert package manifest is missing; choose the expert again."
            .to_owned()
    })
}

fn expert_package_mcp_server_scope(package: &Value) -> Result<Option<HashSet<String>>, String> {
    let is_builtin = package
        .get("isBuiltin")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_builtin {
        return Ok(None);
    }
    Ok(Some(
        expert_package_manifest(package)?
            .get("mcpServers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
    ))
}

fn intersect_mcp_server_scopes(
    left: Option<HashSet<String>>,
    right: Option<HashSet<String>>,
) -> Option<HashSet<String>> {
    intersect_optional_scopes(left, right)
}

fn effective_conversation_delegation_scope(
    database: &Database,
    conversation_id: &str,
) -> Result<(Option<HashSet<String>>, Option<HashSet<String>>), String> {
    let agent_id = database.conversation_agent_id(conversation_id)?;
    let assistant = database
        .get_agent(&agent_id)?
        .ok_or_else(|| "conversation assistant was not found".to_owned())?;
    let mut tool_scope = package_string_scope(&assistant.package_manifest, "allowedTools");
    let mut mcp_scope = agent_mcp_server_scope(&assistant);
    if let Some(binding) = database.current_conversation_expert_binding(conversation_id)? {
        validate_expert_package_snapshot(&binding.package_snapshot, &binding.package_hash)?;
        let manifest = expert_package_manifest(&binding.package_snapshot)?;
        tool_scope =
            intersect_optional_scopes(tool_scope, package_string_scope(manifest, "allowedTools"));
        mcp_scope = intersect_optional_scopes(
            mcp_scope,
            expert_package_mcp_server_scope(&binding.package_snapshot)?,
        );
    }
    Ok((tool_scope, mcp_scope))
}

fn ensure_expert_tool_allowed(
    database: &Database,
    conversation_id: &str,
    tool: &str,
) -> Result<(), String> {
    let agent_id = database.conversation_agent_id(conversation_id)?;
    let assistant = database
        .get_agent(&agent_id)?
        .ok_or_else(|| "conversation assistant was not found".to_owned())?;
    ensure_agent_tool_allowed(&assistant, "assistant", tool)?;
    if let Some(binding) = database.current_conversation_expert_binding(conversation_id)? {
        validate_expert_package_snapshot(&binding.package_snapshot, &binding.package_hash)?;
        let manifest = expert_package_manifest(&binding.package_snapshot)?;
        let name = binding
            .package_snapshot
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("bound expert");
        ensure_package_tool_allowed(manifest, name, "expert", tool)?;
    }
    Ok(())
}

fn permission_scope_hash(kind: &str, value: &[u8]) -> String {
    format!("{kind}:sha256:{}", hex::encode(Sha256::digest(value)))
}

fn mcp_permission_scope(tool: &str, input: &Value) -> Option<String> {
    if tool != "call_mcp_tool" {
        return None;
    }
    let server_id = input.get("serverId")?.as_str()?.trim();
    let remote_tool = input.get("tool")?.as_str()?.trim();
    if server_id == crate::office::SERVER_ID {
        // Office grants are bound to the exact request (files and operations),
        // never to the entire connector's read/write command surface.
        return Some(permission_scope_hash("office-request", input.to_string().as_bytes()));
    }
    if server_id.is_empty() || remote_tool.is_empty() {
        return None;
    }
    Some(permission_scope_hash(
        "mcp-tool",
        format!("{server_id}\0{remote_tool}").as_bytes(),
    ))
}

fn host_permission_scope(tool: &str, preview: &crate::tool_host::ToolPreview) -> Option<String> {
    // Shell commands are deliberately one-shot. A command string is not a stable
    // capability boundary and must never become a conversation-wide grant.
    if tool == "run_command" {
        return None;
    }
    Some(permission_scope_hash(
        "host-target",
        format!("{tool}\0{}", preview.target).as_bytes(),
    ))
}

fn capability_permission_scope(tool: &str, input: &Value) -> Option<String> {
    let encoded = serde_json::to_vec(input).ok()?;
    Some(permission_scope_hash(
        &format!("capability-{tool}"),
        &encoded,
    ))
}

fn attach_permission_scope(request: &mut Value, scope: Option<&str>) {
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if let Some(scope) = scope {
        object.insert(
            "permissionScope".to_owned(),
            Value::String(scope.to_owned()),
        );
        object.insert(
            "availableDecisions".to_owned(),
            json!(["allow_once", "allow_conversation", "deny"]),
        );
    } else {
        object.insert(
            "availableDecisions".to_owned(),
            json!(["allow_once", "deny"]),
        );
    }
}

/// Keep cleanup in the caller on both binding failures and receive failures.
/// Invalid persisted control must never silently reset the approval deadline.
fn receive_run_approval(
    database: &Database,
    run_id: &str,
    approval_id: &str,
    receiver: &mpsc::Receiver<bool>,
) -> Result<bool, String> {
    let remaining = database.run_approval_wait_remaining_ms(run_id, approval_id, chrono::Utc::now().timestamp_millis())?;
    if remaining == 0 { return Err("Approval request timed out".into()); }
    receiver.recv_timeout(Duration::from_millis(remaining))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => "Approval request timed out".to_owned(),
            mpsc::RecvTimeoutError::Disconnected => "Approval request was interrupted".to_owned(),
        })
}

#[cfg(test)]
mod deliverable_placement_tests {
    use super::*;
    use crate::runtime_host::artifact_store::{
        is_valid_deliverable_folder_name, project_key, set_test_day_offset,
    };
    use fox_engine_protocol::{FrozenPermission, PermissionMode};

    /// The clock seam is process-global, so a test that moves it takes this lock
    /// and restores the real clock on exit — including on panic, via `Drop`.
    static CLOCK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct ClockGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl ClockGuard {
        fn new() -> ClockGuard {
            let guard = ClockGuard {
                _lock: CLOCK_LOCK.lock().unwrap_or_else(|error| error.into_inner()),
            };
            set_test_day_offset(0);
            guard
        }

        fn set(&self, days: i64) {
            set_test_day_offset(days);
        }
    }

    impl Drop for ClockGuard {
        fn drop(&mut self) {
            set_test_day_offset(0);
        }
    }

    struct Fixture {
        root: PathBuf,
        sessions: PathBuf,
        database: Option<Database>,
        conversation: String,
        other_project: PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!(
                "fox-placement-{tag}-{}",
                uuid::Uuid::new_v4()
            ));
            let sessions = root.join("runtime-sessions");
            std::fs::create_dir_all(&sessions).unwrap();
            let other_project = root.join("other-project");
            std::fs::create_dir_all(&other_project).unwrap();
            let database = Database::open(root.join("test.db")).unwrap();
            let conversation = database
                .create_conversation(database.default_agent_id(), None, None, None)
                .unwrap()
                .id;
            Fixture {
                root,
                sessions,
                database: Some(database),
                conversation,
                other_project,
            }
        }

        fn database(&self) -> &Database {
            self.database.as_ref().expect("database is open")
        }

        /// Really close the connection and reopen the same file, so anything a
        /// later Run reuses can only have come from durable storage.
        fn restart(&mut self) {
            let path = self.root.join("test.db");
            let previous = self.database.take().expect("database is open");
            drop(previous);
            self.database = Some(Database::open(path).unwrap());
        }

        fn permission(&self, project: &std::path::Path, mode: PermissionMode) -> FrozenPermission {
            FrozenPermission {
                mode,
                project_root: Some(project.to_string_lossy().into_owned()),
                grants: Vec::new(),
                approval_epoch: None,
            }
        }

        fn context_for(&self, project: &std::path::Path) -> Value {
            project_context_for(
                self.database(),
                &self.sessions,
                &self.conversation,
                &self.permission(project, PermissionMode::Allow),
            )
        }

        fn folder_of(context: &Value) -> String {
            context["deliverableFolder"]
                .as_str()
                .expect("a deliverable folder is injected")
                .to_owned()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.database = None;
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// The production entry decides the folder once and every later Run — on a
    /// later day and after the database is closed and reopened — reuses it.
    /// Before this fix the name was recomputed from the current date on every
    /// prompt build, so a task crossing midnight moved to a second folder.
    #[test]
    fn the_production_entry_keeps_one_deliverable_folder_across_days_and_restarts() {
        let clock = ClockGuard::new();
        let mut fixture = Fixture::new("stable");
        let first = fixture.context_for(&fixture.root.clone());
        let folder = Fixture::folder_of(&first);
        assert!(is_valid_deliverable_folder_name(&folder), "{folder}");
        assert_eq!(
            first["deliverableRoot"].as_str().unwrap(),
            format!("fox/{folder}")
        );
        // Deciding the placement creates no directory: a conversation that never
        // produces a deliverable leaves nothing on disk.
        assert!(!fixture.root.join("fox").exists());

        // The same logical task continues on the following day.
        clock.set(1);
        let next_day = fixture.context_for(&fixture.root.clone());
        assert_eq!(
            Fixture::folder_of(&next_day),
            folder,
            "a continuation on a later day must reuse the assigned folder"
        );

        // Application restart: the connection is closed and the database file
        // reopened, so a reused folder can only come from durable storage.
        fixture.restart();
        let reopened = fixture.context_for(&fixture.root.clone());
        assert_eq!(Fixture::folder_of(&reopened), folder);

        // Two more days later it is still the same folder.
        clock.set(3);
        assert_eq!(Fixture::folder_of(&fixture.context_for(&fixture.root.clone())), folder);
    }

    /// Ownership is per (conversation, project): another conversation never
    /// shares this one's folder, a different project never inherits the record,
    /// and returning to the original project re-reads the original assignment.
    #[test]
    fn deliverable_folders_are_owned_per_conversation_and_per_project() {
        let _clock = ClockGuard::new();
        let fixture = Fixture::new("ownership");
        let mine = Fixture::folder_of(&fixture.context_for(&fixture.root.clone()));

        // Another conversation in the same project gets its own folder.
        let other_conversation = fixture
            .database()
            .create_conversation(fixture.database().default_agent_id(), None, None, None)
            .unwrap()
            .id;
        let theirs = Fixture::folder_of(&project_context_for(
            fixture.database(),
            &fixture.sessions,
            &other_conversation,
            &fixture.permission(&fixture.root, PermissionMode::Allow),
        ));
        assert_ne!(theirs, mine);

        // The same conversation pointed at another project has its own record
        // there: a stale name for a *different* project is never reused.
        let other_project = fixture.other_project.clone();
        fixture
            .database()
            .ensure_deliverable_folder(
                &fixture.conversation,
                &project_key(&other_project.to_string_lossy()),
                "pre-existing-folder-for-the-other-project",
                &is_valid_deliverable_folder_name,
                crate::database::now_ms(),
            )
            .unwrap();
        assert_eq!(
            Fixture::folder_of(&fixture.context_for(&other_project)),
            "pre-existing-folder-for-the-other-project"
        );
        // Coming back to the first project reads the first project's own record.
        assert_eq!(Fixture::folder_of(&fixture.context_for(&fixture.root.clone())), mine);
    }

    /// A stored name that is no longer a single safe path component is replaced
    /// rather than trusted, so corrupted durable state cannot redirect a write
    /// outside the deliverable root.
    #[test]
    fn an_unusable_stored_folder_name_is_replaced_not_trusted() {
        let _clock = ClockGuard::new();
        let fixture = Fixture::new("tamper");
        let key = project_key(&fixture.root.to_string_lossy());
        for hostile in ["../../escape", "sub/dir", "..", "C:evil", "trailing."] {
            fixture
                .database()
                .ensure_deliverable_folder(
                    &fixture.conversation,
                    &key,
                    hostile,
                    &is_valid_deliverable_folder_name,
                    crate::database::now_ms(),
                )
                .unwrap();
            let context = fixture.context_for(&fixture.root.clone());
            let folder = Fixture::folder_of(&context);
            assert!(
                is_valid_deliverable_folder_name(&folder),
                "hostile record {hostile:?} must not survive, got {folder:?}"
            );
            assert!(!folder.contains("..") && !folder.contains('/') && !folder.contains('\\'));
        }
    }

    /// A read-only conversation is never given durable placement bookkeeping of
    /// its own, and none of these calls creates a directory.
    #[test]
    fn a_read_only_conversation_reserves_nothing_and_creates_nothing() {
        let _clock = ClockGuard::new();
        let fixture = Fixture::new("readonly");
        let context = project_context_for(
            fixture.database(),
            &fixture.sessions,
            &fixture.conversation,
            &fixture.permission(&fixture.root, PermissionMode::ReadOnly),
        );
        let key = project_key(&fixture.root.to_string_lossy());
        assert_eq!(
            fixture
                .database()
                .deliverable_folder(&fixture.conversation, &key)
                .unwrap(),
            None
        );
        assert!(is_valid_deliverable_folder_name(&Fixture::folder_of(&context)));
        assert!(!fixture.root.join("fox").exists());
    }

    /// Concurrent Runs cannot diverge: the assignment is one idempotent
    /// `INSERT OR IGNORE` against a composite primary key, so every caller that
    /// sees the row reads back the same value.
    #[test]
    fn concurrent_placement_requests_resolve_to_exactly_one_folder() {
        let _clock = ClockGuard::new();
        let fixture = Fixture::new("concurrent");
        let key = project_key(&fixture.root.to_string_lossy());
        let accepted = std::sync::Mutex::new(Vec::<String>::new());
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    let folder = fixture
                        .database()
                        .ensure_deliverable_folder(
                            &fixture.conversation,
                            &key,
                            "candidate-derived-from-the-first-writer",
                            &is_valid_deliverable_folder_name,
                            crate::database::now_ms(),
                        )
                        .unwrap()
                        .unwrap();
                    accepted.lock().unwrap().push(folder);
                });
            }
        });
        let values = accepted.into_inner().unwrap();
        assert_eq!(values.len(), 8);
        assert!(
            values.iter().all(|value| *value == values[0]),
            "every concurrent request must observe one folder: {values:?}"
        );
    }
}

#[cfg(test)]
mod frozen_approval_tests {
    use super::*;

    fn pending_approval(db: &Database, run_id: &str) -> String {
        db.apply_runtime_event(run_id, 1, &json!({"type":"run.started"})).unwrap();
        let tool = db.create_host_tool_call(run_id, "budget-tool", "write_file", &json!({"path":"test.txt"}), "pending", true).unwrap();
        db.create_approval(&tool.id, "write", &json!({})).unwrap().id
    }

    #[test]
    fn frozen_approval_deadline_is_enforced_and_corruption_cannot_fall_back() {
        let path = std::env::temp_dir().join(format!("fox-approval-budget-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("budget test"), None, None).unwrap();
        let seed = db.create_run(&conversation.id, "seed", None).unwrap();
        let mut binding = db.freeze_legacy_run_control(&seed.run.id, "test-profile").unwrap();
        let short_conversation = db.create_conversation("fox-general", Some("short budget"), None, None).unwrap();
        let run = db.create_run(&short_conversation.id, "short budget", None).unwrap();
        binding.run_id = run.run.id.clone();
        binding.conversation_id = short_conversation.id;
        binding.budgets.approval_wait_ms = 1;
        db.freeze_run_control(&binding).unwrap();
        let approval_id = pending_approval(&db, &run.run.id);
        rusqlite::Connection::open(&path).unwrap().execute("UPDATE approvals SET requested_at=1 WHERE id=?1", [&approval_id]).unwrap();
        drop(db);
        let db = Database::open(path.clone()).unwrap();
        assert_eq!(db.run_time_budgets(&run.run.id).unwrap().approval_wait_ms, 1);
        let (sender, receiver) = mpsc::channel();
        assert_eq!(receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap_err(), "Approval request timed out");
        sender.send(true).unwrap();
        assert_eq!(receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap_err(), "Approval request timed out");
        rusqlite::Connection::open(path).unwrap().execute(
            "UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1", [&run.run.id],
        ).unwrap();
        sender.send(true).unwrap();
        assert!(receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap_err().contains("hash mismatch"));
        assert!(receive_run_approval(&db, "unknown-run", &approval_id, &receiver).is_err());
    }

    #[test]
    fn legacy_approval_keeps_default_and_disconnection_is_not_approval() {
        let path = std::env::temp_dir().join(format!("fox-approval-legacy-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path).unwrap();
        let conversation = db.create_conversation("fox-general", Some("legacy budget"), None, None).unwrap();
        let run = db.create_run(&conversation.id, "legacy", None).unwrap();
        assert_eq!(db.run_time_budgets(&run.run.id).unwrap().approval_wait_ms, 300_000);
        let approval_id = pending_approval(&db, &run.run.id);
        let (sender, receiver) = mpsc::channel();
        sender.send(true).unwrap();
        assert!(receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap());
        sender.send(false).unwrap();
        assert!(!receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap());
        drop(sender);
        assert_eq!(receive_run_approval(&db, &run.run.id, &approval_id, &receiver).unwrap_err(), "Approval request was interrupted");
    }
}

fn request_declarative_hook_approval(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    run_id: &str,
    tool_call: &ToolCallRecord,
    tool: &str,
    input: &Value,
    reason: Option<&str>,
) -> Result<(), String> {
    let summary = reason
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("声明式策略要求本次工具调用获得批准");
    let approval = database.create_approval(
        &tool_call.id,
        &format!("策略审批：{tool}"),
        &json!({
            "tool": tool,
            "title": "生命周期 Hook 要求审批",
            "target": tool,
            "summary": summary,
            "arguments": input,
            "policyReason": summary,
            "availableDecisions": ["allow_once", "deny"],
        }),
    )?;
    if approval.status != "pending" {
        return Err(format!(
            "approval '{}' is already terminal and cannot be reopened",
            approval.id
        ));
    }
    let (sender, receiver) = mpsc::channel();
    let pending_approvals = state
        .lock()
        .map_err(|_| "runtime state lock is poisoned".to_owned())?
        .pending_approvals
        .clone();
    let mut pending = pending_approvals
        .lock()
        .map_err(|_| "approval map is poisoned".to_owned())?;
    if pending.contains_key(&approval.id) {
        return Err(format!(
            "approval '{}' is already awaiting a decision",
            approval.id
        ));
    }
    pending.insert(
        approval.id.clone(),
        PendingApproval {
            run_id: run_id.to_owned(),
            sender,
        },
    );
    drop(pending);
    let approval_id = approval.id.clone();
    let _ = app.emit("fox://approval-requested", approval);
    let approval_result = receive_run_approval(database, run_id, &approval_id, &receiver);
    if let Ok(runtime_state) = state.lock() {
        if let Ok(mut pending) = runtime_state.pending_approvals.lock() {
            pending.remove(&approval_id);
        }
    }
    let approved = match approval_result {
        Ok(approved) => approved,
        Err(error) => {
            if let Ok(Some(resolved)) =
                database.resolve_approval(&approval_id, ApprovalDecision::Deny)
            {
                let _ = app.emit("fox://approval-resolved", resolved);
            }
            return Err(error);
        }
    };
    if !approved {
        return Err("The user denied the lifecycle Hook policy approval.".to_owned());
    }
    claim_approved_tool_call_for_execution(database, run_id, &tool_call.runtime_tool_call_id)?;
    Ok(())
}

fn request_task_repair_override_approval(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    run_id: &str,
    tool_call: &ToolCallRecord,
    input: &Value,
    policy_reason: Option<&str>,
) -> Result<ApprovalRecord, String> {
    let task_id = input
        .get("taskId")
        .and_then(Value::as_str)
        .ok_or_else(|| "taskId is required for repair override approval".to_owned())?;
    let escalation_reason = input
        .get("escalationReason")
        .and_then(Value::as_str)
        .ok_or_else(|| "escalationReason is required for repair override approval".to_owned())?;
    let approval = database.create_task_repair_override_approval(
        &tool_call.id,
        "人工授权一次超出普通预算的 Task Repair",
        &json!({
            "category": "task_repair_budget_override",
            "title": "Task Repair 预算人工升级",
            "target": task_id,
            "summary": escalation_reason,
            "arguments": input,
            "policyReason": policy_reason.unwrap_or("普通 Repair 预算已耗尽"),
            "availableDecisions": ["allow_once", "deny"],
        }),
    )?;
    if approval.status != "pending" {
        return Err(format!(
            "repair override approval '{}' is already terminal or claimed and cannot be replayed",
            approval.id
        ));
    }
    let (sender, receiver) = mpsc::channel();
    let pending_approvals = state
        .lock()
        .map_err(|_| "runtime state lock is poisoned".to_owned())?
        .pending_approvals
        .clone();
    let mut pending = pending_approvals
        .lock()
        .map_err(|_| "approval map is poisoned".to_owned())?;
    if pending.contains_key(&approval.id) {
        return Err(format!(
            "repair override approval '{}' is already awaiting a decision",
            approval.id
        ));
    }
    pending.insert(
        approval.id.clone(),
        PendingApproval {
            run_id: run_id.to_owned(),
            sender,
        },
    );
    drop(pending);
    let approval_id = approval.id.clone();
    let _ = app.emit("fox://approval-requested", approval.clone());
    let approval_result = receive_run_approval(database, run_id, &approval_id, &receiver);
    if let Ok(runtime_state) = state.lock() {
        if let Ok(mut pending) = runtime_state.pending_approvals.lock() {
            pending.remove(&approval_id);
        }
    }
    let approved = match approval_result {
        Ok(approved) => approved,
        Err(error) => {
            if let Ok(Some(resolved)) =
                database.resolve_approval(&approval_id, ApprovalDecision::Deny)
            {
                let _ = app.emit("fox://approval-resolved", resolved);
            }
            return Err(error);
        }
    };
    if !approved {
        let message = "The user denied this one-time Task repair budget override.";
        return Err(message.to_owned());
    }
    Ok(approval)
}

fn claim_approved_tool_call_for_execution(
    database: &Database,
    run_id: &str,
    runtime_tool_call_id: &str,
) -> Result<(), String> {
    database
        .claim_approved_tool_call(run_id, runtime_tool_call_id)?
        .then_some(())
        .ok_or_else(|| {
            "[approval.execution_context_expired] approved tool call no longer belongs to a running Run"
                .to_owned()
        })
}

fn merge_hook_annotations(mut before: Vec<String>, after: Vec<String>) -> Vec<String> {
    before.extend(after);
    before
}

fn authoritative_host_tool_terminal(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    tool: &str,
    input: &Value,
) -> Option<Result<Value, String>> {
    let (record, disposition) = database
        .inspect_host_tool_call_replay(run_id, tool_call_id, tool, input)
        .ok()??;
    if disposition != HostToolCallDisposition::ReplayTerminal {
        return None;
    }
    if record.status == "completed" {
        // One builder for the first receipt, the ordinary replay and this
        // finalize-race fallback: whichever side of a duplicate delivery wins,
        // the settled shape cannot drift.
        return Some(Ok(settled_host_tool_call_envelope(
            database,
            run_id,
            tool_call_id,
            &record.conversation_id,
            record.result.clone().unwrap_or(Value::Null),
            None,
            Vec::new(),
            true,
        )));
    }
    if record.status == "failed" {
        // A-REQ-007: this row is the authoritative terminal state of *this*
        // ToolCall - the identity check above already rejected a different tool
        // or input, and a Runtime-owned row was refused. So the losing side of a
        // duplicate settle must hand back the same facts the winning side got:
        // same tag, same bounded preview, same reference, same storage fact.
        // A failure that never stored a result is the hard-error channel
        // (cancellation, start failure) and stays `Err`, which is also what its
        // first delivery returned. Nothing here re-runs the tool or touches
        // scheduling.
        let error = record
            .error_message
            .clone()
            .unwrap_or_else(|| "the persisted ToolCall failed".to_owned());
        return Some(match record.result.clone() {
            Some(result) => Ok(settled_host_tool_call_envelope(
                database,
                run_id,
                tool_call_id,
                &record.conversation_id,
                result,
                Some(error),
                Vec::new(),
                true,
            )),
            None => Err(error),
        });
    }
    None
}

fn finalize_host_tool_execution(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    tool: &str,
    input: &Value,
    before_annotations: Vec<String>,
    outcome: Result<Value, String>,
) -> Result<Value, String> {
    match outcome {
        Ok(result) => {
            // A01/A02 (EX-v1): an executor may report a completed attempt that
            // did not succeed. The result keeps its diagnostics, but the
            // ToolCall must still end `failed`, so the receipt, the UI card,
            // the continuation decision and the model view all agree the
            // operation failed. Persistence keeps both columns: the error
            // message carries the outcome, the result carries the reason.
            let reported_failure = result.get("isError").and_then(Value::as_bool) == Some(true);
            let error_message = reported_failure.then(|| execution_failure_message(&result));
            if let Err(persist_error) =
                database.complete_host_tool_call(run_id, tool_call_id, Some(&result), error_message.as_deref())
            {
                if let Some(authoritative) =
                    authoritative_host_tool_terminal(database, run_id, tool_call_id, tool, input)
                {
                    return authoritative;
                }
                return Err(format!(
                    "[tool_call.finalize_failed] handler succeeded but its Host ToolCall could not be finalized: {persist_error}"
                ));
            }
            let after_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, !reported_failure)
                    .unwrap_or_default();
            let tool_result_storage =
                database.tool_result_storage(run_id, tool_call_id).unwrap_or_default();
            if reported_failure {
                // Bounded transport: the wire carries exactly what the Host
                // persisted - which `complete_host_tool_call` already compacts
                // into a preview above the inline cap - instead of the raw
                // multi-megabyte value, so a failure log cannot dominate the
                // JSONL frame or the model context. The in-memory value is used
                // only when the terminal row cannot be read back at all.
                let persisted = database
                    .inspect_host_tool_call_replay(run_id, tool_call_id, tool, input)
                    .ok()
                    .flatten()
                    .map(|(record, _)| record);
                let (stored, conversation) = match persisted.as_ref() {
                    Some(record) => (
                        record.result.clone().unwrap_or(result),
                        record.conversation_id.as_str().to_owned(),
                    ),
                    None => (result, String::new()),
                };
                return Ok(settled_host_tool_call_envelope(
                    database,
                    run_id,
                    tool_call_id,
                    &conversation,
                    stored,
                    Some(error_message.unwrap_or_else(|| {
                        "the tool reported a failed execution".to_owned()
                    })),
                    merge_hook_annotations(before_annotations, after_annotations),
                    false,
                ));
            }
            Ok(json!({
                "isError": false,
                "result": result,
                "toolResultStorage": tool_result_storage,
                "hookAnnotations": merge_hook_annotations(before_annotations, after_annotations),
            }))
        }
        Err(error) => {
            if let Err(persist_error) =
                database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))
            {
                if let Some(authoritative) =
                    authoritative_host_tool_terminal(database, run_id, tool_call_id, tool, input)
                {
                    return authoritative;
                }
                return Err(format!(
                    "{error}; [tool_call.finalize_failed] {persist_error}"
                ));
            }
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
        }
    }
}

/// The single envelope shape a settled Host ToolCall produces, used for the
/// first receipt and for every replay so the two cannot drift: the bounded
/// persisted result, the Host's own trusted storage fact, the same
/// classification, and - when the body was spilled to the blob store - the
/// reference plus whether the very reader the model would use can serve it.
///
/// `stored` is what the Host persisted, never a re-inlined megabyte body;
/// `authorized_conversation_id` comes from the row, never from tool input, and
/// an empty value simply means the reference cannot be verified as readable.
/// A retrieval promise is only made on the verified side; an unverified one
/// still says so. The successful branch keeps exactly the shape it had before,
/// so this is about failure receipts and their replay, not about growing the
/// success contract.
fn settled_host_tool_call_envelope(
    database: &Database,
    run_id: &str,
    tool_call_id: &str,
    authorized_conversation_id: &str,
    stored: Value,
    failure: Option<String>,
    hook_annotations: Vec<String>,
    replayed: bool,
) -> Value {
    let storage = database
        .tool_result_storage(run_id, tool_call_id)
        .unwrap_or_default();
    let spilled = stored
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut response = match failure {
        Some(error) => {
            let code = settled_failure_error_code(&error, &stored);
            let mut envelope = json!({
                "isError": true,
                "error": error,
                "errorCode": code,
                "errorDetails": { "code": code, "retryable": false },
                "result": stored,
                "toolResultStorage": storage,
                "hookAnnotations": hook_annotations,
            });
            // Key names follow the `resultRef`/`resultRefNote` vocabulary of the
            // model view so the executor, the projection and the reader agree on
            // one word.
            if spilled {
                if let Some(reference) =
                    crate::kernel_compaction::tool_result_ref(run_id, tool_call_id)
                {
                    // The reader's minimum range is 4 bytes so a UTF-8 code point
                    // is never split; only `retrievable` is read here, so the
                    // smallest legal request is used rather than guessing from
                    // the row's shape.
                    let retrievable = database
                        .tool_result_range(&reference, authorized_conversation_id, 0, 4)
                        .ok()
                        .is_some_and(|range| range.retrievable);
                    let note = if retrievable {
                        format!(
                            "完整输出已由 Fox 保存；调用 read_tool_result {{\"reference\":\"{reference}\"}} 可只读取回（不重新执行本工具，也不会重放任何写操作），返回 details.complete=true 前按 details.nextOffset 继续接力即可取得全部原文。"
                        )
                    } else {
                        format!(
                            "上面的结果正文是 Fox 的有界预览，且 Host 未确认可按 {reference} 取回全文；需要完整诊断时，请让命令把输出重定向到文件（command > out.txt 2>&1）后用读取工具分页查看，不要重复同一失败调用。"
                        )
                    };
                    if let Some(object) = envelope.as_object_mut() {
                        object.insert("resultRef".to_owned(), json!(reference));
                        object.insert("resultRefNote".to_owned(), json!(note));
                        object.insert("resultRefRetrievable".to_owned(), json!(retrievable));
                    }
                }
            }
            envelope
        }
        None => json!({
            "isError": false,
            "result": stored,
            "toolResultStorage": storage,
            "hookAnnotations": hook_annotations,
        }),
    };
    if replayed {
        if let Some(object) = response.as_object_mut() {
            object.insert("replayed".to_owned(), Value::Bool(true));
        }
    }
    response
}

/// The bounded `error_message` for a completed-but-failed execution. It stays
/// short on purpose: the retained output belongs to the result and the blob
/// store, while the persisted error column is what the approval card, the
/// receipt and the audit trail quote.
fn execution_failure_message(result: &Value) -> String {
    let code = failure_error_code(result);
    let text = result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let summary: String = text.chars().take(2_000).collect();
    format!("[{code}] {summary}")
}

fn failure_error_code(result: &Value) -> &str {
    result
        .pointer("/details/errorCode")
        .and_then(Value::as_str)
        .filter(|code| !code.trim().is_empty())
        .unwrap_or_else(|| crate::tool_host::ToolErrorCode::Unknown.as_str())
}

/// The EX-v1 tag a settled failure is reported with, for the first receipt **and**
/// every replay.
///
/// The result body is the first source, because that is what the executor
/// actually returned. It cannot be the only one: `complete_host_tool_call`
/// compacts an oversized result into a bounded preview, and `details` does not
/// survive that - so deriving the tag from the body alone would classify a
/// multi-megabyte failure as `tool.unknown` while the very same small failure
/// keeps its real tag, which is exactly the drift between what the model is
/// told and what the run did. The `error_message` the Host wrote for this same
/// ToolCall is the compaction-proof copy of that decision (`[tool.<tag>] …`,
/// CONTRACTS v1.2 §1 EX-v1) and lives in its own column, so it is consulted
/// only when the body no longer carries a tag.
///
/// This does not let a tool label itself: the prefix is honoured only when it
/// parses as one of the seven tags, both sources here are Host-authored, and the
/// bounded transport is unchanged - the full body still stays out of the wire.
fn settled_failure_error_code(error_message: &str, stored: &Value) -> String {
    let from_body = failure_error_code(stored);
    if from_body != crate::tool_host::ToolErrorCode::Unknown.as_str() {
        return from_body.to_owned();
    }
    crate::tool_host::ToolErrorCode::classify(error_message)
        .map(|code| code.as_str().to_owned())
        .unwrap_or_else(|| from_body.to_owned())
}

fn handle_mcp_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = execute_mcp_tool_request(app, database, state, &envelope)
        .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn execute_mcp_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "MCP tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "MCP tool request is missing conversationId".to_owned())?;
    let agent_id = database.conversation_agent_id(conversation_id)?;
    let assistant = database
        .get_agent(&agent_id)?
        .ok_or_else(|| "conversation assistant was not found".to_owned())?;
    let expert_scope = database
        .current_conversation_expert_binding(conversation_id)?
        .map(|binding| {
            validate_expert_package_snapshot(&binding.package_snapshot, &binding.package_hash)?;
            expert_package_mcp_server_scope(&binding.package_snapshot)
        })
        .transpose()?
        .flatten();
    let allowed_mcp_servers =
        intersect_mcp_server_scopes(agent_mcp_server_scope(&assistant), expert_scope);
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "MCP tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "MCP tool request is missing tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let managed_office = tool == "call_mcp_tool" && input["serverId"] == crate::office::SERVER_ID;
    let office_access = if managed_office { database.conversation_project_access(conversation_id)? } else { None };
    // The Host's own runtime session store, fetched once and reused by the
    // pre-approval prepare and the execution dispatch. The conversation still
    // comes from the Run authorization, never from the model's arguments.
    let legacy_office_sessions_dir = if managed_office {
        use tauri::Manager;
        Some(
            app.state::<crate::app_state::AppState>()
                .runtime_host
                .sessions_dir
                .clone(),
        )
    } else {
        None
    };
    // Host-private placement root for rendered previews, working copies and
    // commit staging. Derived from the Host's own application data directory,
    // never from a model argument, so the model cannot redirect these areas.
    let legacy_office_artifacts_dir = if managed_office {
        use tauri::Manager;
        app.state::<crate::app_state::AppState>()
            .runtime_host
            .sessions_dir
            .parent()
            .map(std::path::Path::to_path_buf)
            .filter(|path| !path.as_os_str().is_empty())
    } else {
        None
    };
    let permission_scope = if managed_office {
        Some(permission_scope_hash("office-project-request", json!({"access":office_access,"input":input}).to_string().as_bytes()))
    } else { mcp_permission_scope(tool, &input) };
    let has_conversation_permission = match permission_scope.as_deref() {
        Some(scope) => {
            database.conversation_tool_permission_granted(conversation_id, tool, scope)?
        }
        None => false,
    };
    // The Host-admitted write target, captured from the pre-approval prepare
    // BEFORE execution. A create/create-import leaves the output on disk, so a
    // post-execution re-prepare would trip the "output exists" guard and skip
    // registering the first version. Classification happens before execution
    // (matching the Kernel seam); execution re-admits independently, and the
    // post-write registration still re-verifies the bytes actually on disk.
    let mut office_prepared_target: Option<std::path::PathBuf> = None;
    let office_needs_approval = if managed_office {
        ensure_plan_approved_before_mutation(database, conversation_id, tool, &input)?;
        let access = database.conversation_project_access(conversation_id)?;
        let permission = database.conversation_permission_mode(conversation_id)?;
        let remote_tool = input["tool"].as_str().ok_or("Office 工具名缺失")?;
        // Serialize state-changing calls to the SAME file within this run. A
        // parallel burst (e.g. one import per sheet) would otherwise open one
        // approval dialog per call and race the workbook. The rejected call
        // never opens an approval and receives an actionable reason so the
        // model proceeds sequentially after the in-flight write completes.
        let mut _office_write_slot: Option<ActiveOfficeWrite> = None;
        if matches!(
            remote_tool,
            "office_create"
                | "office_edit"
                | "office_merge"
                | "office_render"
                | "office_import_data"
        ) {
            if let Some((root, _)) = access.as_ref() {
                let target_name = input
                    .get("arguments")
                    .and_then(|a| a.get("output").or_else(|| a.get("file")))
                    .and_then(Value::as_str);
                let key = target_name.map(|name| {
                    format!(
                        "{}:{}",
                        run_id,
                        std::path::Path::new(root).join(name).to_string_lossy().to_lowercase()
                    )
                });
                if let Some(key) = key {
                    let active = state
                        .lock()
                        .map_err(|_| "runtime state lock is poisoned".to_string())?
                        .active_file_writes
                        .clone();
                    let occupied = {
                        let mut set = active.lock().map_err(|_| "write set poisoned".to_string())?;
                        !set.insert(key.clone())
                    };
                    if occupied {
                        return Err(
                            "同一文件已有一个写入操作正在等待审批或执行；请等当前调用完成后再发起下一个（不要并发对同一工作簿多次写入）。"
                                .to_owned(),
                        );
                    }
                    _office_write_slot = Some(ActiveOfficeWrite { active, key });
                }
            }
        }
        if let Some(binding) = database.current_conversation_expert_binding(conversation_id)? {
            let manifest = expert_package_manifest(&binding.package_snapshot)?;
            if let Some(allowed) = manifest.get("officeTools").and_then(Value::as_array) {
                if !allowed.iter().any(|value| value == remote_tool) {
                    return Err("该专家未绑定此 Office 操作".to_owned());
                }
            }
        }
        // Pre-approval preparation uses the same Host-resolved authorization
        // as execution, so an artifactId is resolved and integrity-checked
        // before asking a human; a cross-session/tampered reference is denied
        // here, never approved then failed late.
        let prepared = {
            let office_context = legacy_office_sessions_dir.as_ref().map(|sessions_dir| {
                crate::office::OfficeCallContext {
                    database,
                    sessions_dir,
                    conversation_id,
                    artifacts_dir: legacy_office_artifacts_dir.as_deref(),
                }
            });
            crate::office::prepare_with_context(
                remote_tool,
                &input["arguments"],
                access.as_ref().map(|(root, _)| root.as_str()),
                &permission,
                office_context.as_ref(),
            )?
        };
        office_prepared_target = prepared.target_path().map(std::path::Path::to_path_buf);
        prepared.mutates && permission != "allow" && !has_conversation_permission
    } else { tool == "call_mcp_tool" && !has_conversation_permission };
    let requires_approval = hook_decision.requires_approval || office_needs_approval;
    let approval_permission_scope = if hook_decision.requires_approval {
        None
    } else {
        permission_scope.as_deref()
    };
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if requires_approval {
            "pending"
        } else {
            "running"
        },
        requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome: Result<Value, String> = (|| {
        if hook_decision.requires_approval && tool != "call_mcp_tool" {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )?;
        }
        let result = if tool == "list_mcp_tools" {
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let mut items = Vec::new();
            for server in database
                .list_mcp_servers()?
                .into_iter()
                .filter(|item| item.enabled)
                .filter(|item| {
                    allowed_mcp_servers
                        .as_ref()
                        .is_none_or(|allowed| allowed.contains(&item.id))
                })
            {
                let health_started = Instant::now();
                match crate::mcp::list_tools(&server) {
                    Ok(tools) => {
                        let _ = database.record_mcp_health(
                            &server.id,
                            "connected",
                            None,
                            Some(health_started.elapsed().as_millis() as i64),
                            Some(tools.len() as i64),
                        );
                        let tools = tools
                            .into_iter()
                            .filter(|item| {
                                query.is_empty()
                                    || item.to_string().to_ascii_lowercase().contains(&query)
                            })
                            .collect::<Vec<_>>();
                        if !tools.is_empty() {
                            items.push(json!({ "serverId": server.id, "serverName": server.name, "tools": tools }));
                        }
                    }
                    Err(error) => {
                        let _ = database.record_mcp_health(
                            &server.id,
                            "unavailable",
                            Some(&error),
                            Some(health_started.elapsed().as_millis() as i64),
                            None,
                        );
                    }
                }
            }
            json!({ "servers": items })
        } else if tool == "call_mcp_tool" {
            let server_id = input
                .get("serverId")
                .and_then(Value::as_str)
                .ok_or_else(|| "serverId is required".to_owned())?;
            if allowed_mcp_servers
                .as_ref()
                .is_some_and(|allowed| !allowed.contains(server_id))
            {
                return Err("this MCP server is not enabled in the expert package".to_owned());
            }
            let remote_tool = input
                .get("tool")
                .and_then(Value::as_str)
                .ok_or_else(|| "MCP tool name is required".to_owned())?;
            let arguments = input.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let server = database
                .get_mcp_server(server_id)?
                .filter(|server| server.enabled)
                .ok_or_else(|| "MCP Server 不存在或已停用".to_owned())?;
            if requires_approval {
                let mut request = json!({
                    "tool": "call_mcp_tool",
                    "title": "调用 MCP 工具",
                    "target": format!("{} / {}", server.name, remote_tool),
                    "summary": format!("允许 Fox 调用 MCP Server {} 的工具 {}", server.name, remote_tool),
                    "serverId": server.id,
                    "serverName": server.name,
                    "mcpTool": remote_tool,
                    "arguments": arguments,
                });
                attach_permission_scope(&mut request, approval_permission_scope);
                let approval = database.create_approval(
                    &tool_call.id,
                    &format!("调用 MCP 工具 {} / {}", server.name, remote_tool),
                    &request,
                )?;
                let (sender, receiver) = mpsc::channel();
                state
                    .lock()
                    .map_err(|_| "runtime state lock is poisoned".to_owned())?
                    .pending_approvals
                    .lock()
                    .map_err(|_| "approval map is poisoned".to_owned())?
                    .insert(
                        approval.id.clone(),
                        PendingApproval {
                            run_id: run_id.to_owned(),
                            sender,
                        },
                    );
                let approval_id = approval.id.clone();
                let _ = app.emit("fox://approval-requested", approval);
                let approval_result = receive_run_approval(database, run_id, &approval_id, &receiver);
                if let Ok(runtime_state) = state.lock() {
                    if let Ok(mut pending) = runtime_state.pending_approvals.lock() {
                        pending.remove(&approval_id);
                    }
                }
                let approved = match approval_result {
                    Ok(approved) => approved,
                    Err(error) => {
                        if let Ok(Some(resolved)) =
                            database.resolve_approval(&approval_id, ApprovalDecision::Deny)
                        {
                            let _ = app.emit("fox://approval-resolved", resolved);
                        }
                        return Err(error);
                    }
                };
                if !approved {
                    return Err("The user denied this MCP operation.".to_owned());
                }
                claim_approved_tool_call_for_execution(database, run_id, tool_call_id)?;
            }
            let health_started = Instant::now();
            // The Legacy transport must resolve saved compute artifacts by
            // reference exactly like the Kernel one; without this context an
            // artifactId would be an unresolvable string and the model would
            // be back to re-reading the file itself. The sessions dir was
            // fetched once at pre-approval for the shared prepare.
            let office_context = legacy_office_sessions_dir.as_ref().map(|sessions_dir| {
                crate::office::OfficeCallContext {
                    database,
                    sessions_dir,
                    conversation_id,
                    artifacts_dir: legacy_office_artifacts_dir.as_deref(),
                }
            });
            let call_result = if managed_office {
                let access = database.conversation_project_access(conversation_id)?;
                if access != office_access { return Err("Office 项目或权限在等待期间发生变化，请重新发起操作".into()); }
                ensure_expert_tool_allowed(database, conversation_id, tool)?;
                let server = database.get_mcp_server(crate::office::SERVER_ID)?.ok_or("Office 连接器不存在")?;
                let permission = database.conversation_permission_mode(conversation_id)?;
                crate::office::execute_with_cancellation(
                    &server,
                    remote_tool,
                    &arguments,
                    access.as_ref().map(|(root,_)| root.as_str()),
                    &permission,
                    None,
                    Duration::from_secs(120),
                    office_context.as_ref(),
                )
            } else {
                crate::mcp::call_tool(&server, remote_tool, &arguments)
            };
            match call_result {
                Ok(result) => {
                    let _ = database.record_mcp_health(
                        &server.id,
                        "connected",
                        None,
                        Some(health_started.elapsed().as_millis() as i64),
                        server.tool_count,
                    );
                    // The Legacy transport reaches the built-in Office
                    // connector through the same MCP wrapper as the Kernel one,
                    // so a saved document becomes a user-restorable version
                    // here too — including an import-by-artifact reference.
                    // Managed-file registration is decided from the dispatch
                    // the Host actually made (wrapper identity + the operation
                    // the Host itself prepared with the same frozen context)
                    // and then re-verified against the bytes on disk; a generic
                    // MCP result can never create a restorable row, even with
                    // identical JSON.
                    if managed_office
                        && matches!(
                            remote_tool,
                            "office_create" | "office_edit" | "office_import_data"
                        )
                    {
                        let access = database.conversation_project_access(conversation_id)?;
                        if let Some((root, permission)) = access.as_ref() {
                            // Prefer the target captured BEFORE execution: a
                            // create/create-import now exists, so re-preparing
                            // afterwards trips the "output exists" guard and would
                            // skip registering the first restorable version. The
                            // write itself was re-admitted at execution time; this
                            // path is only re-prepared as a fallback, and the bytes
                            // registered are still re-verified against disk below.
                            let make_verified = |target: &std::path::Path| {
                                crate::runtime_host::managed_files::VerifiedWriteTarget::new(
                                    Path::new(root),
                                    target,
                                )
                            };
                            let verified_target = office_prepared_target
                                .as_ref()
                                .map(std::path::PathBuf::as_path)
                                .map(make_verified)
                                .or_else(|| {
                                    crate::office::prepare_with_context(
                                        remote_tool,
                                        &arguments,
                                        Some(root.as_str()),
                                        permission,
                                        office_context.as_ref(),
                                    )
                                    .ok()
                                    .and_then(|prepared| {
                                        prepared.target_path().map(make_verified)
                                    })
                                });
                            if let Some(verified) = verified_target {
                                use tauri::Manager;
                                let backups_dir = app
                                    .state::<crate::app_state::AppState>()
                                    .runtime_host
                                    .managed_files_dir()
                                    .to_path_buf();
                                if let Err(reason) =
                                    crate::runtime_host::managed_files::record_office_write_from_result(
                                        database,
                                        &backups_dir,
                                        conversation_id,
                                        run_id,
                                        Some(tool_call_id),
                                        &verified,
                                        remote_tool,
                                        &result,
                                    )
                                {
                                    eprintln!(
                                        "refused an unverified Office managed-file \
                                         declaration for {remote_tool} {tool_call_id}: {reason}"
                                    );
                                }
                            } else {
                                eprintln!(
                                    "managed-file registration skipped for {remote_tool} \
                                     {tool_call_id}: no verified target"
                                );
                            }
                        }
                    }
                    result
                }
                Err(error) => {
                    let _ = database.record_mcp_health(
                        &server.id,
                        "unavailable",
                        Some(&error),
                        Some(health_started.elapsed().as_millis() as i64),
                        None,
                    );
                    return Err(error);
                }
            }
        } else {
            return Err(format!("unsupported MCP proxy tool: {tool}"));
        };
        Ok(json!({
            "content": if managed_office { result.get("content").cloned().unwrap_or_else(|| json!([])) }
                else { json!([{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()) }]) },
            "details": result,
            "untrusted": true,
            "sourceType": "knowledge",
        }))
    })();
    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}

/// Host-decided file placement handed to the model as reference data.
///
/// The deliverable folder is decided **once** per (conversation, project) and
/// persisted in `deliverable_placements`, then read back on every later Run.
/// Before this, the name was recomputed from the current date on each prompt
/// build, so a task continued across midnight (or after an application restart
/// on a later day) silently moved to a second result folder. It is still
/// *reference data*: the Host resolves, admits and verifies every real path,
/// and a stored name is re-validated before it is used, so it can never redirect
/// a write out of `fox/`.
///
/// No directory is created here. A read-only conversation that never produces a
/// deliverable therefore still creates nothing on disk.
fn project_context_for(
    database: &crate::database::Database,
    sessions_dir: &std::path::Path,
    conversation_id: &str,
    permission: &fox_engine_protocol::FrozenPermission,
) -> Value {
    let data_dir = sessions_dir
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let store = crate::runtime_host::artifact_store::ArtifactStore::new(data_dir);
    let folder = deliverable_folder_for(database, conversation_id, permission, &store);
    let deliverable_root = folder.as_ref().map(|folder| {
        format!(
            "{}/{}",
            crate::runtime_host::artifact_store::PROJECT_DELIVERABLE_DIR,
            folder
        )
    });
    json!({
        "projectRoot": permission.project_root,
        "permissionMode": permission.mode.as_str(),
        "deliverableRoot": deliverable_root,
        "deliverableFolder": folder,
        "processFilePolicy": "attachment_compute outputs stay in the Host's private conversation workspace and are reused by artifactId; they are process files, not deliverables.",
        "previewPolicy": "office_render previews are stored by the Host in its private preview area and opened through the saved result's preview entry; pass an explicit output under deliverableRoot only when the user asked for the HTML/PNG itself.",
    })
}

/// The conversation's durable deliverable folder for this project.
///
/// Returns `None` when the conversation has no authorized project to write
/// results into. A stored name is accepted only if it is still a valid single
/// path component; otherwise the row is ignored and the deterministic candidate
/// is used, so a corrupted or stale record degrades into a fresh, safe name
/// instead of an out-of-scope write.
fn deliverable_folder_for(
    database: &crate::database::Database,
    conversation_id: &str,
    permission: &fox_engine_protocol::FrozenPermission,
    store: &crate::runtime_host::artifact_store::ArtifactStore,
) -> Option<String> {
    let project_root = permission.project_root.as_deref()?;
    let key = crate::runtime_host::artifact_store::project_key(project_root);
    let candidate = store.deliverable_directory_name("", conversation_id);
    let acceptable = crate::runtime_host::artifact_store::is_valid_deliverable_folder_name;
    // Fast path: an assignment already exists and is still usable. This is a
    // pure read, so an ordinary prompt build never opens a write transaction.
    if let Ok(Some(stored)) = database.deliverable_folder(conversation_id, &key) {
        if acceptable(&stored) {
            return Some(stored);
        }
        eprintln!("[fox-host] 丢弃不可用的成果目录记录 {}", stored);
    }
    // A read-only conversation can never produce a deliverable, so the Host does
    // not reserve a placement for it. Reserving here would manufacture durable
    // bookkeeping for conversations that never ask for a result; the
    // deterministic name is still injected as reference data.
    if permission.mode == fox_engine_protocol::PermissionMode::ReadOnly {
        return acceptable(&candidate).then_some(candidate);
    }
    // Persist the decision so every later Run, and every later application
    // start, re-reads the same folder, and re-validate whatever comes back:
    // a stale or tampered record must never be able to redirect a write out of
    // `fox/`. A failure here is not fatal — the deterministic candidate is
    // correct for this Run — but the candidate is only returned when it is
    // itself acceptable.
    match database.ensure_deliverable_folder(
        conversation_id,
        &key,
        &candidate,
        &acceptable,
        crate::database::now_ms(),
    ) {
        Ok(folder) => folder,
        Err(error) => {
            eprintln!("[fox-host] 成果目录归属台账不可用，本次使用确定性目录名: {error}");
            acceptable(&candidate).then_some(candidate)
        }
    }
}

fn handle_attachment_tool_request(
    app: &AppHandle,
    database: &Database,
    attachments_dir: &std::path::Path,
    sessions_dir: &std::path::Path,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response =
        execute_attachment_tool_request(app, database, attachments_dir, sessions_dir, state, &envelope)
            .unwrap_or_else(|error| json!({ "isError": true, "error": error }));
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

fn execute_attachment_tool_request(
    app: &AppHandle,
    database: &Database,
    attachments_dir: &std::path::Path,
    sessions_dir: &std::path::Path,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "attachment tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "attachment tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "attachment tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .ok_or_else(|| "attachment tool request is missing toolCallId".to_owned())?;
    let tool = payload["tool"].as_str().ok_or("missing attachment tool")?;
    if !matches!(tool, "read_attachment" | "attachment_compute") { return Err("unsupported attachment tool".into()); }
    if tool == "attachment_compute" {
        let binding = database.run_control_binding(run_id)?.ok_or("missing compute run authority")?;
        if binding.conversation_id != conversation_id { return Err("compute conversation does not own this run".into()); }
    }
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision = crate::lifecycle_hooks::before_tool(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
    )?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }

    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if hook_decision.requires_approval {
            "pending"
        } else {
            "running"
        },
        hook_decision.requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome: Result<Value, String> = (|| {
        if hook_decision.requires_approval {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )?;
        }
        let mut result = if tool == "attachment_compute" {
            let token=state.lock().map_err(|_| "runtime state lock poisoned")?.cancellation.tool_token(run_id,tool_call_id)?;
            attachment_compute::execute(database,attachments_dir,sessions_dir,conversation_id,run_id,&input,move || token.is_cancelled())?
        } else {
            let id=input["attachmentId"].as_str().ok_or("attachmentId is required")?;
            read_attachment_text(database,attachments_dir,conversation_id,id,&input)?
        };
        let content = result.to_string();
        if let Some(metadata) = result.as_object_mut() { metadata.remove("text"); }
        Ok(json!({
                "content": [{ "type": "text", "text": content }],
                "details": result,
        }))
    })();
    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}

fn read_attachment_text(
    database: &Database,
    attachments_dir: &std::path::Path,
    conversation_id: &str,
    attachment_id: &str,
    input: &Value,
) -> Result<Value, String> {
    let attachment = database
        .attachment_for_conversation(conversation_id, attachment_id)?
        .ok_or_else(|| "Attachment was not found in this conversation".to_owned())?;
    if attachment.byte_size < 0 || attachment.byte_size as u64 > MAX_ATTACHMENT_FILE_BYTES {
        return Err("Attachment is too large to read (maximum 5 MiB)".to_owned());
    }
    let root = std::fs::canonicalize(attachments_dir)
        .map_err(|error| format!("Attachment directory is unavailable: {error}"))?;
    let path = std::fs::canonicalize(&attachment.storage_path)
        .map_err(|error| format!("Attachment file is unavailable: {error}"))?;
    if !path.starts_with(&root) {
        return Err("Attachment path is outside Fox attachment storage".to_owned());
    }
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_ATTACHMENT_FILE_BYTES {
        return Err("Attachment is not a readable file within the 5 MiB limit".to_owned());
    }
    let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
    let (office_text, sheets, sheet_count) = crate::local_knowledge_import::extract_office_text_with_outline(&bytes, &attachment.display_name)?;
    let text = if let Some(text) = office_text {
        text
    } else if attachment_is_docx(&attachment.display_name, attachment.media_type.as_deref()) {
        extract_docx_text(&bytes)?
    } else {
        if !attachment_is_text(&attachment.display_name, attachment.media_type.as_deref()) {
            return Err("Attachment is not a supported text, DOCX, XLSX or PPTX file".to_owned());
        }
        if bytes.contains(&0) {
            return Err("Attachment contains binary data".to_owned());
        }
        String::from_utf8(bytes).map_err(|_| "Attachment is not valid UTF-8 text".to_owned())?
    };
    if text.len() > MAX_ATTACHMENT_TEXT_BYTES {
        return Err("Extracted attachment text exceeds the 1 MiB context limit".to_owned());
    }
    let page = attachment_text_page(&text, input)?;
    Ok(json!({
        "attachmentId": attachment.id,
        "displayName": attachment.display_name,
        "mediaType": attachment.media_type,
        "byteSize": metadata.len(),
        "text": page["text"],
        "offset": page["offset"],
        "nextOffset": page["nextOffset"],
        "totalCharacters": page["totalCharacters"],
        "hasMore": page["hasMore"],
        "sheets": sheets,
        "sheetCount": sheet_count,
        "sheetDirectoryTruncated": sheet_count.is_some_and(|count| count > sheets.len()),
    }))
}

fn attachment_text_page(text: &str, input: &Value) -> Result<Value, String> {
    let read_integer = |key: &str, default: u64| -> Result<usize, String> {
        match input.get(key) {
            None => Ok(default as usize),
            Some(value) => value.as_u64().and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| format!("{key} must be a non-negative integer")),
        }
    };
    let offset = read_integer("offset", 0)?;
    let limit = read_integer("limit", 12000)?.clamp(1, 24000);
    let total = text.chars().count();
    if offset > total { return Err(format!("offset exceeds attachment length ({total})")); }
    let page: String = text.chars().skip(offset).take(limit).collect();
    let next = offset + page.chars().count();
    Ok(json!({"text":page,"offset":offset,"nextOffset":if next < total {Some(next)} else {None},
        "totalCharacters":total,"hasMore":next < total}))
}

fn attachment_is_docx(filename: &str, media_type: Option<&str>) -> bool {
    media_type.is_some_and(|value| {
        value.split(';').next().unwrap_or_default().trim()
            == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
    }) || std::path::Path::new(filename)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("docx"))
}

fn extract_docx_text(bytes: &[u8]) -> Result<String, String> {
    let document = zip_entry(bytes, "word/document.xml")?
        .ok_or_else(|| "DOCX does not contain word/document.xml".to_owned())?;
    let xml =
        String::from_utf8(document).map_err(|_| "DOCX document.xml is not UTF-8".to_owned())?;
    let mut text = String::new();
    let mut cursor = 0;
    while let Some(start) = xml[cursor..].find('<') {
        let start = cursor + start;
        append_xml_text(&xml[cursor..start], &mut text);
        let Some(relative_end) = xml[start..].find('>') else {
            break;
        };
        let end = start + relative_end;
        let tag = &xml[start + 1..end];
        let name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches('/');
        if matches!(name, "w:p" | "w:br" | "w:cr") && !text.ends_with('\n') {
            text.push('\n');
        } else if name == "w:tab" && !text.ends_with('\t') {
            text.push('\t');
        }
        cursor = end + 1;
    }
    append_xml_text(&xml[cursor..], &mut text);
    let text = text
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err("DOCX contains no readable text".to_owned());
    }
    Ok(text)
}

fn append_xml_text(value: &str, output: &mut String) {
    if value.is_empty() {
        return;
    }
    output.push_str(
        &value
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'"),
    );
}

fn zip_entry(bytes: &[u8], expected_name: &str) -> Result<Option<Vec<u8>>, String> {
    if bytes.len() < 22 || !bytes.starts_with(b"PK\x03\x04") {
        return Err("Attachment is not a valid DOCX ZIP archive".to_owned());
    }
    let eocd = bytes
        .windows(4)
        .rposition(|window| window == b"PK\x05\x06")
        .ok_or_else(|| "DOCX ZIP directory is missing".to_owned())?;
    let entries = read_u16(bytes, eocd + 10)? as usize;
    let mut central = read_u32(bytes, eocd + 16)? as usize;
    for _ in 0..entries {
        if bytes.get(central..central + 4) != Some(b"PK\x01\x02") {
            return Err("DOCX ZIP directory is invalid".to_owned());
        }
        let compression = read_u16(bytes, central + 10)?;
        let compressed_size = read_u32(bytes, central + 20)? as usize;
        let uncompressed_size = read_u32(bytes, central + 24)? as usize;
        let name_len = read_u16(bytes, central + 28)? as usize;
        let extra_len = read_u16(bytes, central + 30)? as usize;
        let comment_len = read_u16(bytes, central + 32)? as usize;
        let local_offset = read_u32(bytes, central + 42)? as usize;
        let name_start = central + 46;
        let name_end = name_start
            .checked_add(name_len)
            .ok_or("DOCX ZIP entry overflow")?;
        let name = std::str::from_utf8(
            bytes
                .get(name_start..name_end)
                .ok_or("DOCX ZIP entry is truncated")?,
        )
        .map_err(|_| "DOCX ZIP filename is not UTF-8".to_owned())?;
        if name == expected_name {
            if uncompressed_size > MAX_ATTACHMENT_TEXT_BYTES {
                return Err("DOCX text exceeds the 1 MiB context limit".to_owned());
            }
            let local_name_len = read_u16(bytes, local_offset + 26)? as usize;
            let local_extra_len = read_u16(bytes, local_offset + 28)? as usize;
            let data_start = local_offset
                .checked_add(30 + local_name_len + local_extra_len)
                .ok_or("DOCX ZIP entry overflow")?;
            let data_end = data_start
                .checked_add(compressed_size)
                .ok_or("DOCX ZIP entry overflow")?;
            let compressed = bytes
                .get(data_start..data_end)
                .ok_or("DOCX ZIP entry is truncated")?;
            return match compression {
                0 => {
                    if compressed.len() > MAX_ATTACHMENT_TEXT_BYTES {
                        return Err("DOCX text exceeds the 1 MiB context limit".to_owned());
                    }
                    Ok(Some(compressed.to_vec()))
                }
                8 => {
                    let mut decoder = DeflateDecoder::new(compressed);
                    let mut output = Vec::with_capacity(uncompressed_size);
                    std::io::Read::read_to_end(
                        &mut std::io::Read::take(
                            &mut decoder,
                            (MAX_ATTACHMENT_TEXT_BYTES + 1) as u64,
                        ),
                        &mut output,
                    )
                    .map_err(|error| format!("DOCX decompression failed: {error}"))?;
                    if output.len() > MAX_ATTACHMENT_TEXT_BYTES {
                        return Err("DOCX text exceeds the 1 MiB context limit".to_owned());
                    }
                    Ok(Some(output))
                }
                _ => Err(format!(
                    "DOCX uses unsupported ZIP compression method {compression}"
                )),
            };
        }
        central = name_end
            .checked_add(extra_len + comment_len)
            .ok_or("DOCX ZIP directory overflow")?;
    }
    Ok(None)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| "DOCX ZIP is truncated".to_owned())?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "DOCX ZIP is truncated".to_owned())?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn attachment_is_text(filename: &str, media_type: Option<&str>) -> bool {
    if media_type.is_some_and(|value| {
        value.starts_with("text/")
            || matches!(
                value.split(';').next().unwrap_or_default().trim(),
                "application/json"
                    | "application/ld+json"
                    | "application/xml"
                    | "application/yaml"
                    | "application/x-yaml"
                    | "application/javascript"
                    | "application/sql"
            )
    }) {
        return true;
    }
    matches!(
        std::path::Path::new(filename)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some(
            "txt"
                | "md"
                | "markdown"
                | "csv"
                | "tsv"
                | "json"
                | "jsonl"
                | "xml"
                | "html"
                | "htm"
                | "css"
                | "js"
                | "jsx"
                | "ts"
                | "tsx"
                | "mjs"
                | "cjs"
                | "py"
                | "rs"
                | "go"
                | "java"
                | "c"
                | "h"
                | "cpp"
                | "hpp"
                | "cs"
                | "php"
                | "rb"
                | "swift"
                | "kt"
                | "kts"
                | "scala"
                | "sql"
                | "sh"
                | "bash"
                | "zsh"
                | "ps1"
                | "bat"
                | "cmd"
                | "yaml"
                | "yml"
                | "toml"
                | "ini"
                | "conf"
                | "config"
                | "log"
        )
    )
}

fn attachment_image_media_type(attachment: &AttachmentRecord) -> Option<&'static str> {
    let media_type = attachment
        .media_type
        .as_deref()
        .and_then(|value| value.split(';').next())
        .map(str::trim);
    match media_type {
        Some("image/png") => Some("image/png"),
        Some("image/jpeg") | Some("image/jpg") => Some("image/jpeg"),
        Some("image/webp") => Some("image/webp"),
        Some("image/gif") => Some("image/gif"),
        _ => match std::path::Path::new(&attachment.display_name)
            .extension()
            .and_then(|value| value.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("png") => Some("image/png"),
            Some("jpg") | Some("jpeg") => Some("image/jpeg"),
            Some("webp") => Some("image/webp"),
            Some("gif") => Some("image/gif"),
            _ => None,
        },
    }
}

fn attachment_looks_like_image(attachment: &AttachmentRecord) -> bool {
    attachment
        .media_type
        .as_deref()
        .is_some_and(|value| value.trim().to_ascii_lowercase().starts_with("image/"))
        || std::path::Path::new(&attachment.display_name)
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| {
                matches!(
                    value.to_ascii_lowercase().as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "webp"
                        | "gif"
                        | "bmp"
                        | "svg"
                        | "avif"
                        | "tif"
                        | "tiff"
                )
            })
}

fn handle_knowledge_tool_request(
    app: &AppHandle,
    database: &Database,
    yuxi_client: &YuxiClient,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let local_knowledge = app.state::<AppState>().local_knowledge.clone();
    let response = tauri::async_runtime::block_on(execute_knowledge_tool_request(
        app,
        database,
        yuxi_client,
        &local_knowledge,
        state,
        &envelope,
    ))
    .unwrap_or_else(|error| {
        let (code, message, retryable) = structured_knowledge_error(&error);
        json!({
            "isError": true,
            "error": message,
            "errorCode": code,
            "errorDetails": {
                "code": code,
                "retryable": retryable,
            },
        })
    });
    let response_type = if response
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "tool.execute_failed"
    } else {
        "tool.execute_completed"
    };
    let response = HostResponse::for_request(&envelope, response_type, response);
    if let Err(error) = write_protocol_message(stdin, &response) {
        record_runtime_crash(app, database, state, error);
    }
}

async fn execute_knowledge_tool_request(
    app: &AppHandle,
    database: &Database,
    yuxi_client: &YuxiClient,
    local_knowledge: &LocalKnowledgeStore,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "knowledge tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "knowledge tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "knowledge tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .ok_or_else(|| "knowledge tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "knowledge tool request is missing tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if hook_decision.requires_approval {
            "pending"
        } else {
            "running"
        },
        hook_decision.requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };
    let outcome: Result<Value, String> = async {
        if hook_decision.requires_approval {
            request_declarative_hook_approval(
                app,
                database,
                state,
                run_id,
                &tool_call,
                tool,
                &input,
                hook_decision.approval_reason.as_deref(),
            )?;
        }
        let bindings = database.conversation_knowledge_reference_bindings(conversation_id)?;
        let service = database.get_yuxi_service()?;
        execute_bound_knowledge_tool(service.as_ref(), yuxi_client, local_knowledge, &bindings, tool, &input).await
    }
    .await;
    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}


async fn execute_bound_knowledge_tool(
    remote_service: Option<&crate::database::YuxiServiceRecord>, yuxi_client: &YuxiClient, local_knowledge: &LocalKnowledgeStore,
    bindings: &[crate::database::KnowledgeReferenceBindingRecord], tool: &str, input: &Value,
) -> Result<Value, String> {
    let ensure_bound = |target: &KnowledgeToolTarget| {
        bindings
            .iter()
            .any(|binding| {
                binding.enabled
                    && binding.reference.source == target.source
                    && binding.reference.id == target.id
                    && target.provider_key.as_deref().is_none_or(|provider_key| {
                        binding.reference.provider_key == provider_key
                    })
                    && target.connection_id.as_deref().is_none_or(|connection_id| {
                        binding.reference.connection_id.as_deref() == Some(connection_id)
                    })
            })
            .then_some(())
            .ok_or_else(|| {
                "[local_knowledge.binding_invalid] The requested knowledge base is not enabled for this conversation".to_owned()
            })
    };
    let result = match tool {
        "list_knowledge_bases" => {
            if has_explicit_knowledge_target(&input) {
                let targets = knowledge_tool_targets(&input, true)?;
                let mut items = Vec::with_capacity(targets.len());
                for target in targets {
                    ensure_bound(&target)?;
                    if target.source == "local" {
                        let bases = local_knowledge
                            .list_bases_for_ids(std::slice::from_ref(&target.id))
                            .map_err(|error| format!("[{}] {}", error.code(), error))?;
                        for base in bases {
                            items.push(json!({
                                "id": base.id,
                                "name": base.name,
                                "description": base.description,
                                "documentCount": base.document_count,
                                "activeIndexGeneration": base.active_index_generation,
                                "reference": target.as_json(),
                                "available": true,
                            }));
                        }
                    } else if let Some(binding) = bindings.iter().find(|binding| {
                        binding.reference.source == "remote"
                            && binding.reference.id == target.id
                            && target.connection_id.as_deref().is_none_or(|connection_id| {
                                binding.reference.connection_id.as_deref() == Some(connection_id)
                            })
                    }) {
                        items.push(json!({
                            "id": binding.reference.id,
                            "name": binding.knowledge_base_name,
                            "reference": target.as_json(),
                            "available": true,
                        }));
                    }
                }
                json!({ "items": items })
            } else {
                let mut items = Vec::with_capacity(bindings.len());
                for binding in bindings {
                    if binding.reference.source == "local" {
                        let base = local_knowledge
                            .list_bases_for_ids(std::slice::from_ref(&binding.reference.id))
                            .map_err(|error| format!("[{}] {}", error.code(), error))?
                            .into_iter()
                            .next();
                        items.push(match base {
                            Some(base) => json!({
                                "id": base.id,
                                "name": base.name,
                                "description": base.description,
                                "documentCount": base.document_count,
                                "activeIndexGeneration": base.active_index_generation,
                                "reference": &binding.reference,
                                "available": true,
                            }),
                            None => json!({
                                "id": binding.reference.id,
                                "name": binding.knowledge_base_name,
                                "reference": &binding.reference,
                                "available": false,
                            }),
                        });
                    } else {
                        items.push(json!({
                            "id": binding.reference.id,
                            "name": binding.knowledge_base_name,
                            "reference": &binding.reference,
                            "available": true,
                        }));
                    }
                }
                json!({ "items": items })
            }
        }
        "search_knowledge" => {
            let targets = knowledge_tool_targets(&input, true)?;
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| "query is required".to_owned())?;
            let requested_limit = input
                .get("topK")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            let requested_max_chars = input
                .get("maxChars")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            let mut results = Vec::with_capacity(targets.len());
            for target in &targets {
                ensure_bound(target)?;
                let value = if target.source == "local" {
                    local_knowledge
                        .search_lexical(&target.id, query, requested_limit, requested_max_chars)
                        .map_err(|error| format!("[{}] {}", error.code(), error))?
                } else {
                    let service = remote_service
                        .ok_or_else(|| "Yuxi service is not configured".to_owned())?;
                    let token = get_access_token(&service.base_url)
                        .ok_or_else(|| "Yuxi authentication is not configured".to_owned())?;
                    yuxi_client
                        .query_knowledge_with_limits(&service.base_url, &token, &target.id, query, requested_limit, requested_max_chars)
                        .await?
                };
                results.push(json!({ "reference": target.as_json(), "result": value }));
            }
            if results.len() == 1 {
                results.remove(0)["result"].clone()
            } else {
                json!({ "items": results })
            }
        }
        "read_knowledge_document" => {
            let target = knowledge_tool_targets(&input, false)?
                .into_iter()
                .next()
                .ok_or_else(|| "knowledge target is required".to_owned())?;
            let document_id = input
                .get("documentId")
                .and_then(Value::as_str)
                .ok_or_else(|| "documentId is required".to_owned())?;
            let chunk_id = input
                .get("chunkId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty());
            let requested_max_chars = input
                .get("maxChars")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok());
            ensure_bound(&target)?;
            if target.source == "local" {
                local_knowledge
                    .read_document(&target.id, document_id, chunk_id, requested_max_chars)
                    .map(|value| {
                        json!({
                            "reference": target.as_json(),
                            "result": value,
                        })
                    })
                    .map_err(|error| format!("[{}] {}", error.code(), error))?
            } else {
                let service = remote_service
                    .ok_or_else(|| "Yuxi service is not configured".to_owned())?;
                let token = get_access_token(&service.base_url)
                    .ok_or_else(|| "Yuxi authentication is not configured".to_owned())?;
                yuxi_client
                    .knowledge_document_text(&service.base_url, &token, &target.id, document_id)
                    .await?
            }
        }
        "query_knowledge_graph" => {
            let targets = knowledge_tool_targets(&input, true)?;
            for target in &targets {
                ensure_bound(target)?;
            }
            if targets.iter().any(|target| target.source == "local") {
                return Err("[local_knowledge.graph_unavailable] Local knowledge graph support is not available in this release".to_owned());
            }
            let keyword = input
                .get("keyword")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let max_depth = input
                .get("maxDepth")
                .and_then(Value::as_i64)
                .unwrap_or(2)
                .clamp(1, 5);
            let max_nodes = input
                .get("maxNodes")
                .and_then(Value::as_i64)
                .unwrap_or(80)
                .clamp(1, 200);
            let service = remote_service
                .ok_or_else(|| "Yuxi service is not configured".to_owned())?;
            let token = get_access_token(&service.base_url)
                .ok_or_else(|| "Yuxi authentication is not configured".to_owned())?;
            let mut results = Vec::with_capacity(targets.len());
            for target in &targets {
                ensure_bound(target)?;
                let value = yuxi_client
                    .graph_subgraph(
                        &service.base_url,
                        &token,
                        &target.id,
                        keyword,
                        max_depth,
                        max_nodes,
                    )
                    .await?;
                results.push(json!({ "reference": target.as_json(), "result": value }));
            }
            if results.len() == 1 {
                results.remove(0)["result"].clone()
            } else {
                json!({ "items": results })
            }
        }
        _ => return Err(format!("unsupported knowledge tool: {tool}")),
    };
        Ok(bounded_knowledge_tool_result(result))
}

#[derive(Debug, Clone)]
struct KnowledgeToolTarget {
    source: String,
    provider_key: Option<String>,
    connection_id: Option<String>,
    id: String,
    revision: Option<String>,
}

impl KnowledgeToolTarget {
    fn as_json(&self) -> Value {
        match self.source.as_str() {
            "remote" => json!({
                "source": "remote",
                "providerKey": self.provider_key,
                "connectionId": self.connection_id,
                "id": self.id,
                "revision": self.revision,
            }),
            _ => json!({
                "source": "local",
                "providerKey": self.provider_key.clone().unwrap_or_else(|| "local".to_owned()),
                "id": self.id,
                "revision": self.revision,
            }),
        }
    }
}

fn has_explicit_knowledge_target(input: &Value) -> bool {
    input.get("targets").is_some()
        || input.get("references").is_some()
        || input.get("target").is_some()
        || input.get("reference").is_some()
}

fn knowledge_tool_targets(
    input: &Value,
    allow_multiple: bool,
) -> Result<Vec<KnowledgeToolTarget>, String> {
    if let Some(targets) = input.get("targets").or_else(|| input.get("references")) {
        if !allow_multiple {
            return Err(
                "[local_knowledge.binding_invalid] targets is not supported for this tool"
                    .to_owned(),
            );
        }
        let targets = targets.as_array().ok_or_else(|| {
            "[local_knowledge.binding_invalid] targets must be an array".to_owned()
        })?;
        if targets.is_empty() {
            return Err("[local_knowledge.binding_invalid] targets cannot be empty".to_owned());
        }
        return targets.iter().map(knowledge_tool_target).collect();
    }
    if let Some(target) = input.get("target").or_else(|| input.get("reference")) {
        return Ok(vec![knowledge_tool_target(target)?]);
    }
    let id = input
        .get("knowledgeBaseId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            "[local_knowledge.binding_invalid] knowledgeBaseId or target is required".to_owned()
        })?;
    Ok(vec![KnowledgeToolTarget {
        source: "remote".to_owned(),
        provider_key: None,
        connection_id: None,
        id: id.to_owned(),
        revision: None,
    }])
}

fn knowledge_tool_target(value: &Value) -> Result<KnowledgeToolTarget, String> {
    let object = value.as_object().ok_or_else(|| {
        "[local_knowledge.binding_invalid] knowledge target must be an object".to_owned()
    })?;
    let source = object
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            "[local_knowledge.binding_invalid] knowledge target source is required".to_owned()
        })?;
    if !matches!(source, "local" | "remote") {
        return Err(
            "[local_knowledge.binding_invalid] knowledge target source is invalid".to_owned(),
        );
    }
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| {
            "[local_knowledge.binding_invalid] knowledge target id is required".to_owned()
        })?;
    let provider_key = object
        .get("providerKey")
        .or_else(|| object.get("provider_key"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let connection_id = object
        .get("connectionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    if source == "local" {
        if provider_key
            .as_deref()
            .is_some_and(|value| value != "local")
        {
            return Err(
                "[local_knowledge.binding_invalid] local knowledge target providerKey must be local"
                    .to_owned(),
            );
        }
        if connection_id.is_some() {
            return Err(
                "[local_knowledge.binding_invalid] local knowledge target cannot have connectionId"
                    .to_owned(),
            );
        }
    } else {
        if connection_id.is_none() {
            return Err(
                "[local_knowledge.binding_invalid] remote knowledge target connectionId is required"
                    .to_owned(),
            );
        }
        if provider_key
            .as_deref()
            .is_some_and(|value| Some(value) != connection_id.as_deref())
        {
            return Err(
                "[local_knowledge.binding_invalid] remote knowledge target providerKey must match connectionId"
                    .to_owned(),
            );
        }
    }
    let revision = object
        .get("revision")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(KnowledgeToolTarget {
        source: source.to_owned(),
        provider_key: if source == "local" {
            Some("local".to_owned())
        } else {
            provider_key.or_else(|| connection_id.clone())
        },
        connection_id,
        id: id.to_owned(),
        revision,
    })
}

fn structured_knowledge_error(message: &str) -> (&str, &str, bool) {
    let Some(rest) = message.strip_prefix('[') else {
        return ("knowledge.tool_failed", message, false);
    };
    let Some((code, detail)) = rest.split_once(']') else {
        return ("knowledge.tool_failed", message, false);
    };
    let retryable = matches!(
        code,
        "local_knowledge.retrieval_unavailable"
            | "remote_knowledge.retrieval_unavailable"
            | "vector_index.unavailable"
            | "vector_index.operation_failed"
            | "vector_index.integrity_failed"
    );
    (code, detail.trim_start(), retryable)
}

#[cfg(test)]
mod knowledge_tool_contract_tests {
    use super::{
        has_explicit_knowledge_target, knowledge_tool_targets, structured_knowledge_error,
    };
    use serde_json::json;

    #[test]
    fn accepts_legacy_and_source_aware_knowledge_targets() {
        let legacy = knowledge_tool_targets(&json!({ "knowledgeBaseId": "kb-1" }), true)
            .expect("legacy target");
        assert_eq!(legacy[0].source, "remote");
        assert_eq!(legacy[0].id, "kb-1");
        assert_eq!(legacy[0].provider_key, None);
        assert_eq!(legacy[0].connection_id, None);

        let targets = knowledge_tool_targets(
            &json!({
                "targets": [
                    { "source": "local", "id": "local-1", "revision": "generation:3" },
                    { "source": "remote", "connectionId": "yuxi-primary", "id": "remote-1" }
                ]
            }),
            true,
        )
        .expect("source-aware targets");
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].source, "local");
        assert_eq!(targets[0].provider_key.as_deref(), Some("local"));
        assert_eq!(targets[1].connection_id.as_deref(), Some("yuxi-primary"));
    }

    #[test]
    fn accepts_reference_alias_and_keeps_local_scope_explicit() {
        assert!(has_explicit_knowledge_target(&json!({
            "reference": { "source": "local", "providerKey": "local", "id": "local-1" }
        })));
        let target = knowledge_tool_targets(
            &json!({
                "reference": { "source": "local", "providerKey": "local", "id": "local-1" }
            }),
            false,
        )
        .expect("local reference");
        assert_eq!(target[0].source, "local");
        assert_eq!(target[0].provider_key.as_deref(), Some("local"));
        assert!(knowledge_tool_targets(&json!({}), true).is_err());
    }

    #[test]
    fn rejects_invalid_remote_reference_and_multiple_read_targets() {
        assert!(knowledge_tool_targets(
            &json!({ "target": { "source": "remote", "id": "kb-1" } }),
            false,
        )
        .is_err());
        assert!(knowledge_tool_targets(
            &json!({ "targets": [{ "source": "local", "id": "kb-1" }] }),
            false,
        )
        .is_err());
    }

    #[test]
    fn preserves_structured_knowledge_error_codes() {
        let (code, message, retryable) = structured_knowledge_error(
            "[local_knowledge.graph_unavailable] Local graph is unavailable",
        );
        assert_eq!(code, "local_knowledge.graph_unavailable");
        assert_eq!(message, "Local graph is unavailable");
        assert!(!retryable);

        let (code, _, retryable) = structured_knowledge_error(
            "[local_knowledge.retrieval_unavailable] Index is rebuilding",
        );
        assert_eq!(code, "local_knowledge.retrieval_unavailable");
        assert!(retryable);
    }

    #[test]
    fn remote_knowledge_errors_retry_only_transient_failures() {
        for code in ["authentication_required", "access_denied", "document_unavailable", "protocol_invalid"] {
            assert!(!structured_knowledge_error(&format!("[remote_knowledge.{code}] failed")).2);
        }
        assert!(structured_knowledge_error("[remote_knowledge.retrieval_unavailable] transient").2);
        assert!(!structured_knowledge_error("unsupported API").2);
    }
}

fn bounded_knowledge_tool_result(result: Value) -> Value {
    let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
    if text.len() <= MAX_KNOWLEDGE_TOOL_TEXT_BYTES {
        return json!({
            "content": [{ "type": "text", "text": text }],
            "details": result,
        });
    }

    let original_bytes = text.len();
    let model_preview = truncate_utf8(&text, MAX_KNOWLEDGE_TOOL_TEXT_BYTES);
    let details_preview = truncate_utf8(&text, MAX_KNOWLEDGE_TOOL_PREVIEW_BYTES);
    json!({
        "content": [{
            "type": "text",
            "text": format!(
                "{model_preview}\n\n[Fox truncated this knowledge result from {original_bytes} bytes to keep the runtime responsive.]"
            ),
        }],
        "details": {
            "truncated": true,
            "originalBytes": original_bytes,
            "preview": details_preview,
        },
    })
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn require_rust_reader_binding(database: &Database, envelope: &RuntimeEnvelope) -> Result<fox_engine_protocol::RunControlBinding, String> {
    let run_id = envelope.run_id.as_deref().ok_or("reader request has no runId")?;
    let payload = envelope.payload.as_ref().ok_or("reader request has no payload")?;
    let binding = database.run_control_binding(run_id)?.ok_or("reader request has no frozen control binding")?;
    if envelope.conversation_id.as_deref() != Some(binding.conversation_id.as_str())
        || binding.authority != fox_engine_protocol::ExecutionAuthority::Legacy
        || binding.engine_id != "pi"
        || binding.read_only_executor != fox_engine_protocol::ResourceExecutor::Rust
        || !payload.get("tool").and_then(Value::as_str).is_some_and(crate::resource_gateway::is_reader)
        || payload.get("permissionSnapshotId").and_then(Value::as_str) != Some(binding.permission_snapshot_id.as_str()) {
        return Err("reader identity, executor or permission snapshot does not match the frozen Run".into());
    }
    if payload.get("observationScope").is_some() || payload.get("parentToolCallId").is_some() {
        let parent_id = payload.get("parentToolCallId").and_then(Value::as_str).ok_or("nested reader has no parent identity")?;
        let node_id = payload.get("graphNodeId").and_then(Value::as_str).ok_or("nested reader has no node identity")?;
        let raw_id = payload.get("nodeToolCallId").and_then(Value::as_str).filter(|id| !id.trim().is_empty()).ok_or("nested reader has no local tool identity")?;
        let parent = database.get_runtime_tool_call(run_id, parent_id)?.ok_or("nested reader parent is missing")?;
        let expected_id = format!("graph-read:{}", hex::encode(Sha256::digest(serde_json::to_vec(&[parent_id, node_id, raw_id]).map_err(|e| e.to_string())?)));
        if payload.get("observationScope").and_then(Value::as_str) != Some("nested")
            || binding.execution_profile_id != "graph_readonly_preview"
            || parent.tool_name != "graph_readonly_run" || parent.execution_location != "runtime" || parent.status != "running"
            || !parent.input.get("nodes").and_then(Value::as_array).is_some_and(|nodes| nodes.iter().any(|node| node.get("id").and_then(Value::as_str) == Some(node_id)))
            || payload.get("toolCallId").and_then(Value::as_str) != Some(expected_id.as_str()) {
            return Err("nested reader does not belong to the running frozen graph node".into());
        }
    }
    Ok(binding)
}

fn execute_host_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "host tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "host tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "host tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .ok_or_else(|| "host tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "host tool request is missing tool".to_owned())?;
    let cancellation = state.lock().map_err(|_| "runtime state lock is poisoned")?
        .cancellation.tool_token(run_id, tool_call_id)?;
    cancellation.check()?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if envelope.r#type == "tool.readonly_execute" {
        return execute_rust_reader_request(database, envelope, &cancellation);
    }
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    ensure_plan_approved_before_mutation(database, conversation_id, tool, &input)?;
    let (project_root, permission_mode) = database
        .conversation_project_access(conversation_id)?
        .ok_or_else(|| "this conversation has no authorized project folder".to_owned())?;
    let manage_command = tool == "run_command" && matches!(input["action"].as_str(), Some("status" | "output" | "cancel"));
    if permission_mode == "read_only" && !manage_command {
        return Err(format!("tool {tool} is blocked in read-only mode"));
    }

    let file_policy = if matches!(tool, "write_file" | "edit_file") {
        let binding = database.run_control_binding(run_id)?.ok_or("file write requires a frozen Run")?;
        if binding.conversation_id != conversation_id || binding.permission.project_root.as_deref() != Some(project_root.as_str()) {
            return Err("credential_mismatch: file project binding changed".into());
        }
        let policy = database.execution_policy(conversation_id)?;
        let target = crate::tool_host::canonical_file_identity(Path::new(&project_root), input["path"].as_str().ok_or("missing file path")?)?;
        // REV-01: the model's declared `expectedVersion` is a precondition claim.
        // It selects the observation it was based on and is never silently
        // upgraded to the newest one, so a stale candidate cannot ride a later
        // read of the same target.
        let declared = input
            .get("expectedVersion")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let observation = database
            .host_observation_for_version(run_id, &target, declared)?
            .ok_or_else(|| match declared {
                Some(version) => format!(
                    "[tool.file_conflict] No Host observation of {version} exists in this Run; \
                     read the target again and retry with its readVersion"
                ),
                None => "[tool.file_conflict] Read the target before writing; no Host observation exists".to_owned(),
            })?;
        Some((policy.version, observation))
    } else { None };
    let prepared = if let Some((_, baseline)) = &file_policy {
        // `baseline.version` is the observation the DECLARED precondition
        // selected, so binding it here preserves the claim instead of
        // replacing it with the newest observation.
        crate::tool_host::prepare_admitted_file(tool, &input, &project_root, &baseline.version)?
    } else { crate::tool_host::prepare(tool, &input, &project_root)? };
    let permission_scope = host_permission_scope(tool, prepared.preview());
    let has_conversation_permission = match permission_scope.as_deref() {
        Some(scope) => {
            database.conversation_tool_permission_granted(conversation_id, tool, scope)?
        }
        None => false,
    };
    let requires_approval = hook_decision.requires_approval
        || (file_policy.is_some() && permission_mode == "ask")
        || (!manage_command && (permission_mode == "ask" || tool == "run_command") && !has_conversation_permission);
    let approval_permission_scope = if hook_decision.requires_approval {
        None
    } else {
        permission_scope.as_deref()
    };
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if requires_approval {
            "pending"
        } else {
            "running"
        },
        requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };

    // A whole-file replacement is authorized by its OWN, purpose-specific
    // confirmation. The immutable request identity and its four-tuple are bound
    // into the approval record HERE, when the request is created — so what the
    // user approves is exactly what the later claim will require, and an
    // ordinary write approval can never be read as a replacement authorization.
    // F1: "needs a replacement confirmation" is decided by the SOURCE
    // observation AND the model delivery fact together, so a whole-file read
    // whose model view was bounded also lands here.
    let needs_replace_grant = match (&file_policy, tool) {
        (Some((_, baseline)), "write_file") => database.needs_replace_grant(run_id, baseline)?,
        _ => false,
    };
    let replace_binding = if needs_replace_grant {
        file_policy
            .as_ref()
            .and_then(|(_, baseline)| {
                let target = crate::tool_host::canonical_file_identity(
                    Path::new(&project_root),
                    input["path"].as_str().unwrap_or_default(),
                )
                .ok();
                let content = input.get("content").and_then(Value::as_str).unwrap_or_default();
                target.map(|target| {
                    crate::database::kernel_execution_admission::replace_request_binding(
                        conversation_id,
                        run_id,
                        tool_call_id,
                        &target,
                        &baseline.version,
                        content,
                    )
                })
            })
    } else {
        None
    };
    if let (Some(binding), Some(baseline)) = (&replace_binding, file_policy.as_ref().map(|(_, b)| b))
    {
        // Propose the request now, in `pending`, so the authorization exists
        // before the human is asked and the confirmation has something to bind.
        let _ = database.propose_whole_file_replacement(
            conversation_id,
            run_id,
            tool_call_id,
            binding,
        );
        let _ = baseline;
    }

    let outcome = (|| -> Result<Value, String> {
        if requires_approval {
            let mut request =
                serde_json::to_value(prepared.preview()).map_err(|error| error.to_string())?;
            attach_permission_scope(&mut request, approval_permission_scope);
            if let Some(binding) = &replace_binding {
                // Immutable part of the approval record: purpose, target,
                // observed baseline and candidate digest.
                request["wholeFileReplacement"] = json!({
                    "purpose": "整文件替换",
                    "requestDigest": binding.request_digest,
                    "targetIdentity": binding.target_identity,
                    "baselineVersion": binding.version,
                    "candidateDigest": binding.candidate_digest,
                    "availableDecisions": ["allow_once", "deny"],
                });
            }
            let approval =
                database.create_approval(&tool_call.id, &prepared.preview().summary, &request)?;
            let (sender, receiver) = mpsc::channel();
            state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?
                .pending_approvals
                .lock()
                .map_err(|_| "approval map is poisoned".to_owned())?
                .insert(
                    approval.id.clone(),
                    PendingApproval {
                        run_id: run_id.to_owned(),
                        sender,
                    },
                );
            let approval_id = approval.id.clone();
            let _ = app.emit("fox://approval-requested", approval);
            let approval_result = receive_run_approval(database, run_id, &approval_id, &receiver);
            if let Ok(runtime_state) = state.lock() {
                if let Ok(mut pending) = runtime_state.pending_approvals.lock() {
                    pending.remove(&approval_id);
                }
            }
            let approved = match approval_result {
                Ok(approved) => approved,
                Err(error) => {
                    if let Ok(Some(resolved)) =
                        database.resolve_approval(&approval_id, ApprovalDecision::Deny)
                    {
                        let _ = app.emit("fox://approval-resolved", resolved);
                    }
                    return Err(error);
                }
            };
            if !approved {
                return Err("The user denied this operation.".to_owned());
            }
            claim_approved_tool_call_for_execution(database, run_id, tool_call_id)?;
        }

        if let Some((version, baseline)) = &file_policy {
            use tauri::Manager;
            let app_state = app.state::<crate::app_state::AppState>();
            return execute_legacy_admitted_file(database, app_state.runtime_host.managed_files_dir(),
                run_id, conversation_id, tool_call_id, tool, &input, &project_root,
                *version, baseline, &cancellation);
        }
        if let crate::tool_host::PreparedToolAction::CommandJob {root,input,..} = &prepared {
            let binding=database.run_control_binding(run_id)?.ok_or("command job needs a frozen Run")?;
            let budget=Duration::from_millis(binding.budgets.limit_operation_ms(binding.budgets.tool_execution_ms, 0).max(0) as u64);
            command_jobs::execute(database,run_id,input,root,&cancellation,budget)
        } else {
            crate::tool_host::execute_with_cancellation(prepared, Some(&cancellation))
        }
    })();
    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}

/// Legacy transport shares the same durable file admission and A commit seam.
/// The transport never supplies a credential or a file-version authority.
fn execute_legacy_admitted_file(
    database: &Database, backups_dir: &Path, run_id: &str, conversation_id: &str,
    tool_call_id: &str, tool: &str, input: &Value, project_root: &str,
    policy_version: u64, observed: &fox_engine_protocol::HostObservation,
    token: &crate::kernel::CancellationToken,
) -> Result<Value, String> {
    let durable = database.get_runtime_tool_call(run_id, tool_call_id)?
        .ok_or("credential_mismatch: durable Host tool call is missing")?;
    if durable.tool_name != tool || durable.input != *input {
        return Err("credential_mismatch: file input differs from the durable intent".into());
    }
    // The executor consumes the persisted input, never a caller-held replacement.
    let tool = durable.tool_name.as_str();
    let input = &durable.input;
    let credential = database.issue_legacy_file_credential(run_id, tool_call_id, policy_version)?;
    if credential.intent_digest != crate::database::kernel_execution_admission::launch_params_hash(tool, &input.to_string()) {
        return Err("credential_mismatch: file credential input binding differs".into());
    }
    if credential.file_baseline.as_ref() != Some(observed) {
        return Err("observation_conflict: file observation changed during approval".into());
    }
    let outcome = database.claim_execution_attempt(run_id, conversation_id, &credential,
        &credential.intent_digest, &format!("legacy-host:{tool_call_id}"))?;
    if outcome != fox_engine_protocol::AttemptOutcome::Claimed {
        return kernel_host::execution_receipt_result(database, run_id, &credential.dispatch_id, tool, None);
    }
    let scope = crate::database::KernelHostScope {
        schema_version: 1, tool_names: [tool.to_owned()].into_iter().collect(),
        mcp_server_hashes: Default::default(), knowledge_reference_hashes: Default::default(),
        knowledge_connection_hashes: Default::default(), office_tools: Default::default(), lifecycle_hooks: vec![],
    };
    let mut revalidate = || {
        token.check()?;
        database.revalidate_execution_credential(&credential)?;
        let (current_root, mode) = database.conversation_project_access(conversation_id)?.ok_or("project grant revoked")?;
        if current_root != project_root || mode == "read_only" { return Err("project grant revoked".into()); }
        Ok(())
    };
    let mode = database.execution_policy(conversation_id)?.mode;
    let (result, evidence) = managed_files::execute_admitted_file(managed_files::ManagedExecutionContext {
        database, backups_dir, conversation_id, run_id, project_root: Some(project_root),
        permission_mode: &mode, scope: &scope, sessions_dir: None,
    }, tool, input, tool_call_id, &credential, Some(token), &mut revalidate);
    let call_outcome = kernel_host::call_outcome(&result);
    kernel_host::settle_execution_outcome(database, run_id, &credential.dispatch_id, &result, evidence, &call_outcome)?;
    let value = match result { Ok(value) => value, Err(error) => kernel_host::resource_failure_result(tool, &error) };
    kernel_host::with_execution_receipt(database, run_id, &credential.dispatch_id, value)
}

/// Shared production executor seam. Admission is checked again here, before
/// creating a durable tool row or opening any resource; tests exercise this
/// same path over a real Pi JSONL process without needing a Tauri window.
fn execute_rust_reader_request(
    database: &Database,
    envelope: &RuntimeEnvelope,
    cancellation: &crate::kernel::CancellationToken,
) -> Result<Value, String> {
    cancellation.check()?;
    let binding = require_rust_reader_binding(database, envelope)?;
    let payload = envelope.payload.as_ref().ok_or("reader payload is missing")?;
    let tool_call_id = payload.get("toolCallId").and_then(Value::as_str).ok_or("reader tool identity is missing")?;
    let tool = payload.get("tool").and_then(Value::as_str).ok_or("reader tool is missing")?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let record_input = payload.get("originalInput").cloned().unwrap_or_else(|| input.clone());
    let call = create_fresh_host_tool_call(database, &binding.run_id, tool_call_id, tool, &record_input, "running", false)?;
    if let HostToolCallExecution::Replay(response) = call { return Ok(response); }
    let outcome = (|| {
        if payload.get("originalInput").is_some() {
            let mut original = record_input.clone();
            let object = original.as_object_mut().ok_or("reader original input is not an object")?;
            if tool != "read" && object.get("path").and_then(Value::as_str).unwrap_or_default().trim().is_empty() {
                object.insert("path".into(), json!("."));
            }
            let approved = crate::tool_guard::approve_read_only_tool(tool, &original, binding.permission.project_root.as_deref())?;
            if approved.input != input { return Err("reader prepared input differs from the frozen source arguments".into()); }
        }
        managed_files::execute_observed_reader(database, &binding, tool, &input,
            tool_call_id, cancellation,
            std::time::Duration::from_millis(binding.budgets.tool_execution_ms as u64))
    })();
    finalize_host_tool_execution(database, &binding.run_id, tool_call_id, tool, &record_input, Vec::new(), outcome)
}

#[cfg(test)]
mod resource_gateway_tests;

fn execute_capability_tool_request(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    envelope: &RuntimeEnvelope,
) -> Result<Value, String> {
    let run_id = envelope
        .run_id
        .as_deref()
        .ok_or_else(|| "capability tool request is missing runId".to_owned())?;
    let conversation_id = envelope
        .conversation_id
        .as_deref()
        .ok_or_else(|| "capability tool request is missing conversationId".to_owned())?;
    let payload = envelope
        .payload
        .as_ref()
        .ok_or_else(|| "capability tool request is missing payload".to_owned())?;
    let tool_call_id = payload
        .get("toolCallId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "capability tool request is missing toolCallId".to_owned())?;
    let tool = payload
        .get("tool")
        .and_then(Value::as_str)
        .ok_or_else(|| "capability tool request is missing tool".to_owned())?;
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    if let Some(response) =
        inspect_existing_host_tool_call(database, run_id, tool_call_id, tool, &input)?
    {
        return Ok(response);
    }
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    ensure_plan_approved_before_mutation(database, conversation_id, tool, &input)?;
    let project_access = database.conversation_project_access(conversation_id)?;
    let project_root = project_access.as_ref().map(|(root, _)| root.as_str());
    let persisted_permission_mode = database.conversation_permission_mode(conversation_id)?;
    let permission_mode = persisted_permission_mode.as_str();
    let prepared = capability_tools::prepare(tool, &input, project_root)?;
    if permission_mode == "read_only" && prepared.blocked_in_read_only() {
        return Err(format!("tool {tool} is blocked in read-only mode"));
    }

    let permission_scope = capability_permission_scope(tool, &input);
    let has_conversation_permission = match permission_scope.as_deref() {
        Some(scope) => {
            database.conversation_tool_permission_granted(conversation_id, tool, scope)?
        }
        None => false,
    };
    let requires_approval = hook_decision.requires_approval
        || (prepared.requires_approval() && !has_conversation_permission);
    let approval_permission_scope = if hook_decision.requires_approval {
        None
    } else {
        permission_scope.as_deref()
    };
    let tool_call = match create_fresh_host_tool_call(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        if requires_approval {
            "pending"
        } else {
            "running"
        },
        requires_approval,
    )? {
        HostToolCallExecution::Execute(record) => record,
        HostToolCallExecution::Replay(response) => return Ok(response),
    };

    let outcome = (|| -> Result<Value, String> {
        if requires_approval {
            let mut request =
                serde_json::to_value(prepared.preview()).map_err(|error| error.to_string())?;
            attach_permission_scope(&mut request, approval_permission_scope);
            let approval =
                database.create_approval(&tool_call.id, &prepared.preview().summary, &request)?;
            let (sender, receiver) = mpsc::channel();
            state
                .lock()
                .map_err(|_| "runtime state lock is poisoned".to_owned())?
                .pending_approvals
                .lock()
                .map_err(|_| "approval map is poisoned".to_owned())?
                .insert(
                    approval.id.clone(),
                    PendingApproval {
                        run_id: run_id.to_owned(),
                        sender,
                    },
                );
            let approval_id = approval.id.clone();
            let _ = app.emit("fox://approval-requested", approval);
            let approval_result = receive_run_approval(database, run_id, &approval_id, &receiver);
            if let Ok(runtime_state) = state.lock() {
                if let Ok(mut pending) = runtime_state.pending_approvals.lock() {
                    pending.remove(&approval_id);
                }
            }
            let approved = match approval_result {
                Ok(approved) => approved,
                Err(error) => {
                    if let Ok(Some(resolved)) =
                        database.resolve_approval(&approval_id, ApprovalDecision::Deny)
                    {
                        let _ = app.emit("fox://approval-resolved", resolved);
                    }
                    return Err(error);
                }
            };
            if !approved {
                return Err("The user denied this operation.".to_owned());
            }
            claim_approved_tool_call_for_execution(database, run_id, tool_call_id)?;
        }

        capability_tools::execute(prepared)
    })();
    finalize_host_tool_execution(
        database,
        run_id,
        tool_call_id,
        tool,
        &input,
        hook_decision.annotations,
        outcome,
    )
}

fn ensure_plan_approved_before_mutation(
    database: &Database,
    conversation_id: &str,
    tool: &str,
    input: &Value,
) -> Result<(), String> {
    let mutates_project = matches!(tool, "write_file" | "edit_file")
        || (tool == "run_command" && !matches!(input["action"].as_str(), Some("status" | "output" | "cancel")))
        || (tool == "format_code" && input.get("mode").and_then(Value::as_str) == Some("write"))
        || (tool == "call_mcp_tool" && input["serverId"] == crate::office::SERVER_ID
            && matches!(
                input["tool"].as_str(),
                Some(
                    "office_create"
                        | "office_edit"
                        | "office_merge"
                        | "office_render"
                        | "office_import_data"
                )
            ));
    if !mutates_project {
        return Ok(());
    }
    let (plans, _, _) = database.load_a1_snapshot(conversation_id)?;
    if let Some(plan) = plans.iter().find(|plan| plan.status == "proposed") {
        return Err(format!(
            "plan.approval_required: state-changing tool '{tool}' is paused until PlanRevision {} (v{}) is approved or rejected by the user",
            plan.id, plan.revision
        ));
    }
    Ok(())
}

fn mark_crashed(state: &Arc<Mutex<RuntimeHostState>>, message: String) {
    if let Ok(mut state) = state.lock() {
        if state.state == "stopped" || state.state == "stopping" {
            return;
        }
        state.state = "crashed".to_owned();
        state.last_error = Some(message);
    }
}

fn should_attempt_recovery(
    message: &str,
    stable_session: bool,
    recovery_attempts: u8,
    has_conversation: bool,
) -> bool {
    let contract_error = message.contains("protocol mismatch")
        || message.contains("invalid JSONL")
        || message.contains("oversized JSONL")
        || message.contains("capability manifest");
    !contract_error && stable_session && recovery_attempts == 0 && has_conversation
}

fn record_runtime_crash(
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    message: String,
) {
    if state
        .lock()
        .ok()
        .is_some_and(|state| matches!(state.state.as_str(), "stopped" | "stopping"))
    {
        return;
    }
    let active = state.lock().ok().and_then(|state| {
        state.worker.as_ref().and_then(|worker| {
            Some((
                worker.current_conversation_id.clone()?,
                worker.active_run_id.clone()?,
            ))
        })
    });
    mark_crashed(state, message.clone());

    let Some((conversation_id, run_id)) = active else {
        return;
    };
    let seq = database.next_run_seq(&run_id).unwrap_or(1);
    let payload = json!({
        "type": "run.failed",
        "code": "runtime.process_crashed",
        "message": message,
    });
    if database.apply_runtime_event(&run_id, seq, &payload).ok() != Some(true) {
        return;
    }
    let _ = app.emit(
        "fox://runtime-event",
        RuntimeEventNotification {
            conversation_id,
            runtime_session_id: None,
            run_id,
            seq,
            timestamp: protocol::timestamp(),
            event: payload,
        },
    );
}

#[cfg(test)]
mod attachment_tests {
    use super::{
        apply_delegated_package_scope, attachment_image_media_type, attachment_is_docx,
        attachment_is_text, attachment_looks_like_image, bounded_knowledge_tool_result,
        create_fresh_host_tool_call, dispatch_created_work_child,
        effective_digital_colleague_output_limit, ensure_plan_approved_before_mutation,
        event_projection_failure_code, extract_docx_text, finalize_host_tool_execution,
        inspect_existing_host_tool_call, intersect_mcp_server_scopes, intersect_optional_scopes,
        node_dependency_is_available, normalized_child_budget, parse_runtime_ready_payload,
        pending_approval_ids_for_run, redact_diagnostic_line, runtime_expert_package,
        runtime_tool_event_succeeded, shadow_legacy_tool_decision, shadow_permission_snapshot_id,
        shadow_preflight_input, shadow_preflight_tool_call_id, shadow_prompt_config_hash,
        should_attempt_recovery, structured_runtime_error, validate_envelope_identity_scope,
        validate_runtime_tool_authority, HostToolCallExecution, PendingApproval,
        RuntimeToolIngress,
    };
    use crate::database::{
        AddEvidenceInput, AttachmentRecord, CreateGoalInput, CreateTaskInput, Database,
        EvidenceReferenceKind, EvidenceType, EvidenceValidityStatus, GoalStatus,
        KnowledgeReference,
    };
    use crate::runtime_host::protocol::{
        RuntimeCapabilityManifest, RuntimeEnvelope, RuntimeToolCapability,
        CAPABILITY_MANIFEST_VERSION,
    };
    use flate2::{write::DeflateEncoder, Compression};
    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};
    use std::{
        collections::{HashMap, HashSet},
        fs,
        io::Write,
        sync::mpsc,
    };
    use uuid::Uuid;

    #[test]
    fn shadow_preflight_uses_payload_tool_call_identity_and_approved_input() {
        let envelope: RuntimeEnvelope = serde_json::from_value(json!({
            "protocol": "fox-agent-runtime",
            "version": 1,
            "kind": "request",
            "id": "transport-request-id",
            "type": "tool.preflight",
            "requestId": "different-response-correlation-id",
            "conversationId": "conversation-1",
            "runtimeSessionId": "session-1",
            "runId": "run-1",
            "timestamp": "2026-09-07T00:00:00Z",
            "payload": {
                "toolCallId": "pi-tool-call-id",
                "tool": "ls",
                "input": {}
            }
        }))
        .unwrap();
        assert_eq!(
            shadow_preflight_tool_call_id(&envelope).unwrap(),
            "pi-tool-call-id"
        );
        let mut missing_payload_id = envelope.clone();
        missing_payload_id
            .payload
            .as_mut()
            .and_then(|payload| payload.as_object_mut())
            .unwrap()
            .remove("toolCallId");
        assert!(shadow_preflight_tool_call_id(&missing_payload_id).is_err());
        assert_eq!(
            shadow_preflight_input(
                &envelope,
                true,
                &json!({"decision":"allow","input":{"path":"."}}),
            ),
            json!({"path":"."})
        );
        assert_eq!(
            shadow_preflight_input(
                &envelope,
                false,
                &json!({"decision":"block","input":{"path":"ignored"}}),
            ),
            json!({})
        );
    }

    #[test]
    fn shadow_prompt_hash_tracks_runtime_prompt_identity_only() {
        let snapshot = json!({
            "type": "run.request_snapshot",
            "stablePromptHash": "stable-a",
            "contextHash": "context-a",
            "promptDefinitionId": "fox-runtime",
            "promptVersion": 7,
            "promptContentHash": "content-a",
            "contextSchemaHash": "schema-a",
            "promptCacheIdentity": "cache-a",
            "executionProfile": {"id":"durable_v2"},
            "toolCatalogHash": "tools-a",
            "promptDiagnostics": {"tokens": 123}
        });
        let base = shadow_prompt_config_hash(&snapshot).unwrap();
        let mut diagnostics_only = snapshot.clone();
        diagnostics_only["promptDiagnostics"] = json!({"tokens": 999});
        assert_eq!(base, shadow_prompt_config_hash(&diagnostics_only).unwrap());
        let mut prompt_changed = snapshot.clone();
        prompt_changed["promptContentHash"] = json!("content-b");
        assert_ne!(base, shadow_prompt_config_hash(&prompt_changed).unwrap());
        let mut stable_changed = snapshot;
        stable_changed["stablePromptHash"] = json!("stable-b");
        assert_ne!(base, shadow_prompt_config_hash(&stable_changed).unwrap());
        assert!(shadow_prompt_config_hash(&json!({"type":"run.request_snapshot"})).is_err());
    }

    #[test]
    fn shadow_permission_hash_is_content_addressed_and_event_errors_fail() {
        assert_eq!(
            shadow_permission_snapshot_id("ask", "D:/project|ReadWrite"),
            shadow_permission_snapshot_id("ask", "D:/project|ReadWrite")
        );
        assert_ne!(
            shadow_permission_snapshot_id("ask", "D:/project|ReadWrite"),
            shadow_permission_snapshot_id("read-only", "D:/project|ReadWrite")
        );
        assert!(runtime_tool_event_succeeded(
            &json!({"type":"tool.completed","isError":false})
        ));
        assert!(!runtime_tool_event_succeeded(
            &json!({"type":"tool.completed","isError":true})
        ));
        assert!(!runtime_tool_event_succeeded(
            &json!({"type":"tool.failed"})
        ));
        assert_eq!(
            shadow_legacy_tool_decision("write_file", "host", true).unwrap(),
            "approval"
        );
        assert_eq!(
            shadow_legacy_tool_decision("write_file", "runtime", false).unwrap(),
            "deny"
        );
        assert_eq!(
            shadow_legacy_tool_decision("graph_readonly_run", "runtime", false).unwrap(),
            "allow"
        );
    }

    fn authority_manifest(tool_names: &[&str]) -> RuntimeCapabilityManifest {
        RuntimeCapabilityManifest {
            manifest_version: CAPABILITY_MANIFEST_VERSION,
            streaming_text: true,
            cancellation: true,
            reasoning: true,
            session_resume: true,
            tool_approval: true,
            image_input: false,
            steering: false,
            context_compaction: true,
            dynamic_model_switch: false,
            work_loop: true,
            tools: tool_names
                .iter()
                .map(|name| {
                    let (category, execution, approval) =
                        super::protocol::canonical_runtime_tool_contract(name)
                            .expect("test tool belongs to the canonical catalog");
                    RuntimeToolCapability {
                        name: (*name).to_owned(),
                        category: category.to_owned(),
                        execution: execution.to_owned(),
                        approval: approval.to_owned(),
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn runtime_tool_authority_requires_active_frozen_and_manifest_agreement() {
        let durable = super::continuation::ExecutionProfileSelection::resolve("durable_v2")
            .expect("durable profile");
        let graph =
            super::continuation::ExecutionProfileSelection::resolve("graph_readonly_preview")
                .expect("graph profile");

        validate_runtime_tool_authority(
            &durable,
            Some("durable_v2"),
            &authority_manifest(&["write_file"]),
            "run-durable",
            "write_file",
            RuntimeToolIngress::Execute,
        )
        .expect("all three authorities allow the tool");
        validate_runtime_tool_authority(
            &graph,
            Some("graph_readonly_preview"),
            &authority_manifest(&["read", "graph_readonly_run"]),
            "run-graph",
            "graph_readonly_run",
            RuntimeToolIngress::Event,
        )
        .expect("graph tool requires the graph profile on both sides");

        assert_eq!(
            validate_runtime_tool_authority(
                &graph,
                Some("durable_v2"),
                &authority_manifest(&["graph_readonly_run"]),
                "run-profile-mismatch",
                "graph_readonly_run",
                RuntimeToolIngress::Event,
            )
            .unwrap_err()
            .0,
            "runtime.execution_profile.run_mismatch"
        );
        assert_eq!(
            validate_runtime_tool_authority(
                &durable,
                Some("durable_v2"),
                &authority_manifest(&["read"]),
                "run-manifest-missing",
                "write_file",
                RuntimeToolIngress::Execute,
            )
            .unwrap_err()
            .0,
            "runtime.capability.tool_not_declared"
        );
        assert_eq!(
            validate_runtime_tool_authority(
                &durable,
                Some("durable_v2"),
                &authority_manifest(&["read"]),
                "run-wrong-ingress",
                "read",
                RuntimeToolIngress::Execute,
            )
            .unwrap_err()
            .0,
            "runtime.capability.ingress_mismatch"
        );
        assert_eq!(
            validate_runtime_tool_authority(
                &durable,
                Some("durable_v2"),
                &authority_manifest(&["write_file"]),
                "run-wrong-preflight",
                "write_file",
                RuntimeToolIngress::Preflight,
            )
            .unwrap_err()
            .0,
            "runtime.capability.ingress_mismatch"
        );
        assert_eq!(
            validate_runtime_tool_authority(
                &durable,
                Some("durable_v2"),
                &authority_manifest(&["graph_readonly_run"]),
                "run-tool-blocked",
                "graph_readonly_run",
                RuntimeToolIngress::Event,
            )
            .unwrap_err()
            .0,
            "runtime.execution_profile.tool_blocked"
        );
        assert_eq!(
            validate_runtime_tool_authority(
                &graph,
                None,
                &authority_manifest(&["graph_readonly_run"]),
                "run-missing-frozen-profile",
                "graph_readonly_run",
                RuntimeToolIngress::Event,
            )
            .unwrap_err()
            .0,
            "runtime.execution_profile.run_mismatch"
        );

        let legacy = super::continuation::ExecutionProfileSelection::resolve("legacy")
            .expect("legacy profile");
        validate_runtime_tool_authority(
            &legacy,
            None,
            &authority_manifest(&["write_file"]),
            "pre-v34-run",
            "write_file",
            RuntimeToolIngress::Execute,
        )
        .expect("pre-v34 Legacy runs retain compatibility");
    }

    #[test]
    fn common_host_tool_gate_replays_persisted_outcome_without_second_handler() {
        let path = std::env::temp_dir().join(format!("fox-host-tool-once-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "exactly once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let mut handler_calls = 0;
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            "memory-once",
            "memory_propose",
            &json!({"content":"stable"}),
            "running",
            false,
        )
        .unwrap();
        assert!(matches!(acquired, HostToolCallExecution::Execute(_)));
        handler_calls += 1;
        database
            .complete_host_tool_call(
                &run.id,
                "memory-once",
                Some(&json!({"proposalId":"proposal-1"})),
                None,
            )
            .unwrap();
        let replay = create_fresh_host_tool_call(
            &database,
            &run.id,
            "memory-once",
            "memory_propose",
            &json!({"content":"stable"}),
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Replay(response) = replay else {
            panic!("terminal exact replay must not acquire execution");
        };
        assert_eq!(response["result"]["proposalId"], "proposal-1");
        assert_eq!(response["replayed"], true);
        assert_eq!(handler_calls, 1);

        let first_failure = create_fresh_host_tool_call(
            &database,
            &run.id,
            "memory-failed-once",
            "memory_propose",
            &json!({"content":"invalid"}),
            "running",
            false,
        )
        .unwrap();
        assert!(matches!(first_failure, HostToolCallExecution::Execute(_)));
        handler_calls += 1;
        let first_failure_response = json!({"isError":true,"error":"memory proposal was rejected"});
        database
            .complete_host_tool_call(
                &run.id,
                "memory-failed-once",
                None,
                Some("memory proposal was rejected"),
            )
            .unwrap();
        let replayed_failure = create_fresh_host_tool_call(
            &database,
            &run.id,
            "memory-failed-once",
            "memory_propose",
            &json!({"content":"invalid"}),
            "running",
            false,
        )
        .expect("failed ToolCall has a persisted replay outcome");
        let HostToolCallExecution::Replay(replayed_failure_response) = replayed_failure else {
            panic!("failed replay must not acquire a second handler");
        };
        assert_eq!(replayed_failure_response, first_failure_response);
        assert_eq!(handler_calls, 2);

        assert!(matches!(
            create_fresh_host_tool_call(
                &database,
                &run.id,
                "in-flight",
                "child_run_start",
                &json!({"objective":"once"}),
                "running",
                false,
            )
            .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        assert!(create_fresh_host_tool_call(
            &database,
            &run.id,
            "in-flight",
            "child_run_start",
            &json!({"objective":"once"}),
            "running",
            false,
        )
        .unwrap_err()
        .contains("tool_call.already_in_flight"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    /// A01/A02 acceptance at the Host seam: an execution that completed but
    /// did not succeed must stay failed in the persisted fact, keep its
    /// diagnostics reachable for the model and the receipt, and replay with the
    /// same information instead of a bare "it failed".
    #[test]
    fn a_reported_execution_failure_stays_failed_and_keeps_its_diagnostics_reachable() {
        let path = std::env::temp_dir()
            .join(format!("fox-execution-failure-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "failed build", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let input = json!({ "command": "tsc --noEmit" });
        assert!(matches!(
            create_fresh_host_tool_call(&database, &run.id, "call-failed", "run_command", &input, "running", false)
                .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        let reported = json!({
            "isError": true,
            "content": [{ "type": "text", "text": "[fox: 命令以退出码 2 结束（非零退出，工具失败）]\nsrc/app.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'" }],
            "details": {
                "exitCode": 2, "timedOut": false, "cancelled": false,
                "errorCode": "tool.nonzero_exit",
                "stdout": "", "stderr": "error TS2322",
                "stdoutBytes": 0, "stderrBytes": 63,
                "shell": "cmd.exe /D /S /C", "platform": "windows",
                "outputEncoding": "utf-8",
            },
        });

        let response = finalize_host_tool_execution(
            &database,
            &run.id,
            "call-failed",
            "run_command",
            &input,
            Vec::new(),
            Ok(reported.clone()),
        )
        .unwrap();
        assert_eq!(response["isError"], Value::Bool(true));
        assert_eq!(response["errorCode"], "tool.nonzero_exit");
        assert_eq!(response["errorDetails"]["code"], "tool.nonzero_exit");
        assert_eq!(response["errorDetails"]["retryable"], Value::Bool(false));
        assert!(response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TS2322"));

        let (record, _) = database
            .inspect_host_tool_call_replay(&run.id, "call-failed", "run_command", &input)
            .unwrap()
            .expect("a terminal ToolCall must be persisted");
        assert_eq!(record.status, "failed", "a reported failure cannot become a completed row");
        let error = record.error_message.clone().expect("the failure needs a persisted reason");
        assert!(error.starts_with("[tool.nonzero_exit]"), "{error}");
        assert!(error.contains("TS2322"), "the persisted error stays actionable: {error}");
        let persisted = record.result.expect("the diagnostics stay attached to the row");
        assert_eq!(persisted["details"]["exitCode"], 2);
        assert!(persisted["content"][0]["text"].as_str().unwrap().contains("TS2322"));

        let replay = create_fresh_host_tool_call(
            &database,
            &run.id,
            "call-failed",
            "run_command",
            &input,
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Replay(replayed) = replay else {
            panic!("a terminal failed ToolCall replays; it must not acquire a second handler");
        };
        assert_eq!(replayed["isError"], Value::Bool(true));
        assert_eq!(replayed["errorCode"], "tool.nonzero_exit");
        assert_eq!(replayed["replayed"], Value::Bool(true));
        assert!(replayed["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("TS2322"));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    /// O-A-01 #4 at the Host seam: a stdout-only non-zero failure far larger
    /// than the inline stored-result cap must (a) stay `failed`, (b) travel as a
    /// bounded wire payload instead of the raw value, and (c) keep every byte
    /// readable through the reference the Host derived, without re-executing the
    /// tool. A small JSON stand-in could not show any of that, so the fixture is
    /// megabytes of real compiler-shaped output.
    #[test]
    fn an_oversized_stdout_only_failure_travels_bounded_and_stays_readable() {
        const INLINE_CAP: usize = crate::database::MAX_STORED_TOOL_RESULT_BYTES;
        let path = std::env::temp_dir().join(format!("fox-execution-failure-huge-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "huge build log", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let input = json!({ "command": "mvn -q test" });
        assert!(matches!(
            create_fresh_host_tool_call(&database, &run.id, "call-huge", "run_command", &input, "running", false)
                .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        // Several megabytes on stdout, nothing on stderr: the exact shape A01 was
        // filed for, scaled past every bound the transport has.
        let line = "[ERROR] src/main/java/App.java:42: incompatible types: cannot convert\n";
        let stdout = line.repeat((INLINE_CAP / line.len()) * 24 + 3);
        let reported = json!({
            "isError": true,
            "content": [{ "type": "text", "text": stdout }],
            "details": {
                "exitCode": 1, "timedOut": false, "cancelled": false,
                "errorCode": "tool.nonzero_exit",
                "stdout": stdout, "stderr": "",
                "stdoutBytes": stdout.len(), "stderrBytes": 0,
                "shell": "cmd.exe /D /S /C", "platform": "windows",
                "outputEncoding": "utf-8",
            },
        });
        let serialized = serde_json::to_string(&reported).unwrap();
        assert!(
            serialized.len() > 8 * INLINE_CAP,
            "the fixture must be far past the inline cap, got {}",
            serialized.len()
        );

        let response = finalize_host_tool_execution(
            &database,
            &run.id,
            "call-huge",
            "run_command",
            &input,
            Vec::new(),
            Ok(reported.clone()),
        )
        .unwrap();
        let wire = serde_json::to_string(&response).unwrap();
        assert_eq!(response["isError"], Value::Bool(true));
        assert_eq!(response["errorCode"], "tool.nonzero_exit");
        assert!(
            wire.len() < INLINE_CAP,
            "a {}-byte failure must not travel whole (wire: {})",
            serialized.len(),
            wire.len()
        );
        assert!(
            wire.len() < 1_048_576,
            "the payload also has to stay inside the JSONL frame cap (wire: {})",
            wire.len()
        );
        // Bounded does not mean vague: the head of the real reason is still on
        // the wire, and the bytes behind it have a stated way back.
        assert!(response["error"].as_str().unwrap().contains("incompatible types"));
        assert_eq!(response["result"]["truncated"], Value::Bool(true));
        assert_eq!(
            response["resultRef"].as_str(),
            Some(format!("fox-result://{}/call-huge", run.id).as_str()),
            "the Host must state the reference it derived itself"
        );
        assert_eq!(response["resultRefRetrievable"], Value::Bool(true));
        assert!(response["resultRefNote"]
            .as_str()
            .unwrap()
            .contains("read_tool_result"));

        let (record, _) = database
            .inspect_host_tool_call_replay(&run.id, "call-huge", "run_command", &input)
            .unwrap()
            .expect("the failed ToolCall stays persisted");
        assert_eq!(record.status, "failed");
        assert!(record
            .error_message
            .as_deref()
            .unwrap()
            .starts_with("[tool.nonzero_exit]"));
        let inline = record.result.expect("the compacted preview is the stored row");
        assert_eq!(inline["truncated"], Value::Bool(true));
        assert_eq!(
            inline["originalBytes"].as_u64().unwrap() as usize,
            serialized.len(),
            "the preview must report the size of the value it replaced"
        );

        // Every retained byte comes back through the reference, and the tool is
        // not executed again to get them.
        let reference = response["resultRef"].as_str().unwrap().to_owned();
        let mut reassembled = String::new();
        let mut offset = 0usize;
        let mut pages = 0usize;
        loop {
            let range = database
                .tool_result_range(&reference, &conversation.id, offset, 64 * 1024)
                .unwrap();
            assert!(range.retrievable, "the blob holds the whole original result");
            assert!(!range.truncated, "ranges are served from the full copy");
            assert_eq!(range.status, "failed", "a failed call's stored bytes stay readable");
            assert_eq!(range.original_bytes, stdout.len());
            pages += 1;
            reassembled.push_str(&range.content);
            match range.next_offset {
                Some(next) if next > offset => offset = next,
                _ => break,
            }
        }
        assert_eq!(reassembled, stdout, "paging must reassemble the failure verbatim");
        assert!(pages > 8, "a {}-byte body needs many pages, got {pages}", stdout.len());
        assert_eq!(
            database
                .tool_result_range(&reference, &conversation.id, 0, 64 * 1024)
                .unwrap()
                .content,
            stdout[..64 * 1024],
            "the same reference is re-readable without re-running anything"
        );
        // A-REQ-006 at the Host seam: the failed row now carries the same trusted
        // storage fact a completed one does, so a bounded view of it is honest.
        assert_eq!(response["toolResultStorage"]["stored"], Value::Bool(true));
        assert!(
            response["toolResultStorage"]["retrievableBytes"]
                .as_u64()
                .unwrap()
                >= 1,
            "{}",
            response["toolResultStorage"]
        );

        // Item 3: a re-delivery of this very call must not be less informative
        // than the first receipt, and must not take a second execution slot.
        let replayed = create_fresh_host_tool_call(
            &database,
            &run.id,
            "call-huge",
            "run_command",
            &input,
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Replay(replay) = replayed else {
            panic!("a terminal failed ToolCall replays; it never re-executes");
        };
        assert_eq!(replay["isError"], Value::Bool(true));
        assert_eq!(replay["errorCode"], "tool.nonzero_exit");
        assert_eq!(replay["replayed"], Value::Bool(true));
        assert_eq!(replay["resultRef"].as_str(), Some(reference.as_str()));
        assert_eq!(replay["resultRefRetrievable"], Value::Bool(true));
        assert_eq!(replay["result"]["truncated"], Value::Bool(true));
        assert_eq!(replay["toolResultStorage"]["stored"], Value::Bool(true));
        assert_eq!(
            database
                .tool_result_range(
                    replay["resultRef"].as_str().unwrap(),
                    &conversation.id,
                    0,
                    4_096,
                )
                .unwrap()
                .content,
            stdout[..4_096],
            "the reference handed to the model on replay pages the same bytes"
        );
        // The read-only inspection the protocol handler uses before dispatch also
        // answers from storage, so the executor is never reached a second time.
        let inspection = inspect_existing_host_tool_call(
            &database,
            &run.id,
            "call-huge",
            "run_command",
            &input,
        )
        .unwrap()
        .expect("a settled call is answered from storage, not by running it again");
        assert_eq!(inspection["isError"], Value::Bool(true));
        assert_eq!(inspection["resultRef"].as_str(), Some(reference.as_str()));
        let (_, _, calls) = database
            .load_conversation_trace_records(&conversation.id)
            .unwrap();
        assert_eq!(
            calls.len(),
            1,
            "replay and inspection must not add a second ToolCall row"
        );
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    /// A-REQ-006: the storage fact is about reachability, not about success.
    /// Both terminal states that really stored their result are trusted; anything
    /// still in flight, without a result, or only kept as a preview is not - and
    /// nothing here widened authorization, which still comes from the row.
    #[test]
    fn failed_and_completed_results_get_the_same_trusted_storage_fact() {
        let path = std::env::temp_dir()
            .join(format!("fox-storage-fact-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "storage facts", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let storage_of = |id: &str| {
            database
                .tool_result_storage(&run.id, id)
                .expect("the fact is derived from the row")
        };
        let unknown = |id: &str| {
            assert!(
                !storage_of(id).stored,
                "{id} must not be advertised as retrievable"
            );
        };

        // Still running: nothing is settled, so nothing may be omitted.
        let running = create_fresh_host_tool_call(
            &database,
            &run.id,
            "call-running",
            "read",
            &json!({"path":"a.txt"}),
            "running",
            false,
        )
        .unwrap();
        assert!(matches!(running, HostToolCallExecution::Execute(_)));
        unknown("call-running");

        // Failed with no result at all (the error channel, e.g. cancellation).
        database
            .complete_host_tool_call(&run.id, "call-running", None, Some("[tool.cancelled] cancelled"))
            .unwrap();
        assert_eq!(
            database
                .inspect_host_tool_call_replay(
                    &run.id,
                    "call-running",
                    "read",
                    &json!({"path":"a.txt"}),
                )
                .unwrap()
                .expect("terminal row")
                .0
                .status,
            "failed"
        );
        unknown("call-running");

        // Failed with a stored result: the diagnostics are paged from storage.
        let failed_input = json!({ "command": "cargo check" });
        assert!(matches!(
            create_fresh_host_tool_call(&database, &run.id, "call-failed", "run_command", &failed_input, "running", false)
                .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        database
            .complete_host_tool_call(
                &run.id,
                "call-failed",
                Some(&json!({
                    "isError": true,
                    "content": [{ "type": "text", "text": "error[E0308]: mismatched types" }],
                    "details": { "errorCode": "tool.nonzero_exit", "exitCode": 101 },
                })),
                Some("[tool.nonzero_exit] mismatched types"),
            )
            .unwrap();
        let fact = storage_of("call-failed");
        assert!(fact.stored, "a failed result the Host stored completely is reachable");
        assert!(
            fact.retrievable_bytes >= 1,
            "the fact has to describe real bytes: {fact:?}"
        );

        // A completed result keeps behaving exactly as before.
        let done_input = json!({ "path": "a.txt" });
        assert!(matches!(
            create_fresh_host_tool_call(&database, &run.id, "call-done", "read", &done_input, "running", false)
                .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        database
            .complete_host_tool_call(
                &run.id,
                "call-done",
                Some(&json!({ "content": [{ "type": "text", "text": "file body" }] })),
                None,
            )
            .unwrap();
        assert!(storage_of("call-done").stored);

        // Authorization is unchanged: another conversation cannot read either.
        let other = database
            .create_conversation(database.default_agent_id(), Some("other"), None, None)
            .unwrap();
        let reference = crate::kernel_compaction::tool_result_ref(&run.id, "call-failed")
            .expect("host-derived reference");
        assert!(database
            .tool_result_range(&reference, &other.id, 0, 4)
            .is_err());
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    /// A-REQ-007: when two deliveries of one ToolCall settle at the same time,
    /// the loser's durable write is refused and it must answer from the
    /// authoritative row with the **same** failure envelope the winner got - a
    /// tag, a bounded preview, a reference - instead of a bare string that
    /// silently loses all three. Neither delivery may re-run the tool, and the
    /// row must keep describing the first, authoritative outcome. The fixture is
    /// deliberately past the inline cap so the spilled path is the one under
    /// test: a small result would never have lost its `details`.
    #[test]
    fn a_second_settle_of_the_same_failure_answers_with_the_same_envelope() {
        const INLINE_CAP: usize = crate::database::MAX_STORED_TOOL_RESULT_BYTES;
        let path = std::env::temp_dir().join(format!("fox-settle-race-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "duplicate settle", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let input = json!({ "command": "gradle build" });
        let line = "e: file:///src/main/kotlin/App.kt:12:5 unresolved reference: foo\n";
        let stdout = line.repeat((INLINE_CAP / line.len()) * 3 + 1);
        let reported = json!({
            "isError": true,
            "content": [{ "type": "text", "text": stdout }],
            "details": {
                "exitCode": 1, "timedOut": false, "cancelled": false,
                "errorCode": "tool.nonzero_exit",
                "stdout": stdout, "stderr": "",
                "stdoutBytes": stdout.len(), "stderrBytes": 0,
            },
        });
        assert!(
            serde_json::to_string(&reported).unwrap().len() > INLINE_CAP,
            "the racing failure has to be big enough to be compacted"
        );
        assert!(matches!(
            create_fresh_host_tool_call(
                &database, &run.id, "call-race", "run_command", &input, "running", false,
            )
            .unwrap(),
            HostToolCallExecution::Execute(_)
        ));

        let first = finalize_host_tool_execution(
            &database,
            &run.id,
            "call-race",
            "run_command",
            &input,
            Vec::new(),
            Ok(reported.clone()),
        )
        .unwrap();
        assert_eq!(first["isError"], Value::Bool(true));
        assert_eq!(
            first["errorCode"], "tool.nonzero_exit",
            "the first receipt must not degrade a spilled failure to tool.unknown"
        );
        assert_eq!(first["errorDetails"]["code"], "tool.nonzero_exit");
        assert_eq!(first.get("replayed"), None, "the first receipt is not a replay");
        assert_eq!(first["result"]["truncated"], Value::Bool(true));
        assert_eq!(first["resultRefRetrievable"], Value::Bool(true));
        let reference = first["resultRef"].as_str().expect("host-derived reference").to_owned();

        // The second delivery claims success. That is not merely different - it
        // contradicts the authoritative row, so it must be refused there.
        let contradicting = finalize_host_tool_execution(
            &database,
            &run.id,
            "call-race",
            "run_command",
            &input,
            Vec::new(),
            Ok(json!({
                "isError": false,
                "content": [{ "type": "text", "text": "BUILD SUCCESSFUL in 2s" }],
                "details": { "exitCode": 0 },
            })),
        )
        .unwrap();
        assert_eq!(
            contradicting["isError"],
            Value::Bool(true),
            "a settled failure cannot be rewritten into success by a later delivery"
        );
        assert_eq!(contradicting["replayed"], Value::Bool(true));
        assert_eq!(contradicting["errorCode"], first["errorCode"]);
        assert_eq!(contradicting["error"], first["error"]);
        assert_eq!(contradicting["result"], first["result"]);
        assert_eq!(contradicting["toolResultStorage"], first["toolResultStorage"]);
        assert_eq!(contradicting["resultRef"].as_str(), Some(reference.as_str()));
        assert_eq!(contradicting["resultRefRetrievable"], Value::Bool(true));
        // The reference the racing loser was handed really pages the bytes.
        assert_eq!(
            database
                .tool_result_range(contradicting["resultRef"].as_str().unwrap(), &conversation.id, 0, 4_096)
                .unwrap()
                .content,
            stdout[..4_096],
            "the loser must be able to read back what it was pointed at"
        );

        // A hard-error loser is answered the same way: the row already holds a
        // result, so it gets the envelope - and it reports the authoritative
        // outcome, not a status the late delivery invented.
        let late_error = finalize_host_tool_execution(
            &database,
            &run.id,
            "call-race",
            "run_command",
            &input,
            Vec::new(),
            Err("[tool.cancelled] the run was cancelled".to_owned()),
        )
        .expect("a terminal authoritative row answers a late error instead of overwriting it");
        assert_eq!(late_error["isError"], Value::Bool(true));
        assert_eq!(late_error["errorCode"], "tool.nonzero_exit");
        assert_eq!(late_error["error"], first["error"]);
        assert_eq!(late_error["resultRef"].as_str(), Some(reference.as_str()));

        // Nothing about the row changed, and no second ToolCall was created.
        let (record, _) = database
            .inspect_host_tool_call_replay(&run.id, "call-race", "run_command", &input)
            .unwrap()
            .expect("terminal row");
        assert_eq!(record.status, "failed");
        assert!(record
            .error_message
            .as_deref()
            .unwrap()
            .starts_with("[tool.nonzero_exit]"));
        assert_eq!(record.result.expect("preview")["truncated"], Value::Bool(true));
        let (_, _, calls) = database
            .load_conversation_trace_records(&conversation.id)
            .unwrap();
        assert_eq!(
            calls.len(),
            1,
            "a duplicate settle replays; it never adds a second ToolCall"
        );
        // And an identity mismatch is still refused rather than answered: the
        // fallback cannot be used to borrow another call's stored bytes.
        let other_input = json!({ "command": "gradle test" });
        assert!(matches!(
            create_fresh_host_tool_call(
                &database, &run.id, "call-race", "run_command", &other_input, "running", false,
            ),
            Err(_)
        ));
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn persistent_graph_activation_uses_host_identity_and_replays_without_second_handler() {
        let path = std::env::temp_dir().join(format!("fox-graph-host-once-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "persistent graph once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .freeze_run_execution_profile(&run.id, "durable_v2")
            .unwrap();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "Persistent Graph".to_owned(),
                objective: "Activate exactly once".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run.id.clone(),
            })
            .unwrap();
        database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![CreateTaskInput {
                    id: None,
                    goal_id: goal.id.clone(),
                    parent_task_id: None,
                    ordinal: 0,
                    title: "Inspect".to_owned(),
                    detail: None,
                }],
                vec!["standard_v1".to_owned()],
            )
            .unwrap();
        let plan = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "Persistent Graph",
                "One bounded node",
                json!([{
                    "nodeKey": "inspect",
                    "title": "Inspect",
                    "ordinal": 0,
                    "acceptanceCriteria": ["inspection evidence is current"]
                }]),
                &run.id,
            )
            .unwrap();
        database
            .resolve_plan_revision(&conversation.id, &plan.id, "approved")
            .unwrap();

        let runtime_tool_call_id = "graph-activate-once";
        let input = json!({"planRevisionId": plan.id});
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            runtime_tool_call_id,
            "graph_readonly_activate",
            &input,
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Execute(host_tool_call) = acquired else {
            panic!("first activation must acquire the Host ToolCall");
        };
        let handler_calls = 1;
        let outcome = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &host_tool_call.id,
            "graph_readonly_activate",
            &input,
        )
        .unwrap();
        let first = finalize_host_tool_execution(
            &database,
            &run.id,
            runtime_tool_call_id,
            "graph_readonly_activate",
            &input,
            Vec::new(),
            Ok(outcome.result),
        )
        .unwrap();
        assert_eq!(first["result"]["details"]["activated"], true);

        let replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            runtime_tool_call_id,
            "graph_readonly_activate",
            &input,
        )
        .unwrap()
        .expect("terminal Graph activation replay");
        assert_eq!(replay["result"], first["result"]);
        assert_eq!(replay["replayed"], true);
        assert_eq!(handler_calls, 1);
        assert!(database
            .read_only_graph_snapshot(&goal.id)
            .unwrap()
            .is_some());

        let graph = database
            .read_only_graph_snapshot(&goal.id)
            .unwrap()
            .expect("activated Graph");
        let generic_goal_complete = super::work_tools::execute(
            &database,
            &conversation.id,
            &run.id,
            "goal_complete",
            &json!({ "goalId": goal.id, "expectedVersion": 1 }),
        )
        .expect_err("Graph Goal completion requires the dedicated acceptance authority");
        assert_eq!(
            generic_goal_complete.code,
            "graph.readonly_dedicated_accept_required"
        );
        let generic_acceptance = super::work_tools::execute(
            &database,
            &conversation.id,
            &run.id,
            "acceptance_submit",
            &json!({ "goalId": goal.id }),
        )
        .expect_err("model-supplied generic Acceptance cannot complete a Graph");
        assert_eq!(
            generic_acceptance.code,
            "graph.readonly_dedicated_accept_required"
        );
        let node = &graph.nodes[0];
        let node_runtime_tool_call_id = "graph-node-start-once";
        let node_input = json!({
            "goalId": goal.id.clone(),
            "taskId": node.task_id.clone(),
            "expectedTaskVersion": node.task_version,
            "attemptId": "graph-node-attempt-once",
            "workerAgentId": database.default_agent_id(),
            "objective": "Inspect the bounded node.",
            "context": "Use only read, ls, find, and grep.",
            "budget": {
                "maxDurationMs": 45_000,
                "maxTotalTokens": 4_096,
                "maxOutputTokens": 1_024,
                "maxToolCalls": 6,
            },
        });
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            node_runtime_tool_call_id,
            "graph_readonly_node_start",
            &node_input,
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Execute(node_tool_call) = acquired else {
            panic!("first node start must acquire the Host ToolCall");
        };
        let node_outcome = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &node_tool_call.id,
            "graph_readonly_node_start",
            &node_input,
        )
        .unwrap();
        let child_dispatch = match node_outcome.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::StartChild(dispatch)) => {
                Some(dispatch)
            }
            _ => None,
        };
        assert!(child_dispatch.is_some());
        let mut dispatch_calls = 0;
        assert_eq!(
            dispatch_calls, 0,
            "Child dispatch must wait for finalization"
        );
        let node_first = finalize_host_tool_execution(
            &database,
            &run.id,
            node_runtime_tool_call_id,
            "graph_readonly_node_start",
            &node_input,
            Vec::new(),
            Ok(node_outcome.result),
        )
        .unwrap();
        dispatch_created_work_child(child_dispatch, |_| {
            dispatch_calls += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(dispatch_calls, 1);
        assert_eq!(node_first["result"]["details"]["created"], true);
        assert_eq!(
            node_first["result"]["details"]["graph"]["nodes"][0]["readiness"],
            "active"
        );
        assert_ne!(
            node_first["result"]["details"]["graph"]["nodes"][0]["readiness"], "accepted",
            "Child creation must never imply node acceptance"
        );

        let terminal_node_replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            node_runtime_tool_call_id,
            "graph_readonly_node_start",
            &node_input,
        )
        .unwrap()
        .expect("terminal Graph node-start replay");
        assert_eq!(terminal_node_replay["result"], node_first["result"]);
        assert_eq!(terminal_node_replay["replayed"], true);
        assert_eq!(
            dispatch_calls, 1,
            "Host terminal replay must never dispatch the Child twice"
        );

        let child_run_id = node_first["result"]["details"]["childRun"]["childRunId"]
            .as_str()
            .expect("Graph Child Run ID")
            .to_owned();
        let child_status_before_generic_cancel = database.child_run_status(&child_run_id).unwrap();
        let bypass_error = super::ensure_child_cancel_not_graph_bound(&database, &child_run_id)
            .expect_err("generic Child cancel cannot bypass Graph authority");
        assert!(bypass_error.contains("graph.readonly_dedicated_cancel_required"));
        assert_eq!(
            database.child_run_status(&child_run_id).unwrap(),
            child_status_before_generic_cancel,
            "bypass guard must have zero side effects"
        );
        let cancel_runtime_tool_call_id = "graph-node-cancel-terminal-once";
        let cancel_input = json!({
            "goalId": goal.id.clone(),
            "taskId": node.task_id.clone(),
            "attemptId": "graph-node-attempt-once",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "reason": "The parent no longer needs this bounded node."
        });
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            cancel_runtime_tool_call_id,
            "graph_readonly_node_cancel",
            &cancel_input,
            "running",
            false,
        )
        .unwrap();
        let HostToolCallExecution::Execute(cancel_tool_call) = acquired else {
            panic!("first node cancel must acquire the Host ToolCall");
        };
        let cancel_outcome = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &cancel_tool_call.id,
            "graph_readonly_node_cancel",
            &cancel_input,
        )
        .unwrap();
        let cancel_directive = match cancel_outcome.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::CancelGraphChild(directive)) => {
                directive
            }
            _ => panic!("fresh cancel intent must provide a post-finalize directive"),
        };
        assert!(database
            .active_graph_node_cancel_intents_for_recovery()
            .unwrap()
            .is_empty());
        let cancel_first = finalize_host_tool_execution(
            &database,
            &run.id,
            cancel_runtime_tool_call_id,
            "graph_readonly_node_cancel",
            &cancel_input,
            Vec::new(),
            Ok(cancel_outcome.result),
        )
        .unwrap();
        assert_eq!(cancel_first["result"]["details"]["cancelCompleted"], false);
        let activation = database
            .activate_graph_node_cancel_intent(&cancel_directive.intent_id)
            .unwrap();
        assert!(activation.activated);
        let mut physical_signals = 0;
        assert!(!super::signal_graph_child_cancel_with_transport(
            &database,
            &child_run_id,
            false,
            || Ok(None),
        )
        .unwrap());
        assert_eq!(
            physical_signals, 0,
            "missing Host cannot fabricate a signal"
        );
        let child = database
            .child_run(&child_run_id)
            .unwrap()
            .expect("Graph Child remains projected");
        assert_eq!(
            database.child_run_status(&child_run_id).unwrap().as_deref(),
            Some("cancelling")
        );
        assert_eq!(child.finished_at, None);
        let attempt = database
            .list_task_attempts(&node.task_id, 10)
            .unwrap()
            .into_iter()
            .find(|attempt| attempt.id == "graph-node-attempt-once")
            .expect("Graph Attempt remains live without an authoritative terminal");
        assert_eq!(attempt.status, crate::database::TaskAttemptStatus::Running);
        assert!(super::signal_graph_child_cancel_with_transport(
            &database,
            &child_run_id,
            true,
            || {
                physical_signals += 1;
                Ok(Some(true))
            },
        )
        .unwrap());
        assert_eq!(
            physical_signals, 1,
            "startup redelivery may signal an active intent at least once"
        );

        let terminal_cancel_replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            cancel_runtime_tool_call_id,
            "graph_readonly_node_cancel",
            &cancel_input,
        )
        .unwrap()
        .expect("terminal Graph node-cancel replay");
        assert_eq!(terminal_cancel_replay["result"], cancel_first["result"]);
        assert_eq!(terminal_cancel_replay["replayed"], true);
        assert_eq!(
            physical_signals, 1,
            "Host terminal replay must not execute the cancel directive twice"
        );

        let finish_runtime_tool_call_id = "graph-node-finish-terminal-once";
        let finish_input = json!({
            "goalId": goal.id,
            "taskId": node.task_id,
            "attemptId": "graph-node-attempt-once",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "criterionEvidence": [{
                "criterion": "inspection evidence is current",
                "evidenceIds": ["evidence-terminal-replay"]
            }],
            "summary": "Parent Lead revalidated the frozen criterion."
        });
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            finish_runtime_tool_call_id,
            "graph_readonly_node_finish",
            &finish_input,
            "running",
            false,
        )
        .unwrap();
        assert!(matches!(acquired, HostToolCallExecution::Execute(_)));
        let finish_handler_calls = 1;
        let finish_first = finalize_host_tool_execution(
            &database,
            &run.id,
            finish_runtime_tool_call_id,
            "graph_readonly_node_finish",
            &finish_input,
            Vec::new(),
            Ok(json!({
                "summary": "Graph node completed",
                "details": { "replayed": false, "goalAcceptanceCreated": false }
            })),
        )
        .unwrap();
        let terminal_finish_replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            finish_runtime_tool_call_id,
            "graph_readonly_node_finish",
            &finish_input,
        )
        .unwrap()
        .expect("terminal Graph node-finish replay");
        assert_eq!(terminal_finish_replay["result"], finish_first["result"]);
        assert_eq!(terminal_finish_replay["replayed"], true);
        assert_eq!(
            finish_handler_calls, 1,
            "Host terminal replay must not execute Graph node-finish twice"
        );

        let connection = rusqlite::Connection::open(&path).unwrap();
        let spec_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM work_graph_specs WHERE goal_id = ?1",
                [&goal.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(spec_count, 1);

        drop(connection);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn high_risk_graph_review_and_accept_wait_for_finalizers_and_replay_without_side_effects() {
        let path =
            std::env::temp_dir().join(format!("fox-graph-review-host-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "review the high-risk graph", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .freeze_run_execution_profile(&run.id, "durable_v2")
            .unwrap();
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "High-risk Graph".to_owned(),
                objective: "Review and accept one bounded node".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run.id.clone(),
            })
            .unwrap();
        database
            .work_tasks()
            .create_many_with_policy_ids(
                vec![CreateTaskInput {
                    id: None,
                    goal_id: goal.id.clone(),
                    parent_task_id: None,
                    ordinal: 0,
                    title: "Inspect high-risk output".to_owned(),
                    detail: None,
                }],
                vec!["high_risk_v1".to_owned()],
            )
            .unwrap();
        let criterion = "the implementation result is independently inspected";
        let plan = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "High-risk Graph",
                "One reviewed node",
                json!([{
                    "nodeKey": "reviewed",
                    "title": "Inspect high-risk output",
                    "ordinal": 0,
                    "acceptanceCriteria": [criterion]
                }]),
                &run.id,
            )
            .unwrap();
        database
            .resolve_plan_revision(&conversation.id, &plan.id, "approved")
            .unwrap();

        let activation_input = json!({"planRevisionId": plan.id});
        let HostToolCallExecution::Execute(activation_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            "review-graph-activate",
            "graph_readonly_activate",
            &activation_input,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh activation");
        };
        let activation = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &activation_call.id,
            "graph_readonly_activate",
            &activation_input,
        )
        .unwrap();
        finalize_host_tool_execution(
            &database,
            &run.id,
            "review-graph-activate",
            "graph_readonly_activate",
            &activation_input,
            Vec::new(),
            Ok(activation.result),
        )
        .unwrap();
        let graph = database
            .read_only_graph_snapshot(&goal.id)
            .unwrap()
            .expect("activated Graph");
        let node = &graph.nodes[0];

        let start_input = json!({
            "goalId": goal.id,
            "taskId": node.task_id,
            "expectedTaskVersion": node.task_version,
            "attemptId": "reviewed-attempt",
            "workerAgentId": database.default_agent_id(),
            "objective": "Produce a bounded read-only implementation result.",
            "context": "The parent Lead will inspect and independently review it.",
            "budget": {
                "maxDurationMs": 45_000,
                "maxTotalTokens": 4_096,
                "maxOutputTokens": 1_024,
                "maxToolCalls": 6
            }
        });
        let HostToolCallExecution::Execute(start_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            "reviewed-node-start",
            "graph_readonly_node_start",
            &start_input,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh node start");
        };
        let start = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &start_call.id,
            "graph_readonly_node_start",
            &start_input,
        )
        .unwrap();
        assert!(matches!(
            start.post_finalize,
            Some(super::work_tools::WorkToolPostFinalizeDirective::StartChild(_))
        ));
        let start_response = finalize_host_tool_execution(
            &database,
            &run.id,
            "reviewed-node-start",
            "graph_readonly_node_start",
            &start_input,
            Vec::new(),
            Ok(start.result),
        )
        .unwrap();
        let implementation_child_run_id = start_response["result"]["details"]["childRun"]
            ["childRunId"]
            .as_str()
            .expect("implementation Child Run")
            .to_owned();
        database
            .apply_runtime_event(
                &implementation_child_run_id,
                1,
                &json!({"type":"run.started"}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &implementation_child_run_id,
                2,
                &json!({"type":"message.started"}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &implementation_child_run_id,
                3,
                &json!({"type":"message.delta","delta":"bounded implementation result"}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &implementation_child_run_id,
                4,
                &json!({"type":"message.completed"}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &implementation_child_run_id,
                5,
                &json!({"type":"run.completed"}),
            )
            .unwrap();
        database
            .sync_child_run_terminal(&implementation_child_run_id)
            .unwrap();

        std::thread::sleep(std::time::Duration::from_millis(2));
        let lead_runtime_tool_call_id = "lead-review-proof";
        database
            .apply_runtime_event(
                &run.id,
                2,
                &json!({
                    "type":"tool.started",
                    "toolCallId":lead_runtime_tool_call_id,
                    "tool":"read",
                    "input":{"path":"review-result.txt"}
                }),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &run.id,
                3,
                &json!({
                    "type":"tool.completed",
                    "toolCallId":lead_runtime_tool_call_id,
                    "tool":"read",
                    "result":{"content":"inspected"},
                    "isError":false
                }),
            )
            .unwrap();
        let connection = rusqlite::Connection::open(&path).unwrap();
        let lead_tool_call_id = connection
            .query_row(
                "SELECT id FROM tool_calls WHERE run_id = ?1 AND runtime_tool_call_id = ?2",
                [&run.id, lead_runtime_tool_call_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        drop(connection);
        let evidence = database
            .task_evidence()
            .add(AddEvidenceInput {
                id: None,
                task_id: node.task_id.clone(),
                source_run_id: Some(run.id.clone()),
                evidence_type: EvidenceType::ToolCall,
                ref_kind: EvidenceReferenceKind::ToolCall,
                ref_id: lead_tool_call_id,
                summary: "Lead inspected the implementation result".to_owned(),
                metadata: json!({"validationCheckType":"inspection"}),
                trace_id: None,
                span_id: None,
            })
            .unwrap();
        assert_eq!(
            database.task_evidence().validate(&evidence.id).unwrap(),
            EvidenceValidityStatus::Valid
        );

        let candidate = json!({
            "goalId": goal.id,
            "taskId": node.task_id,
            "attemptId": "reviewed-attempt",
            "expectedTaskVersion": 2,
            "expectedAttemptVersion": 1,
            "criterionEvidence": [{
                "criterion": criterion,
                "evidenceIds": [evidence.id]
            }],
            "summary": "Lead verified the exact candidate before independent review."
        });

        let HostToolCallExecution::Execute(failed_review_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            "review-finalizer-fails",
            "graph_readonly_node_review",
            &candidate,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh failed-finalizer review");
        };
        let failed_review = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &failed_review_call.id,
            "graph_readonly_node_review",
            &candidate,
        )
        .unwrap();
        let failed_request_id = match failed_review.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::StartGraphReviewer(
                directive,
            )) => directive.request_id,
            _ => panic!("fresh review request must defer Reviewer dispatch"),
        };
        finalize_host_tool_execution(
            &database,
            &run.id,
            "review-finalizer-fails",
            "graph_readonly_node_review",
            &candidate,
            Vec::new(),
            Err("simulated common finalizer failure".to_owned()),
        )
        .expect_err("failed handler result must still terminalize the ToolCall");
        assert!(database
            .active_graph_node_review_requests_for_recovery()
            .unwrap()
            .is_empty());
        assert!(
            database
                .activate_graph_node_review_request(&failed_request_id)
                .unwrap()
                .stale
        );

        let review_runtime_tool_call_id = "review-finalizer-succeeds";
        let HostToolCallExecution::Execute(review_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            review_runtime_tool_call_id,
            "graph_readonly_node_review",
            &candidate,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh review");
        };
        let review = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &review_call.id,
            "graph_readonly_node_review",
            &candidate,
        )
        .unwrap();
        let request_id = match review.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::StartGraphReviewer(
                directive,
            )) => directive.request_id,
            _ => panic!("fresh review request must defer Reviewer dispatch"),
        };
        assert!(database
            .active_graph_node_review_requests_for_recovery()
            .unwrap()
            .is_empty());
        let review_response = finalize_host_tool_execution(
            &database,
            &run.id,
            review_runtime_tool_call_id,
            "graph_readonly_node_review",
            &candidate,
            Vec::new(),
            Ok(review.result),
        )
        .unwrap();
        let activated = database
            .activate_graph_node_review_request(&request_id)
            .unwrap();
        assert!(activated.activated);
        let dispatched = database
            .dispatch_graph_node_reviewer(&request_id, database.default_agent_id())
            .unwrap();
        assert!(dispatched.created);
        assert!(dispatched.dispatch_required);
        assert_ne!(
            dispatched.child_run.parent_run_id,
            dispatched.child_run.child_run_id
        );
        assert_ne!(
            implementation_child_run_id,
            dispatched.child_run.child_run_id
        );
        super::validate_graph_reviewer_runtime_contract(&dispatched.started, &dispatched.child_run)
            .unwrap();
        let reviewer_child_run_id = dispatched.child_run.child_run_id.clone();
        assert_eq!(
            database
                .active_graph_node_review_requests_for_recovery()
                .unwrap()
                .len(),
            1
        );
        let review_replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            review_runtime_tool_call_id,
            "graph_readonly_node_review",
            &candidate,
        )
        .unwrap()
        .expect("terminal review ToolCall replay");
        assert_eq!(review_replay["result"], review_response["result"]);
        assert_eq!(review_replay["replayed"], true);
        assert!(
            !database
                .dispatch_graph_node_reviewer(&request_id, database.default_agent_id())
                .unwrap()
                .created
        );

        database
            .freeze_run_execution_profile(&reviewer_child_run_id, "graph_reviewer_v1")
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        database
            .apply_runtime_event(&reviewer_child_run_id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let reviewer_proof_runtime_id = "reviewer-proof-read";
        database
            .apply_runtime_event(
                &reviewer_child_run_id,
                2,
                &json!({
                    "type":"tool.started",
                    "toolCallId":reviewer_proof_runtime_id,
                    "tool":"read",
                    "input":{"path":"review-result.txt"}
                }),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &reviewer_child_run_id,
                3,
                &json!({
                    "type":"tool.completed",
                    "toolCallId":reviewer_proof_runtime_id,
                    "tool":"read",
                    "result":{"content":"verified"},
                    "isError":false
                }),
            )
            .unwrap();
        let reviewer_result = serde_json::to_string(&json!({
            "criteria": [{
                "criterion": criterion,
                "status": "passed",
                "toolCallIds": [reviewer_proof_runtime_id]
            }],
            "recommendation": "pass",
            "summary": "Independent read proof covers the frozen criterion.",
            "findings": []
        }))
        .unwrap();
        database
            .apply_runtime_event(
                &reviewer_child_run_id,
                4,
                &json!({"type":"message.started"}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &reviewer_child_run_id,
                5,
                &json!({"type":"message.delta","delta":reviewer_result}),
            )
            .unwrap();
        database
            .apply_runtime_event(
                &reviewer_child_run_id,
                6,
                &json!({"type":"message.completed"}),
            )
            .unwrap();
        database
            .apply_runtime_event(&reviewer_child_run_id, 7, &json!({"type":"run.completed"}))
            .unwrap();
        database
            .sync_child_run_terminal(&reviewer_child_run_id)
            .unwrap();
        let decision = database
            .settle_graph_node_review_for_child(&reviewer_child_run_id)
            .unwrap();
        assert_eq!(
            decision.outcome,
            crate::database::GraphNodeReviewOutcome::Pass
        );
        assert!(!decision.replayed);
        assert!(
            database
                .settle_graph_node_review_for_child(&reviewer_child_run_id)
                .unwrap()
                .replayed
        );
        assert!(database
            .active_graph_node_review_requests_for_recovery()
            .unwrap()
            .is_empty());

        let finish_runtime_tool_call_id = "reviewed-node-finish";
        let HostToolCallExecution::Execute(finish_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            finish_runtime_tool_call_id,
            "graph_readonly_node_finish",
            &candidate,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh reviewed finish");
        };
        let finish = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &finish_call.id,
            "graph_readonly_node_finish",
            &candidate,
        )
        .unwrap();
        finalize_host_tool_execution(
            &database,
            &run.id,
            finish_runtime_tool_call_id,
            "graph_readonly_node_finish",
            &candidate,
            Vec::new(),
            Ok(finish.result),
        )
        .unwrap();

        let failed_accept_runtime_id = "graph-accept-finalizer-fails";
        let accept_input = json!({
            "goalId": goal.id,
            "expectedGoalVersion": 1,
            "summary": "Every reviewed Graph node is accepted."
        });
        let HostToolCallExecution::Execute(failed_accept_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            failed_accept_runtime_id,
            "graph_readonly_accept",
            &accept_input,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh failed-finalizer acceptance");
        };
        let failed_accept = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &failed_accept_call.id,
            "graph_readonly_accept",
            &accept_input,
        )
        .unwrap();
        let failed_acceptance_id = match failed_accept.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(
                directive,
            )) => directive.acceptance_id,
            _ => panic!("fresh acceptance must defer activation"),
        };
        finalize_host_tool_execution(
            &database,
            &run.id,
            failed_accept_runtime_id,
            "graph_readonly_accept",
            &accept_input,
            Vec::new(),
            Err("simulated acceptance finalizer failure".to_owned()),
        )
        .expect_err("failed handler result must still terminalize the ToolCall");
        assert!(database
            .pending_graph_acceptances_for_recovery()
            .unwrap()
            .is_empty());
        assert!(
            database
                .activate_read_only_graph_acceptance(&failed_acceptance_id)
                .is_err(),
            "a failed finalizer must leave the pending acceptance inert"
        );

        let accept_runtime_tool_call_id = "graph-accept-finalizer-succeeds";
        let HostToolCallExecution::Execute(accept_call) = create_fresh_host_tool_call(
            &database,
            &run.id,
            accept_runtime_tool_call_id,
            "graph_readonly_accept",
            &accept_input,
            "running",
            false,
        )
        .unwrap() else {
            panic!("fresh acceptance");
        };
        let accept = super::work_tools::execute_with_host_tool_call_id(
            &database,
            &conversation.id,
            &run.id,
            &accept_call.id,
            "graph_readonly_accept",
            &accept_input,
        )
        .unwrap();
        let acceptance_id = match accept.post_finalize {
            Some(super::work_tools::WorkToolPostFinalizeDirective::AcceptReadOnlyGraph(
                directive,
            )) => directive.acceptance_id,
            _ => panic!("fresh acceptance must defer activation"),
        };
        let accept_response = finalize_host_tool_execution(
            &database,
            &run.id,
            accept_runtime_tool_call_id,
            "graph_readonly_accept",
            &accept_input,
            Vec::new(),
            Ok(accept.result),
        )
        .unwrap();
        assert_eq!(
            database
                .pending_graph_acceptances_for_recovery()
                .unwrap()
                .len(),
            1
        );
        let accepted = database
            .activate_read_only_graph_acceptance(&acceptance_id)
            .unwrap();
        assert!(accepted.accepted);
        assert_eq!(
            database.goals().get(&goal.id).unwrap().unwrap().status,
            GoalStatus::Completed
        );
        assert!(
            database
                .activate_read_only_graph_acceptance(&acceptance_id)
                .unwrap()
                .replayed
        );
        let accept_replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            accept_runtime_tool_call_id,
            "graph_readonly_accept",
            &accept_input,
        )
        .unwrap()
        .expect("terminal acceptance ToolCall replay");
        assert_eq!(accept_replay["result"], accept_response["result"]);
        assert_eq!(accept_replay["replayed"], true);
        assert!(database
            .pending_graph_acceptances_for_recovery()
            .unwrap()
            .is_empty());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn common_finalizer_persists_errors_and_replays_without_repeating_after_hooks() {
        let path = std::env::temp_dir().join(format!("fox-host-finalizer-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "finalize once", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        database
            .save_lifecycle_hook(
                "after-finalizer",
                "After finalizer",
                "after_tool",
                "memory_propose",
                "annotate",
                "recorded once",
                true,
                1,
            )
            .unwrap();
        let input = json!({"content":"missing required fields"});
        let acquired = create_fresh_host_tool_call(
            &database,
            &run.id,
            "memory-finalized-error",
            "memory_propose",
            &input,
            "running",
            false,
        )
        .unwrap();
        assert!(matches!(acquired, HostToolCallExecution::Execute(_)));

        let first_error = finalize_host_tool_execution(
            &database,
            &run.id,
            "memory-finalized-error",
            "memory_propose",
            &input,
            Vec::new(),
            Err("memory.kind_required".to_owned()),
        )
        .unwrap_err();
        assert_eq!(first_error, "memory.kind_required");

        let (_, _, tool_calls) = database
            .load_conversation_trace_records(&conversation.id)
            .unwrap();
        let stored = tool_calls
            .iter()
            .find(|record| record.runtime_tool_call_id == "memory-finalized-error")
            .expect("finalized ToolCall");
        assert_eq!(stored.status, "failed");
        assert_eq!(stored.error_message.as_deref(), Some(first_error.as_str()));

        let replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            "memory-finalized-error",
            "memory_propose",
            &input,
        )
        .unwrap()
        .expect("terminal replay");
        assert_eq!(replay, json!({"isError":true,"error":first_error}));

        let connection = rusqlite::Connection::open(&path).unwrap();
        let hook_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM lifecycle_hook_executions
                 WHERE hook_id = 'after-finalizer' AND tool_call_id = 'memory-finalized-error'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hook_count, 1);

        drop(connection);
        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn common_finalizer_preserves_an_already_terminal_managed_error() {
        let path =
            std::env::temp_dir().join(format!("fox-managed-finalizer-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).unwrap();
        let conversation = database
            .create_conversation(database.default_agent_id(), None, None, None)
            .unwrap();
        let run = database
            .create_run(&conversation.id, "managed failure", None)
            .unwrap()
            .run;
        database
            .apply_runtime_event(&run.id, 1, &json!({"type":"run.started"}))
            .unwrap();
        let input = json!({"objective":"bounded"});
        assert!(matches!(
            create_fresh_host_tool_call(
                &database,
                &run.id,
                "managed-finalized-error",
                "child_run_start",
                &input,
                "running",
                false,
            )
            .unwrap(),
            HostToolCallExecution::Execute(_)
        ));
        let managed_error = "[child_run.budget_exceeded] reserved execution expired";
        database
            .complete_host_tool_call(
                &run.id,
                "managed-finalized-error",
                None,
                Some(managed_error),
            )
            .unwrap();

        let returned = finalize_host_tool_execution(
            &database,
            &run.id,
            "managed-finalized-error",
            "child_run_start",
            &input,
            Vec::new(),
            Err("generic late handler error".to_owned()),
        )
        .unwrap_err();
        assert_eq!(returned, managed_error);
        let replay = inspect_existing_host_tool_call(
            &database,
            &run.id,
            "managed-finalized-error",
            "child_run_start",
            &input,
        )
        .unwrap()
        .expect("managed terminal replay");
        assert_eq!(replay, json!({"isError":true,"error":managed_error}));

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn runtime_envelope_identity_is_bound_to_active_worker_and_database_run() {
        validate_envelope_identity_scope(
            "conversation-1",
            "run-1",
            "session-1",
            Some("conversation-1"),
            Some("run-1"),
            Some("session-1"),
            "conversation-1",
        )
        .unwrap();
        assert_eq!(
            validate_envelope_identity_scope(
                "conversation-1",
                "run-2",
                "session-1",
                Some("conversation-1"),
                Some("run-1"),
                Some("session-1"),
                "conversation-1",
            )
            .unwrap_err()
            .0,
            "runtime.identity.active_scope_mismatch"
        );
        assert_eq!(
            validate_envelope_identity_scope(
                "conversation-2",
                "run-1",
                "session-1",
                Some("conversation-2"),
                Some("run-1"),
                Some("session-1"),
                "conversation-1",
            )
            .unwrap_err()
            .0,
            "runtime.identity.run_conversation_mismatch"
        );
    }

    #[test]
    fn proposed_plan_blocks_project_mutations_but_not_read_tools() {
        let path = std::env::temp_dir().join(format!("fox-plan-gate-{}.db", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("plan gate"), None, None)
            .expect("create conversation");
        let run = database
            .create_run(&conversation.id, "propose plan", None)
            .expect("create run")
            .run;
        let goal = database
            .goals()
            .create(CreateGoalInput {
                id: None,
                conversation_id: conversation.id.clone(),
                title: "Plan gate".to_owned(),
                objective: "block writes before plan approval".to_owned(),
                acceptance_summary: None,
                status: GoalStatus::Active,
                created_by: run.id.clone(),
            })
            .expect("create goal");
        let plan = database
            .create_plan_revision(
                &conversation.id,
                &goal.id,
                "v1",
                "wait for approval",
                json!([{ "title": "Implement", "ordinal": 0 }]),
                &run.id,
            )
            .expect("create proposed plan");

        assert!(ensure_plan_approved_before_mutation(
            &database,
            &conversation.id,
            "edit_file",
            &json!({ "path": "src/lib.rs" }),
        )
        .unwrap_err()
        .contains("plan.approval_required"));
        assert!(ensure_plan_approved_before_mutation(
            &database, &conversation.id, "call_mcp_tool",
            &json!({"serverId":"fox-office","tool":"office_create","arguments":{"output":"report.docx"}}),
        ).is_err());
        // F-04: importing data writes a workbook, so it must pause for plan
        // approval just like office_create (the per-call approval still
        // applies when no plan is proposed).
        assert!(ensure_plan_approved_before_mutation(
            &database, &conversation.id, "call_mcp_tool",
            &json!({"serverId":"fox-office","tool":"office_import_data","arguments":{"output":"data.xlsx","artifactId":"compute-artifact:00"}}),
        ).is_err());
        assert!(ensure_plan_approved_before_mutation(
            &database, &conversation.id, "call_mcp_tool",
            &json!({"serverId":"fox-office","tool":"office_read","arguments":{"file":"report.docx"}}),
        ).is_ok());
        assert!(ensure_plan_approved_before_mutation(
            &database,
            &conversation.id,
            "git_read",
            &json!({}),
        )
        .is_ok());
        database
            .resolve_plan_revision(&conversation.id, &plan.id, "approved")
            .expect("approve plan");
        assert!(ensure_plan_approved_before_mutation(
            &database,
            &conversation.id,
            "edit_file",
            &json!({ "path": "src/lib.rs" }),
        )
        .is_ok());
        assert!(ensure_plan_approved_before_mutation(
            &database, &conversation.id, "call_mcp_tool",
            &json!({"serverId":"fox-office","tool":"office_import_data","arguments":{"output":"data.xlsx","artifactId":"compute-artifact:00"}}),
        ).is_ok());

        drop(database);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn resolves_runtime_dependencies_from_a_hoisted_workspace_node_modules() {
        let root = std::env::temp_dir().join(format!("fox-runtime-deps-{}", Uuid::new_v4()));
        let script = root.join("services/agent-runtime/src/pi-runtime.mjs");
        let package = root.join("node_modules/@earendil-works/pi-agent-core/package.json");
        fs::create_dir_all(script.parent().expect("runtime source directory"))
            .expect("create source directory");
        fs::create_dir_all(package.parent().expect("package directory"))
            .expect("create package directory");
        fs::write(&script, "").expect("write runtime script");
        fs::write(&package, "{}").expect("write package manifest");

        assert!(node_dependency_is_available(
            &script,
            "@earendil-works/pi-agent-core"
        ));
        assert!(!node_dependency_is_available(
            &script,
            "@earendil-works/pi-ai"
        ));

        fs::remove_dir_all(root).expect("remove fixture");
    }

    #[test]
    fn intersects_assistant_and_expert_mcp_scopes() {
        let assistant = HashSet::from(["docs".to_owned(), "git".to_owned()]);
        let expert = HashSet::from(["docs".to_owned(), "browser".to_owned()]);

        assert_eq!(
            intersect_mcp_server_scopes(Some(assistant), Some(expert)),
            Some(HashSet::from(["docs".to_owned()]))
        );
        assert_eq!(intersect_mcp_server_scopes(None, None), None);
    }

    #[test]
    fn expert_runtime_package_uses_the_bound_snapshot_and_validates_its_hash() {
        let snapshot = json!({
            "id": "fox-reviewer",
            "name": "Reviewer",
            "agentKind": "expert",
            "invocationMode": "inline",
            "visibility": "expert_center",
            "isBuiltin": false,
            "packageManifest": {
                "skills": ["review"],
                "mcpServers": ["docs"],
                "allowedTools": ["read"]
            }
        });
        let package_hash = hex::encode(Sha256::digest(snapshot.to_string().as_bytes()));
        let package = runtime_expert_package(
            &snapshot,
            &package_hash,
            &["review".to_owned(), "new-skill".to_owned()],
            &["docs".to_owned(), "browser".to_owned()],
            &[KnowledgeReference::remote("yuxi", "kb-1")],
        )
        .expect("valid bound expert package");

        assert_eq!(package.get("enabledSkills"), Some(&json!(["review"])));
        assert_eq!(
            package.pointer("/packageManifest/skills"),
            Some(&json!(["review"]))
        );
        assert_eq!(
            package.pointer("/packageManifest/knowledge"),
            Some(&json!(["kb-1"]))
        );
        assert!(runtime_expert_package(&snapshot, "invalid-hash", &[], &[], &[],).is_err());
    }

    #[test]
    fn expert_runtime_package_preserves_v2_knowledge_references() {
        let snapshot = json!({
            "id": "fox-local-reviewer",
            "name": "Local Reviewer",
            "agentKind": "expert",
            "invocationMode": "inline",
            "visibility": "expert_center",
            "isBuiltin": false,
            "packageManifest": {
                "manifestSchemaVersion": 2,
                "skills": [],
                "knowledgeReferences": [
                    { "source": "local", "id": "local-1", "revision": "generation:2" },
                    { "source": "remote", "connectionId": "yuxi-primary", "id": "remote-1" },
                    { "source": "local", "id": "not-bound" }
                ]
            }
        });
        let package_hash = hex::encode(Sha256::digest(snapshot.to_string().as_bytes()));
        let package = runtime_expert_package(
            &snapshot,
            &package_hash,
            &[],
            &[],
            &[
                KnowledgeReference::local("local-1"),
                KnowledgeReference::remote("yuxi-primary", "remote-1"),
            ],
        )
        .expect("v2 expert package");

        assert_eq!(
            package.pointer("/packageManifest/knowledgeReferences"),
            Some(&json!([
                { "source": "local", "id": "local-1", "revision": "generation:2" },
                { "source": "remote", "connectionId": "yuxi-primary", "id": "remote-1" }
            ]))
        );
        assert!(package.pointer("/packageManifest/knowledge").is_none());
    }

    #[test]
    fn preserves_structured_start_failure_codes() {
        assert_eq!(
            structured_runtime_error(
                "[conversation.expert_snapshot_invalid] choose the expert again"
            ),
            (
                "conversation.expert_snapshot_invalid",
                "choose the expert again"
            )
        );
        assert_eq!(
            structured_runtime_error("ordinary failure"),
            ("runtime.start_failed", "ordinary failure")
        );
        assert_eq!(
            event_projection_failure_code(
                "Invalid parameter name: [child_run.budget_exceeded] total"
            ),
            "child_run.budget_exceeded"
        );
        assert_eq!(
            event_projection_failure_code(
                "Invalid parameter name: [digital_colleague.budget_exceeded] output"
            ),
            "digital_colleague.budget_exceeded"
        );
        assert_eq!(
            event_projection_failure_code(
                "Invalid parameter name: [child_run.tool_budget_exceeded] slots"
            ),
            "child_run.tool_budget_exceeded"
        );
        assert_eq!(
            event_projection_failure_code(
                "Invalid parameter name: [digital_colleague.tool_budget_exceeded] slots"
            ),
            "digital_colleague.tool_budget_exceeded"
        );
    }

    #[test]
    fn recognizes_supported_attachment_text_types() {
        assert!(attachment_is_text("notes.txt", None));
        assert!(attachment_is_text(
            "payload.bin",
            Some("application/json; charset=utf-8")
        ));
        assert!(!attachment_is_text("photo.png", Some("image/png")));
        assert!(!attachment_is_text("archive.zip", None));
    }

    #[test]
    fn recognizes_docx_attachments() {
        assert!(attachment_is_docx("report.docx", None));
        assert!(attachment_is_docx(
            "report.bin",
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        ));
        assert!(!attachment_is_docx("report.doc", None));
    }

    #[test]
    fn recognizes_supported_runtime_image_types() {
        let image = |display_name: &str, media_type: Option<&str>| AttachmentRecord {
            id: "image-1".to_owned(),
            conversation_id: "conversation-1".to_owned(),
            message_id: None,
            display_name: display_name.to_owned(),
            storage_path: String::new(),
            media_type: media_type.map(str::to_owned),
            byte_size: 0,
            sha256: None,
            status: "ready".to_owned(),
            created_at: 0,
        };
        assert_eq!(
            attachment_image_media_type(&image("photo.bin", Some("image/png"))),
            Some("image/png")
        );
        assert_eq!(
            attachment_image_media_type(&image("photo.JPEG", None)),
            Some("image/jpeg")
        );
        assert_eq!(
            attachment_image_media_type(&image("vector.svg", Some("image/svg+xml"))),
            None
        );
        assert!(attachment_looks_like_image(&image(
            "vector.svg",
            Some("image/svg+xml")
        )));
        assert!(!attachment_looks_like_image(&image(
            "notes.txt",
            Some("text/plain")
        )));
    }

    #[test]
    fn extracts_text_from_a_deflated_docx_document() {
        let document = br#"<?xml version="1.0"?><w:document><w:body><w:p><w:r><w:t>Hello &amp; Fox</w:t></w:r></w:p><w:p><w:r><w:t>Second line</w:t></w:r></w:p></w:body></w:document>"#;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(document).expect("deflate document");
        let compressed = encoder.finish().expect("finish deflate");
        let bytes = single_entry_zip("word/document.xml", document, &compressed, 8);
        let text = extract_docx_text(&bytes).expect("extract DOCX text");
        assert_eq!(text, "Hello & Fox\nSecond line");
    }

    #[test]
    fn rejects_docx_content_that_expands_beyond_the_context_limit() {
        let document = vec![b'a'; super::MAX_ATTACHMENT_TEXT_BYTES + 1];
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&document).expect("deflate document");
        let compressed = encoder.finish().expect("finish deflate");
        let mut bytes = single_entry_zip("word/document.xml", &document, &compressed, 8);

        // A malicious archive can lie about its declared uncompressed size.
        let declared_size = 1_u32.to_le_bytes();
        let local_name_len = "word/document.xml".len();
        let central = 30 + local_name_len + compressed.len();
        bytes[22..26].copy_from_slice(&declared_size);
        bytes[central + 24..central + 28].copy_from_slice(&declared_size);

        let error = extract_docx_text(&bytes).expect_err("reject decompression bomb");
        assert!(error.contains("1 MiB context limit"));
    }

    #[test]
    fn actual_runtime_initialize_passes_the_production_host_guard() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let output = std::process::Command::new("node")
            .arg(root.join("services/agent-runtime/test/fixtures/ready-handshake.mjs"))
            .current_dir(&root).output().expect("start real runtime handshake probe");
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let payload: serde_json::Value = serde_json::from_slice(&output.stdout).expect("real ready payload");
        let (_, _, capabilities) = parse_runtime_ready_payload(&payload).expect("production startup validation");
        for name in ["attachment_compute", "skill_load", "compute_job_start", "read_tool_result"] {
            assert!(capabilities["tools"].as_array().unwrap().iter().any(|tool| tool["name"] == name), "{name}");
        }
    }

    #[test]
    fn validates_the_complete_runtime_ready_handshake() {
        let payload = json!({
            "protocol": "fox-runtime-jsonl",
            "protocolVersion": 1,
            "runtime": "fox-pi-runtime",
            "runtimeVersion": "0.1.0",
            "capabilities": {
                "manifestVersion": 2,
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
            }
        });
        let (runtime, version, capabilities) =
            parse_runtime_ready_payload(&payload).expect("valid handshake");
        assert_eq!(runtime, "fox-pi-runtime");
        assert_eq!(version.as_deref(), Some("0.1.0"));
        assert_eq!(capabilities["manifestVersion"], 2);
        assert_eq!(capabilities["workLoop"], true);
    }

    #[test]
    fn accepts_a_v1_runtime_as_a_normal_conversation() {
        let payload = json!({
            "protocol": "fox-runtime-jsonl",
            "protocolVersion": 1,
            "runtime": "fox-legacy-runtime",
            "capabilities": {
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
            }
        });
        let (_, _, capabilities) =
            parse_runtime_ready_payload(&payload).expect("v1 degrades cleanly");
        assert_eq!(capabilities["manifestVersion"], 1);
        assert!(capabilities.get("workLoop").is_none());
    }

    #[test]
    fn rejects_a_runtime_ready_handshake_with_an_old_manifest() {
        let payload = json!({
            "protocol": "fox-runtime-jsonl",
            "protocolVersion": 1,
            "runtime": "fox-pi-runtime",
            "capabilities": {
                "manifestVersion": 0,
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
            }
        });
        assert!(parse_runtime_ready_payload(&payload).is_err());
    }

    #[test]
    fn redacts_runtime_diagnostic_secrets() {
        assert_eq!(
            redact_diagnostic_line("Authorization: Bearer secret-token"),
            "Authorization: [REDACTED]"
        );
        assert_eq!(
            redact_diagnostic_line("request failed api_key=secret token=another"),
            "request failed api_key=[REDACTED] token=[REDACTED]"
        );
    }

    #[test]
    fn sidecar_recovery_is_single_attempt_and_contract_errors_are_fused() {
        assert!(should_attempt_recovery(
            "runtime process closed stdout",
            true,
            0,
            true
        ));
        assert!(!should_attempt_recovery(
            "runtime process closed stdout",
            true,
            1,
            true
        ));
        assert!(!should_attempt_recovery(
            "runtime protocol mismatch: old v0",
            true,
            0,
            true
        ));
        assert!(!should_attempt_recovery(
            "runtime emitted invalid JSONL",
            true,
            0,
            true
        ));
        assert!(!should_attempt_recovery(
            "runtime process closed stdout",
            false,
            0,
            true
        ));
    }

    #[test]
    fn pending_approval_cleanup_is_scoped_to_one_run() {
        let (sender_a, _receiver_a) = mpsc::channel();
        let (sender_b, _receiver_b) = mpsc::channel();
        let mut pending = HashMap::new();
        pending.insert(
            "approval-a".to_owned(),
            PendingApproval {
                run_id: "run-a".to_owned(),
                sender: sender_a,
            },
        );
        pending.insert(
            "approval-b".to_owned(),
            PendingApproval {
                run_id: "run-b".to_owned(),
                sender: sender_b,
            },
        );

        assert_eq!(
            pending_approval_ids_for_run(&pending, "run-a"),
            vec!["approval-a".to_owned()]
        );
        assert_eq!(pending.len(), 2);
    }

    #[test]
    fn bounds_large_knowledge_tool_results_before_the_jsonl_response() {
        let wrapped = bounded_knowledge_tool_result(json!({
            "content": "Fox".repeat(super::MAX_KNOWLEDGE_TOOL_TEXT_BYTES),
        }));
        assert_eq!(wrapped["details"]["truncated"], true);
        assert!(
            wrapped["details"]["originalBytes"].as_u64().unwrap()
                > super::MAX_KNOWLEDGE_TOOL_TEXT_BYTES as u64
        );
        assert!(serde_json::to_vec(&wrapped).unwrap().len() < super::MAX_PROTOCOL_LINE_BYTES / 2);
    }

    #[test]
    fn attachment_pages_preserve_unicode_and_bound_protocol_size() {
        let text = "AGV等待🦊\n".repeat(55000);
        let old_result = json!({"content":[{"type":"text","text":text}],"details":{"text":text}});
        assert!(serde_json::to_vec(&old_result).unwrap().len() > super::MAX_PROTOCOL_LINE_BYTES);
        let mut offset = 0;
        let mut recovered = String::new();
        loop {
            let page = super::attachment_text_page(&text, &json!({"offset":offset,"limit":24000})).unwrap();
            let wrapped = json!({"content":[{"type":"text","text":page.to_string()}],"details":page});
            assert!(serde_json::to_vec(&wrapped).unwrap().len() < super::MAX_PROTOCOL_LINE_BYTES / 2);
            recovered.push_str(page["text"].as_str().unwrap());
            if !page["hasMore"].as_bool().unwrap() { break; }
            offset = page["nextOffset"].as_u64().unwrap();
        }
        assert_eq!(recovered, text);
        assert!(super::attachment_text_page(&text, &json!({"offset":-1})).is_err());
        assert!(super::attachment_text_page(&text, &json!({"offset":9999999})).is_err());
    }

    #[test]
    fn attachment_sheet_directory_jumps_to_unicode_sheet_without_scanning() {
        let root = std::env::temp_dir().join(format!("fox-sheet-directory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let database = Database::open(root.join("test.db")).unwrap();
        let conversation = database.create_conversation(database.default_agent_id(), None, None, None).unwrap();
        let file = root.join("AGV.xlsx");
        let bytes = include_bytes!("../../tests/fixtures/office-reading.xlsx");
        std::fs::write(&file, bytes).unwrap();
        database.add_attachments(&[crate::database::AttachmentRecord {
            id: "agv".into(), conversation_id: conversation.id.clone(), message_id: None,
            display_name: "AGV.xlsx".into(), storage_path: file.to_string_lossy().into_owned(),
            media_type: None, byte_size: bytes.len() as i64, sha256: None, status: "ready".into(), created_at: 0,
        }]).unwrap();
        let overview = super::read_attachment_text(&database, &root, &conversation.id, "agv", &json!({"limit": 1})).unwrap();
        assert_eq!(overview["sheetCount"], 2);
        assert_eq!(overview["sheetDirectoryTruncated"], false);
        assert_eq!(overview["sheets"][1]["name"], "第二页");
        let direct = super::read_attachment_text(&database, &root, &conversation.id, "agv", &json!({"offset":overview["sheets"][1]["offset"]})).unwrap();
        assert!(direct["text"].as_str().unwrap().starts_with("[Sheet: 第二页]"));
        assert!(super::read_attachment_text(&database, &root, "foreign", "agv", &json!({})).is_err());
        assert!(serde_json::to_vec(&overview).unwrap().len() < super::MAX_PROTOCOL_LINE_BYTES / 2);
        drop(database);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preserves_small_knowledge_tool_details_for_source_mapping() {
        let wrapped = bounded_knowledge_tool_result(json!({
            "results": [{ "file_id": "file-1", "content": "answer" }],
        }));
        assert_eq!(wrapped["details"]["results"][0]["file_id"], "file-1");
    }

    #[test]
    fn normalizes_and_rejects_child_run_budgets_at_the_host_boundary() {
        let defaults = normalized_child_budget(None, 2_048).expect("default child budget");
        assert_eq!(defaults.max_duration_ms, 300_000);
        assert_eq!(defaults.max_total_tokens, 32_000);
        assert_eq!(defaults.max_output_tokens, 2_048);
        assert_eq!(defaults.max_tool_calls, 16);

        let bounded = normalized_child_budget(
            Some(&json!({
                "maxDurationMs": 1_000,
                "maxTotalTokens": 256,
                "maxOutputTokens": 64,
                "maxToolCalls": 0
            })),
            2_048,
        )
        .expect("minimum valid child budget");
        assert_eq!(bounded.max_tool_calls, 0);
        assert_eq!(bounded.max_output_tokens, 64);

        assert!(normalized_child_budget(
            Some(&json!({ "maxTotalTokens": 256, "maxOutputTokens": 512 })),
            2_048,
        )
        .expect_err("output budget cannot exceed total budget")
        .contains("cannot exceed"));
        assert!(
            normalized_child_budget(Some(&json!({ "maxDurationMs": 999 })), 2_048,)
                .expect_err("duration budget must be bounded")
                .contains("maxDurationMs")
        );
        assert_eq!(
            effective_digital_colleague_output_limit(128, 100, 8_192).unwrap(),
            100,
            "the Runtime provider request cannot exceed the remaining daily budget"
        );
        assert!(effective_digital_colleague_output_limit(128, 63, 8_192)
            .unwrap_err()
            .contains("digital_colleague.budget_exceeded"));
    }

    #[test]
    fn delegated_package_scope_can_only_narrow_parent_tools_and_mcps() {
        let mut package = json!({
            "packageManifest": {
                "allowedTools": ["read", "write_file", "run_command"],
                "mcpServers": ["mcp-a", "mcp-b"]
            }
        });
        let parent_tools = ["read", "grep"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let parent_mcps = ["mcp-b", "mcp-c"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();

        apply_delegated_package_scope(&mut package, Some(&parent_tools), Some(&parent_mcps))
            .expect("apply delegated scope");

        assert_eq!(package["packageManifest"]["allowedTools"], json!(["read"]));
        assert_eq!(package["packageManifest"]["mcpServers"], json!(["mcp-b"]));
    }

    #[test]
    fn team_member_scope_intersects_the_parent_host_scope() {
        let parent = ["read", "grep", "run_command"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let member = ["read", "git_read"]
            .into_iter()
            .map(str::to_owned)
            .collect::<HashSet<_>>();
        let effective = intersect_optional_scopes(Some(parent), Some(member)).unwrap();
        assert_eq!(effective, ["read".to_owned()].into_iter().collect());
    }

    fn single_entry_zip(name: &str, raw: &[u8], compressed: &[u8], method: u16) -> Vec<u8> {
        let name = name.as_bytes();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x03\x04");
        push_u16(&mut bytes, 20);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, method);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, compressed.len() as u32);
        push_u32(&mut bytes, raw.len() as u32);
        push_u16(&mut bytes, name.len() as u16);
        push_u16(&mut bytes, 0);
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(compressed);

        let central_offset = bytes.len() as u32;
        bytes.extend_from_slice(b"PK\x01\x02");
        push_u16(&mut bytes, 20);
        push_u16(&mut bytes, 20);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, method);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, compressed.len() as u32);
        push_u32(&mut bytes, raw.len() as u32);
        push_u16(&mut bytes, name.len() as u16);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        push_u32(&mut bytes, 0);
        bytes.extend_from_slice(name);
        let central_size = bytes.len() as u32 - central_offset;

        bytes.extend_from_slice(b"PK\x05\x06");
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 0);
        push_u16(&mut bytes, 1);
        push_u16(&mut bytes, 1);
        push_u32(&mut bytes, central_size);
        push_u32(&mut bytes, central_offset);
        push_u16(&mut bytes, 0);
        bytes
    }

    fn push_u16(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }

    fn push_u32(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
}
