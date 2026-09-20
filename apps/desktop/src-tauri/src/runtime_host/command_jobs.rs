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
            "idempotencyKey",
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
        let key = input["idempotencyKey"]
            .as_str()
            .filter(|s| !s.trim().is_empty() && s.len() <= 200)
            .ok_or_else(|| invalid("idempotencyKey must be 1..200 bytes"))?;
        let _ = key;
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
        let mut sync = input.clone();
        sync.as_object_mut().unwrap().remove("action");
        let PreparedToolAction::RunCommand { command, cwd, .. } =
            crate::tool_host::prepare("run_command", &sync, &root.to_string_lossy())?
        else {
            unreachable!()
        };
        let requested = unsigned(input, "timeoutSeconds", 600, 3600)?;
        let duration =
            Duration::from_secs(requested).min(budget.saturating_sub(admitted.elapsed()));
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
            idempotency_key: input["idempotencyKey"].as_str().unwrap().into(),
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
                    serde_json::to_value(database.kernel_job_snapshot(&row.job_id)?)
                        .map_err(|e| e.to_string())?,
                    true,
                ));
            }
            if registry.len() >= 32 {
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
                    serde_json::to_value(database.kernel_job_snapshot(&row.job_id)?)
                        .map_err(|e| e.to_string())?,
                    true,
                ));
            }
            registry.insert(row.job_id.clone());
            let manager = Arc::new(ProcessJobManager::new(
                Arc::new(SystemClock),
                Arc::new(SystemSpawner),
                Arc::new(SystemIdentityProbe),
                ManagerConfig::default(),
            ));
            let launch = manager.start(StartRequest {
                run_id: run_id.into(),
                idempotency_key: row.job_id.clone(),
                spec: crate::tool_host::command_spawn_spec(&command, &cwd),
                execution_budget: duration,
            });
            match launch {
                Err(e) => {
                    registry.remove(&row.job_id);
                    database.kernel_job_settle_attempt(
                        &row.job_id,
                        1,
                        JobState::Failed,
                        Some(("tool.start_failed", &e.message())),
                    )?;
                }
                Ok(out) => {
                    let local = out.view().job_id.clone();
                    let db = database.clone();
                    let job = row.job_id.clone();
                    let parent = token.clone();
                    let owner = manager.clone();
                    let spawn=std::thread::Builder::new().name("fox-command-job".into()).spawn(move || {
                        // The manager owns the child; the worker owns durable publication.
                        let work=(||->Result<(),String>{loop {
                            let stored=db.kernel_job_snapshot(&job)?;
                            if parent.check().is_err()||stored.cancel_requested_at.is_some()||db.kernel_job_parent_stopped(&job)? {
                                owner.cancel(&local).map_err(|e|e.message())?;
                            }
                            owner.tick();
                            let snapshot=sample(&owner,&local)?;
                            if owner.status(&local).map_err(|e|e.message())?.is_terminal() {
                                return db.kernel_command_job_publish(&job,1,&snapshot);
                            }
                            db.kernel_command_job_checkpoint(&job,1,&snapshot)?;
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
        serde_json::to_value(row).map_err(|e| e.to_string())?,
        failed,
    ))
}

#[cfg(test)]
#[path = "command_jobs_tests.rs"]
mod tests;
