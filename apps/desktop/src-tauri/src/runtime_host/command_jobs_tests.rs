use super::*;
use crate::database::{JobStartRequest, JobState};
use crate::kernel::CancellationRegistry;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static SERIAL: Mutex<()> = Mutex::new(());

struct Fixture {
    database: Database,
    root: PathBuf,
    run_id: String,
    token: CancellationToken,
}

impl Fixture {
    fn new(label: &str) -> Self {
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
        }
    }

    fn start(&self, key: &str, command: &str) -> Result<Value, String> {
        execute(
            &self.database,
            &self.run_id,
            &json!({
                "action": "start",
                "command": command,
                "timeoutSeconds": 30,
                "idempotencyKey": key
            }),
            &self.root,
            &self.token,
            Duration::from_secs(30),
        )
    }

    fn action(&self, value: Value) -> Result<Value, String> {
        execute(
            &self.database,
            &self.run_id,
            &value,
            &self.root,
            &self.token,
            Duration::from_secs(30),
        )
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

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn job_id(result: &Value) -> &str {
    result["details"]["jobId"]
        .as_str()
        .expect("job id in start result")
}

#[cfg(windows)]
#[test]
fn production_adapter_start_nonzero_and_utf8_byte_paging_are_durable() {
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
    let _serial = serial();
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
