//! Read side of the conversation-level `agent_backend` (four-agent plan
//! v0.3.1, 6.6; Host wiring first batch). Writes happen only in the INSERT of
//! `create_conversation_with_backend`; the v95 trigger refuses any update.
//!
//! Every lookup returns the stored string unchanged. Interpreting it (old-row
//! mapping, misroute refusal, the `FOX_EXTERNAL_BACKENDS` switch) is
//! `fox_agent_backend::host_routing`'s job, so this file holds no policy.
use super::Database;
use rusqlite::OptionalExtension;

/// The frozen backend of the conversation a run or approval belongs to, plus
/// the Kernel `engine_id` for the audit line (`None` when the run has no
/// Kernel record, e.g. a yuxi or not-yet-dispatched run).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunBackendIdentity {
    pub conversation_id: String,
    pub run_id: String,
    pub agent_backend: String,
    pub engine_id: Option<String>,
}

const RUN_IDENTITY_COLUMNS: &str = "r.conversation_id, r.id, c.agent_backend,
     COALESCE((SELECT k.engine_id FROM kernel_runs k WHERE k.run_id = r.id),
              (SELECT b.engine_id FROM run_control_bindings b WHERE b.run_id = r.id))";

fn map_identity(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunBackendIdentity> {
    Ok(RunBackendIdentity {
        conversation_id: row.get(0)?,
        run_id: row.get(1)?,
        agent_backend: row.get(2)?,
        engine_id: row.get(3)?,
    })
}

impl Database {
    /// `Ok(None)` when the conversation does not exist.
    pub(crate) fn conversation_agent_backend(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT agent_backend FROM conversations WHERE id = ?1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    /// `Ok(None)` when the run does not exist.
    pub(crate) fn run_agent_backend(
        &self,
        run_id: &str,
    ) -> Result<Option<RunBackendIdentity>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {RUN_IDENTITY_COLUMNS}
                         FROM runs r JOIN conversations c ON c.id = r.conversation_id
                         WHERE r.id = ?1"
                    ),
                    [run_id],
                    map_identity,
                )
                .optional()
        })
    }

    /// `Ok(None)` when the approval does not exist. Every approval (Kernel or
    /// legacy) is a row of `approvals` keyed to a tool call of one run.
    pub(crate) fn approval_agent_backend(
        &self,
        approval_id: &str,
    ) -> Result<Option<RunBackendIdentity>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    &format!(
                        "SELECT {RUN_IDENTITY_COLUMNS}
                         FROM approvals a
                         JOIN tool_calls t ON t.id = a.tool_call_id
                         JOIN runs r ON r.id = t.run_id
                         JOIN conversations c ON c.id = r.conversation_id
                         WHERE a.id = ?1"
                    ),
                    [approval_id],
                    map_identity,
                )
                .optional()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_conversations_freeze_their_backend_and_reads_return_it() {
        let database = Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let fox = database
            .create_conversation(database.default_agent_id(), Some("fox"), None, None)
            .expect("fox conversation");
        assert_eq!(fox.agent_backend, "fox");
        assert_eq!(database.conversation_agent_backend(&fox.id).unwrap().as_deref(), Some("fox"));

        let pi = database
            .create_conversation_with_backend(database.default_agent_id(), Some("pi"), None, None, "pi")
            .expect("pi conversation");
        assert_eq!(pi.agent_backend, "pi");
        assert_eq!(database.conversation_agent_backend(&pi.id).unwrap().as_deref(), Some("pi"));

        let error = database
            .with_connection(|connection| {
                connection.execute("UPDATE conversations SET agent_backend='fox' WHERE id=?1", [&pi.id])
            })
            .expect_err("frozen");
        assert!(error.contains("agent_backend_immutable"), "{error}");
        assert!(database
            .create_conversation_with_backend(database.default_agent_id(), None, None, None, "deepseek_harness")
            .is_err());
        assert_eq!(database.conversation_agent_backend("missing").unwrap(), None);
        assert_eq!(database.run_agent_backend("missing").unwrap(), None);
        assert_eq!(database.approval_agent_backend("missing").unwrap(), None);
    }

    #[test]
    fn run_identity_carries_the_conversation_backend() {
        let database = Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let conversation = database
            .create_conversation(database.default_agent_id(), Some("run"), None, None)
            .expect("conversation");
        let started = database.create_run(&conversation.id, "hello", None).expect("run");
        let identity = database.run_agent_backend(&started.run.id).unwrap().expect("identity");
        assert_eq!(identity.conversation_id, conversation.id);
        assert_eq!(identity.run_id, started.run.id);
        assert_eq!(identity.agent_backend, "fox");
        assert_eq!(identity.engine_id, None, "no Kernel record before dispatch");
    }
}
