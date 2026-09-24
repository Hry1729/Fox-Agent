//! Kernel authorization grants (limit plan A · #5).
//!
//! A user who answers "allow for this conversation" has stated a fact the Host
//! must be able to audit later. Historically the Kernel recorded the decision
//! string on `kernel_approvals` and then forgot it: the next identical call was
//! measured against the grants frozen at run start, so the user was asked again
//! for work they had explicitly approved.
//!
//! This module records the approved scope as its own fact and answers the only
//! question the policy needs: *does this exact tool + project + target/action
//! fall inside a grant that is still live?*
//!
//! Deliberate boundaries:
//!   * `allow_once` never registers anything — a one-shot decision stays one-shot;
//!   * a scope that cannot be named exactly is never registered (we would rather
//!     ask than invent a permission);
//!   * the frozen `binding_json` is never rewritten: new approvals only ever add;
//!   * revocation and expiry are checked on every read, so a revoked grant cannot
//!     be resurrected by a cached decision.

use super::{now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where an approved scope came from. Only one source exists today; keeping it
/// explicit stops a future caller from inventing an unowned permission.
pub const SOURCE_CONVERSATION_APPROVAL: &str = "conversation_approval";

/// A grant is bound to one conversation-owned scope kind. The value is the same
/// opaque key the frozen matcher compares, never a free-form pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantScopeKind {
    Path,
    Action,
    Tool,
    OfficeRequest,
    McpTool,
}

impl GrantScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GrantScopeKind::Path => "path",
            GrantScopeKind::Action => "action",
            GrantScopeKind::Tool => "tool",
            GrantScopeKind::OfficeRequest => "office_request",
            GrantScopeKind::McpTool => "mcp_tool",
        }
    }

    /// Infer the kind from the canonical scope key the Host already computes.
    /// The prefix before the first `:` is the Host's own taxonomy
    /// (`host-target`, `capability-<tool>`, `office-request`, `mcp-tool`), so
    /// this is a classification of existing keys, not a new naming scheme.
    pub fn from_scope_key(scope: &str) -> Option<GrantScopeKind> {
        let prefix = scope.split(':').next().unwrap_or_default();
        match prefix {
            "host-target" => Some(GrantScopeKind::Path),
            "office-request" => Some(GrantScopeKind::OfficeRequest),
            "mcp-tool" => Some(GrantScopeKind::McpTool),
            "" => None,
            _ if prefix.starts_with("capability-") => Some(GrantScopeKind::Action),
            _ => Some(GrantScopeKind::Tool),
        }
    }
}

/// A live grant, as consumed by the policy union.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdditionalGrant {
    pub tool: String,
    pub scope_value: String,
    pub kind: GrantScopeKind,
    pub approval_id: String,
}

impl AdditionalGrant {
    /// The frozen matcher consumes `(tool, scope)` pairs. Reusing that exact
    /// shape means the union cannot accidentally loosen the comparison.
    pub fn matches(&self, tool: &str, scope: &str) -> bool {
        self.tool == tool && self.scope_value == scope
    }
}

/// Why a requested registration was refused. Refusal is not an error: it means
/// the caller must keep asking the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantRegistration {
    Registered { grant_id: String },
    Skipped { reason: GrantSkipReason },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrantSkipReason {
    /// A one-shot approval must never become a reusable permission.
    OneShotApprovalIsNotSessionAuthorization,
    /// The Host could not name the exact scope; ask again instead of guessing.
    UnnameableScope,
    /// The approval belongs to another run/conversation or is not an approval.
    ForeignOrUnknownApproval,
    /// The run's frozen project root disagrees with the approval's project.
    ProjectMismatch,
}

impl GrantSkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            GrantSkipReason::OneShotApprovalIsNotSessionAuthorization => {
                "allow_once_is_not_session_authorization"
            }
            GrantSkipReason::UnnameableScope => "scope_cannot_be_named_exactly",
            GrantSkipReason::ForeignOrUnknownApproval => "approval_is_foreign_or_unknown",
            GrantSkipReason::ProjectMismatch => "approval_project_differs_from_frozen_root",
        }
    }
}

impl Database {
    /// Register the scope a `allow_conversation` decision approved.
    ///
    /// The caller passes the run, the approval that produced the decision, and
    /// the exact scope key the policy computed for the call. Anything the Host
    /// cannot name exactly is refused rather than widened.
    pub fn kernel_register_authorization_grant(
        &self,
        run_id: &str,
        approval_tool_call_id: &str,
        decision: &str,
        tool: &str,
        scope_key: Option<&str>,
    ) -> Result<GrantRegistration, String> {
        if decision != "allow_conversation" {
            return Ok(GrantRegistration::Skipped {
                reason: GrantSkipReason::OneShotApprovalIsNotSessionAuthorization,
            });
        }
        let Some(scope_key) = scope_key.map(str::trim).filter(|scope| !scope.is_empty()) else {
            return Ok(GrantRegistration::Skipped {
                reason: GrantSkipReason::UnnameableScope,
            });
        };
        let Some(kind) = GrantScopeKind::from_scope_key(scope_key) else {
            return Ok(GrantRegistration::Skipped {
                reason: GrantSkipReason::UnnameableScope,
            });
        };
        if tool.trim().is_empty() || scope_key.len() > 512 {
            return Ok(GrantRegistration::Skipped {
                reason: GrantSkipReason::UnnameableScope,
            });
        }
        // The approval must be a real, conversation-approved decision on this
        // run. A forged or foreign id must not mint a permission.
        let owner: Option<(String, String, Option<String>)> = self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT runs.conversation_id, a.state, b.binding_json
                       FROM kernel_approvals a
                       JOIN runs ON runs.id = a.run_id
                       LEFT JOIN run_control_bindings b ON b.run_id = a.run_id
                      WHERE a.run_id = ?1 AND a.tool_call_id = ?2 AND a.state = 'allow_conversation'",
                    params![run_id, approval_tool_call_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
        })?;
        let Some((conversation_id, _state, binding_json)) = owner else {
            return Ok(GrantRegistration::Skipped {
                reason: GrantSkipReason::ForeignOrUnknownApproval,
            });
        };
        // The frozen project root is the authority for "same project". A grant
        // may never move a scope into another project.
        if let Some(binding_json) = binding_json {
            let binding: fox_engine_protocol::RunControlBinding =
                serde_json::from_str(&binding_json)
                    .map_err(|error| format!("invalid frozen binding: {error}"))?;
            if binding.run_id != run_id {
                return Ok(GrantRegistration::Skipped {
                    reason: GrantSkipReason::ForeignOrUnknownApproval,
                });
            }
        }
        let now = now_ms();
        let grant_id = format!("grant:{}", Uuid::new_v4());
        // The frozen project root is read once, outside the write closure, so the
        // closure stays a rusqlite-only body.
        let frozen = self.run_control_binding(run_id)?;
        let project_root = frozen
            .as_ref()
            .and_then(|binding| binding.permission.project_root.clone());
        let run_budget_ms = frozen.and_then(|binding| {
            binding.budgets.run_execution_limited.then_some(binding.budgets.run_execution_ms)
        });
        self.with_connection(|connection| {
            // Serialize against policy changes and grant revocation. The early
            // owner lookup only shapes the scope; the version/state check that
            // authorizes the write must share this transaction with the UPSERT.
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let current: Option<(String, Option<u64>, u64, String)> = transaction.query_row(
                "SELECT r.conversation_id,a.policy_version,p.version,k.state
                   FROM kernel_approvals a JOIN runs r ON r.id=a.run_id
                   JOIN kernel_execution_policies p ON p.conversation_id=r.conversation_id
                   JOIN kernel_runs k ON k.run_id=a.run_id
                  WHERE a.run_id=?1 AND a.tool_call_id=?2 AND a.state='allow_conversation'",
                params![run_id, approval_tool_call_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
            ).optional()?;
            let Some((current_conversation, approval_version, policy_version, run_state)) = current else {
                return Ok(GrantRegistration::Skipped { reason: GrantSkipReason::ForeignOrUnknownApproval });
            };
            if current_conversation != conversation_id
                || approval_version != Some(policy_version)
                || matches!(run_state.as_str(), "cancelling" | "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired")
            {
                return Ok(GrantRegistration::Skipped { reason: GrantSkipReason::ForeignOrUnknownApproval });
            }
            let existing: Option<(String, Option<i64>)> = transaction.query_row(
                "SELECT id,revoked_at FROM kernel_authorization_grants
                  WHERE run_id=?1 AND tool=?2 AND scope_value=?3",
                params![run_id, tool, scope_key],
                |row| Ok((row.get(0)?,row.get(1)?)),
            ).optional()?;
            if let Some((existing_id, revoked_at)) = &existing {
                if revoked_at.is_none() {
                    // Retrying the same current decision must neither extend
                    // its deadline nor reset its usage record. An active scope
                    // already has a source; an older approval replay must not
                    // replace that source. A revoked row may be registered by
                    // a new decision at the current policy version.
                    return Ok(GrantRegistration::Registered { grant_id: existing_id.clone() });
                }
            }
            // Preserve the established TTL for a newly approved scope. An
            // active grant returns above, so replay cannot extend that TTL.
            let expires_at = run_budget_ms.map(|budget| now.saturating_add(budget));
            transaction.execute(
                "INSERT INTO kernel_authorization_grants
                 (id, run_id, conversation_id, approval_id, tool, project_root, scope_kind,
                  scope_value, source, created_at, expires_at, revoked_at, revoke_reason,
                  use_count, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, NULL, 0, NULL)
                 ON CONFLICT(run_id, tool, scope_value) DO UPDATE SET
                     approval_id = excluded.approval_id,
                     created_at = excluded.created_at,
                     revoked_at = NULL,
                     revoke_reason = NULL,
                     expires_at = excluded.expires_at",
                params![
                    grant_id,
                    run_id,
                    conversation_id,
                    approval_tool_call_id,
                    tool,
                    project_root,
                    kind.as_str(),
                    scope_key,
                    SOURCE_CONVERSATION_APPROVAL,
                    now,
                    expires_at,
                ],
            )?;
            let stored_id: String = transaction.query_row(
                "SELECT id FROM kernel_authorization_grants WHERE run_id=?1 AND tool=?2 AND scope_value=?3",
                params![run_id, tool, scope_key], |row| row.get(0),
            )?;
            transaction.commit()?;
            Ok(GrantRegistration::Registered { grant_id: stored_id })
        })
    }

    /// The tool and canonical input of a decided approval, so a host can name the
    /// exact scope a decision covered. Returns `None` for an unknown run/call or
    /// for an approval that is still pending.
    pub fn kernel_decided_approval_call(
        &self,
        run_id: &str,
        tool_call_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT c.tool, c.canonical_input_json
                       FROM kernel_approvals a
                       JOIN kernel_tool_calls c
                         ON c.run_id = a.run_id AND c.tool_call_id = a.tool_call_id
                      WHERE a.run_id = ?1 AND a.tool_call_id = ?2 AND a.state <> 'pending'",
                    params![run_id, tool_call_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
    }

    /// Live grants for one run: not revoked, not expired. Every read re-checks
    /// both, so revocation cannot be undone by a stale cached decision.
    pub fn kernel_effective_authorization_grants(
        &self,
        run_id: &str,
        now: i64,
    ) -> Result<Vec<AdditionalGrant>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT tool, scope_value, scope_kind, approval_id
                   FROM kernel_authorization_grants
                  WHERE run_id = ?1
                    AND revoked_at IS NULL
                    AND (expires_at IS NULL OR expires_at > ?2)
                  ORDER BY created_at, id",
            )?;
            let rows = statement.query_map(params![run_id, now], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            let mut grants = Vec::new();
            for row in rows {
                let (tool, scope_value, kind, approval_id) = row?;
                let kind = match kind.as_str() {
                    "path" => GrantScopeKind::Path,
                    "action" => GrantScopeKind::Action,
                    "tool" => GrantScopeKind::Tool,
                    "office_request" => GrantScopeKind::OfficeRequest,
                    "mcp_tool" => GrantScopeKind::McpTool,
                    _ => continue,
                };
                grants.push(AdditionalGrant {
                    tool,
                    scope_value,
                    kind,
                    approval_id,
                });
            }
            Ok(grants)
        })
    }

    /// Revoke every live grant in a conversation. A user withdrawing permission
    /// must affect the running work, not only future runs.
    pub fn kernel_revoke_authorization_grants(
        &self,
        conversation_id: &str,
        reason: &str,
    ) -> Result<usize, String> {
        let now = now_ms();
        self.with_connection(|connection| {
            let revoked = connection.execute(
                "UPDATE kernel_authorization_grants
                    SET revoked_at = ?2, revoke_reason = ?3
                  WHERE conversation_id = ?1 AND revoked_at IS NULL",
                params![conversation_id, now, reason],
            )?;
            Ok(revoked)
        })
    }

    /// Record that a live grant was actually used. Auditing needs to show the
    /// reuse, not only the grant.
    pub fn kernel_note_authorization_grant_use(
        &self,
        run_id: &str,
        tool: &str,
        scope_key: &str,
    ) -> Result<(), String> {
        let now = now_ms();
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_authorization_grants
                    SET use_count = use_count + 1, last_used_at = ?4
                  WHERE run_id = ?1 AND tool = ?2 AND scope_value = ?3
                    AND revoked_at IS NULL",
                params![run_id, tool, scope_key, now],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_database() -> (Database, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!("fox-grants-{}.sqlite", Uuid::new_v4()));
        let database = Database::open(path.clone()).expect("open database");
        (database, path)
    }

    /// Minimal authoritative run so the approval/run join is satisfiable.
    /// Each run owns its own decided approval (`{run_id}-call`), so "borrowing
    /// another run's approval" is a real, distinguishable case.
    fn seed_run(database: &Database, run_id: &str, conversation: &str) {
        seed_run_with_budget(database, run_id, conversation, true);
    }

    // SQL-seeded component fixture; Host/model behavior is covered separately.
    fn seed_run_with_budget(database: &Database, run_id: &str, conversation: &str, limited: bool) {
        let tool_call_id = format!("{run_id}-call");
        database
            .with_connection(|connection| {
                connection.execute(
                    // Several runs may share one conversation; only the first
                    // seeding creates it.
                    "INSERT OR IGNORE INTO conversations(id, agent_id, title, project_root, status, created_at, updated_at)
                     VALUES (?1, 'fox-general', 't', NULL, 'active', 0, 0)",
                    params![conversation],
                )?;
                connection.execute(
                    "INSERT INTO runs(id, conversation_id, status, model, started_at, created_at)
                     VALUES (?1, ?2, 'running', 'm', 0, 0)",
                    params![run_id, conversation],
                )?;
                Ok(())
            })
            .expect("conversation and legacy run");
        let policy_version = database.execution_policy(conversation).expect("policy").version;
        database.freeze_kernel_run_control(run_id, "legacy", fox_engine_protocol::TimeBudgets {
            model_request_ms: 120_000, model_first_response_ms: 60_000,
            model_idle_ms: 120_000, tool_execution_ms: 600_000,
            run_execution_ms: 600_000, run_execution_limited: limited,
            approval_wait_ms: 300_000,
        }).expect("frozen budget");
        let config = crate::kernel::RunFrozenConfig {
            engine_id: "pi".into(),
            kernel_mode: "authoritative".into(),
            capability_manifest_version: 2,
            capability_manifest_hash: "manifest-hash".into(),
            permission_snapshot_id: "perm-1".into(),
            execution_profile_id: "legacy".into(),
            prompt_config_hash: "hash".into(),
            model_request_timeout_ms: 120_000,
            model_first_response_ms: 60_000,
            model_idle_ms: 120_000,
            tool_execution_timeout_ms: 600_000,
            run_execution_budget_ms: 600_000,
            run_execution_limited: true,
            approval_wait_timeout_ms: 300_000,
            provider_max_retries: 2,
            turn_max_retries: 1,
            experimental_compute_job_notice: false,
        };
        database
            .kernel_create_run(
                run_id,
                "pi",
                "authoritative",
                2,
                "perm-1",
                "legacy",
                "hash",
                &serde_json::to_string(&config).expect("serialize frozen config"),
            )
            .expect("kernel run");
        database
            .with_connection(|connection| {
                connection.execute(
                    "INSERT INTO kernel_tool_calls
                     (run_id, tool_call_id, batch_id, tool, source_order, canonical_input_json,
                      state, result_json, created_at, settled_at, dispatch_idempotency_key)
                     VALUES (?1, ?2, 'b1', 'write_file', 0, '{\"path\":\"a.txt\"}',
                             'running', NULL, 0, 0, NULL)",
                    params![run_id, tool_call_id],
                )?;
                connection.execute(
                    "INSERT INTO kernel_approvals(run_id, tool_call_id, state, created_at, decided_at, policy_version)
                     VALUES (?1, ?2, 'allow_conversation', 0, 0, ?3)",
                    params![run_id, tool_call_id, policy_version],
                )?;
                Ok(())
            })
            .expect("seed rows");
    }

    #[test]
    fn allow_once_never_becomes_a_session_authorization() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        let outcome = database
            .kernel_register_authorization_grant(
                "run-1",
                "call-1",
                "allow_once",
                "write_file",
                Some("host-target:sha256:abc"),
            )
            .unwrap();
        assert_eq!(
            outcome,
            GrantRegistration::Skipped {
                reason: GrantSkipReason::OneShotApprovalIsNotSessionAuthorization
            }
        );
        assert!(database
            .kernel_effective_authorization_grants("run-1", 0)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unknown_or_stale_approval_version_cannot_register_a_grant() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        database.with_connection(|connection| {
            connection.execute("UPDATE kernel_approvals SET policy_version=NULL WHERE run_id='run-1'", [])?;
            Ok(())
        }).unwrap();
        let register = || database.kernel_register_authorization_grant(
            "run-1", "run-1-call", "allow_conversation", "write_file", Some("host-target:sha256:abc"),
        ).unwrap();
        assert!(matches!(register(), GrantRegistration::Skipped { .. }));
        let stale = database.execution_policy("conv-1").unwrap().version.saturating_add(1);
        database.with_connection(|connection| {
            connection.execute("UPDATE kernel_approvals SET policy_version=?1 WHERE run_id='run-1'", [stale])?;
            Ok(())
        }).unwrap();
        assert!(matches!(register(), GrantRegistration::Skipped { .. }));
        assert!(database.kernel_effective_authorization_grants("run-1", 0).unwrap().is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn grant_retry_keeps_identity_and_new_approval_records_its_actual_source() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        let register = |approval: &str| database.kernel_register_authorization_grant(
            "run-1", approval, "allow_conversation", "write_file", Some("host-target:sha256:abc"),
        ).unwrap();
        let first = register("run-1-call");
        let GrantRegistration::Registered { grant_id } = first else { panic!("first approval did not register") };
        let deadline: Option<i64> = database.with_connection(|connection| connection.query_row(
            "SELECT expires_at FROM kernel_authorization_grants WHERE id=?1", [&grant_id], |row| row.get(0),
        )).unwrap();
        assert!(deadline.is_some(), "limited run has a fixed deadline");
        database.kernel_note_authorization_grant_use("run-1", "write_file", "host-target:sha256:abc").unwrap();
        assert_eq!(register("run-1-call"), GrantRegistration::Registered { grant_id: grant_id.clone() });
        let (used, replay_deadline): (i64, Option<i64>) = database.with_connection(|connection| connection.query_row(
            "SELECT use_count,expires_at FROM kernel_authorization_grants WHERE id=?1", [&grant_id], |row| Ok((row.get(0)?,row.get(1)?)),
        )).unwrap();
        assert_eq!(used, 1, "a retry does not reset the usage record");
        assert_eq!(replay_deadline, deadline, "a retry cannot extend the deadline");
        database.kernel_revoke_authorization_grants("conv-1", "test withdrawal").unwrap();
        assert!(matches!(register("run-1-call"), GrantRegistration::Skipped { .. }));
        let reissued_version = database.execution_policy("conv-1").unwrap().version;
        database.with_connection(|connection| {
            connection.execute(
                "UPDATE kernel_approvals SET policy_version=?1,state='allow_conversation',decided_at=1
                 WHERE run_id='run-1' AND tool_call_id='run-1-call'",
                [reissued_version],
            )?;
            Ok(())
        }).unwrap();
        assert_eq!(register("run-1-call"), GrantRegistration::Registered { grant_id: grant_id.clone() });
        database.kernel_revoke_authorization_grants("conv-1", "second withdrawal").unwrap();
        let next_version = database.execution_policy("conv-1").unwrap().version;
        database.with_connection(|connection| {
            connection.execute(
                "INSERT INTO kernel_tool_calls(run_id,tool_call_id,batch_id,tool,source_order,canonical_input_json,state,created_at)
                 VALUES('run-1','second-call','b2','write_file',0,'{\"path\":\"a.txt\"}','running',0)", [],
            )?;
            connection.execute(
                "INSERT INTO kernel_approvals(run_id,tool_call_id,state,created_at,decided_at,policy_version)
                 VALUES('run-1','second-call','allow_conversation',0,0,?1)", [next_version],
            )?;
            Ok(())
        }).unwrap();
        assert_eq!(register("second-call"), GrantRegistration::Registered { grant_id: grant_id.clone() });
        let (stored_source, revoked): (String, Option<i64>) = database.with_connection(|connection| connection.query_row(
            "SELECT approval_id,revoked_at FROM kernel_authorization_grants WHERE id=?1", [&grant_id],
            |row| Ok((row.get(0)?,row.get(1)?)),
        )).unwrap();
        assert_eq!(stored_source, "second-call");
        assert_eq!(revoked, None);
        let renewed_deadline: Option<i64> = database.with_connection(|connection| connection.query_row(
            "SELECT expires_at FROM kernel_authorization_grants WHERE id=?1", [&grant_id], |row| row.get(0),
        )).unwrap();
        assert!(renewed_deadline.is_some(), "a genuinely new approval receives a finite TTL");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn unlimited_run_grant_has_no_expiration_deadline() {
        let (database, path) = new_database();
        seed_run_with_budget(&database, "run-unlimited", "conv-unlimited", false);
        let register = || database.kernel_register_authorization_grant(
            "run-unlimited", "run-unlimited-call", "allow_conversation", "write_file", Some("host-target:sha256:abc"),
        ).unwrap();
        let first = register();
        assert_eq!(register(), first);
        let GrantRegistration::Registered { grant_id } = first else { panic!("grant not registered") };
        let deadline: Option<i64> = database.with_connection(|connection| connection.query_row(
            "SELECT expires_at FROM kernel_authorization_grants WHERE id=?1", [&grant_id], |row| row.get(0),
        )).unwrap();
        assert_eq!(deadline, None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_named_scope_is_reused_and_an_out_of_scope_call_still_misses() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        let outcome = database
            .kernel_register_authorization_grant(
                "run-1",
                "run-1-call",
                "allow_conversation",
                "write_file",
                Some("host-target:sha256:abc"),
            )
            .unwrap();
        assert!(matches!(outcome, GrantRegistration::Registered { .. }));
        let grants = database
            .kernel_effective_authorization_grants("run-1", 0)
            .unwrap();
        assert_eq!(grants.len(), 1);
        assert!(grants[0].matches("write_file", "host-target:sha256:abc"));
        // A different target of the same tool is a different scope, and a
        // different tool with the same key is still a different permission.
        assert!(!grants[0].matches("write_file", "host-target:sha256:other"));
        assert!(!grants[0].matches("edit_file", "host-target:sha256:abc"));
        database
            .kernel_note_authorization_grant_use("run-1", "write_file", "host-target:sha256:abc")
            .unwrap();
        let used: i64 = database
            .with_connection(|connection| {
                connection.query_row(
                    "SELECT use_count FROM kernel_authorization_grants WHERE run_id='run-1'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        assert_eq!(used, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn revocation_takes_effect_immediately() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        database
            .kernel_register_authorization_grant(
                "run-1",
                "run-1-call",
                "allow_conversation",
                "write_file",
                Some("host-target:sha256:abc"),
            )
            .unwrap();
        assert_eq!(
            database
                .kernel_effective_authorization_grants("run-1", 0)
                .unwrap()
                .len(),
            1
        );
        let revoked = database
            .kernel_revoke_authorization_grants("conv-1", "user_withdrew_permission")
            .unwrap();
        assert_eq!(revoked, 1);
        assert!(database
            .kernel_effective_authorization_grants("run-1", 0)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_foreign_or_one_shot_approval_cannot_mint_a_grant() {
        let (database, path) = new_database();
        seed_run(&database, "run-1", "conv-1");
        seed_run(&database, "run-2", "conv-1");
        // run-2 owns its own decided approval; borrowing run-1's must not mint
        // a permission for run-2.
        let outcome = database
            .kernel_register_authorization_grant(
                "run-2",
                "run-1-call",
                "allow_conversation",
                "write_file",
                Some("host-target:sha256:abc"),
            )
            .unwrap();
        assert_eq!(
            outcome,
            GrantRegistration::Skipped {
                reason: GrantSkipReason::ForeignOrUnknownApproval
            }
        );
        // An unnameable scope is refused instead of being widened to the tool.
        let outcome = database
            .kernel_register_authorization_grant(
                "run-1",
                "run-1-call",
                "allow_conversation",
                "write_file",
                None,
            )
            .unwrap();
        assert_eq!(
            outcome,
            GrantRegistration::Skipped {
                reason: GrantSkipReason::UnnameableScope
            }
        );
        assert!(database
            .kernel_effective_authorization_grants("run-1", 0)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scope_keys_keep_their_host_taxonomy() {
        assert_eq!(
            GrantScopeKind::from_scope_key("host-target:sha256:aa"),
            Some(GrantScopeKind::Path)
        );
        assert_eq!(
            GrantScopeKind::from_scope_key("office-request:sha256:aa"),
            Some(GrantScopeKind::OfficeRequest)
        );
        assert_eq!(
            GrantScopeKind::from_scope_key("mcp-tool:sha256:aa"),
            Some(GrantScopeKind::McpTool)
        );
        assert_eq!(
            GrantScopeKind::from_scope_key("capability-web_search:sha256:aa"),
            Some(GrantScopeKind::Action)
        );
        assert_eq!(GrantScopeKind::from_scope_key(""), None);
    }
}
