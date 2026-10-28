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
    allocated_mapping_is_live, purge_owned_session_files, reset_availability, SessionLifecycle,
    SessionOpenError, SessionRuntimeLink, RESET_LEGACY_CONTEXT, RESET_RUN_ACTIVE,
    RESET_UNSUPPORTED_RUNTIME, SESSION_FILE_OTHER_OWNER, SESSION_FILE_OUTSIDE_DIRECTORY,
    SESSION_NOT_PERSISTED, SESSION_UNAVAILABLE,
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

fn recorded_run_session(fixture: &Fixture, run_id: &str) -> Option<String> {
    fixture.database.with_connection(|connection| {
        connection.query_row(
            "SELECT runtime_session_id FROM runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
    }).unwrap()
}

#[test]
fn rewind_refuses_failed_native_recovery_without_erasing_the_failure() {
    let fixture = Fixture::new("rewind-corrupt-session");
    let path = fixture.seed_generation_one("corrupt.jsonl", "invalid native session");
    let started = fixture.database.create_run(&fixture.conversation_id, "retry this prompt", None).unwrap();
    let refusal = fixture.lifecycle().open_for_run(
        &fixture.conversation_id,
        &started.run.id,
        &mut ScriptedRuntime::with(vec![Scripted::Rejected { code: "session.native_corrupt", message: "invalid JSONL" }]),
        None,
    ).unwrap_err();
    fixture.database.mark_run_failed(&started.run.id, "runtime.start_failed", &refusal.user_message()).unwrap();
    let detail_before = serde_json::to_value(fixture.database.load_conversation(&fixture.conversation_id).unwrap()).unwrap();
    let mapping_before = format!("{:?}", fixture.mapping());
    let generations_before = fixture.generations();
    assert!(fixture.database.conversation_requires_native_recovery(&fixture.conversation_id).unwrap());

    for _ in 0..2 {
        let error = fixture.database.rewind_run(&fixture.conversation_id, &started.user_message.id, "retry this prompt", None).unwrap_err();
        assert!(error.contains("session.native_unavailable"), "{error}");
        assert_eq!(serde_json::to_value(fixture.database.load_conversation(&fixture.conversation_id).unwrap()).unwrap(), detail_before);
        assert_eq!(format!("{:?}", fixture.mapping()), mapping_before);
        assert_eq!(fixture.generations(), generations_before);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "invalid native session");
        assert_eq!(fixture.files(), vec!["corrupt.jsonl"]);
    }
}

#[test]
fn rewind_retires_the_persisted_mapping_before_a_new_generation_is_created() {
    let fixture = Fixture::new("rewind-ready-session");
    let old_path = fixture.seed_generation_one("old.jsonl", "persisted generation one");
    let started = fixture.database.create_run(&fixture.conversation_id, "original prompt", None).unwrap();
    fixture.database.mark_run_failed(&started.run.id, "provider.request_failed", "ordinary provider failure").unwrap();
    assert!(!fixture.database.conversation_requires_native_recovery(&fixture.conversation_id).unwrap());
    let replacement = fixture.database.rewind_run(&fixture.conversation_id, &started.user_message.id, "edited prompt", None).unwrap();
    assert!(fixture.mapping().is_none());
    let retired = fixture.generations();
    assert_eq!(retired.len(), 1);
    assert_eq!((retired[0].0, retired[0].1.as_str(), retired[0].3), (1, "runtime-1", false));
    assert_eq!(fixture.database.next_runtime_session_generation(&fixture.conversation_id).unwrap(), 2);
    let new_path = fixture.file("edited.jsonl");
    fixture.lifecycle().open_for_run(
        &fixture.conversation_id,
        &replacement.run.id,
        &mut ScriptedRuntime::with(vec![Scripted::Created { path: new_path, session_id: "pi-edited".into(), persist: true }]),
        None,
    ).unwrap();
    assert_eq!(fixture.mapping().unwrap().context_generation, 2);
    assert_eq!(std::fs::read_to_string(&old_path).unwrap(), "persisted generation one");
    fixture.database.mark_run_failed(&replacement.run.id, "run.test", "end test run").unwrap();
    let reset = fixture.lifecycle().reset(
        &fixture.conversation_id,
        &mut ScriptedRuntime::with(vec![Scripted::Created { path: fixture.file("reset.jsonl"), session_id: "pi-reset".into(), persist: false }]),
    ).unwrap();
    assert_eq!(reset.context_generation, 3);
    assert_eq!(fixture.generations().iter().map(|row| (row.0, row.3)).collect::<Vec<_>>(), vec![(1, false), (2, false), (3, true)]);
}

#[test]
fn rewind_generation_collision_rolls_back_messages_runs_and_mapping() {
    let fixture = Fixture::new("rewind-generation-collision");
    fixture.seed_generation_one("current.jsonl", "current bytes");
    fixture.seed_retired_generation(&fixture.conversation_id, 1, "different-runtime", &fixture.file("retired.jsonl"), 2);
    let started = fixture.database.create_run(&fixture.conversation_id, "keep this prompt", None).unwrap();
    fixture.database.mark_run_failed(&started.run.id, "provider.request_failed", "ordinary failure").unwrap();
    let before = serde_json::to_value(fixture.database.load_conversation(&fixture.conversation_id).unwrap()).unwrap();
    let mapping = format!("{:?}", fixture.mapping());
    let generations = fixture.generations();
    let error = fixture.database.rewind_run(&fixture.conversation_id, &started.user_message.id, "edited prompt", None).unwrap_err();
    assert!(error.contains("rewind.context_generation_collision"), "{error}");
    assert_eq!(serde_json::to_value(fixture.database.load_conversation(&fixture.conversation_id).unwrap()).unwrap(), before);
    assert_eq!(format!("{:?}", fixture.mapping()), mapping);
    assert_eq!(fixture.generations(), generations);
}

#[test]
fn run_session_binding_covers_create_live_adopt_and_resume() {
    for route in ["create", "live", "adopt", "resume"] {
        let fixture = Fixture::new(&format!("run-binding-{route}"));
        let existing_file = fixture.file("existing.jsonl");
        if route != "create" {
            if route == "resume" {
                std::fs::write(&existing_file, "persisted Pi session").unwrap();
            }
            fixture.seed_mapping(
                &fixture.conversation_id,
                "existing-runtime",
                &existing_file,
                2,
                if route == "resume" { RuntimeSessionStatus::Ready } else { RuntimeSessionStatus::Allocated },
            );
        }
        let run = fixture.database.create_run(&fixture.conversation_id, "读取当前资料", None).unwrap().run;
        assert_eq!(recorded_run_session(&fixture, &run.id), None);
        let response_path = if route == "resume" { existing_file } else { fixture.file("created.jsonl") };
        let mut runtime = if route == "live" {
            ScriptedRuntime::default()
        } else {
            ScriptedRuntime::with(vec![Scripted::Created {
                path: response_path,
                session_id: "pi-context".to_owned(),
                persist: route == "resume",
            }])
        };
        let opened = fixture.lifecycle().open_for_run(
            &fixture.conversation_id,
            &run.id,
            &mut runtime,
            (route == "live").then_some("existing-runtime"),
        ).unwrap();
        assert_eq!(recorded_run_session(&fixture, &run.id).as_deref(), Some(opened.0.as_str()), "route {route}");
        assert_eq!(fixture.mapping().unwrap().runtime_session_id, opened.0);
        let expected_requests = match route {
            "live" => vec![],
            "resume" => vec!["resume_session"],
            _ => vec!["create_session"],
        };
        assert_eq!(runtime.request_types(), expected_requests, "route {route}");
    }
}

#[test]
fn run_session_binding_does_not_claim_a_failed_restore() {
    let fixture = Fixture::new("run-binding-corrupt");
    fixture.seed_generation_one("corrupt.jsonl", "corrupt");
    let run = fixture.database.create_run(&fixture.conversation_id, "继续", None).unwrap().run;
    let mut runtime = ScriptedRuntime::with(vec![Scripted::Rejected {
        code: "session.native_corrupt",
        message: "invalid Pi session",
    }]);
    assert!(fixture.lifecycle().open_for_run(&fixture.conversation_id, &run.id, &mut runtime, None).is_err());
    assert_eq!(recorded_run_session(&fixture, &run.id), None);
    assert_eq!(fixture.mapping().unwrap().status, "unavailable");
}

#[test]
fn run_session_binding_refuses_other_conversations_and_existing_different_bindings() {
    let fixture = Fixture::new("run-binding-owner");
    fixture.seed_generation_one("owned.jsonl", "owned session");
    let run = fixture.database.create_run(&fixture.conversation_id, "继续", None).unwrap().run;
    let other_conversation = fixture.second_conversation();
    let other_run = fixture.database.create_run(&other_conversation, "独立对话", None).unwrap().run;
    assert!(fixture.database.associate_run_runtime_session(&other_run.id, &fixture.conversation_id, "runtime-1").is_err());
    assert!(fixture.database.associate_run_runtime_session(&run.id, &other_conversation, "runtime-1").is_err());
    assert_eq!(recorded_run_session(&fixture, &other_run.id), None);
    assert_eq!(recorded_run_session(&fixture, &run.id), None);
    fixture.database.with_connection(|connection| {
        connection.execute("UPDATE runs SET runtime_session_id = 'previous-runtime' WHERE id = ?1", [&run.id])
    }).unwrap();
    assert!(fixture.database.associate_run_runtime_session(&run.id, &fixture.conversation_id, "runtime-1").is_err());
    assert_eq!(recorded_run_session(&fixture, &run.id).as_deref(), Some("previous-runtime"));
}

#[test]
fn run_session_binding_is_idempotent_but_does_not_change_finished_runs() {
    let fixture = Fixture::new("run-binding-finished");
    fixture.seed_generation_one("owned.jsonl", "owned session");
    let run = fixture.database.create_run(&fixture.conversation_id, "继续", None).unwrap().run;
    for _ in 0..2 {
        fixture.database.associate_run_runtime_session(&run.id, &fixture.conversation_id, "runtime-1").unwrap();
    }
    fixture.database.with_connection(|connection| {
        connection.execute("UPDATE runs SET status = 'completed' WHERE id = ?1", [&run.id])
    }).unwrap();
    assert!(fixture.database.associate_run_runtime_session(&run.id, &fixture.conversation_id, "runtime-1").is_err());
    assert_eq!(recorded_run_session(&fixture, &run.id).as_deref(), Some("runtime-1"));
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

    // A stale live id is never reused as if it were the live session. The
    // generation Pi never persisted is continued by asking Pi for a session —
    // never by resuming the missing file — and adoption keeps the generation.
    let adopted_file = fixture.file("gen-2-adopted.jsonl");
    let mut stale = ScriptedRuntime::with(vec![Scripted::Created {
        path: adopted_file.clone(),
        session_id: "pi-2-adopted".to_owned(),
        persist: false,
    }]);
    let adopted = fixture
        .lifecycle()
        .open_with_live_session(&fixture.conversation_id, &mut stale, Some("runtime-1"))
        .expect("a stale live id does not block continuing the new generation");
    assert_eq!(
        adopted.0, outcome.runtime_session_id,
        "adoption keeps the Fox runtime session id of the generation"
    );
    assert_eq!(
        stale.request_types(),
        vec!["create_session"],
        "a stale live id is never treated as the live session"
    );
    let adopted_mapping = fixture.mapping().expect("the mapping survives adoption");
    assert_eq!(adopted_mapping.context_generation, 2, "adoption keeps the generation");
    assert_eq!(adopted_mapping.status, "allocated");
    assert_eq!(adopted_mapping.native_session_ref.as_deref(), Some("pi-2-adopted"));
    assert_eq!(
        adopted_mapping.native_session_path.as_deref().map(PathBuf::from),
        Some(canonical(&fixture.sessions_dir, "gen-2-adopted.jsonl")),
        "the row names the file Pi will really write, so permanent delete can find it"
    );
    let outcome_session_path = adopted.1;

    // Once Pi writes the file, opening it resumes *that* file with the new
    // generation, and still injects no history.
    std::fs::write(&outcome_session_path, "{\"type\":\"session\"}\n").unwrap();
    let mut later_run = ScriptedRuntime::with(vec![Scripted::Created {
        path: outcome_session_path.clone(),
        session_id: "pi-2-adopted".to_owned(),
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
        json!(outcome_session_path.to_string_lossy())
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

// =====================================================================
// Integration unit (single-Pi final): generation allocation, edit/rerun
// ownership, adoption of a never-persisted generation, and the reset
// support boundary.
// =====================================================================

fn recovery_code(error: &SessionOpenError) -> Option<&str> {
    match error {
        SessionOpenError::RecoveryProblem { code, .. } => Some(code.as_str()),
        SessionOpenError::Failed(_) => None,
    }
}

/// Generations installed by a reset are allocated above the live row *and*
/// every retired row.
///
/// Pre-fix: the next generation came from the live row alone, so a ledger that
/// already held 2 and 5 made the next reset reuse 2 and overwrite that retired
/// session's ownership record.
#[test]
fn a_reset_allocates_above_every_retired_generation() {
    let fixture = Fixture::new("alloc-above-ledger");
    let live_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let retired_two = fixture.file("gen-2.jsonl");
    std::fs::write(&retired_two, "retired two").unwrap();
    let retired_five = fixture.file("gen-5.jsonl");
    std::fs::write(&retired_five, "retired five").unwrap();
    fixture.seed_retired_generation(&fixture.conversation_id, 2, "runtime-2", &retired_two, 2);
    fixture.seed_retired_generation(&fixture.conversation_id, 5, "runtime-5", &retired_five, 2);

    let outcome = fixture
        .lifecycle()
        .reset(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: fixture.file("gen-6.jsonl"),
                session_id: "pi-6".to_owned(),
                persist: false,
            }]),
        )
        .expect("the reset succeeds");
    assert_eq!(
        outcome.context_generation, 6,
        "the next free generation is above the ledger, not one above the live row"
    );

    let rows = fixture
        .database
        .conversation_runtime_session_files(&fixture.conversation_id)
        .unwrap();
    let two = rows
        .iter()
        .find(|row| !row.current && row.context_generation == 2)
        .expect("generation 2 stays retired");
    assert_eq!(two.runtime_session_id, "runtime-2");
    assert_eq!(
        two.native_session_path.as_deref().map(PathBuf::from),
        Some(retired_two.clone()),
        "the retired session keeps its own file"
    );
    let five = rows
        .iter()
        .find(|row| !row.current && row.context_generation == 5)
        .expect("generation 5 stays retired");
    assert_eq!(five.runtime_session_id, "runtime-5");
    assert_eq!(
        five.native_session_path.as_deref().map(PathBuf::from),
        Some(retired_five.clone())
    );
    let one = rows
        .iter()
        .find(|row| !row.current && row.context_generation == 1)
        .expect("the live row was retired as generation 1");
    assert_eq!(one.runtime_session_id, "runtime-1");
    // No file lost its owner or its bytes.
    assert!(live_one.exists());
    assert_eq!(std::fs::read_to_string(&retired_two).unwrap(), "retired two");
    assert_eq!(std::fs::read_to_string(&retired_five).unwrap(), "retired five");
}

/// A ledger generation is never overwritten by a different session: the write
/// is refused, nothing changes, and no file loses its owner.
#[test]
fn a_ledger_generation_is_never_overwritten_by_another_session() {
    let fixture = Fixture::new("ledger-collision");
    let live_one = fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");
    let retired_one = fixture.file("retired-gen-1.jsonl");
    std::fs::write(&retired_one, "retired one").unwrap();
    fixture.seed_retired_generation(&fixture.conversation_id, 1, "runtime-retired", &retired_one, 2);
    let new_file = fixture.file("gen-2.jsonl");

    let error = fixture
        .database
        .reset_native_runtime_session(
            &fixture.conversation_id,
            "runtime-next",
            Some("0.1.0+pi-1.4.0"),
            new_file.to_string_lossy().as_ref(),
            Some("pi-next"),
            2,
            RuntimeSessionStatus::Ready,
        )
        .expect_err("a generation the ledger already holds must be refused");
    assert!(
        error.contains("retired session ledger already holds context generation 1"),
        "{error}"
    );

    let rows = fixture
        .database
        .conversation_runtime_session_files(&fixture.conversation_id)
        .unwrap();
    let retired = rows.iter().find(|row| !row.current).expect("the retired row survives");
    assert_eq!(retired.runtime_session_id, "runtime-retired");
    assert_eq!(
        retired.native_session_path.as_deref().map(PathBuf::from),
        Some(retired_one.clone())
    );
    let live = fixture.mapping().expect("the live row survives");
    assert_eq!(live.runtime_session_id, "runtime-1");
    assert_eq!(live.context_generation, 1);
    assert!(live_one.exists());
    assert!(!new_file.exists(), "the refused install created nothing");
}

/// The edit/rerun `format_version 1` fallback replaces the live row by creating
/// a fresh Pi session. The replaced row is retired first, so the snapshot it
/// owned keeps an owner and permanent delete still finds it.
///
/// Pre-fix: the replacement overwrote the row, orphaned the snapshot and
/// installed generation 1 again (a number the reset ledger could already hold).
#[test]
fn an_edit_rerun_replacement_retires_the_row_it_replaces() {
    let fixture = Fixture::new("rerun-ownership");
    let legacy_snapshot = fixture.file("legacy-snapshot.json");
    std::fs::write(&legacy_snapshot, "{\"formatVersion\":1}").unwrap();
    fixture
        .database
        .ensure_runtime_session(
            &fixture.conversation_id,
            "runtime-legacy",
            Some("0.1.0+pi-1.4.0"),
            legacy_snapshot.to_str(),
        )
        .unwrap();
    assert_eq!(fixture.mapping().unwrap().format_version, 1);

    let (runtime_session_id, new_file) = fixture
        .lifecycle()
        .create(
            &fixture.conversation_id,
            &mut ScriptedRuntime::with(vec![Scripted::Created {
                path: fixture.file("gen-2.jsonl"),
                session_id: "pi-2".to_owned(),
                persist: true,
            }]),
        )
        .expect("the replacement creates a Pi session");

    let live = fixture.mapping().expect("a live mapping");
    assert_eq!(live.runtime_session_id, runtime_session_id);
    assert_eq!(live.format_version, 2);
    assert_eq!(
        live.context_generation, 2,
        "a replacement installs a new generation instead of reusing the old number"
    );
    let rows = fixture
        .database
        .conversation_runtime_session_files(&fixture.conversation_id)
        .unwrap();
    assert_eq!(rows.len(), 2, "live row plus the retired snapshot: {rows:?}");
    let retired = rows.iter().find(|row| !row.current).expect("the replaced row is retired");
    assert_eq!(retired.runtime_session_id, "runtime-legacy");
    assert_eq!(retired.format_version, 1);
    assert_eq!(
        retired.session_path.as_deref().map(PathBuf::from),
        Some(legacy_snapshot.clone()),
        "the legacy snapshot keeps its owner"
    );

    let removed = purge_owned_session_files(
        &fixture.database,
        &fixture.sessions_dir,
        &fixture.conversation_id,
    )
    .expect("permanent delete removes both files");
    let mut names = removed
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec!["gen-2.jsonl".to_owned(), "legacy-snapshot.json".to_owned()]);
    assert!(!legacy_snapshot.exists());
    assert!(!new_file.exists());
}

/// A reset, no message, then a worker restart: the never-written generation is
/// continued by adopting the session Pi creates now — same generation, same Fox
/// runtime session id, no history, no second reset.
#[test]
fn a_just_reset_generation_continues_after_a_worker_restart() {
    let fixture = Fixture::new("adopt-after-restart");
    fixture.seed_generation_one("gen-1.jsonl", "generation one bytes");

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
        .expect("reset");
    assert_eq!(outcome.context_generation, 2);
    assert!(!outcome.session_path.exists(), "Pi never wrote the new generation");

    // The app restarts: the live worker (and its runtime session id) is gone.
    let mut restarting = ScriptedRuntime::with(vec![Scripted::Created {
        path: fixture.file("gen-2-restarted.jsonl"),
        session_id: "pi-2-restarted".to_owned(),
        persist: false,
    }]);
    let opened = fixture
        .lifecycle()
        .open_with_live_session(&fixture.conversation_id, &mut restarting, None)
        .expect("the new generation continues without a second reset");
    assert_eq!(
        opened.0, outcome.runtime_session_id,
        "adoption keeps the generation's Fox runtime session id"
    );
    assert_eq!(
        restarting.request_types(),
        vec!["create_session"],
        "the never-written file is not resumed; a new Pi session is adopted"
    );
    let (_, create_payload) = &restarting.requests[0];
    assert_eq!(
        create_payload["contextGeneration"],
        json!(2),
        "the adopted Pi session belongs to the same generation"
    );
    for forbidden in ["messages", "history", "transcript"] {
        assert!(
            create_payload.get(forbidden).is_none(),
            "nothing may be replayed into the adopted session ({forbidden})"
        );
    }

    let mapping = fixture.mapping().expect("a live mapping");
    assert_eq!(mapping.context_generation, 2, "adoption keeps the generation");
    assert_eq!(mapping.runtime_session_id, outcome.runtime_session_id);
    assert_eq!(mapping.status, "allocated");
    assert_eq!(mapping.native_session_ref.as_deref(), Some("pi-2-restarted"));
    assert_eq!(
        mapping.native_session_path.as_deref().map(PathBuf::from),
        Some(canonical(&fixture.sessions_dir, "gen-2-restarted.jsonl")),
        "the row names the file Pi will really write"
    );
    let retired = fixture
        .database
        .conversation_runtime_session_files(&fixture.conversation_id)
        .unwrap();
    assert_eq!(
        retired.iter().filter(|row| !row.current).count(),
        1,
        "adoption retires nothing: generation 1 stays the only retired row"
    );

    // Once Pi writes that file, the next open resumes exactly it.
    std::fs::write(&opened.1, "{\"type\":\"session\"}\n").unwrap();
    let mut later = ScriptedRuntime::with(vec![Scripted::Created {
        path: opened.1.clone(),
        session_id: "pi-2-restarted".to_owned(),
        persist: false,
    }]);
    let resumed = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut later)
        .expect("the adopted file is resumable");
    assert_eq!(resumed.0, outcome.runtime_session_id);
    assert_eq!(later.request_types(), vec!["resume_session"]);
    assert_eq!(fixture.mapping().unwrap().status, "ready");
}

/// Adoption is only for `allocated` rows whose file does not exist. A `ready`
/// row whose file is gone still refuses — twice — and never becomes an empty
/// context.
#[test]
fn a_ready_row_whose_file_is_missing_refuses_and_never_adopts() {
    let fixture = Fixture::new("ready-missing-file");
    let missing = fixture.file("never-written.jsonl");
    fixture.seed_mapping(
        &fixture.conversation_id,
        "runtime-9",
        &missing,
        1,
        RuntimeSessionStatus::Ready,
    );
    assert!(!missing.exists());

    let mut first = ScriptedRuntime::with(vec![Scripted::Rejected {
        code: "session.native_missing",
        message: "The Pi session file does not exist",
    }]);
    let error = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut first)
        .expect_err("a ready row with no file is a recovery problem");
    assert_eq!(recovery_code(&error), Some("session.native_missing"));
    assert_eq!(first.request_types(), vec!["resume_session"], "the missing file is asked for and refused");
    assert_eq!(fixture.mapping().unwrap().status, "unavailable");

    let mut twice = ScriptedRuntime::default();
    let error = fixture
        .lifecycle()
        .open(&fixture.conversation_id, &mut twice)
        .expect_err("the second attempt refuses as well");
    assert_eq!(recovery_code(&error), Some(SESSION_UNAVAILABLE));
    assert!(twice.requests.is_empty(), "no runtime request may stand in for a missing file");
    assert!(!missing.exists(), "nothing may create an empty context");
}

/// An `allocated` row whose recorded file exists (even with zero bytes) is not
/// adopted: a present file is a real state, not a booked path.
#[test]
fn an_empty_allocated_file_is_not_adopted() {
    let fixture = Fixture::new("empty-allocated-file");
    let path = fixture.file("empty.jsonl");
    std::fs::write(&path, "").unwrap();
    fixture.seed_mapping(
        &fixture.conversation_id,
        "runtime-8",
        &path,
        1,
        RuntimeSessionStatus::Allocated,
    );

    let mut runtime = ScriptedRuntime::default();
    let error = fixture
        .lifecycle()
        .open_with_live_session(&fixture.conversation_id, &mut runtime, None)
        .expect_err("an existing file is not a booked path");
    assert_eq!(recovery_code(&error), Some(SESSION_NOT_PERSISTED));
    assert!(runtime.requests.is_empty());
    assert_eq!(std::fs::read(&path).unwrap(), Vec::<u8>::new(), "the Host never touches Pi's file");
}

/// A legacy `format_version 1` conversation has no Pi session to reset: the
/// lifecycle refuses with its own code and changes nothing.
#[test]
fn a_reset_is_refused_for_a_legacy_snapshot() {
    let fixture = Fixture::new("reset-legacy");
    let snapshot = fixture.file("legacy-snapshot.json");
    std::fs::write(&snapshot, "{\"formatVersion\":1}").unwrap();
    fixture
        .database
        .ensure_runtime_session(
            &fixture.conversation_id,
            "runtime-legacy",
            Some("0.1.0+pi-1.4.0"),
            snapshot.to_str(),
        )
        .unwrap();

    let mut runtime = ScriptedRuntime::default();
    let refusal = fixture
        .lifecycle()
        .reset(&fixture.conversation_id, &mut runtime)
        .expect_err("a legacy snapshot is not a Pi session to reset");
    assert_eq!(refusal.code, RESET_LEGACY_CONTEXT);
    assert!(runtime.requests.is_empty(), "a refused reset must not touch the runtime");
    let mapping = fixture.mapping().expect("the mapping survives");
    assert_eq!(mapping.runtime_session_id, "runtime-legacy");
    assert_eq!(mapping.format_version, 1);
    assert_eq!(fixture.files(), vec!["legacy-snapshot.json".to_owned()]);
    assert_eq!(
        fixture
            .database
            .conversation_runtime_session_files(&fixture.conversation_id)
            .unwrap()
            .len(),
        1,
        "nothing may be retired by a refused reset"
    );
}

/// A Yuxi conversation keeps its context in the remote service: the reset is
/// refused and nothing is touched.
#[test]
fn a_reset_is_refused_for_a_yuxi_conversation() {
    let fixture = Fixture::new("reset-yuxi");
    fixture
        .database
        .with_connection(|connection| {
            connection
                .execute(
                    "INSERT INTO runtime_sessions(
                        id, conversation_id, runtime_type, runtime_session_id,
                        runtime_version, session_path, status, created_at, updated_at
                     ) VALUES ('yuxi-session-row', ?1, 'yuxi', 'yuxi-thread-1', NULL, NULL, 'ready', 1, 1)",
                    [fixture.conversation_id.as_str()],
                )
                .map(|_| ())
        })
        .unwrap();
    assert_eq!(
        fixture
            .database
            .conversation_runtime_type(&fixture.conversation_id)
            .unwrap(),
        "yuxi"
    );

    let mut runtime = ScriptedRuntime::default();
    let refusal = fixture
        .lifecycle()
        .reset(&fixture.conversation_id, &mut runtime)
        .expect_err("a Yuxi conversation has no Pi session to reset");
    assert_eq!(refusal.code, RESET_UNSUPPORTED_RUNTIME);
    assert!(runtime.requests.is_empty());
    assert!(fixture.mapping().is_none(), "no Pi mapping may be created");
}

/// The availability rule itself: the same fact the UI mirrors.
#[test]
fn reset_availability_matches_the_host_boundary() {
    let pi_native = RuntimeSessionRecord {
        runtime_session_id: "runtime-2".into(),
        runtime_version: Some("0.1.0+pi-1.4.0".into()),
        session_path: None,
        status: "allocated".into(),
        native_session_ref: Some("pi-2".into()),
        native_session_path: Some("C:/fox/sessions/gen-2.jsonl".into()),
        format_version: 2,
        context_generation: 2,
    };
    let legacy = RuntimeSessionRecord {
        format_version: 1,
        native_session_ref: None,
        native_session_path: None,
        session_path: Some("C:/fox/sessions/legacy.json".into()),
        ..pi_native.clone()
    };
    assert!(reset_availability("pi", None).supported, "a conversation with no mapping can be reset");
    assert!(reset_availability("pi", Some(&pi_native)).supported);
    let legacy_view = reset_availability("pi", Some(&legacy));
    assert!(!legacy_view.supported);
    assert_eq!(legacy_view.code.as_deref(), Some(RESET_LEGACY_CONTEXT));
    assert!(legacy_view.reason.is_some());
    assert_eq!(
        legacy_view.refusal().map(|refusal| refusal.code),
        Some(RESET_LEGACY_CONTEXT.to_owned()),
        "the command's own refusal uses the same code"
    );
    let yuxi_view = reset_availability("yuxi", Some(&pi_native));
    assert!(!yuxi_view.supported, "the runtime wins over the row's format");
    assert_eq!(yuxi_view.code.as_deref(), Some(RESET_UNSUPPORTED_RUNTIME));
}
