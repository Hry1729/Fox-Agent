//! Stage-two work package E: context reset, the retired-generation ledger and
//! permanent-delete file ownership.
//!
//! Every test fails on the pre-fix code for a behavioural reason, not a compile
//! reason:
//!
//! * a reset was impossible (the single runtime_sessions row was overwritten,
//!   so the old file lost its owner and permanent delete orphaned it);
//! * a reset could race a live Run (the Run owns the session being retired);
//! * permanent delete only removed the legacy snapshot path, so every Pi-native
//!   JSONL survived forever;
//! * nothing checked that a recorded path really belonged to this conversation
//!   and lived inside the Fox session directory before deleting it.

use super::protocol::{timestamp, PROTOCOL_NAME, PROTOCOL_VERSION};
use super::session_lifecycle::{
    allocated_mapping_is_live, purge_owned_session_files, SessionLifecycle, SessionRuntimeLink,
    RESET_RUN_ACTIVE, SESSION_FILE_OTHER_OWNER, SESSION_FILE_OUTSIDE_DIRECTORY,
    SESSION_NOT_PERSISTED,
};
use super::RuntimeEnvelope;
use crate::database::{Database, RuntimeSessionRecord, RuntimeSessionStatus};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

// ---------------------------------------------------------------- harness ---

/// One scripted answer from the runtime.
enum Scripted {
    /// session_created. `persist` decides whether the runtime also wrote the
    /// JSONL (a Pi version that persists eagerly) or only booked the path.
    Created { path: PathBuf, session_id: String, persist: bool },
    /// request_failed with a stable code.
    Rejected { code: &'static str, message: &'static str },
}

/// Scripted stand-in for the runtime's session protocol.
///
/// Records every request so a test can assert what the Host did *not* ask for:
/// "no resume of the old file" and "no history injected" are as important as
/// "a create happened".
#[derive(Default)]
struct ScriptedRuntime {
    requests: Vec<(String, Value)>,
    answers: VecDeque<Scripted>,
}

impl ScriptedRuntime {
    fn with(answers: Vec<Scripted>) -> Self {
        Self { requests: Vec::new(), answers: answers.into() }
    }

    fn request_types(&self) -> Vec<&str> {
        self.requests.iter().map(|(kind, _)| kind.as_str()).collect()
    }
}

impl SessionRuntimeLink for ScriptedRuntime {
    fn send_session_request(
        &mut self,
        request: super::RuntimeRequest,
        _timeout: Duration,
    ) -> Result<RuntimeEnvelope, String> {
        let generation = request
            .payload
            .as_ref()
            .and_then(|payload| payload.get("contextGeneration"))
            .cloned()
            .unwrap_or(json!(1));
        self.requests
            .push((request.r#type.clone(), request.payload.clone().unwrap_or(Value::Null)));
        let answer = self
            .answers
            .pop_front()
            .expect("the Host sent a session request the test did not script");
        let (reply_type, payload) = match answer {
            Scripted::Created { path, session_id, persist } => {
                if persist {
                    // Only the runtime writes the file. The Host never does.
                    std::fs::write(&path, "{\"type\":\"session\"}\n").unwrap();
                }
                (
                    "session_created",
                    json!({
                        "runtimeSessionId": request.runtime_session_id,
                        "format": "pi-native",
                        "nativeSessionPath": path.to_string_lossy(),
                        "nativeSessionId": session_id,
                        "contextGeneration": generation,
                    }),
                )
            }
            Scripted::Rejected { code, message } => {
                ("request_failed", json!({ "code": code, "message": message }))
            }
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
            "fox-context-reset-{label}-{}",
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
        self.database.get_runtime_session(&self.conversation_id).unwrap()
    }

    fn file(&self, name: &str) -> PathBuf {
        self.sessions_dir.join(name)
    }

    fn files(&self) -> Vec<String> {
        let mut files = std::fs::read_dir(&self.sessions_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    /// A native mapping for the given generation (the live row).
    fn seed_mapping(
        &self,
        conversation_id: &str,
        runtime_session_id: &str,
        path: &PathBuf,
        generation: i64,
        status: RuntimeSessionStatus,
    ) {
        self.database
            .ensure_native_runtime_session(
                conversation_id,
                runtime_session_id,
                Some("0.1.0+pi-1.4.0"),
                path.to_string_lossy().as_ref(),
                Some("pi-session-7"),
                generation,
                status,
            )
            .unwrap();
    }

    /// Seed the fixture conversation's live mapping at generation 1 and write
    /// the bytes Pi would own.
    fn seed_generation_one(&self, file_name: &str, bytes: &str) -> PathBuf {
        let file = self.file(file_name);
        std::fs::write(&file, bytes).unwrap();
        self.seed_mapping(
            &self.conversation_id,
            "runtime-1",
            &file,
            1,
            RuntimeSessionStatus::Ready,
        );
        file
    }

    /// Insert a retired generation exactly as a reset would, with an explicit
    /// path (so a test can simulate a row whose stored path is not in the
    /// session directory).
    fn seed_retired_generation(
        &self,
        conversation_id: &str,
        generation: i64,
        runtime_session_id: &str,
        path: &std::path::Path,
        format_version: i64,
    ) {
        let snapshot = if format_version >= 2 { None } else { Some(path.to_string_lossy().into_owned()) };
        let native = if format_version >= 2 { Some(path.to_string_lossy().into_owned()) } else { None };
        self.database
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO runtime_session_generations(
                            conversation_id, runtime_type, context_generation, runtime_session_id,
                            runtime_version, session_path, native_session_ref, native_session_path,
                            format_version, status, retired_at
                         ) VALUES (?1, 'pi', ?2, ?3, '0.1.0+pi-1.4.0', ?4, NULL, ?5, ?6, 'ready', 0)",
                        params![conversation_id, generation, runtime_session_id, snapshot, native, format_version],
                    )
                    .map(|_| ())
            })
            .unwrap();
    }

    fn generations(&self) -> Vec<(i64, String, String, bool)> {
        let mut rows = self
            .database
            .conversation_runtime_session_files(&self.conversation_id)
            .unwrap()
            .into_iter()
            .map(|record| {
                let generation = record.context_generation;
                let runtime_session_id = record.runtime_session_id.clone();
                let path = record
                    .file_path()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (generation, runtime_session_id, path, record.current)
            })
            .collect::<Vec<_>>();
        rows.sort();
        rows
    }

    fn message_count(&self, conversation_id: &str) -> usize {
        self.database
            .load_conversation_history(conversation_id, i64::MAX, 200)
            .unwrap()
            .messages
            .len()
    }

    fn second_conversation(&self) -> String {
        self.database
            .create_conversation(
                self.database.default_agent_id(),
                None,
                Some(self.root.to_str().unwrap()),
                Some("allow"),
            )
            .unwrap()
            .id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn canonical(dir: &std::path::Path, name: &str) -> PathBuf {
    std::fs::canonicalize(dir).unwrap().join(name)
}
// ------------------------------------------ (a) two resets, three generations --

/// Two consecutive resets: 1 -> 2 -> 3, each generation has its own runtime
/// session id and its own file, all three are recorded, the old files stay on
/// disk unchanged, and everything that belongs to the chat is untouched.
///
/// Pre-fix: there was no reset at all, and the single row would have been
/// overwritten (losing the old file's owner) if it existed.
#[test]
fn two_resets_record_three_generations_and_keep_the_chat_intact() {
    let fixture = Fixture::new("two-resets");
    let expert = fixture
        .database
        .bind_conversation_expert(&fixture.conversation_id, "fox-debugger", "test")
        .unwrap();
    // The legacy binding table names a real service connection.
    fixture
        .database
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO service_connections(
                        id, service_type, name, base_url, enabled, last_status, created_at, updated_at
                     ) VALUES ('yuxi-primary','yuxi','Yuxi','http://127.0.0.1:1',1,'unknown',1,1)",
                    [],
                )
                .map(|_| ())
        })
        .unwrap();
    fixture
        .database
        .set_knowledge_bindings(
            &fixture.conversation_id,
            &[("kb-1".to_owned(), "知识库".to_owned())],
        )
        .unwrap();
    // One completed Run: a real message in the history and no active Run.
    let failed_run = fixture
        .database
        .create_run(&fixture.conversation_id, "你好", None)
        .unwrap()
        .run
        .id;
    fixture
        .database
        .mark_run_failed(&failed_run, "run.test", "finished by the test")
        .unwrap();
    let messages_before = fixture.message_count(&fixture.conversation_id);
    assert!(messages_before > 0, "the fixture must really have history");
    let backend_before: String = fixture
        .database
        .with_connection(|connection| {
            connection.query_row(
                "SELECT agent_backend FROM conversations WHERE id = ?1",
                [fixture.conversation_id.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();

    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let generation_two = fixture.file("gen-2.jsonl");
    let generation_three = fixture.file("gen-3.jsonl");

    let first = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: generation_two.clone(),
                session_id: "pi-2".to_owned(),
                persist: true,
            }]),
        )
        .expect("the first reset succeeds");
    assert_eq!(first.context_generation, 2);
    assert_eq!(first.session_path, canonical(&fixture.sessions_dir, "gen-2.jsonl"));
    std::fs::write(&generation_two, "generation two bytes").unwrap();

    let second = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: generation_three.clone(),
                session_id: "pi-3".to_owned(),
                persist: true,
            }]),
        )
        .expect("the second reset succeeds");
    assert_eq!(second.context_generation, 3);
    assert_ne!(first.runtime_session_id, second.runtime_session_id);
    assert_ne!(first.session_path, second.session_path);
    std::fs::write(&generation_three, "generation three bytes").unwrap();

    // The live mapping is generation 3.
    let live = fixture.mapping().expect("a live mapping");
    assert_eq!(live.context_generation, 3);
    assert_eq!(live.runtime_session_id, second.runtime_session_id);
    assert_eq!(
        live.native_session_path.as_deref().map(PathBuf::from),
        Some(canonical(&fixture.sessions_dir, "gen-3.jsonl"))
    );

    // All three generations are recorded, each with its own id and file.
    let generations = fixture.generations();
    assert_eq!(
        generations.iter().map(|row| row.0).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "a reset must not lose a generation"
    );
    let mut ids = generations.iter().map(|row| row.1.clone()).collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "every generation has a distinct runtime session id: {generations:?}");
    let mut paths = generations.iter().map(|row| row.2.clone()).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), 3, "every generation has its own file: {generations:?}");
    assert_eq!(
        generations
            .iter()
            .filter(|row| row.3)
            .map(|row| row.0)
            .collect::<Vec<_>>(),
        vec![3],
        "exactly one generation is live"
    );

    // The old files are still on disk, byte for byte.
    assert_eq!(
        std::fs::read_to_string(&generation_one).unwrap(),
        "generation one bytes",
        "the first generation's file must never be written, truncated or deleted"
    );
    assert_eq!(
        std::fs::read_to_string(&generation_two).unwrap(),
        "generation two bytes"
    );
    assert_eq!(
        std::fs::read_to_string(&generation_three).unwrap(),
        "generation three bytes"
    );

    // The chat itself is untouched.
    let live_expert = fixture
        .database
        .current_conversation_expert_binding(&fixture.conversation_id)
        .unwrap()
        .expect("the expert binding survives a reset");
    assert_eq!(live_expert.id, expert.id);
    assert_eq!(live_expert.expert_id, "fox-debugger");
    let knowledge = fixture
        .database
        .conversation_knowledge_bindings(&fixture.conversation_id)
        .unwrap();
    assert_eq!(knowledge.len(), 1);
    assert_eq!(knowledge[0].knowledge_base_id, "kb-1");
    assert_eq!(
        fixture.message_count(&fixture.conversation_id),
        messages_before,
        "history is kept; a reset is not a delete"
    );
    let backend_after: String = fixture
        .database
        .with_connection(|connection| {
            connection.query_row(
                "SELECT agent_backend FROM conversations WHERE id = ?1",
                [fixture.conversation_id.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(backend_after, backend_before, "agent_backend is not a session fact");
}
// --------------------------------------- (b) no old context into the new session --

/// After a reset the next Run must use the new generation: the Host asks the
/// runtime to create a new session (never to resume the old file), and when it
/// opens the new mapping it injects no history at all.
///
/// Pre-fix: the old session was resumed, so the new context was the old one.
#[test]
fn a_reset_never_reopens_the_old_context_on_the_next_run() {
    let fixture = Fixture::new("no-replay");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "the old context");

    let mut resetting = ScriptedRuntime::with(vec![Scripted::Created {
        path: fixture.file("gen-2.jsonl"),
        session_id: "pi-2".to_owned(),
        persist: false,
    }]);
    let outcome = fixture
        .lifecycle()
        .reset(&fixture.conversation_id, &mut resetting)
        .expect("reset");
    assert_eq!(outcome.context_generation, 2);
    assert_eq!(
        resetting.request_types(),
        vec!["create_session"],
        "a reset creates a new session and never resumes the old one"
    );
    let (_, create_payload) = &resetting.requests[0];
    assert_eq!(create_payload["contextGeneration"], json!(2));
    for forbidden in ["messages", "history", "transcript", "sessionPath", "nativeSessionPath"] {
        assert!(
            create_payload.get(forbidden).is_none(),
            "a reset must not replay the old context ({forbidden}): {create_payload}"
        );
    }
    assert!(
        !outcome.session_path.exists(),
        "the fixture assumes Pi has not written the new file yet"
    );

    // The next Run opens the mapping while the live worker still holds the new
    // session: nothing is replayed, nothing is reopened.
    let mut next_run = ScriptedRuntime::default();
    let opened = fixture
        .lifecycle()
        .open_with_live_session(
            &fixture.conversation_id,
            &mut next_run,
            Some(&outcome.runtime_session_id),
        )
        .expect("the live, just-reset session is reusable");
    assert_eq!(opened.0, outcome.runtime_session_id);
    assert_eq!(opened.1, outcome.session_path);
    assert!(
        next_run.requests.is_empty(),
        "reusing the live session must not ask the runtime to open anything: {:?}",
        next_run.request_types()
    );

    // Another (stale) live id never unlocks a session Pi did not persist.
    let mut stale = ScriptedRuntime::default();
    let error = fixture
        .lifecycle()
        .open_with_live_session(&fixture.conversation_id, &mut stale, Some("runtime-1"))
        .expect_err("a stale live id must not stand in for a persisted file");
    assert_eq!(
        match &error {
            super::session_lifecycle::SessionOpenError::RecoveryProblem { code, .. } => code.as_str(),
            _ => "",
        },
        SESSION_NOT_PERSISTED
    );
    assert!(stale.requests.is_empty());

    // Once Pi writes the file, opening it resumes *that* file with the new
    // generation, and still injects no history.
    std::fs::write(&outcome.session_path, "{\"type\":\"session\"}\n").unwrap();
    let mut later_run = ScriptedRuntime::with(vec![Scripted::Created {
        path: outcome.session_path.clone(),
        session_id: "pi-2".to_owned(),
        persist: false,
    }]);
    let reopened = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut later_run)
        .expect("the persisted new generation opens");
    assert_eq!(reopened.0, outcome.runtime_session_id);
    assert_eq!(later_run.request_types(), vec!["resume_session"]);
    let (_, resume_payload) = &later_run.requests[0];
    assert_eq!(
        resume_payload["nativeSessionPath"],
        json!(outcome.session_path.to_string_lossy())
    );
    assert_ne!(
        resume_payload["nativeSessionPath"],
        json!(generation_one.to_string_lossy()),
        "the old file must never be reopened"
    );
    assert_eq!(resume_payload["contextGeneration"], json!(2));
    for forbidden in ["messages", "history", "transcript"] {
        assert!(
            resume_payload.get(forbidden).is_none(),
            "no history may be replayed into the new session ({forbidden}): {resume_payload}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&generation_one).unwrap(),
        "the old context",
        "the old generation's bytes are untouched"
    );
}
// --------------------------------------------------- (c) refusal and rollback --

/// A reset while a Run is active is refused and changes nothing.
///
/// Pre-fix: a reset did not exist; a naive implementation would retire the
/// mapping out from under the live Run.
#[test]
fn a_reset_while_a_run_is_active_is_refused_and_changes_nothing() {
    let fixture = Fixture::new("run-active");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let active_run = fixture
        .database
        .create_run(&fixture.conversation_id, "正在运行", None)
        .unwrap()
        .run
        .id;
    assert!(fixture
        .database
        .conversation_has_active_run(&fixture.conversation_id)
        .unwrap());

    // No answers: any runtime request at all would be a bug.
    let mut runtime = ScriptedRuntime::default();
    let refusal = fixture
        .lifecycle()
        .reset(&fixture.conversation_id, &mut runtime)
        .expect_err("a live Run must refuse the reset");
    assert_eq!(refusal.code, RESET_RUN_ACTIVE);
    assert!(runtime.requests.is_empty(), "a refused reset must not touch the runtime");

    let mapping = fixture.mapping().expect("the mapping must survive");
    assert_eq!(mapping.context_generation, 1);
    assert_eq!(mapping.runtime_session_id, "runtime-1");
    assert_eq!(
        mapping.native_session_path.as_deref().map(PathBuf::from),
        Some(generation_one.clone())
    );
    assert_eq!(fixture.files(), vec!["gen-1.jsonl".to_owned()], "no new file");
    assert!(
        fixture
            .database
            .conversation_runtime_session_files(&fixture.conversation_id)
            .unwrap()
            .iter()
            .all(|record| record.current),
        "no generation may be retired by a refused reset"
    );
    assert!(!active_run.is_empty());
}

/// A failure after the new session was created rolls back: the previous mapping
/// survives and still opens.
///
/// Pre-fix: nothing to roll back, but a naive reset (create, then upsert) would
/// leave a half-written state.
#[test]
fn a_failed_reset_rolls_back_and_the_previous_session_still_opens() {
    let fixture = Fixture::new("reset-rollback");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let generation_two = fixture.file("gen-2.jsonl");

    crate::database::native_session_fault::fail_next_run_association();
    let outcome = fixture.lifecycle().reset(
        &fixture.conversation_id,
        &mut ScriptedRuntime::with(vec![Scripted::Created {
            path: generation_two.clone(),
            session_id: "pi-2".to_owned(),
            persist: true,
        }]),
    );
    let fired = crate::database::native_session_fault::fired();
    crate::database::native_session_fault::disarm();

    assert!(fired, "the fault must fire inside the reset transaction");
    let refusal = outcome.expect_err("a failed mapping write must refuse the reset");
    assert_eq!(refusal.code, super::session_lifecycle::RESET_FAILED);
    // The previous mapping is untouched.
    let mapping = fixture.mapping().expect("the previous mapping survives");
    assert_eq!(mapping.context_generation, 1);
    assert_eq!(mapping.runtime_session_id, "runtime-1");
    assert_eq!(
        mapping.native_session_path.as_deref().map(PathBuf::from),
        Some(generation_one.clone())
    );
    assert_eq!(
        fixture.files(),
        vec!["gen-1.jsonl".to_owned()],
        "the new, unreferenced file is compensated away"
    );
    assert_eq!(
        fixture
            .database
            .conversation_runtime_session_files(&fixture.conversation_id)
            .unwrap()
            .len(),
        1,
        "no generation may be retired by a rolled-back reset"
    );

    // The old generation still opens, by its own file.
    let mut reopening = ScriptedRuntime::with(vec![Scripted::Created {
        path: generation_one.clone(),
        session_id: "pi-session-7".to_owned(),
        persist: false,
    }]);
    let opened = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut reopening)
        .expect("the previous generation is still openable");
    assert_eq!(opened.0, "runtime-1");
    assert_eq!(opened.1, generation_one);
    assert_eq!(reopening.request_types(), vec!["resume_session"]);
    assert_eq!(
        std::fs::read_to_string(&fixture.file("gen-1.jsonl")).unwrap(),
        "generation one bytes"
    );
}
// ------------------------------------------------ (d) permanent delete ownership --

/// Two generations: permanent delete removes both files and nothing else.
#[test]
fn permanent_delete_removes_every_owned_generation_file_and_nothing_else() {
    let fixture = Fixture::new("purge-two-generations");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let generation_two = fixture.file("gen-2.jsonl");
    let outcome = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: generation_two.clone(),
                session_id: "pi-2".to_owned(),
                persist: true,
            }]),
        )
        .expect("reset");
    std::fs::write(&outcome.session_path, "generation two bytes").unwrap();
    // Files that must survive: another conversation's session, and a file in
    // the directory that no row owns.
    let other_conversation = fixture.second_conversation();
    let other_file = fixture.file("other-conversation.jsonl");
    std::fs::write(&other_file, "other conversation").unwrap();
    fixture.seed_mapping(
        &other_conversation,
        "runtime-other",
        &other_file,
        1,
        RuntimeSessionStatus::Ready,
    );
    let unowned = fixture.file("not-a-session.txt");
    std::fs::write(&unowned, "unowned").unwrap();

    let removed = purge_owned_session_files(
        &fixture.database,
        &fixture.sessions_dir,
        &fixture.conversation_id,
    )
    .expect("the two owned generations are deletable");
    let mut removed = removed
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    removed.sort();
    assert_eq!(removed, vec!["gen-1.jsonl".to_owned(), "gen-2.jsonl".to_owned()]);
    assert!(!generation_one.exists());
    assert!(!outcome.session_path.exists());
    assert!(other_file.exists(), "another conversation's file is never deleted");
    assert!(unowned.exists(), "an unowned file is never deleted");
    assert!(
        fixture.database.get_runtime_session(&other_conversation).unwrap().is_some(),
        "the other conversation's mapping is untouched"
    );
}

/// A recorded path outside the Fox session directory is refused, and because a
/// refusal is all-or-nothing, the valid file of the other generation survives
/// too.
#[test]
fn a_session_file_outside_the_sessions_directory_is_refused() {
    let fixture = Fixture::new("purge-outside");
    let inside = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    // A live row whose stored path is outside the session directory. The Host
    // validates paths when it records them; a seeded row simulates a database
    // that came from somewhere else.
    let outside = fixture.root.join("escaped.jsonl");
    std::fs::write(&outside, "not ours").unwrap();
    fixture.seed_retired_generation(&fixture.conversation_id, 2, "runtime-2", &outside, 2);

    let refusal = purge_owned_session_files(
        &fixture.database,
        &fixture.sessions_dir,
        &fixture.conversation_id,
    )
    .expect_err("a path outside the session directory must be refused");
    assert_eq!(refusal.code, SESSION_FILE_OUTSIDE_DIRECTORY);
    assert!(outside.exists(), "a file outside the directory is never deleted");
    assert!(inside.exists(), "a refusal deletes nothing at all");
}

/// A file that another conversation still references is refused (deleted by
/// nobody), even though this conversation's row names it.
#[test]
fn a_session_file_owned_by_another_conversation_is_refused() {
    let fixture = Fixture::new("purge-foreign-owner");
    let shared = fixture.seed_generation_one("shared.jsonl", "shared bytes");
    let other_conversation = fixture.second_conversation();
    fixture.seed_mapping(
        &other_conversation,
        "runtime-other",
        &shared,
        1,
        RuntimeSessionStatus::Ready,
    );

    let refusal = purge_owned_session_files(
        &fixture.database,
        &fixture.sessions_dir,
        &fixture.conversation_id,
    )
    .expect_err("a file another conversation owns must be refused");
    assert_eq!(refusal.code, SESSION_FILE_OTHER_OWNER);
    assert!(shared.exists(), "the shared file must not be deleted");
    assert!(
        fixture.database.get_runtime_session(&other_conversation).unwrap().is_some(),
        "the other conversation's mapping survives"
    );
}

/// Trash and restore never delete a file: only permanent delete does.
#[test]
fn trash_and_restore_keep_every_generation_file() {
    let fixture = Fixture::new("trash-restore");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let generation_two = fixture.file("gen-2.jsonl");
    let outcome = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: generation_two.clone(),
                session_id: "pi-2".to_owned(),
                persist: true,
            }]),
        )
        .expect("reset");
    std::fs::write(&outcome.session_path, "generation two bytes").unwrap();

    assert!(fixture
        .database
        .trash_conversation(&fixture.conversation_id)
        .unwrap()
        .is_some());
    assert!(generation_one.exists(), "moving to trash deletes no session file");
    assert!(outcome.session_path.exists());
    assert!(fixture
        .database
        .restore_trashed_conversation(&fixture.conversation_id)
        .unwrap()
        .is_some());
    assert!(generation_one.exists(), "restoring deletes no session file");
    assert!(outcome.session_path.exists());
    assert_eq!(
        fixture
            .database
            .conversation_runtime_session_files(&fixture.conversation_id)
            .unwrap()
            .len(),
        2,
        "trash/restore never touches the generation ledger"
    );
    // And permanent delete afterwards still finds both.
    let removed = purge_owned_session_files(
        &fixture.database,
        &fixture.sessions_dir,
        &fixture.conversation_id,
    )
    .expect("permanent delete removes both generations");
    assert_eq!(removed.len(), 2);
    assert!(!generation_one.exists());
    assert!(!outcome.session_path.exists());
}

// ------------------------------------------------------------ pure decision ---

/// Reusing an allocated session is only allowed for the exact live runtime
/// session id: nothing else may stand in for a file Pi never wrote.
#[test]
fn an_allocated_session_is_reused_only_by_its_exact_live_worker() {
    let record = |status: RuntimeSessionStatus| RuntimeSessionRecord {
        runtime_session_id: "runtime-2".into(),
        runtime_version: Some("0.1.0+pi-1.4.0".into()),
        session_path: None,
        status: status.as_str().into(),
        native_session_ref: Some("pi-2".into()),
        native_session_path: Some("C:/fox/sessions/gen-2.jsonl".into()),
        format_version: 2,
        context_generation: 2,
    };
    assert!(allocated_mapping_is_live(
        &record(RuntimeSessionStatus::Allocated),
        Some("runtime-2")
    ));
    assert!(!allocated_mapping_is_live(
        &record(RuntimeSessionStatus::Allocated),
        Some("runtime-1")
    ));
    assert!(!allocated_mapping_is_live(&record(RuntimeSessionStatus::Allocated), None));
    // Only `allocated` may be reused: `ready` reopens the file (proving it),
    // `unavailable` is a recovery problem the user must reset explicitly.
    assert!(!allocated_mapping_is_live(
        &record(RuntimeSessionStatus::Ready),
        Some("runtime-2")
    ));
    assert!(!allocated_mapping_is_live(
        &record(RuntimeSessionStatus::Unavailable),
        Some("runtime-2")
    ));
}
// ------------------------------------------------ recovery via an explicit reset --

/// The main recovery story: a Pi context that cannot be restored (corrupted
/// file, restore failed) is recovered by an explicit reset, and the corrupt
/// file is kept for permanent delete rather than deleted by the reset.
#[test]
fn a_reset_recovers_an_unavailable_session_without_touching_its_file() {
    let fixture = Fixture::new("recover-unavailable");
    let corrupted = fixture.seed_generation_one("gen-1.jsonl", "not a pi session");
    fixture
        .database
        .mark_runtime_session_unavailable(&fixture.conversation_id)
        .unwrap();
    assert_eq!(fixture.mapping().unwrap().status, "unavailable");

    let outcome = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: fixture.file("gen-2.jsonl"),
                session_id: "pi-2".to_owned(),
                persist: false,
            }]),
        )
        .expect("an explicit reset is exactly how an unavailable session is recovered");
    assert_eq!(outcome.context_generation, 2);
    assert_eq!(fixture.mapping().unwrap().status, "allocated");
    assert_eq!(
        std::fs::read_to_string(&corrupted).unwrap(),
        "not a pi session",
        "a reset never rewrites or deletes the old file"
    );
    let retired = fixture
        .database
        .conversation_runtime_session_files(&fixture.conversation_id)
        .unwrap();
    let retired_one = retired.iter().find(|row| !row.current).expect("generation 1 is retired");
    assert_eq!(retired_one.context_generation, 1);
    assert_eq!(retired_one.status, "unavailable");
    assert_eq!(
        retired_one.native_session_path.as_deref().map(PathBuf::from),
        Some(corrupted.clone()),
        "the retired row keeps naming the file it owned"
    );
}

/// If the runtime cannot create the new session, the reset fails before the
/// database is touched: the previous mapping is still the live one.
#[test]
fn a_reset_that_cannot_create_a_session_leaves_the_old_mapping_untouched() {
    let fixture = Fixture::new("create-failure");
    let generation_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");

    let refusal = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Rejected {
                code: "session.native_directory_unusable",
                message: "The Pi session directory cannot be used",
            }]),
        )
        .expect_err("a failed create must refuse the reset");
    assert_eq!(refusal.code, super::session_lifecycle::RESET_FAILED);
    assert!(refusal.message.contains("session directory"), "{refusal:?}");

    let mapping = fixture.mapping().expect("the old mapping survives");
    assert_eq!(mapping.context_generation, 1);
    assert_eq!(mapping.runtime_session_id, "runtime-1");
    assert_eq!(
        mapping.native_session_path.as_deref().map(PathBuf::from),
        Some(generation_one.clone())
    );
    assert_eq!(fixture.files(), vec!["gen-1.jsonl".to_owned()]);
    assert_eq!(
        fixture
            .database
            .conversation_runtime_session_files(&fixture.conversation_id)
            .unwrap()
            .len(),
        1,
        "nothing may be retired when the new session was never created"
    );
}