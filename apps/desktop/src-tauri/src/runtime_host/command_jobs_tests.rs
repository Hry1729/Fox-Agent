use super::*;
use crate::database::{JobStartRequest, JobState};
use crate::kernel::CancellationRegistry;
use crate::process_jobs::BackendCapabilityProof;
use serde_json::{json, Value};
use rusqlite::OptionalExtension;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The proof this fixture's default `start`/`action` use.
///
/// An authorizing proof is injected per fixture, so these tests need no global
/// switch and no serialization against tests that assert the refusal. A test
/// that wants the production decision uses [`Fixture::refusing`], or passes
/// `HostVerifiedBackend` explicitly through `start_with_proof`.
fn authorized_proof() -> Arc<dyn crate::process_jobs::BackendCapabilityProof> {
    Arc::new(crate::process_jobs::AvailableInTests::host_user_unconfined(
        "command-jobs-adapter-tests",
    ))
}

/// The production decision, for the refusal tests.
fn refusing_proof() -> Arc<dyn crate::process_jobs::BackendCapabilityProof> {
    Arc::new(crate::process_jobs::HostVerifiedBackend)
}

struct Fixture {
    database: Database,
    root: PathBuf,
    run_id: String,
    token: CancellationToken,
    proof: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
}

impl Fixture {
    fn new(label: &str) -> Self {
        Self::with_proof(label, authorized_proof())
    }

    /// A fixture whose default operations run under the production refusal.
    fn refusing(label: &str) -> Self {
        Self::with_proof(label, Arc::new(crate::process_jobs::HostVerifiedBackend))
    }

    fn with_proof(
        label: &str,
        proof: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
    ) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fox-command-adapter-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create command fixture");
        let database = Database::open(root.join("facts.db")).expect("open real sqlite fixture");
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                None,
                Some(root.to_str().expect("utf8 fixture root")),
                Some("ask"),
            )
            .expect("create conversation");
        let run_id = database
            .create_run(&conversation.id, "command adapter test", None)
            .expect("create run")
            .run
            .id;
        database
            .freeze_legacy_run_control(&run_id, "command-adapter-test")
            .expect("freeze run control");
        let registry = CancellationRegistry::default();
        registry
            .register_run(&run_id)
            .expect("register cancellation run");
        let token = registry.run_token(&run_id).expect("run token");
        Self {
            database,
            root,
            run_id,
            token,
            proof,
        }
    }

    fn start(&self, key: &str, command: &str) -> Result<Value, String> {
        self.start_with_proof(key, command, Arc::clone(&self.proof))
    }

    fn action(&self, value: Value) -> Result<Value, String> {
        self.action_with_proof(value, Arc::clone(&self.proof))
    }

    /// `start` with an explicit execution decision, so a test can exercise the
    /// authorized and the refusing path from the same fixture without any
    /// process-global switch.
    fn start_with_proof(
        &self,
        key: &str,
        command: &str,
        proof: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
    ) -> Result<Value, String> {
        // `key` is this fixture Host's tool-call identity, never tool input.
        let input = json!({"action":"start","command":command,"timeoutSeconds":30});
        let binding = self.database.run_control_binding(&self.run_id)?.unwrap();
        let dispatch = fox_engine_protocol::encode_dispatch_id(&self.run_id, key)?;
        let digest = crate::database::kernel_execution_admission::launch_params_hash("run_command", &input.to_string());
        let credential = if let Some(stored) = self.database.read_execution_credential(&self.run_id, &dispatch)? {
            if stored.intent_digest != digest { return Err("job_idempotency_conflict".into()); }
            stored
        } else {
            let proof_digest = match proof.availability() {
                crate::process_jobs::BackendAvailability::Available { digest, .. } => Some(digest),
                _ => None,
            };
            let mut c = fox_engine_protocol::ExecutionCredential::new(
                dispatch.clone(), self.run_id.clone(), binding.conversation_id.clone(),
                digest, fox_engine_protocol::ActionClass::Execute, None,
                binding.execution_profile_id.clone(), binding.permission_snapshot_id.clone(),
                Some(1), binding.budgets.tool_execution_ms, None,
                fox_engine_protocol::BackendRequirement { required:"fixture".into(), evidence_digest:proof_digest })?;
            // Fixture authority only: production derives these requirements from
            // durable policy, and its Host proof remains unavailable.
            c.realtime_requirements.clear();
            c.credential_digest = c.recompute_digest();
            self.database.issue_execution_credential(&c)?;
            c
        };
        match self.database.claim_execution_attempt(&self.run_id, &binding.conversation_id,
            &credential, &credential.intent_digest, "fixture-host")? {
            fox_engine_protocol::AttemptOutcome::Claimed => {},
            fox_engine_protocol::AttemptOutcome::AlreadyRefused { code } => {
                return Ok(result(json!({"jobId":Value::Null,"state":"refused",
                    "errorCode":code,"executionStarted":false,"sideEffectState":"none"}), true));
            },
            _ => {
                let id: Option<String> = self.database.with_connection(|connection| {
                    connection.query_row("SELECT job_id FROM kernel_jobs WHERE run_id=?1 AND idempotency_key=?2",
                        rusqlite::params![self.run_id, format!("job:{dispatch}")], |r| r.get(0)).optional()
                }).map_err(|e| e.to_string())?;
                let row = id.map(|id| self.database.kernel_job_snapshot(&id)).transpose()?;
                return match row {
                    Some(row) => Ok(result(public_status(&self.database, &row)?, false)),
                    None => Ok(result(json!({"state":"uncertain","errorCode":"kernel.uncertain_execution"}), true)),
                };
            }
        }
        let mut evidence = ExecutionEvidence::NotStarted;
        let mut outcome = CallOutcome::Completed;
        let response = execute_inner(&self.database, &self.run_id, &input, &self.root,
            &self.token, Duration::from_secs(30), proof, Some(&credential), &mut evidence, &mut outcome);
        match evidence {
            ExecutionEvidence::Started => self.database.record_attempt_started(&self.run_id, &dispatch)?,
            ExecutionEvidence::Unknown => self.database.mark_attempt_unknown(&self.run_id, &dispatch)?,
            ExecutionEvidence::NotStarted => {
                let code = match outcome { CallOutcome::Refused { code } | CallOutcome::Failed { code } => code,
                    _ => "tool.start_failed".into() };
                self.database.record_attempt_refusal(&self.run_id, &dispatch, &code)?;
            }
        }
        response
    }

    fn action_with_proof(
        &self, value: Value,
        proof: Arc<dyn crate::process_jobs::BackendCapabilityProof>,
    ) -> Result<Value, String> {
        execute_inner(&self.database, &self.run_id, &value, &self.root, &self.token,
            Duration::from_secs(30), proof, None,
            &mut ExecutionEvidence::NotStarted, &mut CallOutcome::Completed)
    }

    fn wait_terminal(&self, job_id: &str) -> crate::database::JobSnapshot {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let row = self
                .database
                .kernel_job_snapshot(job_id)
                .expect("job snapshot");
            if row.state.is_terminal() {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "job did not become terminal: {row:?}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

// Authorization is injected per fixture (`Fixture::with_proof`), so these tests
// hold no shared lock and install no global state. Every test here can run in
// parallel with every other test, including ones that assert the refusal.

/// A refusal creates no job, but keeps the Host attempt terminal and readable.
///
/// The refusal is injected per fixture rather than installed globally, so this
/// test is unaffected by any other test in the binary that authorizes
/// execution, and it may run in parallel with them.
#[test]
fn start_refusal_is_durable_and_same_dispatch_never_retries() {
    let fixture = Fixture::refusing("refused");
    let refused = fixture
        .start("denied", "echo should-never-run")
        .expect("a refusal is a structured result, not a transport error");
    assert_eq!(refused["details"]["errorCode"], "sandbox_unavailable");
    assert_eq!(refused["details"]["executionStarted"], false);
    assert_eq!(refused["details"]["sideEffectState"], "none");
    assert!(
        refused["details"]["jobId"].is_null(),
        "a refused start must not allocate a job id: {refused}"
    );
    assert_eq!(refused["isError"], true);

    assert!(fixture.database.kernel_jobs_for_run(&fixture.run_id).unwrap().is_empty());
    let dispatch = fox_engine_protocol::encode_dispatch_id(&fixture.run_id, "denied").unwrap();
    assert!(matches!(fixture.database.read_execution_attempt(&fixture.run_id, &dispatch).unwrap().unwrap().state,
        AttemptState::Refused { .. }));
    // Restoring backend availability cannot revive the same dispatch.
    let retried = fixture
        .start_with_proof("denied", "echo should-never-run", authorized_proof())
        .expect("repeat reads the original refusal");
    assert_eq!(retried["details"]["errorCode"], "sandbox_unavailable");
}

/// The same fixture can drive both decisions, which is the point of per-instance
/// injection: one object, no global state, both outcomes observable.
#[test]
fn one_fixture_can_refuse_and_then_authorize_without_global_state() {
    let fixture = Fixture::new("both-decisions");
    let refused = fixture
        .start_with_proof("a", "echo nope", refusing_proof())
        .expect("refusal is a structured result");
    assert_eq!(refused["details"]["errorCode"], "sandbox_unavailable");

    // A new logical attempt needs a new Host tool-call/dispatch identity.
    let started = fixture
        .start_with_proof("b", "echo nope", Arc::clone(&fixture.proof))
        .expect("the authorized decision starts");
    let id = job_id(&started).to_owned();
    assert_ne!(started["details"]["errorCode"], "sandbox_unavailable");
    assert_eq!(fixture.wait_terminal(&id).state, JobState::Completed);
}

/// The production adapter must relay the caller's own decision — verdict **and
/// digest** — instead of synthesising one from a test type.
///
/// This is the regression test for R1-C01's second half: the adapter used to
/// convert any allowing proof into `AvailableInTests` with the hard-coded digest
/// `"command-job-caller-supplied-proof"`, which discarded the real decision
/// digest and made production name a test type. It now forwards the caller's own
/// proof object to both the manager and the spawner.
#[test]
fn adapter_relays_the_caller_digest_and_never_synthesises_one() {
    // A sentinel that no test helper would produce on its own.
    const REAL_DIGEST: &str = "sha256:real-decided-snapshot-c0-r2";
    let caller: Arc<dyn crate::process_jobs::BackendCapabilityProof> =
        Arc::new(crate::process_jobs::AvailableInTests {
            backend: crate::process_jobs::ExecutionBackend::HostUserUnconfined,
            digest: REAL_DIGEST.to_owned(),
        });

    // The proof handed to the adapter answers with exactly the caller's digest;
    // nothing in the adapter rewrites it.
    match caller.availability() {
        crate::process_jobs::BackendAvailability::Available { digest, .. } => {
            assert_eq!(digest, REAL_DIGEST)
        }
        other => panic!("the caller proof must stay available, got {other:?}"),
    }

    // Driving a real start through the adapter succeeds, so the adapter used the
    // caller's verdict rather than a re-derived or synthetic one.
    let fixture = Fixture::new("digest-relay");
    let started = fixture
        .start_with_proof("digest", "echo digest", Arc::clone(&caller))
        .expect("authorized start");
    let id = job_id(&started).to_owned();
    assert_eq!(fixture.wait_terminal(&id).state, JobState::Completed);
}

/// The production adapter must not name a test type at all: its only proof is
/// the refusing production one, so a production build cannot reach the grant.
#[test]
fn production_adapter_uses_only_the_refusing_proof() {
    let rendered = format!("{:?}", refusing_proof().availability());
    assert!(
        rendered.contains("NoVerifiedBackend"),
        "the production proof must refuse: {rendered}"
    );
    assert!(!rendered.contains("Available"));
}

/// The refusal must not take the safe query surface with it: an already
/// authorized job stays readable and cancellable.
#[test]
fn refusal_preserves_status_output_and_cancel_for_existing_jobs() {
    let fixture = Fixture::refusing("refusal-surface");
    let started = fixture
        .database
        .kernel_job_start(&JobStartRequest {
            run_id: fixture.run_id.clone(),
            kind: "command".into(),
            idempotency_key: "pre-existing".into(),
            params: json!({"command":"already-authorized"}),
            deadline_ms: Some(crate::database::now_ms() + 60_000),
            progress_total: None,
        })
        .expect("seed an existing authorized job")
        .snapshot()
        .job_id
        .clone();

    let status = fixture
        .action(json!({"action":"status","jobId":started}))
        .expect("status must survive a closed gate");
    assert_eq!(status["details"]["jobId"], started.as_str());

    let cancelled = fixture
        .action(json!({"action":"cancel","jobId":started}))
        .expect("cancellation must survive a closed gate");
    assert!(
        cancelled["details"]["cancelRequestedAt"].is_number(),
        "{cancelled}"
    );

    // A new start is still refused even though existing jobs remain reachable.
    let refused = fixture.start("new-after-close", "echo nope").unwrap();
    assert_eq!(refused["details"]["errorCode"], "sandbox_unavailable");
}

/// The raw spawner must refuse too, so reaching the low-level seam cannot be a
/// way around the manager-level check.
#[test]
fn raw_system_spawner_refuses_without_a_verified_backend() {
    use crate::process_jobs::{ProcessSpawner, SpawnSpec, SystemSpawner};
    let spawner = SystemSpawner;
    let outcome = spawner.spawn(refusing_proof().as_ref(), &SpawnSpec {
        program: "cmd.exe".into(),
        args: vec!["/C".into(), "echo unconfined".into()],
        cwd: std::env::temp_dir().to_string_lossy().into_owned(),
        env: crate::process_jobs::EnvMode::Inherit,
    });
    // `Box<dyn ChildHandle>` is not `Debug`, so match rather than `expect_err`.
    let error = match outcome {
        Ok(child) => panic!(
            "the unconfined spawner created a live child with pid {}",
            child.pid()
        ),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(error.to_string().contains("sandbox_unavailable"), "{error}");
}

/// The refusal is a property of the artifact, not of the environment: no
/// environment variable, working directory or request field changes it.
#[test]
fn refusal_cannot_be_talked_out_of_by_request_or_environment() {
    let fixture = Fixture::refusing("no-bypass");
    // Fields a caller might hope are honored by the backend selection.
    for extra in [
        json!({"backend":"srt_windows"}),
        json!({"sandbox":true}),
        json!({"allowRoots":["C:\\"]}),
        json!({"helper":"C:\\tmp\\fake-srt.exe"}),
    ] {
        let mut request = json!({
            "action": "start",
            "command": "echo nope",
            "timeoutSeconds": 30,
            "idempotencyKey": format!("bypass-{}", extra),
        });
        for (key, value) in extra.as_object().unwrap() {
            request[key] = value.clone();
        }
        // Such fields are not part of the accepted schema, so they are rejected
        // as invalid input *before* reaching the backend decision.
        let rejected = fixture
            .action(request)
            .expect_err("unknown field must be rejected");
        assert!(rejected.contains("unknown command job field"), "{rejected}");
    }

    let refused = fixture.start("env-ignored", "echo nope").unwrap();
    assert_eq!(refused["details"]["errorCode"], "sandbox_unavailable");
}

fn job_id(result: &Value) -> &str {
    result["details"]["jobId"]
        .as_str()
        .expect("job id in start result")
}

#[cfg(windows)]
#[test]
fn production_adapter_start_nonzero_and_utf8_byte_paging_are_durable() {
    let fixture = Fixture::new("result");
    let started = fixture
        .start("nonzero", "powershell.exe -NoProfile -Command \"[Console]::OutputEncoding=[Text.UTF8Encoding]::new();[Console]::Write('A😀BC');exit 7\"")
        .expect("start command");
    let id = job_id(&started).to_owned();
    let terminal = fixture.wait_terminal(&id);
    assert_eq!(terminal.state, JobState::Failed);
    assert_eq!(terminal.error_code.as_deref(), Some("tool.nonzero_exit"));
    assert!(
        terminal.result_ref.is_some(),
        "failure output must be published durably"
    );

    let first = fixture
        .action(json!({"action":"output","jobId":id,"stream":"stdout","offset":0,"limit":4}))
        .expect("first byte page");
    assert_eq!(first["details"]["content"], "A");
    assert_eq!(first["details"]["nextOffset"], 1);
    let second = fixture
        .action(json!({"action":"output","jobId":id,"stream":"stdout","offset":1,"limit":4}))
        .expect("unicode byte page");
    assert_eq!(second["details"]["content"], "😀");
    assert_eq!(second["details"]["nextOffset"], 5);
    assert_eq!(second["details"]["errorCode"], "tool.nonzero_exit");
}

#[cfg(windows)]
#[test]
fn replacement_text_pages_by_raw_pipe_offsets_without_reslicing_lossy_text() {
    let fixture = Fixture::new("replacement-offset");
    let started = fixture
        .start(
            "invalid-utf8",
            "powershell.exe -NoProfile -Command \"$o=[Console]::OpenStandardOutput();$b=[byte[]](255,65);$o.Write($b,0,2)\"",
        )
        .expect("start invalid utf8 writer");
    let id = job_id(&started).to_owned();
    assert_eq!(fixture.wait_terminal(&id).state, JobState::Completed);
    let whole = fixture
        .action(json!({"action":"output","jobId":id,"stream":"stdout","offset":0,"limit":4}))
        .expect("canonical replacement page");
    assert_eq!(whole["details"]["content"], "�A");
    assert_eq!(whole["details"]["nextOffset"], 2);
    let tail = fixture
        .action(json!({"action":"output","jobId":id,"stream":"stdout","offset":1,"limit":4}))
        .expect("tail after replacement character");
    assert_eq!(tail["details"]["content"], "A");
    assert_eq!(tail["details"]["offset"], 1);
    assert_eq!(tail["details"]["nextOffset"], 2);
}

#[cfg(windows)]
#[test]
fn production_adapter_idempotency_cancel_and_cross_run_authorization_hold() {
    let fixture = Fixture::new("identity");
    let first = fixture
        .start("same-key", "ping -n 60 127.0.0.1 > nul")
        .expect("start");
    let id = job_id(&first).to_owned();
    let duplicate = fixture
        .start("same-key", "ping -n 60 127.0.0.1 > nul")
        .expect("duplicate");
    assert_eq!(job_id(&duplicate), id);
    let conflict = fixture
        .start("same-key", "echo different")
        .expect_err("changed request");
    assert!(conflict.contains("job_idempotency_conflict"), "{conflict}");

    let cancelled = fixture
        .action(json!({"action":"cancel","jobId":id}))
        .expect("request cancellation");
    assert!(cancelled["details"]["cancelRequestedAt"].is_number());
    let terminal = fixture.wait_terminal(&id);
    assert_eq!(terminal.state, JobState::Cancelled);
    assert!(terminal.cancel_acknowledged_at.is_some());

    let foreign_conversation = fixture
        .database
        .create_conversation(
            fixture.database.default_agent_id(),
            None,
            Some(fixture.root.to_str().expect("utf8 fixture root")),
            Some("ask"),
        )
        .expect("foreign conversation");
    let foreign_run = fixture
        .database
        .create_run(&foreign_conversation.id, "foreign run", None)
        .expect("foreign run")
        .run
        .id;
    fixture
        .database
        .freeze_legacy_run_control(&foreign_run, "command-adapter-test")
        .expect("freeze foreign run");
    let registry = CancellationRegistry::default();
    registry
        .register_run(&foreign_run)
        .expect("register foreign run");
    let foreign_token = registry.run_token(&foreign_run).expect("foreign token");
    let denied = execute(
        &fixture.database,
        &foreign_run,
        &json!({"action":"status","jobId":id}),
        &fixture.root,
        &foreign_token,
        Duration::from_secs(5),
    )
    .expect_err("foreign run must not read a job");
    assert!(denied.contains("outside this Run"), "{denied}");
}

#[test]
fn interrupted_command_recovery_fails_closed_and_never_becomes_resumable() {
    let fixture = Fixture::new("recovery");
    let started = fixture
        .database
        .kernel_job_start(&JobStartRequest {
            run_id: fixture.run_id.clone(),
            kind: "command".into(),
            idempotency_key: "interrupted".into(),
            params: json!({"command":"side-effect"}),
            deadline_ms: Some(crate::database::now_ms() + 30_000),
            progress_total: None,
        })
        .expect("seed command job");
    let id = started.snapshot().job_id.clone();
    fixture
        .database
        .kernel_job_claim_attempt(&id, 1)
        .expect("claim attempt");
    fixture
        .database
        .with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_jobs SET owner_pid=2147483647, owner_started_at=1 WHERE job_id=?1",
                [&id],
            )?;
            Ok(())
        })
        .expect("simulate dead owner");

    let reconciled = fixture
        .database
        .kernel_jobs_reconcile_orphans()
        .expect("reconcile");
    assert!(reconciled.iter().any(|row| row.job_id == id));
    let row = fixture
        .database
        .kernel_job_snapshot(&id)
        .expect("recovered row");
    assert_eq!(row.state, JobState::Failed);
    assert_eq!(row.error_code.as_deref(), Some("job.interrupted_unknown"));
    assert!(!row.resumable);
    assert!(fixture.database.kernel_job_resume(&id).is_err());

    let queued = fixture
        .database
        .kernel_job_start(&JobStartRequest {
            run_id: fixture.run_id.clone(),
            kind: "command".into(),
            idempotency_key: "queued-before-claim".into(),
            params: json!({"command":"never-started"}),
            deadline_ms: Some(crate::database::now_ms() - 1),
            progress_total: None,
        })
        .expect("seed queued crash window")
        .snapshot()
        .job_id
        .clone();
    fixture
        .database
        .with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_jobs SET owner_pid=2147483647, owner_started_at=1 WHERE job_id=?1",
                [&queued],
            )?;
            Ok(())
        })
        .expect("simulate queued creator death");
    fixture
        .database
        .kernel_jobs_reconcile_orphans()
        .expect("reconcile queued command");
    let queued = fixture
        .database
        .kernel_job_snapshot(&queued)
        .expect("queued recovery row");
    assert_eq!(queued.state, JobState::Failed);
    assert_eq!(
        queued.error_code.as_deref(),
        Some("job.interrupted_unknown")
    );
    assert!(!queued.resumable);

    let live = fixture
        .database
        .kernel_job_start(&JobStartRequest {
            run_id: fixture.run_id.clone(),
            kind: "command".into(),
            idempotency_key: "queued-live-creator".into(),
            params: json!({"command":"not-yet-claimed"}),
            deadline_ms: Some(crate::database::now_ms() + 30_000),
            progress_total: None,
        })
        .expect("seed live queued command")
        .snapshot()
        .job_id
        .clone();
    fixture
        .database
        .kernel_jobs_reconcile_orphans()
        .expect("reconcile live owner");
    assert_eq!(
        fixture
            .database
            .kernel_job_snapshot(&live)
            .expect("live queued row")
            .state,
        JobState::Queued,
        "recovery must not kill the current process's start-before-claim window"
    );
}

#[test]
fn r2_status_excludes_full_checkpoints_and_unchanged_checkpoint_does_not_write() {
    let fixture=Fixture::new("status-window");
    let id=fixture.database.kernel_job_start(&JobStartRequest {run_id:fixture.run_id.clone(),kind:"command".into(),
        idempotency_key:"status".into(),params:json!({}),deadline_ms:None,progress_total:None}).unwrap().snapshot().job_id.clone();
    fixture.database.kernel_job_claim_attempt(&id,1).unwrap();
    let p=json!({"data":base64::engine::general_purpose::STANDARD.encode(vec![b'x';256*1024]),
        "offset":100,"nextOffset":262244,"totalBytes":262244,"droppedBytes":100,"streamClosed":false});
    let value=json!({"state":"running","stdout":p,"stderr":p,"exitCode":null});
    fixture.database.kernel_command_job_checkpoint(&id,1,&value).unwrap();
    fixture.database.with_connection(|conn| conn.execute_batch(
        "CREATE TRIGGER reject_duplicate_checkpoint BEFORE UPDATE OF cursor ON kernel_jobs BEGIN SELECT RAISE(ABORT,'unexpected duplicate checkpoint'); END;")).unwrap();
    fixture.database.kernel_command_job_checkpoint(&id,1,&value).expect("identical checkpoint must issue no UPDATE");
    for _ in 0..5 {
        let status=fixture.action(json!({"action":"status","jobId":id})).unwrap();
        assert!(status.to_string().len()<2000,"{status}");
        assert!(status["details"].get("cursor").is_none());
        assert!(status["details"]["stdout"].get("data").is_none());
        assert_eq!(status["details"]["stderr"]["totalBytes"],262244);
        assert_eq!(status["details"]["stdout"]["offset"],100);
    }
    let page=fixture.action(json!({"action":"output","jobId":id,"offset":100,"limit":4})).unwrap();
    assert_eq!(page["details"]["content"],"xxxx");
    assert_eq!(page["details"]["nextOffset"],104);
    fixture.database.with_connection(|conn| conn.execute_batch("DROP TRIGGER reject_duplicate_checkpoint;")).unwrap();
    let mut done=value;done["state"]=json!("completed");done["exitCode"]=json!(0);
    fixture.database.kernel_command_job_publish(&id,1,&done).unwrap();
    let status=fixture.action(json!({"action":"status","jobId":id})).unwrap();
    assert_eq!(status["details"]["exitCode"],0);
    assert!(status.to_string().len()<2000);
}

#[cfg(windows)]
#[test]
fn r2_live_noisy_command_status_remains_small_and_cancel_is_responsive() {
    let fixture=Fixture::new("status-live");
    let started=fixture.start("noisy", "powershell.exe -NoProfile -Command \"[Console]::Out.Write(('x'*400000));[Console]::Error.Write(('y'*400000));Start-Sleep -Seconds 20\"").unwrap();
    let id=job_id(&started).to_owned();
    let deadline=Instant::now()+Duration::from_secs(15);
    loop {
        let status=fixture.action(json!({"action":"status","jobId":id})).unwrap();
        assert!(status.to_string().len()<2000);
        assert!(!status.to_string().contains("cursor"));
        if status["details"]["stderr"]["totalBytes"].as_u64().unwrap_or(0)>=400000 {break;}
        assert!(Instant::now()<deadline,"noisy command produced no checkpoint: {status}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let page=fixture.action(json!({"action":"output","jobId":id,"stream":"stderr","limit":8})).unwrap();
    assert_eq!(page["details"]["content"],"yyyyyyyy");
    let stopped=Instant::now();
    let status=fixture.action(json!({"action":"cancel","jobId":id})).unwrap();
    assert!(status.to_string().len()<2000);
    assert_eq!(fixture.wait_terminal(&id).state,JobState::Cancelled);
    assert!(stopped.elapsed()<Duration::from_secs(5));
}

#[test]
fn command_boundary_rejects_rehashed_changed_authoritative_fields() {
    let fixture = Fixture::refusing("tampered-snapshot");
    fixture.start("bound", "echo nope").unwrap();
    let dispatch = fox_engine_protocol::encode_dispatch_id(&fixture.run_id, "bound").unwrap();
    let mut credential = fixture.database.read_execution_credential(&fixture.run_id, &dispatch).unwrap().unwrap();
    credential.resolved_profile = "attacker-rehashed".into();
    credential.credential_digest = credential.recompute_digest();
    let input = json!({"action":"start","command":"echo nope","timeoutSeconds":30});
    let error = verify_start(&fixture.database, &fixture.run_id, &input,
        &credential, authorized_proof().as_ref()).unwrap_err();
    assert!(error.contains("credential_mismatch"), "{error}");
    assert!(fixture.database.kernel_jobs_for_run(&fixture.run_id).unwrap().is_empty());
}

#[test]
fn model_cannot_supply_a_job_key_or_execution_credential() {
    let fixture = Fixture::new("no-model-identity");
    for field in ["idempotencyKey", "dispatch_id", "credential_digest", "credential"] {
        let mut input = json!({"action":"start","command":"echo nope"});
        input[field] = json!("self-issued");
        assert!(prepare(&input, &fixture.root).is_err());
    }
}

fn claimed_fixture_credential(fixture: &Fixture, call: &str, input: &Value, digest: &str) -> ExecutionCredential {
    let binding = fixture.database.run_control_binding(&fixture.run_id).unwrap().unwrap();
    let mut credential = ExecutionCredential::new(
        fox_engine_protocol::encode_dispatch_id(&fixture.run_id, call).unwrap(),
        fixture.run_id.clone(), binding.conversation_id.clone(),
        crate::database::kernel_execution_admission::launch_params_hash("run_command", &input.to_string()),
        fox_engine_protocol::ActionClass::Execute, None,
        binding.execution_profile_id, binding.permission_snapshot_id, Some(1),
        binding.budgets.tool_execution_ms, None,
        fox_engine_protocol::BackendRequirement { required: "fixture".into(), evidence_digest: Some(digest.into()) },
    ).unwrap();
    credential.realtime_requirements.clear();
    credential.credential_digest = credential.recompute_digest();
    fixture.database.issue_execution_credential(&credential).unwrap();
    assert_eq!(fixture.database.claim_execution_attempt(&fixture.run_id, &binding.conversation_id,
        &credential, &credential.intent_digest, "test-host").unwrap(), fox_engine_protocol::AttemptOutcome::Claimed);
    credential
}

#[test]
fn backend_change_at_manager_boundary_keeps_job_audit_without_start_evidence() {
    struct ChangingProof(std::sync::atomic::AtomicUsize);
    impl BackendCapabilityProof for ChangingProof {
        fn availability(&self) -> crate::process_jobs::BackendAvailability {
            if self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                crate::process_jobs::AvailableInTests::host_user_unconfined("boundary-proof").availability()
            } else { crate::process_jobs::HostVerifiedBackend.availability() }
        }
    }
    let fixture = Fixture::new("manager-proof-change");
    let input = json!({"action":"start","command":"echo never","timeoutSeconds":30});
    let credential = claimed_fixture_credential(&fixture, "call", &input, "boundary-proof");
    let mut evidence = ExecutionEvidence::NotStarted;
    let mut outcome = CallOutcome::Completed;
    let response = execute_inner(&fixture.database, &fixture.run_id, &input, &fixture.root,
        &fixture.token, Duration::from_secs(30),
        Arc::new(ChangingProof(std::sync::atomic::AtomicUsize::new(0))), Some(&credential),
        &mut evidence, &mut outcome).unwrap();
    assert_eq!(evidence, ExecutionEvidence::NotStarted);
    assert!(matches!(outcome, CallOutcome::Failed { .. }));
    assert_eq!(response["details"]["errorCode"], "sandbox_unavailable");
    let rows = fixture.database.kernel_jobs_for_run(&fixture.run_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, JobState::Failed);
    // Host settlement must not erase the durable job-creation stage.
    fixture.database.record_attempt_refusal(&fixture.run_id, &credential.dispatch_id,
        "sandbox_unavailable").unwrap();
    let receipt = fixture.database.execution_receipt(&fixture.run_id, &credential.dispatch_id)
        .unwrap().unwrap();
    assert_eq!(receipt.stage, fox_engine_protocol::ExecutionStage::JobCreated);
    assert_eq!(receipt.execution_started, fox_engine_protocol::TriState::False);
    assert_eq!(receipt.control_plane, fox_engine_protocol::ControlPlaneState::NotApplied);
}

#[cfg(windows)]
#[test]
fn real_child_start_returns_positive_evidence_independent_of_later_exit_failure() {
    let fixture = Fixture::new("real-start-evidence");
    let input = json!({"action":"start","command":"exit /b 7","timeoutSeconds":30});
    let credential = claimed_fixture_credential(&fixture, "call", &input, "command-jobs-adapter-tests");
    let mut evidence = ExecutionEvidence::NotStarted;
    let mut outcome = CallOutcome::Completed;
    let response = execute_inner(&fixture.database, &fixture.run_id, &input, &fixture.root,
        &fixture.token, Duration::from_secs(30), authorized_proof(), Some(&credential),
        &mut evidence, &mut outcome).unwrap();
    assert_eq!(evidence, ExecutionEvidence::Started);
    let row = fixture.wait_terminal(job_id(&response));
    assert_eq!(row.state, JobState::Failed);
    assert_eq!(evidence, ExecutionEvidence::Started, "exit failure cannot erase start evidence");
}

#[test]
fn admitted_backend_snapshot_cannot_change_between_manager_and_spawner() {
    let bound = SnapshotBackendProof { provider: authorized_proof(), expected_digest: "different-issued-digest".into() };
    assert!(matches!(bound.availability(), crate::process_jobs::BackendAvailability::Unavailable(
        crate::process_jobs::BackendRefusal::CredentialMismatch)));
    let matching = SnapshotBackendProof { provider: authorized_proof(), expected_digest: "command-jobs-adapter-tests".into() };
    assert!(matching.availability().is_available());
}
