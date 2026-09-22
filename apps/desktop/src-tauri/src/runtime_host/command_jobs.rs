//! Durable adapter for run_command jobs. Admission and approval remain in the
//! ordinary Host tool path; a claimed command is never resumed or replayed.
use crate::{
    database::{Database, JobStartOutcome, JobStartRequest, JobState},
    kernel::CancellationToken,
    process_jobs::{
        ManagerConfig, ProcessJobManager, StartRequest, Stream, SystemClock, SystemIdentityProbe,
        SystemSpawner,
    },
    tool_host::{PreparedToolAction, ToolPreview},
};
use base64::Engine;
use fox_engine_protocol::{ExecutionCredential, ExecutionEvidence, CallOutcome, AttemptState};
use serde_json::{json, Value};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

fn active() -> &'static Mutex<HashSet<String>> {
    static ACTIVE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    ACTIVE.get_or_init(Default::default)
}
fn invalid(s: impl Into<String>) -> String {
    crate::tool_host::ToolErrorCode::InvalidInput.error(s)
}
fn unsigned(input: &Value, key: &str, default: u64, max: u64) -> Result<u64, String> {
    let n = input
        .get(key)
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| invalid(format!("{key} must be an integer")))
        })
        .transpose()?
        .unwrap_or(default);
    if n > max {
        return Err(invalid(format!("{key} exceeds {max}")));
    }
    Ok(n)
}

pub(crate) fn prepare(input: &Value, root: &Path) -> Result<PreparedToolAction, String> {
    let action = input["action"]
        .as_str()
        .ok_or_else(|| invalid("action must be sync/start/status/output/cancel"))?;
    let fields: &[&str] = match action {
        "start" => &[
            "action",
            "command",
            "cwd",
            "timeoutSeconds",
        ],
        "status" | "cancel" => &["action", "jobId"],
        "output" => &["action", "jobId", "stream", "offset", "limit"],
        _ => return Err(invalid("unsupported command action")),
    };
    if input
        .as_object()
        .is_none_or(|o| o.keys().any(|k| !fields.contains(&k.as_str())))
    {
        return Err(invalid("unknown command job field"));
    }
    let preview = if action == "start" {
        if unsigned(input, "timeoutSeconds", 600, 3600)? == 0 {
            return Err(invalid("timeoutSeconds must be positive"));
        }
        let mut sync = input.clone();
        sync.as_object_mut().unwrap().remove("action");
        crate::tool_host::prepare("run_command", &sync, &root.to_string_lossy())?
            .preview()
            .clone()
    } else {
        let id = input["jobId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 200)
            .ok_or_else(|| invalid("jobId required"))?;
        if action == "output" {
            if input
                .get("stream")
                .is_some_and(|v| v != "stdout" && v != "stderr")
            {
                return Err(invalid("stream must be stdout or stderr"));
            }
            unsigned(input, "offset", 0, u64::MAX)?;
            if unsigned(input, "limit", 16384, 65536)? < 4 {
                return Err(invalid("limit must be 4..65536 bytes"));
            }
        }
        ToolPreview {
            tool: "run_command".into(),
            title: format!("Command {action}"),
            target: id.into(),
            summary: format!("{action} owned command job {id}"),
            diff: None,
            command: None,
            cwd: None,
        }
    };
    Ok(PreparedToolAction::CommandJob {
        root: root.to_path_buf(),
        input: input.clone(),
        preview,
    })
}

fn page(manager: &ProcessJobManager, id: &str, stream: Stream) -> Result<Value, String> {
    let p = manager
        .output(id, stream, 0, 256 * 1024)
        .map_err(|e| e.message())?;
    Ok(
        json!({"data":base64::engine::general_purpose::STANDARD.encode(&p.raw_bytes),"offset":p.offset,"nextOffset":p.next_offset,
        "atEndOfAvailable":p.at_end_of_available,"streamClosed":p.stream_closed,
        "lostBeforeOffset":p.lost_before_offset,"droppedBytes":p.dropped_bytes,"totalBytes":p.total_bytes}),
    )
}
fn sample(manager: &ProcessJobManager, id: &str) -> Result<Value, String> {
    let s = manager.status(id).map_err(|e| e.message())?;
    let error_code = match s.state {
        crate::process_jobs::JobRunState::Cancelled => Some("tool.cancelled"),
        crate::process_jobs::JobRunState::TimedOut => Some("tool.timed_out"),
        crate::process_jobs::JobRunState::Interrupted => Some("job.interrupted_unknown"),
        crate::process_jobs::JobRunState::Failed => Some(match s.error_code.as_deref() {
            Some("nonzero_exit") => "tool.nonzero_exit",
            Some("start_failed") => "tool.start_failed",
            Some("sandbox_unavailable") => "sandbox_unavailable",
            _ => "tool.unknown",
        }),
        _ => None,
    };
    Ok(json!({"state":s.production_state(),"exitCode":s.exit_code,
        "errorCode":error_code,
        "errorMessage":s.error_message,"stdout":page(manager,id,Stream::Stdout)?,"stderr":page(manager,id,Stream::Stderr)?,
        "readersOrphaned":s.readers_orphaned}))
}
fn result(value: Value, failed: bool) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],"details":value,"isError":failed})
}
/// Model-facing allowlist. Internal checkpoints and result blobs never cross
/// this boundary; only output explicitly returns a page of log bytes.
fn public_status(database: &Database, row: &crate::database::JobSnapshot) -> Result<Value, String> {
    let snapshot: Value = if row.result_ref.is_some() {
        database.kernel_job_result_value(&row.conversation_id, &row.job_id)?
    } else {
        row.cursor.as_deref().map(serde_json::from_str).transpose()
            .map_err(|e| format!("Invalid command checkpoint: {e}"))?.unwrap_or(json!({}))
    };
    let stream = |key: &str| {
        let p = &snapshot[key];
        json!({"totalBytes":p["totalBytes"].as_u64().unwrap_or(0),
            "offset":p["offset"].as_u64().unwrap_or(0),
            "nextOffset":p["nextOffset"].as_u64().unwrap_or(0),
            "droppedBytes":p["droppedBytes"].as_u64().unwrap_or(0),
            "streamClosed":p["streamClosed"].as_bool().unwrap_or(row.state.is_terminal())})
    };
    Ok(json!({"jobId":row.job_id,"state":row.state,"exitCode":snapshot["exitCode"],
        "errorCode":row.error_code.as_deref().or(snapshot["errorCode"].as_str()),
        "stdout":stream("stdout"),"stderr":stream("stderr"),
        "cancelRequested":row.cancel_requested_at.is_some(),
        "cancelRequestedAt":row.cancel_requested_at,"cancelAcknowledgedAt":row.cancel_acknowledged_at,
        "resumable":row.resumable}))
}

fn stop_failed(manager: &ProcessJobManager, local: &str, db: &Database, job: &str, reason: &str) {
    let _ = manager.cancel(local);
    let until = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < until {
        manager.tick();
        if manager.status(local).is_ok_and(|s| s.is_terminal()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let stored = sample(manager, local).and_then(|mut snapshot| {
        snapshot["state"] = json!("failed");
        snapshot["errorCode"] = json!("tool.unknown");
        snapshot["errorMessage"] = json!(reason);
        db.kernel_command_job_publish(job, 1, &snapshot)
    });
    if stored.is_err() {
        // Existing checkpoint output remains readable even if terminal blob storage fails.
        let _ = db.kernel_job_settle_attempt(
            job,
            1,
            JobState::Failed,
            Some(("job.result_storage_failed", reason)),
        );
    }
}

pub(super) fn execute(
    database: &Database,
    run_id: &str,
    input: &Value,
    root: &Path,
    token: &CancellationToken,
    budget: Duration,
) -> Result<Value, String> {
    // Production always refuses, because this is the refusing proof.
    execute_inner(
        database,
        run_id,
        input,
        root,
        token,
        budget,
        Arc::new(crate::process_jobs::HostVerifiedBackend),
        None,
        &mut ExecutionEvidence::NotStarted,
        &mut CallOutcome::Completed,
    )
}

/// Consumes the Host's non-cloneable first-claim capability. No model value
/// can construct this token, and this adapter never claims a second time.
pub(super) fn execute_authorized(
    database: &Database, run_id: &str, input: &Value, root: &Path,
    token: &CancellationToken, budget: Duration,
    claim: super::kernel_host::ClaimedExecution,
) -> (Result<Value, String>, ExecutionEvidence, CallOutcome) {
    let mut evidence = ExecutionEvidence::NotStarted;
    let mut outcome = CallOutcome::Completed;
    let response = execute_inner(database, run_id, input, root, token, budget,
        Arc::new(crate::process_jobs::HostVerifiedBackend), Some(claim.credential()),
        &mut evidence, &mut outcome);
    if let Err(error) = &response {
        outcome = CallOutcome::Failed { code: error.clone() };
    }
    (response, evidence, outcome)
}

/// Field equality against durable authority, not digest self-consistency.
/// This runs both before job creation and at the manager reception boundary.
fn verify_start(
    database: &Database, run_id: &str, input: &Value,
    credential: &ExecutionCredential,
    proof: &dyn crate::process_jobs::BackendCapabilityProof,
) -> Result<(), String> {
    database.verify_execution_credential(run_id, credential)?;
    // Re-read live policy generation, cancellation and parent revocation;
    // immutable snapshot equality alone does not revalidate those facts.
    database.revalidate_execution_credential(credential)?;
    if credential.run_id != run_id
        || credential.action_class != fox_engine_protocol::ActionClass::Execute
        || credential.intent_digest != crate::database::kernel_execution_admission::launch_params_hash(
            "run_command", &input.to_string()) {
        return Err("credential_mismatch: command scope or input differs".into());
    }
    let binding = database.run_control_binding(run_id)?.ok_or("command run binding missing")?;
    if credential.conversation_id != binding.conversation_id
        || credential.resolved_profile != binding.execution_profile_id
        || credential.policy_snapshot_id != binding.permission_snapshot_id {
        return Err("credential_mismatch: frozen run scope differs".into());
    }
    if binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative {
        database.kernel_validate_resource_acquisition_for(run_id, Some("run_command"))?;
    }
    let row = database.read_execution_attempt(run_id, &credential.dispatch_id)?
        .ok_or("credential_mismatch: no Host claim exists")?;
    if row.state != AttemptState::Claimed || row.params_hash != credential.intent_digest
        || row.conversation_id != credential.conversation_id {
        return Err("credential_mismatch: dispatch is not the Host's first claimed attempt".into());
    }
    match proof.availability() {
        crate::process_jobs::BackendAvailability::Available { digest, .. }
            if credential.backend_requirement.evidence_digest.as_deref() == Some(digest.as_str()) => Ok(()),
        crate::process_jobs::BackendAvailability::Available { .. } =>
            Err("credential_mismatch: backend evidence changed".into()),
        crate::process_jobs::BackendAvailability::Unavailable(_) =>
            Err("sandbox_unavailable: no verified execution backend".into()),
    }
}

/// Keeps the exact verified decision bound even if the provider changes
/// between the manager's check and the spawner's own final check. This is a
/// production wrapper around the real provider, never a synthetic grant.
struct SnapshotBackendProof {
    provider: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
    expected_digest: String,
}
impl crate::process_jobs::BackendCapabilityProof for SnapshotBackendProof {
    fn availability(&self) -> crate::process_jobs::BackendAvailability {
        match self.provider.availability() {
            crate::process_jobs::BackendAvailability::Available { backend, digest } => {
                if digest == self.expected_digest {
                    crate::process_jobs::BackendAvailability::Available { backend, digest }
                } else {
                    crate::process_jobs::BackendAvailability::Unavailable(
                        crate::process_jobs::BackendRefusal::CredentialMismatch)
                }
            }
            unavailable => unavailable,
        }
    }
}

fn execute_inner(
    database: &Database,
    run_id: &str,
    input: &Value,
    root: &Path,
    token: &CancellationToken,
    budget: Duration,
    proof: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
    credential: Option<&ExecutionCredential>,
    evidence: &mut ExecutionEvidence,
    outcome: &mut CallOutcome,
) -> Result<Value, String> {
    let admitted = std::time::Instant::now();
    token.check()?;
    prepare(input, root)?;
    let binding = database
        .run_control_binding(run_id)?
        .ok_or("command job needs a frozen Run")?;
    if binding
        .permission
        .project_root
        .as_deref()
        .map(Path::new)
        .and_then(|p| p.canonicalize().ok())
        != Some(root.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("[tool.permission_denied] command project differs from the frozen Run".into());
    }
    if binding.authority == fox_engine_protocol::ExecutionAuthority::Authoritative {
        database.kernel_validate_resource_acquisition_for(run_id, Some("run_command"))?;
    }
    let action = input["action"].as_str().unwrap();
    let id = if action == "start" {
        if budget.is_zero() {
            return Err("[tool.timed_out] command budget exhausted".into());
        }
        if let Some(credential) = credential {
            database.verify_execution_credential(run_id, credential)?;
            if credential.intent_digest != crate::database::kernel_execution_admission::launch_params_hash(
                "run_command", &input.to_string()) {
                return Err("credential_mismatch: command input differs".into());
            }
        }
        // A refusal may follow durable admission/claim. Report only the
        // external-process fact; never claim that nothing was persisted.
        if let Err(refusal) = proof.availability().authorize() {
            *outcome = CallOutcome::Refused { code: "sandbox_unavailable".into() };
            return Ok(result(json!({"jobId":Value::Null,"state":"refused",
                "errorCode":"sandbox_unavailable", "errorMessage":format!(
                    "{} No external process was started.", refusal.reason()),
                "executionStarted":false,"sideEffectState":"none"}), true));
        }
        let credential = credential.ok_or("credential_missing: Host first-claim token required")?;
        verify_start(database, run_id, input, credential, proof.as_ref())?;
        let mut sync = input.clone();
        sync.as_object_mut().unwrap().remove("action");
        let PreparedToolAction::RunCommand { command, cwd, .. } =
            crate::tool_host::prepare("run_command", &sync, &root.to_string_lossy())?
        else {
            unreachable!()
        };
        let requested = unsigned(input, "timeoutSeconds", 600, 3600)?;
        let duration = Duration::from_secs(requested)
            .min(Duration::from_millis(credential.budget_ceiling_ms.max(0) as u64))
            .min(budget.saturating_sub(admitted.elapsed()));
        if duration.is_zero() {
            return Err(
                "[tool.timed_out] command budget exhausted before durable admission".into(),
            );
        }
        let deadline =
            crate::database::now_ms() + duration.as_millis().min(i64::MAX as u128) as i64;
        // Store stable requested parameters, not the decreasing remaining Run budget.
        let params = json!({"command":command,"cwd":cwd,"timeoutSeconds":requested,"root":root});
        let mut registry = active().lock().map_err(|_| "command registry poisoned")?;
        let started = database.kernel_job_start(&JobStartRequest {
            run_id: run_id.into(),
            kind: "command".into(),
            idempotency_key: format!("job:{}", credential.dispatch_id),
            params,
            deadline_ms: Some(deadline),
            progress_total: None,
        })?;
        let row = started.snapshot().clone();
        // Only the creator may claim. Existing queued/uncertain work is never replayed.
        if matches!(started, JobStartOutcome::Created(_)) {
            database.kernel_job_claim_attempt(&row.job_id, 1)?;
            let duration = Duration::from_millis(
                deadline.saturating_sub(crate::database::now_ms()).max(0) as u64,
            )
            .min(budget.saturating_sub(admitted.elapsed()));
            if duration.is_zero() {
                *outcome = CallOutcome::Failed { code: "tool.timed_out".into() };
                database.kernel_job_settle_attempt(
                    &row.job_id,
                    1,
                    JobState::Failed,
                    Some((
                        "tool.timed_out",
                        "Command budget exhausted before launch; no process started",
                    )),
                )?;
                return Ok(result(
                    public_status(database, &database.kernel_job_snapshot(&row.job_id)?)?,
                    true,
                ));
            }
            if registry.len() >= 32 {
                *outcome = CallOutcome::Failed { code: "tool.start_failed".into() };
                database.kernel_job_settle_attempt(
                    &row.job_id,
                    1,
                    JobState::Failed,
                    Some((
                        "tool.start_failed",
                        "Command job capacity exhausted; no process started",
                    )),
                )?;
                return Ok(result(
                    public_status(database, &database.kernel_job_snapshot(&row.job_id)?)?,
                    true,
                ));
            }
            registry.insert(row.job_id.clone());
            // Manager and spawner both consult the live provider through an
            // immutable expected-digest guard. A changed decision is a refusal,
            // never a silently substituted execution profile.
            let manager = Arc::new(ProcessJobManager::with_backend_proof(
                Arc::new(SystemClock),
                Arc::new(SystemSpawner),
                Arc::new(SystemIdentityProbe),
                ManagerConfig::default(),
                Arc::new(SnapshotBackendProof {
                    provider: Arc::clone(&proof),
                    expected_digest: credential.backend_requirement.evidence_digest.clone()
                        .ok_or("credential_mismatch: backend evidence missing")?,
                }),
            ));
            let launch = manager.start_authorized(StartRequest {
                run_id: run_id.into(),
                idempotency_key: credential.dispatch_id.clone(),
                spec: crate::tool_host::command_spawn_spec(&command, &cwd),
                execution_budget: duration,
            }, |_| {
                token.check().map_err(crate::process_jobs::JobError::InvalidRequest)?;
                verify_start(database, run_id, input, credential, proof.as_ref())
                    .map_err(|error| if error.starts_with("sandbox_unavailable") {
                        crate::process_jobs::JobError::SandboxUnavailable(error)
                    } else { crate::process_jobs::JobError::CredentialMismatch(error) })
            });
            match launch {
                Err(e) => {
                    registry.remove(&row.job_id);
                    database.kernel_job_settle_attempt(
                        &row.job_id,
                        1,
                        JobState::Failed,
                        Some((e.code(), &e.message())),
                    )?;
                }
                Ok(out) => {
                    // Only the manager's actual child identity/start timestamp
                    // confirms launch. A successful API result may be a spawn refusal.
                    if out.view().started_at.is_some() {
                        *evidence = ExecutionEvidence::Started;
                    }
                    let local = out.view().job_id.clone();
                    if out.view().started_at.is_none() && out.view().is_terminal() {
                        // Spawn itself failed before a child identity existed.
                        // Publish this real terminal fact synchronously rather
                        // than briefly reporting an unstarted queued job as success.
                        *outcome = CallOutcome::Failed { code: out.view().error_code.clone()
                            .unwrap_or_else(|| "tool.start_failed".into()) };
                        database.kernel_command_job_publish(&row.job_id, 1, &sample(&manager, &local)?)?;
                        registry.remove(&row.job_id);
                        return Ok(result(public_status(database,
                            &database.kernel_job_snapshot(&row.job_id)?)?, true));
                    }
                    let db = database.clone();
                    let job = row.job_id.clone();
                    let parent = token.clone();
                    let owner = manager.clone();
                    let spawn=std::thread::Builder::new().name("fox-command-job".into()).spawn(move || {
                        // The manager owns the child; the worker owns durable publication.
                        let work=(||->Result<(),String>{
                            let mut last_view = None;
                            let mut last_checkpoint = std::time::Instant::now() - Duration::from_secs(1);
                            loop {
                            if parent.check().is_err() || db.kernel_command_job_stop_requested(&job)? {
                                owner.cancel(&local).map_err(|e|e.message())?;
                            }
                            owner.tick();
                            let view=owner.status(&local).map_err(|e|e.message())?;
                            if view.is_terminal() {
                                return db.kernel_command_job_publish(&job,1,&sample(&owner,&local)?);
                            }
                            // Tick/cancel every 100 ms; copy/encode output at most once a
                            // second and only after state/output changed. Idle jobs do no writes.
                            if last_checkpoint.elapsed() >= Duration::from_secs(1)
                                && last_view.as_ref() != Some(&view) {
                                db.kernel_command_job_checkpoint(&job,1,&sample(&owner,&local)?)?;
                                last_view=Some(view);
                                last_checkpoint=std::time::Instant::now();
                            }
                            std::thread::sleep(Duration::from_millis(100));
                        }})();
                        if work.is_err() {
                            // A storage failure never leaves a detached process running.
                            stop_failed(&owner,&local,&db,&job,"Command supervision/storage failed; execution must not be replayed");
                        }
                        if let Ok(mut r)=active().lock(){r.remove(&job);}
                    });
                    if spawn.is_err() {
                        stop_failed(
                            &manager,
                            &out.view().job_id,
                            database,
                            &row.job_id,
                            "Cannot start command supervisor; command effects must not be replayed",
                        );
                        registry.remove(&row.job_id);
                    }
                }
            }
        }
        row.job_id
    } else {
        input["jobId"].as_str().unwrap().to_owned()
    };
    let mut row = database.kernel_job_snapshot(&id)?;
    if row.kind != "command"
        || row.run_id != run_id
        || row.conversation_id != binding.conversation_id
    {
        return Err("[tool.permission_denied] command job is outside this Run".into());
    }
    if action == "cancel" {
        row = database.kernel_job_request_cancel(&binding.conversation_id, &id)?;
    }
    let failed = matches!(row.state, JobState::Failed | JobState::Cancelled);
    if failed {
        *outcome = CallOutcome::Failed { code: row.error_code.clone().unwrap_or_else(|| "tool.unknown".into()) };
    }
    if action == "output" {
        let snapshot = if row.result_ref.is_some() {
            database.kernel_job_result_value(&binding.conversation_id, &id)?
        } else {
            row.cursor
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| e.to_string())?
                .unwrap_or(json!({}))
        };
        let stream = input["stream"].as_str().unwrap_or("stdout");
        let p = &snapshot[stream];
        let base = p["offset"].as_u64().unwrap_or(0);
        let requested = unsigned(input, "offset", 0, u64::MAX)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(p["data"].as_str().unwrap_or(""))
            .map_err(|_| "Invalid stored command output")?;
        let start = requested.saturating_sub(base).min(bytes.len() as u64) as usize;
        if requested > p["nextOffset"].as_u64().unwrap_or(0) {
            return Err(invalid("output offset is beyond available output"));
        }
        let mut end = start
            .saturating_add(unsigned(input, "limit", 16384, 65536)? as usize)
            .min(bytes.len());
        // Preserve a valid trailing UTF-8 scalar across pages. Malformed bytes
        // are consumed exactly once; replacement text never changes the cursor.
        if end < bytes.len() {
            while end > start && (bytes[end] & 0xc0) == 0x80 {
                end -= 1;
            }
        }
        if end == start {
            end = start.saturating_add(4).min(bytes.len());
        }
        return Ok(result(
            json!({"jobId":id,"state":row.state,"stream":stream,"content":String::from_utf8_lossy(&bytes[start..end]),
            "offsetUnit":"raw_bytes","encoding":"utf8_lossy",
            "offset":base+start as u64,"nextOffset":base+end as u64,
            "atEndOfAvailable":end==bytes.len(),"streamClosed":p["streamClosed"].as_bool().unwrap_or(row.state.is_terminal()),
            "lostBeforeOffset":base.saturating_sub(requested),"droppedBytes":p["droppedBytes"].as_u64().unwrap_or(0),
            "errorCode":row.error_code,"exitCode":snapshot["exitCode"]}),
            failed,
        ));
    }
    Ok(result(
        public_status(database, &row)?,
        failed,
    ))
}

#[cfg(test)]
#[path = "command_jobs_tests.rs"]
mod tests;
