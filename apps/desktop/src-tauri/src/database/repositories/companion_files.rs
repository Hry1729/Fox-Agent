//! C05: the contact's files & outputs list.
//!
//! "Provided" rows are the private chat's message-bound attachments; "generated"
//! rows are artifacts of the private chat plus every bound background execution
//! conversation (K2). Sources use K1 stable identities: a user bubble's id is
//! its message id, a chat artifact points at the run's last delivered companion
//! bubble, and a task artifact points at the report bubble recorded in
//! `companion_task_reports`. Files dropped into the workspace by hand have no
//! message record and stay behind the working-directory entry — no source is
//! invented for them.
//!
//! Opening, exporting and source jumps never trust a frontend-supplied contact
//! id or path: `artifact_scope_on` re-derives the ownership chain from the
//! persisted rows — execution conversation, contact, report chat and the Run
//! that produced the file — and a foreign or forged combination is refused.
//! The list runs the same check, so a broken chain never keeps showing a file
//! or its old report jump that open / save-as would refuse.

use super::companion_ownership::{
    resolve_conversation_on, resolve_run_on, CompanionScope, OWNERSHIP_MISSING,
};
use super::{ArtifactRecord, AttachmentRecord, Database};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

pub(crate) const FILE_NOT_FOUND: &str = "companion.file_not_found";

fn not_found() -> String {
    format!("{FILE_NOT_FOUND}：找不到属于该联系人的文件")
}

/// The stable bubble a file came from, plus its source message and time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionFileSource {
    pub bubble_id: String,
    pub message_id: String,
    pub at: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CompanionFileEntry {
    /// `attachment` (provided) or `artifact` (generated).
    pub kind: String,
    pub id: String,
    pub name: String,
    pub updated_at: i64,
    pub byte_size: i64,
    pub media_type: Option<String>,
    /// `chat` for the private chat, `task` for a scheduled background run.
    pub origin: String,
    pub task_name: Option<String>,
    pub source: Option<CompanionFileSource>,
    pub missing: bool,
    #[serde(skip)]
    pub storage_path: String,
}

/// A single file whose ownership chain has already been verified.
#[derive(Debug)]
pub(crate) enum CompanionFile {
    Attachment(AttachmentRecord),
    Artifact(ArtifactRecord),
}

impl CompanionFile {
    pub(crate) fn name(&self) -> &str {
        match self {
            Self::Attachment(record) => &record.display_name,
            Self::Artifact(record) => &record.display_name,
        }
    }
}

fn private_chat_on(connection: &Connection, companion_id: &str) -> Result<String, String> {
    connection
        .query_row(
            "SELECT id FROM conversations WHERE companion_id = ?1",
            [companion_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("{FILE_NOT_FOUND}：联系人没有固定私聊"))
}

/// The run's last delivered companion bubble; the artifact cards render under
/// that reply. Runs without any bubble (e.g. an empty final answer) fall back
/// to the assistant turn anchor, which is the message id.
fn chat_artifact_source(
    connection: &Connection,
    chat: &str,
    run_id: Option<&str>,
) -> Result<Option<CompanionFileSource>, String> {
    let Some(run_id) = run_id else { return Ok(None) };
    let bubble: Option<(String, String, i64)> = connection
        .query_row(
            "SELECT bubble_id, source_message_id, delivered_at FROM contact_bubbles
             WHERE conversation_id = ?1 AND run_id = ?2 AND author = 'companion'
             ORDER BY delivery_order DESC LIMIT 1",
            params![chat, run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if let Some((bubble_id, message_id, at)) = bubble {
        return Ok(Some(CompanionFileSource { bubble_id, message_id, at }));
    }
    let message: Option<(String, i64)> = connection
        .query_row(
            "SELECT id, updated_at FROM messages
             WHERE run_id = ?1 AND role = 'assistant' AND status NOT IN ('streaming', 'superseded')
             ORDER BY ordinal DESC LIMIT 1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(message.map(|(id, at)| CompanionFileSource { bubble_id: id.clone(), message_id: id, at }))
}

/// A task artifact's source is the report bubble in the fixed private chat.
/// The ledger is keyed by the source Run and verified against both the contact
/// and the report chat, so a forged run/artifact pairing cannot borrow a
/// source. The report is projected in `message` mode, so bubble id == message id.
fn task_artifact_source(
    connection: &Connection,
    chat: &str,
    companion_id: &str,
    run_id: Option<&str>,
) -> Result<Option<CompanionFileSource>, String> {
    let Some(run_id) = run_id else { return Ok(None) };
    let report: Option<(String, i64)> = connection
        .query_row(
            "SELECT message_id, created_at FROM companion_task_reports
             WHERE run_id = ?1 AND companion_id = ?2 AND report_conversation_id = ?3",
            params![run_id, companion_id, chat],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(report.map(|(message_id, at)| CompanionFileSource {
        bubble_id: message_id.clone(),
        message_id,
        at,
    }))
}

/// K2: the ownership check shared by the list and by open / save-as. The
/// artifact's own row names its execution conversation, and a background
/// artifact must also name the Run that executed there. Comparing the Run's
/// resolved scope with the conversation's covers Run existence, the Run's
/// execution conversation and its contact / report chat in one step, so a
/// forged run/artifact pairing cannot borrow another task's report bubble.
fn artifact_scope_on(
    connection: &Connection,
    companion_id: &str,
    chat: &str,
    conversation_id: &str,
    run_id: Option<&str>,
) -> Result<CompanionScope, String> {
    let scope = resolve_conversation_on(connection, conversation_id)?.ok_or_else(not_found)?;
    if scope.companion_id != companion_id {
        return Err(not_found());
    }
    // The report jump target must be this contact's own fixed private chat.
    if scope.background && scope.report_conversation_id.as_deref() != Some(chat) {
        return Err(not_found());
    }
    let run_id = match run_id {
        Some(run_id) => run_id,
        // A task deliverable always comes from its Run. The private chat also
        // keeps host-written files that never recorded one.
        None if scope.background => return Err(not_found()),
        None => return Ok(scope),
    };
    let run_scope = resolve_run_on(connection, run_id)?.ok_or_else(not_found)?;
    if run_scope != scope {
        return Err(not_found());
    }
    Ok(scope)
}

fn list_on(connection: &Connection, companion_id: &str) -> Result<Vec<CompanionFileEntry>, String> {
    let chat = private_chat_on(connection, companion_id)?;
    let mut entries = Vec::new();

    let mut statement = connection
        .prepare(
            "SELECT id, display_name, media_type, byte_size, storage_path, created_at, message_id
             FROM attachments
             WHERE conversation_id = ?1 AND status = 'ready' AND message_id IS NOT NULL
             ORDER BY created_at DESC, id DESC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([&chat], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (id, name, media_type, byte_size, storage_path, created_at, message_id) =
            row.map_err(|error| error.to_string())?;
        entries.push(CompanionFileEntry {
            kind: "attachment".into(),
            id,
            name,
            updated_at: created_at,
            byte_size,
            media_type,
            origin: "chat".into(),
            task_name: None,
            // K1: a user bubble's identity is its message id; the turn anchor
            // carries the same id even for attachment-only messages.
            source: Some(CompanionFileSource {
                bubble_id: message_id.clone(),
                message_id,
                at: created_at,
            }),
            missing: !std::fs::metadata(&storage_path).map(|m| m.is_file()).unwrap_or(false),
            storage_path,
        });
    }

    // Host-private rendered previews keep their own preview entry and are not
    // listed as outputs; deliverables and process files are. The SQL only
    // collects candidates: each row is verified by `artifact_scope_on` below.
    let mut statement = connection
        .prepare(
            "SELECT id, conversation_id, run_id, display_name, media_type, byte_size, storage_path, updated_at
             FROM artifacts
             WHERE status IN ('ready', 'completed') AND artifact_class IN ('deliverable', 'process')
               AND (conversation_id = ?1 OR conversation_id IN (
                    SELECT conversation_id FROM digital_colleagues
                    WHERE companion_id = ?2 AND companion_bound = 1))
             ORDER BY updated_at DESC, id DESC",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![chat, companion_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    for row in rows {
        let (id, conversation_id, run_id, name, media_type, byte_size, storage_path, updated_at) =
            row.map_err(|error| error.to_string())?;
        // A chain that no longer resolves hides the file and its old report
        // jump; the rest of the list stays usable. Database failures are not
        // ownership verdicts and still surface to the caller.
        let scope = match artifact_scope_on(
            connection,
            companion_id,
            &chat,
            &conversation_id,
            run_id.as_deref(),
        ) {
            Ok(scope) => scope,
            Err(error)
                if error.starts_with(FILE_NOT_FOUND) || error.starts_with(OWNERSHIP_MISSING) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let (origin, task_name, source) = if scope.background {
            let task_name: Option<String> = connection
                .query_row(
                    "SELECT name FROM digital_colleagues WHERE conversation_id = ?1",
                    [&conversation_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?;
            (
                "task".to_owned(),
                task_name,
                task_artifact_source(connection, &chat, companion_id, run_id.as_deref())?,
            )
        } else {
            (
                "chat".to_owned(),
                None,
                chat_artifact_source(connection, &chat, run_id.as_deref())?,
            )
        };
        entries.push(CompanionFileEntry {
            kind: "artifact".into(),
            id,
            name,
            updated_at,
            byte_size,
            media_type,
            origin,
            task_name,
            source,
            missing: !std::fs::metadata(&storage_path).map(|m| m.is_file()).unwrap_or(false),
            storage_path,
        });
    }
    Ok(entries)
}

fn artifact_by_id_on(connection: &Connection, artifact_id: &str) -> Result<Option<ArtifactRecord>, String> {
    connection
        .query_row(
            "SELECT id, conversation_id, run_id, display_name, artifact_type,
                    artifact_class, artifact_origin, storage_path,
                    media_type, byte_size, sha256, status, created_at, updated_at,
                    turn_call_id, declared_at, declared_description
             FROM artifacts WHERE id = ?1",
            [artifact_id],
            |row| {
                Ok(ArtifactRecord {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    run_id: row.get(2)?,
                    display_name: row.get(3)?,
                    artifact_type: row.get(4)?,
                    artifact_class: row.get(5)?,
                    artifact_origin: row.get(6)?,
                    storage_path: row.get(7)?,
                    media_type: row.get(8)?,
                    byte_size: row.get(9)?,
                    sha256: row.get(10)?,
                    status: row.get(11)?,
                    created_at: row.get(12)?,
                    updated_at: row.get(13)?,
                    turn_call_id: row.get(14)?,
                    declared_at: row.get(15)?,
                    declared_description: row.get(16)?,
                    delivery: None,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

/// K2 verification for open / save-as / source jumps. The artifact's own row
/// determines its execution conversation and its Run; the persisted chain must
/// resolve to exactly the requesting contact, and the Run must have executed in
/// that same conversation. A frontend-supplied contact id, run id or path never
/// substitutes for this check.
fn resolve_on(
    connection: &Connection,
    companion_id: &str,
    kind: &str,
    file_id: &str,
) -> Result<CompanionFile, String> {
    match kind {
        "attachment" => {
            let chat = private_chat_on(connection, companion_id)?;
            let record = connection
                .query_row(
                    "SELECT id, conversation_id, message_id, display_name, storage_path, media_type,
                            byte_size, sha256, status, created_at
                     FROM attachments
                     WHERE id = ?1 AND conversation_id = ?2 AND status = 'ready'",
                    params![file_id, chat],
                    |row| {
                        Ok(AttachmentRecord {
                            id: row.get(0)?,
                            conversation_id: row.get(1)?,
                            message_id: row.get(2)?,
                            display_name: row.get(3)?,
                            storage_path: row.get(4)?,
                            media_type: row.get(5)?,
                            byte_size: row.get(6)?,
                            sha256: row.get(7)?,
                            status: row.get(8)?,
                            created_at: row.get(9)?,
                        })
                    },
                )
                .optional()
                .map_err(|error| error.to_string())?
                .ok_or_else(not_found)?;
            Ok(CompanionFile::Attachment(record))
        }
        "artifact" => {
            let record = artifact_by_id_on(connection, file_id)?.ok_or_else(not_found)?;
            let chat = private_chat_on(connection, companion_id)?;
            artifact_scope_on(
                connection,
                companion_id,
                &chat,
                &record.conversation_id,
                record.run_id.as_deref(),
            )?;
            Ok(CompanionFile::Artifact(record))
        }
        _ => Err(format!("{FILE_NOT_FOUND}：不支持的文件类型")),
    }
}

impl Database {
    pub(crate) fn companion_files_list(
        &self,
        companion_id: &str,
    ) -> Result<Vec<CompanionFileEntry>, String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        list_on(&connection, companion_id)
    }

    pub(crate) fn companion_file_resolve(
        &self,
        companion_id: &str,
        kind: &str,
        file_id: &str,
    ) -> Result<CompanionFile, String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        resolve_on(&connection, companion_id, kind, file_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{CreateDigitalColleagueInput, KnowledgeReference};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        db: Database,
        a: String,
        a_chat: String,
        b: String,
        b_chat: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fox-companion-files-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            let db = Database::open(root.join("fox.db")).unwrap();
            let a = db.add_companion(&root, "fox-debugger").unwrap();
            let b = db.add_companion(&root, "fox-reviewer").unwrap();
            Self {
                root,
                db,
                a: a.profile.id,
                a_chat: a.conversation_id,
                b: b.profile.id,
                b_chat: b.conversation_id,
            }
        }

        fn message(&self, chat: &str, id: &str, role: &str, run_id: Option<&str>, ordinal: i64) {
            self.db.with_connection(|connection| {
                connection.execute(
                    "INSERT INTO messages(id, conversation_id, run_id, role, kind, content, status, ordinal, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'text', '内容', 'completed', ?5, 100, 100)",
                    params![id, chat, run_id, role, ordinal],
                )?;
                Ok(())
            }).unwrap();
        }

        fn attachment(&self, chat: &str, id: &str, message_id: &str, name: &str, bytes: &[u8]) -> PathBuf {
            let dir = self.root.join("attachments").join(chat);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{id}.txt"));
            fs::write(&path, bytes).unwrap();
            self.db.add_attachments(&[AttachmentRecord {
                id: id.into(),
                conversation_id: chat.into(),
                message_id: Some(message_id.into()),
                display_name: name.into(),
                storage_path: path.to_string_lossy().into_owned(),
                media_type: Some("text/plain".into()),
                byte_size: bytes.len() as i64,
                sha256: None,
                status: "ready".into(),
                created_at: 100,
            }]).unwrap();
            path
        }

        fn artifact(&self, chat: &str, id: &str, run_id: Option<&str>, name: &str, bytes: &[u8]) -> PathBuf {
            let dir = self.root.join("out").join(chat);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(name);
            fs::write(&path, bytes).unwrap();
            self.db.with_connection(|connection| {
                connection.execute(
                    "INSERT INTO artifacts(id, conversation_id, run_id, display_name, artifact_type,
                        artifact_class, artifact_origin, storage_path, byte_size, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'created_file', 'deliverable', 'project', ?5, ?6, 'ready', 100, 200)",
                    params![id, chat, run_id, name, path.to_string_lossy().as_ref(), bytes.len() as i64],
                )?;
                Ok(())
            }).unwrap();
            path
        }

        fn project_bubbles(&self, run_id: &str) {
            self.db.with_connection(|connection| {
                super::super::contact_bubbles::project_run_in_transaction(connection, run_id)
            }).unwrap();
        }

        /// A bound contact task with its accepted trigger Run. The report is
        /// delivered separately so a test can break the chain after delivery.
        fn task(&self, companion_id: &str, key: &str) -> BackgroundTask {
            let colleague = self.db.create_digital_colleague(&CreateDigitalColleagueInput {
                name: "每日整理".into(),
                expert_id: "fox-debugger".into(),
                objective: "整理目录并回报".into(),
                project_id: None,
                project_root: None,
                knowledge_references: Vec::<KnowledgeReference>::new(),
                max_runs_per_day: 8,
                max_tokens_per_day: 10_000,
                max_duration_ms: 60_000,
                max_output_tokens: 2_000,
                max_tool_calls: 20,
                companion_id: Some(companion_id.to_owned()),
            }).unwrap();
            let accepted = self.db.accept_digital_colleague_trigger(
                &colleague.id, "manual", None, key, &json!({}), None, "user",
            ).unwrap();
            BackgroundTask {
                colleague_id: colleague.id,
                conversation_id: colleague.conversation_id,
                run_id: accepted.trigger.run_id.clone().unwrap(),
            }
        }

        fn deliver_report(&self, run_id: &str) {
            self.db.apply_runtime_event(run_id, 1, &json!({"type": "run.started"})).unwrap();
            self.db.apply_runtime_event(run_id, 2, &json!({"type": "run.completed"})).unwrap();
        }

        fn set_report_chat(&self, colleague_id: &str, chat: Option<&str>) {
            self.db.with_connection(|connection| {
                connection.execute(
                    "UPDATE digital_colleagues SET report_conversation_id = ?1 WHERE id = ?2",
                    params![chat, colleague_id],
                )?;
                Ok(())
            }).unwrap();
        }

        /// An artifact whose Run row is gone. Reachable only outside Fox (the
        /// sqlite3 shell keeps foreign keys off), so the fixture forges it the
        /// same way instead of relying on ON DELETE SET NULL.
        fn orphan_artifact(&self, chat: &str, id: &str, run_id: &str) {
            let dir = self.root.join("out").join(chat);
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join(format!("{id}.md"));
            fs::write(&path, b"orphan").unwrap();
            self.db.with_connection(|connection| {
                connection.pragma_update(None, "foreign_keys", "OFF")?;
                let inserted = connection.execute(
                    "INSERT INTO artifacts(id, conversation_id, run_id, display_name, artifact_type,
                        artifact_class, artifact_origin, storage_path, byte_size, status, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'created_file', 'deliverable', 'project', ?5, 6, 'ready', 100, 200)",
                    params![id, chat, run_id, format!("{id}.md"), path.to_string_lossy().as_ref()],
                );
                connection.pragma_update(None, "foreign_keys", "ON")?;
                inserted?;
                Ok(())
            }).unwrap();
        }
    }

    struct BackgroundTask {
        colleague_id: String,
        conversation_id: String,
        run_id: String,
    }

    /// 验收：同一文件交给两只联系人后仍能区分副本。 Each contact keeps its own
    /// attachment row and host copy; one contact's list never shows the other's.
    #[test]
    fn the_same_file_given_to_two_contacts_stays_two_copies() {
        let f = Fixture::new();
        f.message(&f.a_chat, "msg-a", "user", None, 1);
        f.message(&f.b_chat, "msg-b", "user", None, 1);
        let a_path = f.attachment(&f.a_chat, "att-a", "msg-a", "报价.txt", b"contact A");
        let b_path = f.attachment(&f.b_chat, "att-b", "msg-b", "报价.txt", b"contact B");
        assert_ne!(a_path, b_path);

        let a_files = f.db.companion_files_list(&f.a).unwrap();
        let b_files = f.db.companion_files_list(&f.b).unwrap();
        assert_eq!(a_files.len(), 1);
        assert_eq!(b_files.len(), 1);
        assert_eq!(a_files[0].id, "att-a");
        assert_eq!(b_files[0].id, "att-b");
        assert_eq!(a_files[0].name, "报价.txt");
        assert_eq!(a_files[0].source.as_ref().unwrap().message_id, "msg-a");
        assert_eq!(b_files[0].source.as_ref().unwrap().message_id, "msg-b");
        assert!(!a_files[0].missing);
        assert_eq!(fs::read(&a_path).unwrap(), b"contact A");
        assert_eq!(fs::read(&b_path).unwrap(), b"contact B");

        // A second copy for the same contact is a distinct row, not a merge.
        f.message(&f.a_chat, "msg-a2", "user", None, 2);
        f.attachment(&f.a_chat, "att-a2", "msg-a2", "报价.txt", b"contact A again");
        let a_files = f.db.companion_files_list(&f.a).unwrap();
        assert_eq!(a_files.len(), 2);
        assert_ne!(a_files[0].id, a_files[1].id);
        drop(f.db);
        fs::remove_dir_all(f.root).unwrap();
    }

    /// 验收：后台产物重启后可找到并跳回正确来源。 The report bubble is the
    /// source, and it survives closing and reopening the database.
    #[test]
    fn a_background_artifact_survives_restart_and_points_at_its_report() {
        let f = Fixture::new();
        let colleague = f.db.create_digital_colleague(&CreateDigitalColleagueInput {
            name: "每日整理".into(),
            expert_id: "fox-debugger".into(),
            objective: "整理目录并回报".into(),
            project_id: None,
            project_root: None,
            knowledge_references: Vec::<KnowledgeReference>::new(),
            max_runs_per_day: 8,
            max_tokens_per_day: 10_000,
            max_duration_ms: 60_000,
            max_output_tokens: 2_000,
            max_tool_calls: 20,
            companion_id: Some(f.a.clone()),
        }).unwrap();
        let accepted = f.db.accept_digital_colleague_trigger(
            &colleague.id, "manual", None, "manual:c05", &json!({}), None, "user",
        ).unwrap();
        let run_id = accepted.trigger.run_id.clone().unwrap();
        f.artifact(&colleague.conversation_id, "art-task", Some(&run_id), "汇总.md", "# 汇总".as_bytes());
        f.db.apply_runtime_event(&run_id, 1, &json!({"type": "run.started"})).unwrap();
        f.db.apply_runtime_event(&run_id, 2, &json!({"type": "run.completed"})).unwrap();

        // Close the connection, then reopen the same file to simulate a restart.
        let Fixture { root, db, a, a_chat, .. } = f;
        let db_path = root.join("fox.db");
        drop(db);
        let reopened = Database::open(db_path).unwrap();
        let files = reopened.companion_files_list(&a).unwrap();
        let task = files.iter().find(|entry| entry.id == "art-task").expect("task artifact listed");
        assert_eq!(task.kind, "artifact");
        assert_eq!(task.origin, "task");
        assert_eq!(task.task_name.as_deref(), Some("每日整理"));
        let source = task.source.as_ref().expect("report source");
        assert_eq!(source.bubble_id, format!("dc-report-{run_id}"));
        // The source is a real K1 bubble in the fixed private chat.
        reopened.with_connection(|connection| {
            let (bubble_id, conversation_id): (String, String) = connection.query_row(
                "SELECT bubble_id, conversation_id FROM contact_bubbles WHERE bubble_id = ?1",
                [&source.bubble_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!(bubble_id, source.message_id);
            assert_eq!(conversation_id, a_chat);
            Ok(())
        }).unwrap();
        // The verified chain resolves for the owner after the restart too.
        assert!(matches!(
            reopened.companion_file_resolve(&a, "artifact", "art-task").unwrap(),
            CompanionFile::Artifact(_)
        ));
        drop(reopened);
        let _ = fs::remove_dir_all(&root);
    }

    /// 验收：另一联系人产物 ID / 伪造归属 → 拒绝。 Neither another contact's
    /// artifact nor an ordinary conversation's artifact resolves, and a broken
    /// background chain is an error rather than a fallback.
    #[test]
    fn foreign_artifact_ids_and_forged_ownership_are_refused() {
        let f = Fixture::new();
        f.message(&f.a_chat, "msg-a", "user", None, 1);
        f.attachment(&f.a_chat, "att-a", "msg-a", "a.txt", b"a");
        f.message(&f.b_chat, "msg-b", "user", None, 1);
        f.attachment(&f.b_chat, "att-b", "msg-b", "b.txt", b"b");
        let run = f.db.create_run(&f.a_chat, "生成", None).unwrap().run.id;
        f.artifact(&f.a_chat, "art-a", Some(&run), "a.md", b"a");

        let colleague = f.db.create_digital_colleague(&CreateDigitalColleagueInput {
            name: "每日整理".into(),
            expert_id: "fox-debugger".into(),
            objective: "整理目录并回报".into(),
            project_id: None,
            project_root: None,
            knowledge_references: Vec::<KnowledgeReference>::new(),
            max_runs_per_day: 8,
            max_tokens_per_day: 10_000,
            max_duration_ms: 60_000,
            max_output_tokens: 2_000,
            max_tool_calls: 20,
            companion_id: Some(f.a.clone()),
        }).unwrap();
        let accepted = f.db.accept_digital_colleague_trigger(
            &colleague.id, "manual", None, "manual:c05-refuse", &json!({}), None, "user",
        ).unwrap();
        let task_run = accepted.trigger.run_id.clone().unwrap();
        f.artifact(&colleague.conversation_id, "art-task", Some(&task_run), "task.md", b"t");

        // Contact B cannot open A's chat artifact, A's attachment, or A's task
        // artifact — even though it knows the exact ids.
        for (kind, id) in [("artifact", "art-a"), ("attachment", "att-a"), ("artifact", "art-task")] {
            let error = f.db.companion_file_resolve(&f.b, kind, id).unwrap_err();
            assert!(error.contains(FILE_NOT_FOUND), "{kind} {id}: {error}");
        }
        // A nonexistent contact id resolves nothing either.
        assert!(f.db.companion_file_resolve("cmp_forged", "artifact", "art-a").is_err());
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-missing").is_err());
        assert!(f.db.companion_file_resolve(&f.a, "unknown", "art-a").is_err());
        // The owner resolves its own files.
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-a").is_ok());
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-task").is_ok());
        assert!(f.db.companion_file_resolve(&f.a, "attachment", "att-a").is_ok());

        // An ordinary conversation's artifact never belongs to a contact.
        let ordinary = f.db.create_conversation("fox-general", Some("普通对话"), None, None).unwrap();
        f.artifact(&ordinary.id, "art-ordinary", None, "o.md", b"o");
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-ordinary").is_err());

        // A broken background chain refuses instead of falling back.
        f.db.with_connection(|connection| {
            connection.execute(
                "UPDATE digital_colleagues SET report_conversation_id = NULL WHERE id = ?1",
                [&colleague.id],
            )?;
            Ok(())
        }).unwrap();
        let error = f.db.companion_file_resolve(&f.a, "artifact", "art-task").unwrap_err();
        assert!(error.contains(crate::database::OWNERSHIP_MISSING), "{error}");
        drop(f.db);
        fs::remove_dir_all(f.root).unwrap();
    }

    /// 验收：缺失文件给出明确状态。 The row stays listed with `missing`, and a
    /// chat artifact's source is the run's last delivered bubble.
    #[test]
    fn missing_files_are_flagged_and_chat_artifacts_point_at_the_last_bubble() {
        let f = Fixture::new();
        let run = f.db.create_run(&f.a_chat, "生成两个文件", None).unwrap().run.id;
        f.message(&f.a_chat, "assistant-1", "assistant", Some(&run), 2);
        f.project_bubbles(&run);
        let good = f.artifact(&f.a_chat, "art-good", Some(&run), "好的.md", b"ok");
        f.artifact(&f.a_chat, "art-gone", Some(&run), "丢失.md", b"gone");
        fs::remove_file(f.root.join("out").join(&f.a_chat).join("丢失.md")).unwrap();

        let files = f.db.companion_files_list(&f.a).unwrap();
        let good_entry = files.iter().find(|entry| entry.id == "art-good").unwrap();
        let gone_entry = files.iter().find(|entry| entry.id == "art-gone").unwrap();
        assert!(!good_entry.missing);
        assert!(gone_entry.missing);
        assert!(good.exists());
        let source = good_entry.source.as_ref().expect("bubble source");
        assert_eq!(source.message_id, "assistant-1");
        assert_eq!(source.bubble_id, "assistant-1", "message-mode projection keeps the message id");
        assert_eq!(good_entry.origin, "chat");
        assert!(good_entry.task_name.is_none());
        drop(f.db);
        fs::remove_dir_all(f.root).unwrap();
    }

    /// 验收：损坏归属链的列表不再返回后台产物或旧回报跳转。 Once the report chat
    /// is cleared or repointed at another contact's private chat, the list agrees
    /// with open / save-as: the task artifact and its stale report bubble are
    /// gone, while the private chat's own files stay listed.
    #[test]
    fn a_broken_background_chain_drops_the_artifact_and_its_report_jump() {
        let f = Fixture::new();
        f.message(&f.a_chat, "msg-a", "user", None, 1);
        f.attachment(&f.a_chat, "att-a", "msg-a", "a.txt", b"a");
        let chat_run = f.db.create_run(&f.a_chat, "生成", None).unwrap().run.id;
        f.artifact(&f.a_chat, "art-chat", Some(&chat_run), "chat.md", b"c");

        let task = f.task(&f.a, "manual:c05-chain");
        f.artifact(&task.conversation_id, "art-task", Some(&task.run_id), "task.md", b"t");
        f.deliver_report(&task.run_id);
        let report_bubble = format!("dc-report-{}", task.run_id);

        // Intact chain: the artifact is listed with its own report bubble.
        let files = f.db.companion_files_list(&f.a).unwrap();
        let listed = files.iter().find(|entry| entry.id == "art-task").expect("task artifact listed");
        assert_eq!(listed.origin, "task");
        assert_eq!(listed.source.as_ref().unwrap().bubble_id, report_bubble);

        for broken in [None, Some(f.b_chat.as_str())] {
            f.set_report_chat(&task.colleague_id, broken);
            let files = f.db.companion_files_list(&f.a).unwrap();
            assert!(
                files.iter().all(|entry| entry.id != "art-task"),
                "report chat {broken:?}: a broken chain must not keep listing the artifact"
            );
            // No surviving entry may carry the old report jump either.
            assert!(files.iter().all(|entry| {
                entry.source.as_ref().map(|source| source.bubble_id.as_str())
                    != Some(report_bubble.as_str())
            }));
            // The private chat's own files are unaffected by the broken task.
            assert!(files.iter().any(|entry| entry.id == "att-a"));
            assert!(files.iter().any(|entry| entry.id == "art-chat"));
            // List and open / save-as now refuse the same file.
            assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-task").is_err());
            // Repointing the report chat at B never hands B the artifact.
            assert!(f.db.companion_files_list(&f.b).unwrap().iter().all(|entry| entry.id != "art-task"));
            assert!(f.db.companion_file_resolve(&f.b, "artifact", "art-task").is_err());
        }
        drop(f.db);
        fs::remove_dir_all(f.root).unwrap();
    }

    /// 验收：伪造 Run/产物配对 → 拒绝打开或导出。 A background artifact must name
    /// the Run that executed in its own conversation: no Run, a Run from the
    /// private chat, from another task, from another contact, or a Run row that
    /// no longer exists are all refused, and the list hides them too.
    #[test]
    fn forged_run_and_artifact_pairings_are_refused() {
        let f = Fixture::new();
        let task = f.task(&f.a, "manual:c05-run");
        let other_task = f.task(&f.a, "manual:c05-run-other");
        let b_task = f.task(&f.b, "manual:c05-run-b");
        let chat_run = f.db.create_run(&f.a_chat, "生成", None).unwrap().run.id;
        let b_chat_run = f.db.create_run(&f.b_chat, "生成", None).unwrap().run.id;

        // The honest pairing resolves and keeps its own report jump.
        f.artifact(&task.conversation_id, "art-own", Some(&task.run_id), "own.md", b"o");
        f.deliver_report(&task.run_id);
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-own").is_ok());

        let forged: [(&str, Option<&str>); 5] = [
            ("art-no-run", None),
            ("art-chat-run", Some(chat_run.as_str())),
            ("art-other-task-run", Some(other_task.run_id.as_str())),
            ("art-b-task-run", Some(b_task.run_id.as_str())),
            ("art-b-chat-run", Some(b_chat_run.as_str())),
        ];
        for (id, run_id) in forged {
            f.artifact(&task.conversation_id, id, run_id, &format!("{id}.md"), b"x");
            let error = f.db.companion_file_resolve(&f.a, "artifact", id).unwrap_err();
            assert!(error.contains(FILE_NOT_FOUND), "{id}: {error}");
            // Save-as shares this resolve, and the list must not keep offering a
            // file that open / save-as refuse.
            assert!(
                f.db.companion_files_list(&f.a).unwrap().iter().all(|entry| entry.id != id),
                "{id} is still listed"
            );
            assert!(f.db.companion_file_resolve(&f.b, "artifact", id).is_err());
        }

        // A Run row that no longer exists is refused as well.
        f.orphan_artifact(&task.conversation_id, "art-orphan-run", "run_forged");
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-orphan-run").is_err());
        assert!(f.db.companion_files_list(&f.a).unwrap().iter().all(|entry| entry.id != "art-orphan-run"));

        // A chat artifact whose Run executed in a task conversation is refused
        // too: the Run must belong to the artifact's own conversation.
        f.artifact(&f.a_chat, "art-chat-forged", Some(&task.run_id), "chat-forged.md", b"c");
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-chat-forged").is_err());
        assert!(f.db.companion_files_list(&f.a).unwrap().iter().all(|entry| entry.id != "art-chat-forged"));

        // None of the forgeries disturbed the honest artifact or its source.
        let files = f.db.companion_files_list(&f.a).unwrap();
        let own = files.iter().find(|entry| entry.id == "art-own").expect("honest artifact listed");
        assert_eq!(own.origin, "task");
        assert_eq!(own.source.as_ref().unwrap().bubble_id, format!("dc-report-{}", task.run_id));
        assert!(f.db.companion_file_resolve(&f.a, "artifact", "art-own").is_ok());
        drop(f.db);
        fs::remove_dir_all(f.root).unwrap();
    }
}