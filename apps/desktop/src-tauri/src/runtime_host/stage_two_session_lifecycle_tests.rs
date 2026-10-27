//! Stage-two regression tests for the Pi-native session lifecycle.
//!
//! Each of these fails on the pre-fix code for a behavioural reason, not a
//! compile reason:
//!
//! * `a_failed_native_restore_stays_failed_on_the_next_open` — the second open
//!   used to create a brand-new empty session.
//! * `a_session_that_pi_never_persisted_is_never_called_recoverable` — a create
//!   used to record `ready` before Pi had written anything.
//! * `a_failed_mapping_write_leaves_no_row_pointing_at_the_removed_file` — the
//!   mapping and the Run association used to be two independent statements, so
//!   the row survived while the compensation deleted the file.

use super::session_lifecycle::{
    event_proves_persistence, native_session_file_is_persisted, open_decision, OpenDecision,
    SessionLifecycle, SessionOpenError, SessionRuntimeLink, SESSION_NOT_PERSISTED,
    SESSION_UNAVAILABLE,
};
use super::protocol::{timestamp, PROTOCOL_NAME, PROTOCOL_VERSION};
use super::RuntimeEnvelope;
use crate::database::{Database, RuntimeSessionRecord, RuntimeSessionStatus};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::Duration;

// ---------------------------------------------------------------- harness ---

/// One scripted answer from the runtime.
enum Scripted {
    /// `session_created`. `persist` decides whether the runtime also wrote the
    /// JSONL (a Pi version that persists eagerly) or only booked the path.
    Created { path: PathBuf, session_id: String, persist: bool },
    /// `request_failed` with a stable code.
    Rejected { code: &'static str, message: &'static str },
}

/// Scripted stand-in for the runtime's session protocol.
///
/// It records every request so a test can assert what the Host did *not* ask
/// for — "no create happened" is as important as "an error came back".
#[derive(Default)]
struct ScriptedRuntime {
    requests: Vec<(String, Value)>,
    answers: VecDeque<Scripted>,
}

impl ScriptedRuntime {
    fn with(answers: Vec<Scripted>) -> Self {
        Self {
            requests: Vec::new(),
            answers: answers.into(),
        }
    }

    fn request_types(&self) -> Vec<&str> {
        self.requests
            .iter()
            .map(|(kind, _)| kind.as_str())
            .collect()
    }
}

impl SessionRuntimeLink for ScriptedRuntime {
    fn send_session_request(
        &mut self,
        request: super::RuntimeRequest,
        _timeout: Duration,
    ) -> Result<RuntimeEnvelope, String> {
        self.requests
            .push((request.r#type.clone(), request.payload.clone().unwrap_or(Value::Null)));
        let answer = self.answers.pop_front().expect(
            "the Host sent a session request the test did not script \
             — an unrecoverable conversation must not ask the runtime anything",
        );
        let (reply_type, payload) = match answer {
            Scripted::Created {
                path,
                session_id,
                persist,
            } => {
                if persist {
                    // Only the runtime writes the file. The Host never does.
                    std::fs::write(&path, "{\"type\":\"session\"}\n").unwrap();
                }
                // The real runtime echoes the generation it was asked for
                // (pi-runtime.mjs `create_session`), so the double does too:
                // a hardcoded 1 would make a generation-2 install look like a
                // runtime that renamed the Host's fact.
                let echoed_generation = request
                    .payload
                    .as_ref()
                    .and_then(|payload| payload.get("contextGeneration"))
                    .cloned()
                    .unwrap_or(json!(1));
                (
                    "session_created",
                    json!({
                        "runtimeSessionId": request.runtime_session_id,
                        "format": "pi-native",
                        "nativeSessionPath": path.to_string_lossy(),
                        "nativeSessionId": session_id,
                        "contextGeneration": echoed_generation,
                    }),
                )
            }
            Scripted::Rejected { code, message } => (
                "request_failed",
                json!({ "code": code, "message": message }),
            ),
        };
        Ok(RuntimeEnvelope {
            protocol: PROTOCOL_NAME.to_owned(),
            version: PROTOCOL_VERSION,
            kind: "response".to_owned(),
            id: uuid::Uuid::new_v4().to_string(),
            r#type: reply_type.to_owned(),
            request_id: Some(request.id),
            conversation_id: request.conversation_id.clone(),
            runtime_session_id: request.runtime_session_id.clone(),
            run_id: None,
            seq: None,
            timestamp: timestamp(),
            payload: Some(payload),
        })
    }
}

struct Fixture {
    root: PathBuf,
    database: Database,
    conversation_id: String,
    sessions_dir: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "fox-stage-two-{label}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let database = Database::open(root.join("facts.db")).unwrap();
        let conversation = database
            .create_conversation(
                database.default_agent_id(),
                None,
                Some(root.to_str().unwrap()),
                Some("allow"),
            )
            .unwrap();
        let sessions_dir = root.join("runtime-sessions");
        std::fs::create_dir_all(&sessions_dir).unwrap();
        Self {
            root,
            database,
            conversation_id: conversation.id,
            sessions_dir,
        }
    }

    fn lifecycle(&self) -> SessionLifecycle<'_> {
        SessionLifecycle::new(
            &self.database,
            &self.sessions_dir,
            Some("0.1.0+pi-1.4.0".to_owned()),
        )
    }

    fn mapping(&self) -> Option<RuntimeSessionRecord> {
        self.database
            .get_runtime_session(&self.conversation_id)
            .unwrap()
    }

    fn queued_run(&self) -> String {
        self.database
            .create_run(&self.conversation_id, "hello", None)
            .unwrap()
            .run
            .id
    }

    fn run_runtime_session_id(&self, run_id: &str) -> Option<String> {
        self.database
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT runtime_session_id FROM runs WHERE id = ?1",
                        [run_id],
                        |row| row.get::<_, Option<String>>(0),
                    )
                    .map_err(|error| error)
            })
            .unwrap()
    }

    /// Every file in the session directory, so "no new file" is observable.
    fn session_files(&self) -> Vec<String> {
        let mut files = std::fs::read_dir(&self.sessions_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    fn session_file(&self, name: &str) -> PathBuf {
        self.sessions_dir.join(name)
    }

    fn seed_native_mapping(&self, runtime_session_id: &str, file_name: &str, status: RuntimeSessionStatus) -> PathBuf {
        let file = self.session_file(file_name);
        self.database
            .ensure_native_runtime_session(
                &self.conversation_id,
                runtime_session_id,
                Some("0.1.0+pi-1.4.0"),
                file.to_string_lossy().as_ref(),
                Some("pi-session-7"),
                1,
                status,
            )
            .unwrap();
        file
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn recovery_code(error: &SessionOpenError) -> Option<&str> {
    match error {
        SessionOpenError::RecoveryProblem { code, .. } => Some(code.as_str()),
        SessionOpenError::Failed(_) => None,
    }
}

// ------------------------------------------------------- (a) sticky failure --

/// Two consecutive native resume failures: the second open must still report a
/// recovery problem, must not create a session, must not touch the row and must
/// not add a file.
///
/// Pre-fix: the row was `unavailable`, so the second open silently ran
/// `create_session` and returned a brand-new empty context.
#[test]
fn a_failed_native_restore_stays_failed_on_the_next_open() {
    let fixture = Fixture::new("sticky-resume");
    let file = fixture.seed_native_mapping(
        "runtime-1",
        "pi-session-7.jsonl",
        RuntimeSessionStatus::Ready,
    );
    // A persisted file that Pi can no longer read (corrupt) is a realistic
    // restore failure that is *not* "the file is gone".
    std::fs::write(&file, "not a pi session\n").unwrap();
    let run_id = fixture.queued_run();
    let before = fixture.mapping().expect("seeded mapping");
    assert_eq!(before.status, "ready");

    let mut first = ScriptedRuntime::with(vec![Scripted::Rejected {
        code: "session.native_corrupt",
        message: "The Pi session file cannot be opened: bad header",
    }]);
    let first_error = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut first)
        .expect_err("a corrupted native session must not open");
    assert_eq!(recovery_code(&first_error), Some("session.native_corrupt"), "{first_error:?}");
    assert_eq!(
        fixture.mapping().unwrap().status,
        "unavailable",
        "a failed restore has to be recorded"
    );

    // Second open: no scripted answers at all — any runtime request would be a
    // bug, and the fake would panic on it.
    let mut second = ScriptedRuntime::default();
    let second_error = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut second)
        .expect_err("the second open must still report the recovery problem");
    assert_eq!(
        recovery_code(&second_error),
        Some(SESSION_UNAVAILABLE),
        "second open reported {second_error:?}"
    );
    assert!(
        second.requests.is_empty(),
        "the Host must not ask the runtime anything for an unrecoverable session: {:?}",
        second.request_types()
    );

    let after = fixture.mapping().expect("the mapping must still exist");
    assert_eq!(after.runtime_session_id, "runtime-1", "no new session row");
    assert_eq!(after.status, "unavailable");
    assert_eq!(
        after.native_session_path.as_deref(),
        Some(file.to_string_lossy().as_ref()),
        "the failed row must keep naming the file it could not reopen"
    );
    assert_eq!(
        fixture.session_files(),
        vec!["pi-session-7.jsonl".to_owned()],
        "no new session file may appear"
    );
    assert_eq!(
        fixture.run_runtime_session_id(&run_id),
        None,
        "no queued Run may be associated with a session the Host refused to open"
    );
}

// ------------------------------------------- (b) allocated vs. persisted --

/// The process exits before the first message is written: the mapping must not
/// claim `ready`, and a later resume must not pretend the session is
/// recoverable.
///
/// Pre-fix: the row was written `ready` as soon as the path was allocated.
#[test]
fn a_session_that_pi_never_persisted_is_never_called_recoverable() {
    let fixture = Fixture::new("never-persisted");
    let file = fixture.session_file("20261027_pi-new.jsonl");
    assert!(!file.exists(), "fixture assumes Pi has not written yet");

    let mut creating = ScriptedRuntime::with(vec![Scripted::Created {
        path: file.clone(),
        session_id: "pi-new".to_owned(),
        persist: false,
    }]);
    let (runtime_session_id, opened) = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut creating)
        .expect("a fresh conversation may create a session");
    // The Host records the canonical path it validated, which is the same file
    // named by the runtime (the parent directory is canonicalized).
    let expected = std::fs::canonicalize(&fixture.sessions_dir)
        .unwrap()
        .join("20261027_pi-new.jsonl");
    assert_eq!(opened, expected);
    assert_eq!(creating.request_types(), vec!["create_session"]);

    let mapping = fixture.mapping().expect("created mapping");
    assert_eq!(mapping.status, "allocated", "the file was never written");
    assert!(!file.exists(), "the Host must never pre-create Pi's file");
    assert_eq!(mapping.runtime_session_id, runtime_session_id);

    // The process dies here (the worker that held the session is gone). The
    // later open must not claim a recoverable context and must never resume the
    // missing file — but the generation the user already created is continued
    // by adopting the session Pi creates now, in the same generation, with no
    // history replayed. Refusing here is what forced a second reset.
    let adopted_file = fixture.session_file("20261027_pi-adopted.jsonl");
    let mut later = ScriptedRuntime::with(vec![Scripted::Created {
        path: adopted_file.clone(),
        session_id: "pi-adopted".to_owned(),
        persist: false,
    }]);
    let (adopted_id, adopted_path) = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut later)
        .expect("a generation Pi never wrote is continued, not refused");
    assert_eq!(
        adopted_id, runtime_session_id,
        "adoption keeps the conversation's Fox runtime session id"
    );
    assert_eq!(
        later.request_types(),
        vec!["create_session"],
        "the missing file is never resumed: the new generation adopts a new Pi session"
    );
    let mapping = fixture.mapping().expect("the mapping survives adoption");
    assert_eq!(mapping.status, "allocated", "adoption never claims the file exists");
    assert_eq!(mapping.runtime_session_id, runtime_session_id);
    assert_eq!(mapping.context_generation, 1, "adoption keeps the generation");
    assert_eq!(mapping.native_session_ref.as_deref(), Some("pi-adopted"));
    let expected_adopted = std::fs::canonicalize(&fixture.sessions_dir)
        .unwrap()
        .join("20261027_pi-adopted.jsonl");
    assert_eq!(adopted_path, expected_adopted);
    assert_eq!(!file.exists(), true);
}

/// An `allocated` mapping whose file did appear (Pi wrote it after the Host
/// last checked) must heal: the next open promotes it to `ready` and resumes.
#[test]
fn an_allocated_mapping_becomes_ready_once_pi_has_written_the_file() {
    let fixture = Fixture::new("allocated-heals");
    let file = fixture.seed_native_mapping(
        "runtime-2",
        "pi-session-8.jsonl",
        RuntimeSessionStatus::Allocated,
    );
    assert!(!file.exists());

    // Nothing to promote yet: the explicit verification call says so without
    // inventing a file.
    assert_eq!(
        fixture
            .lifecycle()
            .verify(&fixture.conversation_id)
            .unwrap(),
        Some(RuntimeSessionStatus::Allocated)
    );
    assert!(!file.exists());

    // Pi persisted in the meantime (its own writer, not the Host's).
    std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();

    let mut resuming = ScriptedRuntime::with(vec![Scripted::Created {
        path: file.clone(),
        session_id: "pi-session-8".to_owned(),
        persist: false,
    }]);
    let (opened_id, opened_path) = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut resuming)
        .expect("a persisted file is resumable");
    assert_eq!(opened_id, "runtime-2");
    assert_eq!(opened_path, file);
    assert_eq!(resuming.request_types(), vec!["resume_session"]);
    assert_eq!(
        fixture.mapping().unwrap().status,
        "ready",
        "opening the file proves it is recoverable"
    );
}

/// `verify` is the explicit verification call: it promotes only once the file
/// exists, and never writes the file itself.
#[test]
fn verification_promotes_an_allocated_mapping_without_touching_the_file() {
    let fixture = Fixture::new("verify-promotes");
    let file = fixture.seed_native_mapping(
        "runtime-3",
        "pi-session-9.jsonl",
        RuntimeSessionStatus::Allocated,
    );
    std::fs::write(&file, "{\"type\":\"session\"}\n").unwrap();
    let before = std::fs::read(&file).unwrap();

    assert_eq!(
        fixture
            .lifecycle()
            .verify(&fixture.conversation_id)
            .unwrap(),
        Some(RuntimeSessionStatus::Ready)
    );
    assert_eq!(fixture.mapping().unwrap().status, "ready");
    assert_eq!(std::fs::read(&file).unwrap(), before, "verify must not rewrite Pi's file");
}

// ------------------------------------------- (c) atomic mapping + runs ----

/// The second (database) step fails: no row may reference the removed file and
/// the compensation must leave nothing dangling.
///
/// Pre-fix: the INSERT was committed on its own, the Run association failed,
/// the Host deleted the file, and the surviving row pointed at a deleted file.
#[test]
fn a_failed_mapping_write_leaves_no_row_pointing_at_the_removed_file() {
    let fixture = Fixture::new("atomic-mapping");
    let run_id = fixture.queued_run();
    let file = fixture.session_file("20261027_pi-atomic.jsonl");
    // The runtime persisted the file, so the mapping would be `ready` and the
    // compensation has something real to remove.
    let mut creating = ScriptedRuntime::with(vec![Scripted::Created {
        path: file.clone(),
        session_id: "pi-atomic".to_owned(),
        persist: true,
    }]);
    crate::database::native_session_fault::fail_next_run_association();
    let result = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut creating);
    let fired = crate::database::native_session_fault::fired();
    crate::database::native_session_fault::disarm();

    assert!(fired, "the fault must fire at the Run association, not elsewhere");
    assert!(
        matches!(result, Err(SessionOpenError::Failed(_))),
        "the mapping write must fail visibly, got {result:?}"
    );
    assert!(
        fixture.mapping().is_none(),
        "a failed transaction must leave no runtime_sessions row behind"
    );
    assert!(
        !file.exists(),
        "the compensation must remove the file nothing references"
    );
    assert_eq!(
        fixture.run_runtime_session_id(&run_id),
        None,
        "no Run may be associated with a mapping that was rolled back"
    );
    assert!(fixture.session_files().is_empty());
}

// --------------------------------------------- legacy behaviour unchanged --

/// A `format_version` 1 row is the legacy Fox JSON snapshot: it still resumes by
/// path, and a snapshot that cannot be reopened still starts a fresh Pi-native
/// session exactly as it did before stage two.
#[test]
fn a_legacy_snapshot_still_resumes_and_still_falls_back_to_a_fresh_session() {
    let fixture = Fixture::new("legacy");
    let snapshot = fixture.session_file("legacy-1.json");
    std::fs::write(&snapshot, "{}").unwrap();
    fixture
        .database
        .ensure_runtime_session(
            &fixture.conversation_id,
            "legacy-1",
            None,
            snapshot.to_str(),
        )
        .unwrap();

    let mut resuming = ScriptedRuntime::with(vec![Scripted::Created {
        path: snapshot.clone(),
        session_id: "legacy-1".to_owned(),
        persist: false,
    }]);
    let (opened_id, opened_path) = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut resuming)
        .expect("a legacy snapshot still opens");
    assert_eq!(opened_id, "legacy-1");
    assert_eq!(opened_path, snapshot);
    assert_eq!(resuming.request_types(), vec!["resume_session"]);
    let (_, payload) = &resuming.requests[0];
    assert_eq!(payload["sessionPath"], json!(snapshot.to_string_lossy()));
    assert!(
        payload.get("sessionFormat").is_none()
            && payload.get("nativeSessionPath").is_none(),
        "a legacy row must keep asking for its snapshot by path: {payload}"
    );
    let mapping = fixture.mapping().unwrap();
    assert_eq!(mapping.format_version, 1, "a legacy row stays legacy");
    assert_eq!(mapping.status, "ready");

    // The snapshot is gone, so the legacy reopen fails: pre-stage-two behaviour
    // is to start a fresh Pi-native session (and only the native path refuses).
    std::fs::remove_file(&snapshot).unwrap();
    let fresh = fixture.session_file("20261027_fresh.jsonl");
    let mut falling_back = ScriptedRuntime::with(vec![
        Scripted::Rejected {
            code: "session.native_missing",
            message: "The Pi session file does not exist",
        },
        Scripted::Created {
            path: fresh.clone(),
            session_id: "pi-fresh".to_owned(),
            persist: false,
        },
    ]);
    let (new_id, new_path) = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut falling_back)
        .expect("a failed legacy reopen still starts a fresh session");
    assert_ne!(new_id, "legacy-1");
    assert_eq!(falling_back.request_types(), vec!["resume_session", "create_session"]);
    let mapping = fixture.mapping().unwrap();
    assert_eq!(mapping.runtime_session_id, new_id);
    assert_eq!(mapping.format_version, 2, "the fresh session is Pi-native");
    assert_eq!(mapping.status, "allocated");
    let expected = std::fs::canonicalize(&fixture.sessions_dir)
        .unwrap()
        .join("20261027_fresh.jsonl");
    assert_eq!(new_path, expected);
}

// ------------------------------------------------------ pure decision table --

fn native_record(status: RuntimeSessionStatus) -> RuntimeSessionRecord {
    RuntimeSessionRecord {
        runtime_session_id: "runtime-1".into(),
        runtime_version: Some("0.1.0+pi-1.4.0".into()),
        session_path: None,
        status: status.as_str().into(),
        native_session_ref: Some("pi-session-7".into()),
        native_session_path: Some("C:/fox/sessions/pi-session-7.jsonl".into()),
        format_version: 2,
        context_generation: 1,
    }
}

fn legacy_record(status: &str) -> RuntimeSessionRecord {
    RuntimeSessionRecord {
        runtime_session_id: "runtime-1".into(),
        runtime_version: None,
        session_path: Some("C:/fox/runtime-1.json".into()),
        status: status.into(),
        native_session_ref: None,
        native_session_path: None,
        format_version: 1,
        context_generation: 1,
    }
}

#[test]
fn the_open_decision_never_turns_a_native_failure_into_a_fresh_session() {
    assert_eq!(open_decision(None, false), OpenDecision::Create);
    assert_eq!(
        open_decision(Some(&native_record(RuntimeSessionStatus::Ready)), false),
        OpenDecision::Resume
    );
    // `allocated` is decided by the filesystem, not by the status alone.
    assert_eq!(
        open_decision(Some(&native_record(RuntimeSessionStatus::Allocated)), true),
        OpenDecision::Resume
    );
    assert!(matches!(
        open_decision(Some(&native_record(RuntimeSessionStatus::Allocated)), false),
        OpenDecision::Refuse { code: SESSION_NOT_PERSISTED, .. }
    ));
    assert!(matches!(
        open_decision(Some(&native_record(RuntimeSessionStatus::Unavailable)), true),
        OpenDecision::Refuse { code: SESSION_UNAVAILABLE, .. }
    ));
    // A status this build does not understand is never "recoverable".
    let mut unknown = native_record(RuntimeSessionStatus::Ready);
    unknown.status = "mystery".into();
    assert!(matches!(
        open_decision(Some(&unknown), true),
        OpenDecision::Refuse { code: SESSION_UNAVAILABLE, .. }
    ));
}

#[test]
fn legacy_rows_keep_their_pre_stage_two_behaviour() {
    assert_eq!(
        open_decision(Some(&legacy_record("ready")), false),
        OpenDecision::Resume
    );
    // Unchanged from before stage two: an unavailable legacy snapshot falls
    // back to a fresh Pi-native session.
    assert_eq!(
        open_decision(Some(&legacy_record("unavailable")), false),
        OpenDecision::Create
    );
    // And an unknown legacy status stays resumable, as it did before.
    assert_eq!(
        open_decision(Some(&legacy_record("mystery")), false),
        OpenDecision::Resume
    );
}

#[test]
fn only_a_real_persistence_event_may_promote_a_mapping() {
    assert!(!event_proves_persistence("run.started"), "emitted before the first append");
    assert!(!event_proves_persistence("message.delta"));
    assert!(!event_proves_persistence("run.phase"));
    for event in [
        "message.completed",
        "run.completed",
        "run.failed",
        "run.cancelled",
        "run.interrupted",
    ] {
        assert!(event_proves_persistence(event), "{event}");
    }
}

#[test]
fn an_empty_or_missing_file_is_not_a_persisted_session() {
    let root = std::env::temp_dir().join(format!("fox-stage-two-isset-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let missing = root.join("missing.jsonl");
    let empty = root.join("empty.jsonl");
    let written = root.join("written.jsonl");
    std::fs::write(&empty, "").unwrap();
    std::fs::write(&written, "{\"type\":\"session\"}\n").unwrap();
    assert!(!native_session_file_is_persisted(&missing));
    assert!(!native_session_file_is_persisted(&empty));
    assert!(!native_session_file_is_persisted(&root), "a directory is not a session file");
    assert!(native_session_file_is_persisted(&written));
    let _ = std::fs::remove_dir_all(Path::new(&root));
}
