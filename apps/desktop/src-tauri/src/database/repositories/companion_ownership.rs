//! C03b/K2: the persistent background ownership chain.
//!
//! A contact task runs in its own execution conversation, so the fixed private
//! chat keeps its unique `conversations.companion_id` binding. The chain
//! companion -> execution conversation -> report chat is persisted on
//! `digital_colleagues` before any dispatch, and this module is the single
//! identity entry point shared by the private chat and every background task:
//! prompt context, notes, directory checks, capability grants, budget and model
//! resolution all read the same effective companion.
//!
//! A task marked `companion_bound` whose chain no longer resolves is an error,
//! never a fallback to the unrestricted ordinary conversation path.
use super::Database;
use rusqlite::{Connection, OptionalExtension};

pub(crate) const OWNERSHIP_MISSING: &str = "companion.background_ownership_missing";

/// K2: a contact task's execution conversation carries the task context, not an
/// ordinary chat, so the primary conversation surfaces (list, archive, trash,
/// search, artifact source jump) skip it. The task stays discoverable through
/// `digital_colleagues`, and an unbound legacy colleague keeps its own chat.
pub(crate) const ORDINARY_CHAT_FILTER: &str = " AND NOT EXISTS (SELECT 1 FROM digital_colleagues dc
                        WHERE dc.conversation_id = c.id AND dc.companion_bound = 1)";

/// The effective companion identity of a conversation or Run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CompanionScope {
    pub companion_id: String,
    /// The conversation the Run actually executes in.
    pub conversation_id: String,
    /// True for a contact background task, false for the fixed private chat.
    pub background: bool,
    pub colleague_id: Option<String>,
    /// The fixed private chat results and decision cards are reported to.
    pub report_conversation_id: Option<String>,
}

impl CompanionScope {
    /// The companion execution position: a foreground chat keeps it while
    /// waiting for an answer, a background task releases it (K2).
    pub fn queue_slot(&self) -> (String, bool) {
        (self.companion_id.clone(), self.background)
    }
}

struct BackgroundLink {
    colleague_id: String,
    bound: bool,
    companion_id: Option<String>,
    report_conversation_id: Option<String>,
}

pub(crate) struct BackgroundTaskTarget {
    pub companion_id: String,
    pub report_conversation_id: String,
    pub workspace_dir: String,
    pub template_agent_id: Option<String>,
}

fn background_link(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<BackgroundLink>, String> {
    connection
        .query_row(
            "SELECT id, companion_bound, companion_id, report_conversation_id
             FROM digital_colleagues WHERE conversation_id = ?1",
            [conversation_id],
            |row| {
                Ok(BackgroundLink {
                    colleague_id: row.get(0)?,
                    bound: row.get::<_, i64>(1)? != 0,
                    companion_id: row.get(2)?,
                    report_conversation_id: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(|error| error.to_string())
}

fn missing(reason: &str) -> String {
    format!("{OWNERSHIP_MISSING}: {reason}")
}

/// Resolve the effective companion of an execution conversation. `Ok(None)`
/// means an ordinary conversation or a legacy colleague that was never bound to
/// a contact; those keep their existing unrestricted behaviour.
pub(crate) fn resolve_conversation_on(
    connection: &Connection,
    conversation_id: &str,
) -> Result<Option<CompanionScope>, String> {
    let literal: Option<String> = connection
        .query_row(
            "SELECT companion_id FROM conversations WHERE id = ?1",
            [conversation_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    if let Some(companion_id) = literal {
        return Ok(Some(CompanionScope {
            companion_id,
            conversation_id: conversation_id.to_owned(),
            background: false,
            colleague_id: None,
            report_conversation_id: None,
        }));
    }
    let Some(link) = background_link(connection, conversation_id)? else {
        return Ok(None);
    };
    if !link.bound {
        return Ok(None);
    }
    let companion_id = link
        .companion_id
        .ok_or_else(|| missing("后台任务缺少联系人归属，已停止执行"))?;
    let report_conversation_id = link
        .report_conversation_id
        .ok_or_else(|| missing("后台任务缺少回报私聊，已停止执行"))?;
    let state: Option<(bool, bool)> = connection
        .query_row(
            "SELECT archived_at IS NOT NULL, deleting_at IS NOT NULL
             FROM companion_profiles WHERE id = ?1",
            [&companion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match state {
        None => return Err(missing("后台任务的联系人已不存在，已停止执行")),
        Some((_, true)) => return Err(missing("后台任务的联系人正在删除，已停止执行")),
        Some((true, _)) => return Err(missing("后台任务的联系人已归档，已停止执行")),
        Some((false, false)) => {}
    }
    let fixed_chat: Option<String> = connection
        .query_row(
            "SELECT id FROM conversations WHERE companion_id = ?1",
            [&companion_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    if fixed_chat.as_deref() != Some(report_conversation_id.as_str()) {
        return Err(missing("后台任务的回报私聊与联系人不符，已停止执行"));
    }
    Ok(Some(CompanionScope {
        companion_id,
        conversation_id: conversation_id.to_owned(),
        background: true,
        colleague_id: Some(link.colleague_id),
        report_conversation_id: Some(report_conversation_id),
    }))
}

pub(crate) fn resolve_run_on(
    connection: &Connection,
    run_id: &str,
) -> Result<Option<CompanionScope>, String> {
    let conversation: Option<String> = connection
        .query_row(
            "SELECT conversation_id FROM runs WHERE id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match conversation {
        Some(conversation) => resolve_conversation_on(connection, &conversation),
        None => Ok(None),
    }
}

/// K2: an archived or deleting contact refuses new execution, including a Run
/// that was already queued when the stop sweep began.
pub(crate) fn ensure_executable_on(
    connection: &Connection,
    companion_id: &str,
) -> Result<(), String> {
    let state: Option<(bool, bool)> = connection
        .query_row(
            "SELECT archived_at IS NOT NULL, deleting_at IS NOT NULL
             FROM companion_profiles WHERE id = ?1",
            [companion_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    match state {
        None => Err(missing("联系人已不存在，已停止执行")),
        Some((_, true)) => Err(missing("联系人正在删除，已停止执行")),
        Some((true, _)) => Err(missing("联系人已归档，已停止执行")),
        Some((false, false)) => Ok(()),
    }
}

/// Every non-terminal Run of a contact: the fixed private chat plus each bound
/// background execution conversation. The archive/delete sweep stops exactly
/// this set, not just the chat.
pub(crate) fn active_run_ids_on(
    connection: &Connection,
    companion_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT r.id FROM runs r
             WHERE r.status IN ('queued', 'running', 'cancelling', 'awaiting_confirmation')
               AND (r.conversation_id IN (SELECT id FROM conversations WHERE companion_id = ?1)
                 OR r.conversation_id IN (SELECT conversation_id FROM digital_colleagues
                                          WHERE companion_id = ?1 AND companion_bound = 1))
             ORDER BY r.created_at, r.id",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([companion_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Every execution conversation of a contact's background tasks, used by the
/// archive/delete stop sweep and by the companion-wide active Run probe.
pub(crate) fn background_conversations_on(
    connection: &Connection,
    companion_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare(
            "SELECT conversation_id FROM digital_colleagues
             WHERE companion_id = ?1 AND companion_bound = 1",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([companion_id], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

impl Database {
    /// The persisted pieces a contact task is bound to at creation: its
    /// contact, the contact's workspace and the fixed private chat that will
    /// receive results and decision cards.
    pub(crate) fn companion_background_target(
        &self,
        companion_id: &str,
    ) -> Result<BackgroundTaskTarget, String> {
        self.with_connection(|connection| {
            let row: Option<(Option<String>, String, bool, bool)> = connection
                .query_row(
                    "SELECT template_agent_id, workspace_dir,
                            archived_at IS NOT NULL, deleting_at IS NOT NULL
                     FROM companion_profiles WHERE id = ?1",
                    [companion_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            let Some((template_agent_id, workspace_dir, archived, deleting)) = row else {
                return Err(rusqlite::Error::InvalidParameterName(
                    "digital_colleague.companion_not_found".into(),
                ));
            };
            if archived || deleting {
                return Err(rusqlite::Error::InvalidParameterName(
                    "digital_colleague.companion_unavailable".into(),
                ));
            }
            let report_conversation_id: Option<String> = connection
                .query_row(
                    "SELECT id FROM conversations WHERE companion_id = ?1",
                    [companion_id],
                    |row| row.get(0),
                )
                .optional()?;
            let Some(report_conversation_id) = report_conversation_id else {
                return Err(rusqlite::Error::InvalidParameterName(
                    "digital_colleague.companion_chat_missing".into(),
                ));
            };
            Ok(BackgroundTaskTarget {
                companion_id: companion_id.to_owned(),
                report_conversation_id,
                workspace_dir,
                template_agent_id,
            })
        })
    }

    pub(crate) fn conversation_companion_scope(
        &self,
        conversation_id: &str,
    ) -> Result<Option<CompanionScope>, String> {
        self.with_connection(|connection| {
            resolve_conversation_on(connection, conversation_id)
                .map_err(rusqlite::Error::InvalidParameterName)
        })
    }

    pub(crate) fn run_companion_scope(
        &self,
        run_id: &str,
    ) -> Result<Option<CompanionScope>, String> {
        self.with_connection(|connection| {
            resolve_run_on(connection, run_id).map_err(rusqlite::Error::InvalidParameterName)
        })
    }

    /// The companion execution position a Run needs, or None when the Run is
    /// outside any contact's serial range. A broken background chain is an
    /// error so the caller stops instead of queueing without an identity.
    pub(crate) fn run_companion_slot(
        &self,
        run_id: &str,
    ) -> Result<Option<(String, bool)>, String> {
        Ok(self.run_companion_scope(run_id)?.map(|scope| scope.queue_slot()))
    }

    pub(crate) fn companion_background_conversations(
        &self,
        companion_id: &str,
    ) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            background_conversations_on(connection, companion_id)
                .map_err(rusqlite::Error::InvalidParameterName)
        })
    }

    pub(crate) fn ensure_companion_executable(&self, companion_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            ensure_executable_on(connection, companion_id)
                .map_err(rusqlite::Error::InvalidParameterName)
        })
    }

    pub(crate) fn companion_active_run_ids(&self, companion_id: &str) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            active_run_ids_on(connection, companion_id).map_err(rusqlite::Error::InvalidParameterName)
        })
    }

    /// D3: every conversation whose in-memory session owner the archive drain
    /// must wait for — the contact's fixed private chat plus all of its bound
    /// background execution conversations. Other contacts' sessions are excluded
    /// so a still-running colleague does not block this archive.
    pub(crate) fn companion_session_conversations(&self, companion_id: &str) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT id FROM conversations WHERE companion_id = ?1
                 UNION
                 SELECT conversation_id FROM digital_colleagues
                 WHERE companion_id = ?1 AND companion_bound = 1",
            )?;
            let rows = statement.query_map([companion_id], |row| row.get::<_, String>(0))?;
            rows.collect()
        })
    }
}
