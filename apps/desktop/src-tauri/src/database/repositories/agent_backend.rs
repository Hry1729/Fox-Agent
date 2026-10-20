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

/// Where an external backend runs for a conversation: the project's database
/// id (private HOME key) and the authorized root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExternalConversationContext {
    pub project_id: Option<String>,
    pub project_root: Option<String>,
}

impl Database {
    /// `Ok(None)` when the conversation does not exist.
    pub(crate) fn external_conversation_context(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ExternalConversationContext>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT c.project_id, COALESCE(p.root_path, c.project_root)
                     FROM conversations c LEFT JOIN projects p ON p.id = c.project_id
                     WHERE c.id = ?1",
                    [conversation_id],
                    |row| {
                        Ok(ExternalConversationContext {
                            project_id: row.get(0)?,
                            project_root: row.get(1)?,
                        })
                    },
                )
                .optional()
        })
    }

    /// The backend's own session id last recorded for the conversation (the
    /// `agent_backend.session` run event the external runtime writes when it
    /// opens or reopens a native session). Used to `session/load` it after a
    /// restart. No schema change: the run events are the record.
    pub(crate) fn latest_native_session(&self, conversation_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            let json: Option<String> = connection
                .query_row(
                    "SELECT e.event_json FROM run_events e JOIN runs r ON r.id = e.run_id
                     WHERE r.conversation_id = ?1 AND e.event_type = 'agent_backend.session'
                     ORDER BY e.created_at DESC, e.seq DESC LIMIT 1",
                    [conversation_id],
                    |row| row.get(0),
                )
                .optional()?;
            Ok(json
                .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
                .and_then(|event| event.get("nativeSessionId").and_then(|id| id.as_str()).map(str::to_owned)))
        })
    }

    /// Runs of external-backend conversations that a previous Fox process
    /// left non-terminal. Their backend process died with that Fox.
    pub(crate) fn external_runs_left_active(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT r.id FROM runs r JOIN conversations c ON c.id = r.conversation_id
                 WHERE c.agent_backend <> 'fox'
                   AND r.status IN ('queued', 'running', 'cancelling')",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect()
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
    fn external_lookups_read_context_native_session_and_left_runs() {
        let database = Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        let pi = database
            .create_conversation_with_backend(database.default_agent_id(), Some("pi"), None, None, "pi")
            .expect("pi conversation");
        let context = database.external_conversation_context(&pi.id).unwrap().expect("context");
        assert_eq!(context.project_id, None);
        assert_eq!(database.external_conversation_context("missing").unwrap(), None);
        assert_eq!(database.latest_native_session(&pi.id).unwrap(), None);
        let started = database.create_run(&pi.id, "hello", None).expect("run");
        assert_eq!(database.external_runs_left_active().unwrap(), vec![started.run.id.clone()]);
        let run_id = started.run.id.clone();
        database
            .record_external_event(&run_id, &serde_json::json!({ "type": "run.started" }))
            .expect("started");
        database
            .record_external_event(
                &run_id,
                &serde_json::json!({ "type": "agent_backend.session", "nativeSessionId": "sess-1" }),
            )
            .expect("session event");
        assert_eq!(database.latest_native_session(&pi.id).unwrap().as_deref(), Some("sess-1"));
        database
            .record_external_event(&run_id, &serde_json::json!({ "type": "run.completed" }))
            .expect("completed");
        assert!(database.external_runs_left_active().unwrap().is_empty());
        let fox = database
            .create_conversation(database.default_agent_id(), Some("fox"), None, None)
            .expect("fox conversation");
        database.create_run(&fox.id, "hi", None).expect("fox run");
        assert!(database.external_runs_left_active().unwrap().is_empty(), "Fox runs are never touched");
    }

    /// External runs carry no `run_control_bindings` row. The shared approval
    /// and completion paths must still work for them: `resolve_approval` reads
    /// its wait window through `run_time_budgets`, which falls back to the
    /// default budget when no binding exists, and `run.completed` goes through
    /// `enforce_managed_run_completion_budget`, which only constrains child and
    /// digital-colleague runs. Pins both so a later change that starts requiring
    /// a binding there fails here instead of in a live external session.
    fn approval_status(database: &Database, approval_id: &str) -> String {
        database
            .with_connection(|connection| {
                connection.query_row("SELECT status FROM approvals WHERE id = ?1", [approval_id], |row| row.get::<_, String>(0))
            })
            .expect("approval status")
    }

    fn approval_reason(database: &Database, approval_id: &str) -> String {
        let decision: String = database
            .with_connection(|connection| {
                connection.query_row("SELECT decision_json FROM approvals WHERE id = ?1", [approval_id], |row| row.get::<_, String>(0))
            })
            .expect("approval decision");
        serde_json::from_str::<serde_json::Value>(&decision).expect("decision json")["reason"].as_str().unwrap_or_default().to_owned()
    }

    #[test]
    fn external_runs_without_a_binding_resolve_approvals_and_complete() {
        use crate::database::ApprovalDecision;
        let database = Database::open(std::path::PathBuf::from(":memory:")).expect("database");
        // One conversation per case so no case depends on another run having ended.
        let approve_once = |key: &str| {
            let pi = database
                .create_conversation_with_backend(database.default_agent_id(), Some(key), None, None, "pi")
                .expect("pi conversation");
            let started = database.create_run(&pi.id, "hello", None).expect("run");
            let run_id = started.run.id.clone();
            database
                .record_external_event(&run_id, &serde_json::json!({ "type": "run.started" }))
                .expect("started");
            assert!(database.run_control_binding(&run_id).unwrap().is_none(), "external runs are never bound");
            let (tool, _) = database
                .create_host_tool_call_once(&run_id, key, "agent_permission", &serde_json::json!({}), "pending", true)
                .expect("permission tool call");
            let approval = database
                .create_approval(&tool.id, "run a command", &serde_json::json!({ "availableDecisions": ["allow_once", "deny"] }))
                .expect("approval");
            (run_id, approval.id)
        };

        let (approved_run, approved_id) = approve_once("perm-allow");
        let resolved = database
            .resolve_approval(&approved_id, ApprovalDecision::AllowOnce)
            .expect("no binding is not an error")
            .expect("resolved");
        assert_eq!(resolved.status, "approved");
        assert_eq!(
            database.approval_agent_backend(&approved_id).unwrap().map(|identity| identity.agent_backend).as_deref(),
            Some("pi")
        );

        let (_denied_run, denied_id) = approve_once("perm-deny");
        let denied = database
            .resolve_approval(&denied_id, ApprovalDecision::Deny)
            .expect("deny without binding")
            .expect("resolved");
        assert_eq!(denied.status, "denied");

        // Approvals wait for the user: hours after the request (far past the
        // old default 5 minute window) the approval is still pending, and the
        // user's late "allow once" is a real grant.
        let (_late_run, late_id) = approve_once("perm-late");
        database
            .with_connection(|connection| {
                connection.execute("UPDATE approvals SET requested_at = requested_at - 36000000 WHERE id = ?1", [&late_id])
            })
            .expect("age the approval by ten hours");
        assert_eq!(approval_status(&database, &late_id), "pending", "a long unanswered approval stays pending");
        let late = database
            .resolve_approval(&late_id, ApprovalDecision::AllowOnce)
            .expect("late approval without binding")
            .expect("resolved");
        assert_eq!(late.status, "approved", "approvals no longer expire");

        // Cancelling the turn ends its open approval as cancelled.
        let (cancel_run, cancel_id) = approve_once("perm-cancel");
        database.mark_run_cancelling(&cancel_run).expect("cancelling").expect("was running");
        assert_eq!(approval_status(&database, &cancel_id), "cancelled");
        assert!(database.resolve_approval(&cancel_id, ApprovalDecision::AllowOnce).unwrap().is_none());

        // Closing the conversation (archive / trash / delete: backend session
        // closed, the turn ends interrupted) ends its open approval as
        // `cancelled` (not `expired`); it can no longer be granted.
        let (closed_run, closed_id) = approve_once("perm-closed");
        database
            .record_external_event(
                &closed_run,
                &serde_json::json!({ "type": "run.interrupted", "code": "agent_backend_session_closed", "message": "外部后端会话已关闭", "outcome": "unknown" }),
            )
            .expect("run.interrupted");
        assert_eq!(approval_status(&database, &closed_id), "cancelled");
        assert_eq!(approval_reason(&database, &closed_id), "conversation_closed");
        assert!(database.resolve_approval(&closed_id, ApprovalDecision::AllowOnce).unwrap().is_none());

        // Fox exited with the approval open; the restart interrupts the run
        // and the approval ends `cancelled` too.
        let (restarted_run, restarted_id) = approve_once("perm-restart");
        database
            .mark_run_interrupted(&restarted_run, "agent_backend_host_restarted", "Fox 已重启")
            .expect("interrupted at startup");
        assert_eq!(approval_status(&database, &restarted_id), "cancelled");
        assert_eq!(approval_reason(&database, &restarted_id), "app_restarted");
        assert!(database.resolve_approval(&restarted_id, ApprovalDecision::AllowOnce).unwrap().is_none());

        // Any other interruption still records `expired` + `run_interrupted`.
        let (other_run, other_id) = approve_once("perm-other");
        database
            .mark_run_interrupted(&other_run, "runtime.cancel_not_active", "gone")
            .expect("interrupted");
        assert_eq!(approval_status(&database, &other_id), "expired");
        assert_eq!(approval_reason(&database, &other_id), "run_interrupted");

        database
            .record_external_event(&approved_run, &serde_json::json!({ "type": "run.completed" }))
            .expect("run.completed without a binding")
            .expect("applied");
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
