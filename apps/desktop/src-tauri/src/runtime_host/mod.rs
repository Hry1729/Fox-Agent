mod capability_tools;
mod protocol;
mod work_tools;

#[cfg(test)]
pub(crate) use work_tools::WORK_TOOLS;

use crate::yuxi::{get_access_token, YuxiClient};
use crate::{
    app_state::AppState,
    database::{
        AgentRecord, ApprovalDecision, AttachmentRecord, ChildRunBudget, ChildRunRecord, Database,
        EvaluationRunSummary, KnowledgeReference, MemoryProposalInput,
        PreparedDigitalColleagueTrigger, StartRunResult, ToolCallRecord,
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
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(10);
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
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

struct WorkerHandle {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: PendingResponses,
    current_conversation_id: Option<String>,
    active_run_id: Option<String>,
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
    state: String,
    worker: Option<WorkerHandle>,
    stderr: VecDeque<String>,
    last_error: Option<String>,
    runtime: String,
    runtime_version: Option<String>,
    capabilities: Value,
    pending_approvals: PendingApprovals,
    queued_runs: VecDeque<QueuedRun>,
    dispatching_run_id: Option<String>,
    dispatching_conversation_id: Option<String>,
    cancelled_dispatches: HashSet<String>,
    recovery_attempts: u8,
    last_recovery_at: Option<i64>,
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
    yuxi_client: YuxiClient,
    child_hosts: Arc<Mutex<HashMap<String, RuntimeHost>>>,
    run_budget_override: Option<ChildRunBudget>,
    tool_allowlist_override: Option<HashSet<String>>,
    mcp_server_scope_override: Option<HashSet<String>>,
    digital_scheduler_started: Arc<AtomicBool>,
    digital_scheduler_stop: Arc<AtomicBool>,
}

impl RuntimeHost {
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
        Self {
            app,
            database,
            state: Arc::new(Mutex::new(RuntimeHostState {
                state: "stopped".to_owned(),
                worker: None,
                stderr: VecDeque::new(),
                last_error: None,
                runtime: "not-started".to_owned(),
                runtime_version: None,
                capabilities: json!({}),
                pending_approvals: Arc::new(Mutex::new(HashMap::new())),
                queued_runs: VecDeque::new(),
                dispatching_run_id: None,
                dispatching_conversation_id: None,
                cancelled_dispatches: HashSet::new(),
                recovery_attempts: 0,
                last_recovery_at: None,
            })),
            run_transition: Arc::new(Mutex::new(())),
            sessions_dir,
            attachments_dir,
            skills_dir,
            yuxi_client,
            child_hosts: Arc::new(Mutex::new(HashMap::new())),
            run_budget_override: None,
            tool_allowlist_override: None,
            mcp_server_scope_override: None,
            digital_scheduler_started: Arc::new(AtomicBool::new(false)),
            digital_scheduler_stop: Arc::new(AtomicBool::new(false)),
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
            "steering": false,
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
            if error == CANCELLED_BEFORE_SUBMISSION {
                return result;
            }
            if let Ok(mut state) = self.state.lock() {
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
            if state.dispatching_run_id.is_some()
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
                    } else {
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
    }

    fn start_run_inner(
        &self,
        started: &StartRunResult,
        text: &str,
        attachments: &[AttachmentRecord],
    ) -> Result<(), String> {
        if let Ok(mut state) = self.state.lock() {
            state.recovery_attempts = 0;
        }
        let run_kind = if self.database.is_digital_colleague_run(&started.run.id)? {
            "digital_colleague"
        } else if self.run_budget_override.is_some() {
            "child"
        } else {
            "primary"
        };
        crate::lifecycle_hooks::run_event(
            &self.database,
            "before_run",
            &started.run.id,
            run_kind,
            &json!({
                "conversationId": started.run.conversation_id,
                "runKind": run_kind,
                "depth": if run_kind == "child" { 1 } else { 0 },
            }),
        )?;
        self.ensure_worker(&started.run.conversation_id)?;
        let (runtime_session_id, _) = self.open_runtime_session(&started.run.conversation_id)?;
        let images = self.runtime_images(&started.run.conversation_id, attachments)?;

        let model_service = self
            .database
            .get_model_service()?
            .ok_or_else(|| "model service is not configured".to_owned())?;
        let effective_max_output_tokens = self
            .run_budget_override
            .as_ref()
            .map(|budget| budget.max_output_tokens)
            .unwrap_or(model_service.max_output_tokens)
            .min(model_service.max_output_tokens);
        let input_tokens = model_service
            .context_window
            .saturating_sub(effective_max_output_tokens)
            .max(4096);
        let input_tokens = self
            .run_budget_override
            .as_ref()
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
        let system_prompt = self
            .database
            .conversation_system_prompt(&started.run.conversation_id)?;
        let agent_id = self
            .database
            .conversation_agent_id(&started.run.conversation_id)?;
        let assistant = self
            .database
            .get_agent(&agent_id)?
            .ok_or_else(|| "conversation assistant was not found".to_owned())?;
        let expert_binding = self
            .database
            .current_conversation_expert_binding(&started.run.conversation_id)?;
        let assistant_skills = self.database.enabled_agent_skills(&assistant.id)?;
        let configured_expert_skills = expert_binding
            .as_ref()
            .map(|binding| self.database.enabled_agent_skills(&binding.expert_id))
            .transpose()?
            .unwrap_or_default();
        let enabled_mcps = self
            .database
            .list_mcp_servers()?
            .into_iter()
            .filter(|server| server.enabled)
            .map(|server| server.id)
            .filter(|server_id| {
                self.mcp_server_scope_override
                    .as_ref()
                    .is_none_or(|scope| scope.contains(server_id))
            })
            .collect::<Vec<_>>();
        let bound_knowledge = self
            .database
            .conversation_knowledge_references(&started.run.conversation_id)?;
        let mut assistant_package = runtime_agent_package(
            &assistant,
            &assistant_skills,
            &enabled_mcps,
            &bound_knowledge,
        )?;
        apply_delegated_package_scope(
            &mut assistant_package,
            self.tool_allowlist_override.as_ref(),
            self.mcp_server_scope_override.as_ref(),
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
        let skill_prompt = crate::skills::enabled_skill_prompt(&self.skills_dir, &enabled_skills)?;
        let system_prompt = if skill_prompt.is_empty() {
            system_prompt
        } else {
            format!(
                "{system_prompt}\n\n# Enabled Assistant and Expert Skills\nThe following instruction-only Skills are enabled for the current assistant and expert. Follow them when relevant; they do not grant additional tool permissions.\n\n{skill_prompt}"
            )
        };
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
        let project_access = self
            .database
            .conversation_project_access(&started.run.conversation_id)?;
        let project_context = match project_access {
            Some((project_root, permission_mode)) => json!({
                "projectRoot": project_root,
                "permissionMode": permission_mode,
            }),
            None => json!({
                "projectRoot": null,
                "permissionMode": "read_only",
            }),
        };
        let work_snapshot = work_tools::snapshot(&self.database, &started.run.conversation_id)?;
        let memory_context = self.database.recall_memories(
            &started.run.conversation_id,
            Some(&started.run.id),
            text,
            8,
            6_000,
        )?;

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
            if let Some(worker) = state.worker.as_mut() {
                worker.active_run_id = Some(started.run.id.clone());
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
                "systemPrompt": system_prompt,
                "images": images,
                "assistantPackage": assistant_package,
                "expertBinding": expert_binding_payload,
                "expertPackage": expert_package,
                "projectContext": project_context,
                "workSnapshot": work_snapshot,
                "memoryContext": memory_context,
                "promptBudget": { "maxPromptTokens": input_tokens },
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
        let child_run_id = child_run.child_run_id.clone();
        let mut child_host = RuntimeHost::new(
            self.app.clone(),
            self.database.clone(),
            self.sessions_dir.clone(),
            self.attachments_dir.clone(),
            self.skills_dir.clone(),
            self.yuxi_client.clone(),
        );
        child_host.run_budget_override = Some(child_run.budget.clone());
        let shared_pending_approvals = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .pending_approvals
            .clone();
        child_host
            .state
            .lock()
            .map_err(|_| "child runtime state lock is poisoned".to_owned())?
            .pending_approvals = shared_pending_approvals;
        let (mut tool_scope, mcp_scope) =
            effective_conversation_delegation_scope(&self.database, &parent_conversation_id)?;
        tool_scope = intersect_optional_scopes(
            tool_scope,
            child_run
                .allowed_tools
                .as_ref()
                .map(|tools| tools.iter().cloned().collect()),
        );
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
            if let Err(error) = child_host.start_run(&started, &prompt, &[]) {
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
            } else {
                if let Ok(Some(record)) = coordinator.database.child_run(&child_run_id) {
                    coordinator.emit_child_run_update(&parent_conversation_id, &record);
                }
                let deadline =
                    Instant::now() + Duration::from_millis(child_run.budget.max_duration_ms as u64);
                loop {
                    let status = coordinator
                        .database
                        .child_run_status(&child_run_id)
                        .ok()
                        .flatten();
                    if status.as_deref().is_some_and(run_status_is_terminal) {
                        break;
                    }
                    let token_budget_exceeded = coordinator
                        .database
                        .child_budget_exceeded(&child_run_id)
                        .unwrap_or(false);
                    let duration_exceeded = Instant::now() >= deadline;
                    if token_budget_exceeded || duration_exceeded {
                        let _ = child_host.cancel_run(&child_run_id);
                        let _ = child_host.stop_worker();
                        let (code, message) = if token_budget_exceeded {
                            (
                                "child_run.token_budget_exceeded",
                                "Child Run exceeded its Host-enforced total-token budget.",
                            )
                        } else {
                            (
                                "child_run.duration_budget_exceeded",
                                "Child Run exceeded its Host-enforced duration budget.",
                            )
                        };
                        let _ = coordinator
                            .database
                            .mark_run_failed(&child_run_id, code, message);
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            let _ = child_host.stop_worker();
            let _ = coordinator.database.sync_child_run_terminal(&child_run_id);
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
        let mut managed_host = RuntimeHost::new(
            self.app.clone(),
            self.database.clone(),
            self.sessions_dir.clone(),
            self.attachments_dir.clone(),
            self.skills_dir.clone(),
            self.yuxi_client.clone(),
        );
        managed_host.run_budget_override = Some(ChildRunBudget {
            max_duration_ms: prepared.colleague.max_duration_ms,
            max_total_tokens: prepared.remaining_tokens.min(200_000),
            max_output_tokens: prepared.colleague.max_output_tokens,
            max_tool_calls: prepared.colleague.max_tool_calls,
        });
        let shared_pending_approvals = self
            .state
            .lock()
            .map_err(|_| "runtime state lock is poisoned".to_owned())?
            .pending_approvals
            .clone();
        managed_host
            .state
            .lock()
            .map_err(|_| "digital colleague runtime state lock is poisoned".to_owned())?
            .pending_approvals = shared_pending_approvals;
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
                    let status = coordinator
                        .database
                        .digital_colleague_run_status(&run_id)
                        .ok()
                        .flatten();
                    if status.as_deref().is_some_and(run_status_is_terminal) {
                        break;
                    }
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

    fn cancel_child_runtime(&self, child_run_id: &str) -> Result<bool, String> {
        let child_host = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .get(child_run_id)
            .cloned();
        let Some(child_host) = child_host else {
            if self
                .database
                .child_run_status(child_run_id)?
                .as_deref()
                .is_some_and(run_status_is_terminal)
            {
                return Ok(false);
            }
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
        let _ = self.database.mark_run_cancelling(child_run_id)?;
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

    pub(crate) fn cancel_managed_run(&self, run_id: &str) -> Result<bool, String> {
        let is_managed = self
            .child_hosts
            .lock()
            .map_err(|_| "managed runtime map is poisoned".to_owned())?
            .contains_key(run_id);
        if is_managed {
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
        for member in &snapshot.members {
            if !run_status_is_terminal(&member.status) {
                let _ = self.cancel_child_runtime(&member.child_run_id);
            }
        }
        self.database.cancel_expert_team(team_run_id, reason)
    }

    pub fn cancel_run(&self, run_id: &str) -> Result<bool, String> {
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
            let _ = pending.sender.send(decision.approved());
        }
        let conversation_id = approval.conversation_id.clone();
        let tool_name = approval.tool_name.clone();
        let _ = self.app.emit("fox://approval-resolved", approval);
        if decision == ApprovalDecision::AllowConversation {
            if let Ok(queued) = self
                .database
                .pending_approvals_for_conversation_tool(&conversation_id, &tool_name)
            {
                for queued_approval in queued {
                    let Ok(Some(resolved)) = self
                        .database
                        .resolve_approval(&queued_approval.id, ApprovalDecision::AllowConversation)
                    else {
                        continue;
                    };
                    let sender = self.state.lock().ok().and_then(|state| {
                        state.pending_approvals.lock().ok()?.remove(&resolved.id)
                    });
                    if let Some(pending) = sender {
                        let _ = pending.sender.send(true);
                    }
                    let _ = self.app.emit("fox://approval-resolved", resolved);
                }
            }
        }
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
        let child_hosts = self
            .child_hosts
            .lock()
            .map_err(|_| "child runtime map is poisoned".to_owned())?
            .drain()
            .collect::<Vec<_>>();
        for (run_id, child_host) in child_hosts {
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
            });
        }

        self.spawn_stdout_reader(stdout, stdin.clone(), pending);
        self.spawn_stderr_reader(stderr);

        let model_service = self
            .database
            .get_model_service()?
            .ok_or_else(|| "model service is not configured".to_owned())?;
        let api_key = crate::model_service::get_api_key(&model_service.base_url);
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
                        .map(|budget| budget.max_output_tokens)
                        .unwrap_or(model_service.max_output_tokens)
                        .min(model_service.max_output_tokens),
                    "supportsImageInput": model_service.supports_image_input,
                    "apiKey": api_key,
                }
            })),
            RESPONSE_TIMEOUT,
        )?;
        if response.r#type != "ready" {
            let error = response_error(&response, "runtime handshake failed");
            let _ = self.stop_worker();
            return Err(error);
        }

        let (runtime, runtime_version, capabilities) = match response
            .payload
            .as_ref()
            .ok_or_else(|| "runtime handshake omitted its payload".to_owned())
            .and_then(parse_runtime_ready_payload)
        {
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
                    let tool = envelope
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("tool"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
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
                    let (project_root, permission_mode) = project_access
                        .as_ref()
                        .map(|(root, mode)| (Some(root.as_str()), mode.as_str()))
                        .unwrap_or((None, "read_only"));
                    let expert_guard = envelope
                        .conversation_id
                        .as_deref()
                        .map(|conversation_id| {
                            ensure_expert_tool_allowed(&database, conversation_id, tool)
                        })
                        .transpose();
                    let delegated_guard = runtime_host.ensure_delegated_tool_allowed(tool);
                    let (allowed, payload) = match delegated_guard.and(expert_guard.map(|_| ())) {
                        Ok(_) => crate::tool_guard::preflight_payload(
                            tool,
                            &input,
                            project_root,
                            permission_mode,
                        ),
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
                    let response = HostResponse::for_request(&envelope, response_type, payload);
                    if let Err(error) = write_protocol_message(&stdin, &response) {
                        runtime_host.handle_runtime_crash(error);
                        return;
                    }
                    continue;
                }

                if envelope.kind == "request" && envelope.r#type == "tool.execute" {
                    let tool = envelope
                        .payload
                        .as_ref()
                        .and_then(|payload| payload.get("tool"))
                        .and_then(Value::as_str)
                        .unwrap_or_default();
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
                    if let Some(run_id) = envelope.run_id.as_deref() {
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
                            if let Err(write_error) = write_protocol_message(&stdin, &response) {
                                runtime_host.handle_runtime_crash(write_error);
                                return;
                            }
                            continue;
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
                    let child_runtime_host = runtime_host.clone();
                    thread::spawn(move || {
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
                        } else if tool == "read_attachment" {
                            handle_attachment_tool_request(
                                &app,
                                &database,
                                &attachments_dir,
                                &state,
                                &stdin,
                                envelope,
                            )
                        } else if capability_tools::is_capability_tool(tool) {
                            handle_capability_tool_request(
                                &app, &database, &state, &stdin, envelope,
                            )
                        } else if work_tools::is_work_tool(tool) {
                            handle_work_tool_request(&app, &database, &state, &stdin, envelope)
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
                    let (Some(conversation_id), Some(run_id), Some(seq), Some(payload)) = (
                        envelope.conversation_id.clone(),
                        envelope.run_id.clone(),
                        envelope.seq,
                        envelope.payload.clone(),
                    ) else {
                        continue;
                    };
                    match database.apply_runtime_event(&run_id, seq, &payload) {
                        Ok(true) => {
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
                                Some("run.completed" | "run.cancelled" | "run.failed")
                            ) {
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
                        Ok(false) => {}
                        Err(error) => {
                            let _ = database.mark_run_failed(
                                &run_id,
                                "storage.event_projection_failed",
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
            if self
                .database
                .apply_runtime_event(&run_id, seq, &payload)
                .ok()
                == Some(true)
            {
                let _ = self.app.emit(
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
    manifest.validate()?;
    Ok((runtime, runtime_version, capabilities))
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
    app: &AppHandle,
    database: &Database,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response = match execute_work_tool_request(app, database, state, &envelope) {
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
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = database.create_host_tool_call(
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
    )?;
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

    let outcome: Result<Value, String> = (|| match tool {
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
            if snapshot
                .members
                .iter()
                .any(|member| matches!(member.status.as_str(), "queued" | "running" | "cancelling"))
            {
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
            Ok(json!({ "teamRunId": snapshot.run.id, "childRun": child_run, "created": created }))
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
        "child_agent_list" => Ok(json!({ "agents": database.list_child_agents()? })),
        "child_run_start" => {
            let objective = required_bounded_text(&input, "objective", 8_000)?;
            let context = optional_bounded_text(&input, "context", 12_000)?;
            let agent_id = input
                .get("agentId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("fox-general");
            let model_max_output = database
                .get_model_service()?
                .ok_or_else(|| "model service is not configured".to_owned())?
                .max_output_tokens;
            let budget = normalized_child_budget(input.get("budget"), model_max_output)?;
            let (started, child_run, created) = database
                .create_child_run(crate::database::CreateChildRunInput {
                    parent_run_id: run_id,
                    tool_call_id,
                    worker_agent_id: agent_id,
                    objective,
                    context,
                    budget: &budget,
                    team_run_id: None,
                    team_member_id: None,
                    allowed_tools: None,
                })
                .map_err(|error| {
                    format!(
                        "child_run.start_rejected: the agent, depth, concurrency, or per-parent limit rejected this Child Run ({error})"
                    )
                })?;
            if created || child_run.status == "queued" {
                runtime_host.start_child_runtime(
                    started,
                    child_run.clone(),
                    conversation_id.to_owned(),
                )?;
            }
            Ok(json!({ "childRun": child_run, "created": created }))
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
                        "one or more Child Runs do not belong to the current parent Run".to_owned(),
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
            let cancelled = runtime_host.cancel_child_runtime(child_run_id)?;
            let child_run = database.child_run(child_run_id)?;
            Ok(json!({ "cancelled": cancelled, "childRun": child_run }))
        }
        _ => unreachable!(),
    })();

    match outcome {
        Ok(result) => {
            database.complete_host_tool_call(run_id, tool_call_id, Some(&result), None)?;
            let hook_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
            Ok(json!({
                "isError": false,
                "result": result,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))?;
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
        }
    }
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
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = database.create_host_tool_call(
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
    )?;
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
    let outcome = match tool {
        "memory_search" => {
            let query = input.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = input.get("limit").and_then(Value::as_u64).unwrap_or(8) as usize;
            database
                .recall_memories(conversation_id, Some(run_id), query, limit, 8_000)
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
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
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()))
        }
        _ => Err(format!("unsupported memory tool: {tool}")),
    };
    match outcome {
        Ok(result) => {
            database.complete_host_tool_call(run_id, tool_call_id, Some(&result), None)?;
            let hook_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
            Ok(json!({
                "isError": false,
                "result": result,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))?;
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
        }
    }
}

fn execute_work_tool_request(
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
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)
            .map_err(work_tools::WorkToolError::from)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(work_tools::WorkToolError::from(format!(
            "[hook.blocked] {reason}"
        )));
    }
    let tool_call = database.create_host_tool_call(
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
    )?;
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
        )
        .map_err(work_tools::WorkToolError::from)?;
    }
    match work_tools::execute(database, conversation_id, run_id, tool, &input) {
        Ok(outcome) => {
            database.complete_host_tool_call(run_id, tool_call_id, Some(&outcome.result), None)?;
            for event in outcome.events {
                let _ = app.emit("fox://work-event", event);
            }
            let hook_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)
                    .map_err(work_tools::WorkToolError::from)?;
            Ok(json!({
                "isError": false,
                "result": outcome.result,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            let message = error.to_string();
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&message))?;
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
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
        }),
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
    let approval_result = receiver.recv_timeout(APPROVAL_TIMEOUT);
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
            return Err(match error {
                std::sync::mpsc::RecvTimeoutError::Timeout => {
                    "Lifecycle Hook approval request timed out".to_owned()
                }
                std::sync::mpsc::RecvTimeoutError::Disconnected => {
                    "Lifecycle Hook approval request was interrupted".to_owned()
                }
            });
        }
    };
    if !approved {
        database.complete_host_tool_call(
            run_id,
            &tool_call.runtime_tool_call_id,
            None,
            Some("The user denied the lifecycle Hook policy approval."),
        )?;
        return Err("The user denied the lifecycle Hook policy approval.".to_owned());
    }
    Ok(())
}

fn merge_hook_annotations(mut before: Vec<String>, after: Vec<String>) -> Vec<String> {
    before.extend(after);
    before
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
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let requires_approval = hook_decision.requires_approval
        || (tool == "call_mcp_tool"
            && !database.conversation_tool_permission_granted(conversation_id, tool)?);
    let tool_call = database.create_host_tool_call(
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
    )?;
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
            let approval = database.create_approval(
                &tool_call.id,
                &format!("调用 MCP 工具 {} / {}", server.name, remote_tool),
                &json!({
                    "tool": "call_mcp_tool",
                    "title": "调用 MCP 工具",
                    "target": format!("{} / {}", server.name, remote_tool),
                    "summary": format!("允许 Fox 调用 MCP Server {} 的工具 {}", server.name, remote_tool),
                    "serverId": server.id,
                    "serverName": server.name,
                    "mcpTool": remote_tool,
                    "arguments": arguments,
                }),
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
            let approval_result = receiver.recv_timeout(APPROVAL_TIMEOUT);
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
                    return Err(match error {
                        std::sync::mpsc::RecvTimeoutError::Timeout => {
                            "MCP approval request timed out".to_owned()
                        }
                        std::sync::mpsc::RecvTimeoutError::Disconnected => {
                            "MCP approval request was interrupted".to_owned()
                        }
                    });
                }
            };
            if !approved {
                database.complete_host_tool_call(
                    run_id,
                    tool_call_id,
                    None,
                    Some("The user denied this MCP operation."),
                )?;
                return Err("The user denied this MCP operation.".to_owned());
            }
        }
        let health_started = Instant::now();
        match crate::mcp::call_tool(&server, remote_tool, &arguments) {
            Ok(result) => {
                let _ = database.record_mcp_health(
                    &server.id,
                    "connected",
                    None,
                    Some(health_started.elapsed().as_millis() as i64),
                    server.tool_count,
                );
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
    let hook_annotations =
        crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
    let wrapped = json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string()) }],
        "details": result,
        "untrusted": true,
        "sourceType": "knowledge",
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
    });
    database.complete_host_tool_call(run_id, tool_call_id, Some(&wrapped), None)?;
    Ok(json!({ "isError": false, "result": wrapped }))
}

fn handle_attachment_tool_request(
    app: &AppHandle,
    database: &Database,
    attachments_dir: &std::path::Path,
    state: &Arc<Mutex<RuntimeHostState>>,
    stdin: &Arc<Mutex<ChildStdin>>,
    envelope: RuntimeEnvelope,
) {
    let response =
        execute_attachment_tool_request(app, database, attachments_dir, state, &envelope)
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
    ensure_expert_tool_allowed(database, conversation_id, "read_attachment")?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let hook_decision = crate::lifecycle_hooks::before_tool(
        database,
        run_id,
        tool_call_id,
        "read_attachment",
        &input,
    )?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let attachment_id = input
        .get("attachmentId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "attachmentId is required".to_owned())?;
    let tool_call = database.create_host_tool_call(
        run_id,
        tool_call_id,
        "read_attachment",
        &input,
        if hook_decision.requires_approval {
            "pending"
        } else {
            "running"
        },
        hook_decision.requires_approval,
    )?;
    if hook_decision.requires_approval {
        request_declarative_hook_approval(
            app,
            database,
            state,
            run_id,
            &tool_call,
            "read_attachment",
            &input,
            hook_decision.approval_reason.as_deref(),
        )?;
    }
    match read_attachment_text(database, attachments_dir, conversation_id, attachment_id) {
        Ok(result) => {
            let wrapped = json!({
                "content": [{ "type": "text", "text": result["text"] }],
                "details": result,
            });
            database.complete_host_tool_call(run_id, tool_call_id, Some(&wrapped), None)?;
            let hook_annotations = crate::lifecycle_hooks::after_tool(
                database,
                run_id,
                tool_call_id,
                "read_attachment",
                true,
            )?;
            Ok(json!({
                "isError": false,
                "result": wrapped,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))?;
            let _ = crate::lifecycle_hooks::after_tool(
                database,
                run_id,
                tool_call_id,
                "read_attachment",
                false,
            );
            Err(error)
        }
    }
}

fn read_attachment_text(
    database: &Database,
    attachments_dir: &std::path::Path,
    conversation_id: &str,
    attachment_id: &str,
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
    let text = if attachment_is_docx(&attachment.display_name, attachment.media_type.as_deref()) {
        extract_docx_text(&bytes)?
    } else {
        if !attachment_is_text(&attachment.display_name, attachment.media_type.as_deref()) {
            return Err("Attachment is not a supported text or DOCX file".to_owned());
        }
        if bytes.contains(&0) {
            return Err("Attachment contains binary data".to_owned());
        }
        String::from_utf8(bytes).map_err(|_| "Attachment is not valid UTF-8 text".to_owned())?
    };
    if text.len() > MAX_ATTACHMENT_TEXT_BYTES {
        return Err("Extracted attachment text exceeds the 1 MiB context limit".to_owned());
    }
    Ok(json!({
        "attachmentId": attachment.id,
        "displayName": attachment.display_name,
        "mediaType": attachment.media_type,
        "byteSize": metadata.len(),
        "text": text,
    }))
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
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    let tool_call = database.create_host_tool_call(
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
    )?;
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
                for binding in &bindings {
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
                    let service = database
                        .get_yuxi_service()?
                        .ok_or_else(|| "Yuxi service is not configured".to_owned())?;
                    let token = get_access_token(&service.base_url)
                        .ok_or_else(|| "Yuxi authentication is not configured".to_owned())?;
                    yuxi_client
                        .query_knowledge(&service.base_url, &token, &target.id, query)
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
                return local_knowledge
                    .read_document(&target.id, document_id, chunk_id, requested_max_chars)
                    .map(|value| {
                        json!({
                            "reference": target.as_json(),
                            "result": value,
                        })
                    })
                    .map_err(|error| format!("[{}] {}", error.code(), error));
            }
            let service = database
                .get_yuxi_service()?
                .ok_or_else(|| "Yuxi service is not configured".to_owned())?;
            let token = get_access_token(&service.base_url)
                .ok_or_else(|| "Yuxi authentication is not configured".to_owned())?;
            yuxi_client
                .knowledge_document_content(&service.base_url, &token, &target.id, document_id)
                .await?
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
            let service = database
                .get_yuxi_service()?
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
    let wrapped = bounded_knowledge_tool_result(result);
    database.complete_host_tool_call(run_id, tool_call_id, Some(&wrapped), None)?;
    let hook_annotations =
        crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
    Ok(json!({
        "isError": false,
        "result": wrapped,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
    }))
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
        return ("knowledge.tool_failed", message, true);
    };
    let Some((code, detail)) = rest.split_once(']') else {
        return ("knowledge.tool_failed", message, true);
    };
    let retryable = matches!(
        code,
        "local_knowledge.retrieval_unavailable"
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
    ensure_expert_tool_allowed(database, conversation_id, tool)?;
    let input = payload.get("input").cloned().unwrap_or_else(|| json!({}));
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    ensure_plan_approved_before_mutation(database, conversation_id, tool, &input)?;
    let (project_root, permission_mode) = database
        .conversation_project_access(conversation_id)?
        .ok_or_else(|| "this conversation has no authorized project folder".to_owned())?;
    if permission_mode == "read_only" {
        return Err(format!("tool {tool} is blocked in read-only mode"));
    }

    let prepared = crate::tool_host::prepare(tool, &input, &project_root)?;
    let requires_approval = hook_decision.requires_approval
        || ((permission_mode == "ask" || tool == "run_command")
            && !database.conversation_tool_permission_granted(conversation_id, tool)?);
    let tool_call = database.create_host_tool_call(
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
    )?;

    if requires_approval {
        let request =
            serde_json::to_value(prepared.preview()).map_err(|error| error.to_string())?;
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
        let approval_result = receiver.recv_timeout(APPROVAL_TIMEOUT);
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
                return Err(match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {
                        "Approval request timed out".to_owned()
                    }
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        "Approval request was interrupted".to_owned()
                    }
                });
            }
        };
        if !approved {
            database.complete_host_tool_call(
                run_id,
                tool_call_id,
                None,
                Some("The user denied this operation."),
            )?;
            return Err("The user denied this operation.".to_owned());
        }
    }

    match crate::tool_host::execute(prepared) {
        Ok(result) => {
            database.complete_host_tool_call(run_id, tool_call_id, Some(&result), None)?;
            let hook_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
            Ok(json!({
                "isError": false,
                "result": result,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))?;
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
        }
    }
}

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
    let hook_decision =
        crate::lifecycle_hooks::before_tool(database, run_id, tool_call_id, tool, &input)?;
    if let Some(reason) = &hook_decision.blocked {
        return Err(format!("[hook.blocked] {reason}"));
    }
    ensure_plan_approved_before_mutation(database, conversation_id, tool, &input)?;
    let project_access = database.conversation_project_access(conversation_id)?;
    let project_root = project_access.as_ref().map(|(root, _)| root.as_str());
    let permission_mode = project_access
        .as_ref()
        .map(|(_, mode)| mode.as_str())
        .unwrap_or("read_only");
    let prepared = capability_tools::prepare(tool, &input, project_root)?;
    if permission_mode == "read_only" && prepared.blocked_in_read_only() {
        return Err(format!("tool {tool} is blocked in read-only mode"));
    }

    let requires_approval = hook_decision.requires_approval
        || (prepared.requires_approval()
            && !database.conversation_tool_permission_granted(conversation_id, tool)?);
    let tool_call = database.create_host_tool_call(
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
    )?;

    if requires_approval {
        let request =
            serde_json::to_value(prepared.preview()).map_err(|error| error.to_string())?;
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
        let approval_result = receiver.recv_timeout(APPROVAL_TIMEOUT);
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
                let message = match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => "Approval request timed out",
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        "Approval request was interrupted"
                    }
                };
                database.complete_host_tool_call(run_id, tool_call_id, None, Some(message))?;
                return Err(message.to_owned());
            }
        };
        if !approved {
            database.complete_host_tool_call(
                run_id,
                tool_call_id,
                None,
                Some("The user denied this operation."),
            )?;
            return Err("The user denied this operation.".to_owned());
        }
    }

    match capability_tools::execute(prepared) {
        Ok(result) => {
            database.complete_host_tool_call(run_id, tool_call_id, Some(&result), None)?;
            let hook_annotations =
                crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, true)?;
            Ok(json!({
                "isError": false,
                "result": result,
                "hookAnnotations": merge_hook_annotations(hook_decision.annotations, hook_annotations),
            }))
        }
        Err(error) => {
            database.complete_host_tool_call(run_id, tool_call_id, None, Some(&error))?;
            let _ = crate::lifecycle_hooks::after_tool(database, run_id, tool_call_id, tool, false);
            Err(error)
        }
    }
}

fn ensure_plan_approved_before_mutation(
    database: &Database,
    conversation_id: &str,
    tool: &str,
    input: &Value,
) -> Result<(), String> {
    let mutates_project = matches!(tool, "write_file" | "edit_file" | "run_command")
        || (tool == "format_code" && input.get("mode").and_then(Value::as_str) == Some("write"));
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
        ensure_plan_approved_before_mutation, extract_docx_text, intersect_mcp_server_scopes,
        intersect_optional_scopes, node_dependency_is_available, normalized_child_budget,
        parse_runtime_ready_payload, pending_approval_ids_for_run, redact_diagnostic_line,
        runtime_expert_package, should_attempt_recovery, structured_runtime_error, PendingApproval,
    };
    use crate::database::{
        AttachmentRecord, CreateGoalInput, Database, GoalStatus, KnowledgeReference,
    };
    use flate2::{write::DeflateEncoder, Compression};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::{
        collections::{HashMap, HashSet},
        fs,
        io::Write,
        sync::mpsc,
    };
    use uuid::Uuid;

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
