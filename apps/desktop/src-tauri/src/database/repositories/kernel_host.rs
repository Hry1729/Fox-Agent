//! Durable UI commands. Only the owning Host loop consumes these; UI callers
//! must never open a second coordinator or mutate the aggregate directly.
use super::{kernel_model_config::read_model_config, now_ms, Database};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct KernelHostScope {
    pub schema_version: u32,
    pub tool_names: BTreeSet<String>,
    /// Hashes of connection definitions, not credentials or health snapshots.
    pub mcp_server_hashes: BTreeMap<String, String>,
    pub knowledge_reference_hashes: BTreeSet<String>,
    pub knowledge_connection_hashes: BTreeMap<String, String>,
    pub office_tools: BTreeSet<String>,
    #[serde(default)]
    pub lifecycle_hooks: Vec<crate::database::LifecycleHookRecord>,
}

fn invalid(message: impl Into<String>) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message.into(),
    )))
}

fn scope_body(scope: &KernelHostScope) -> Result<String, String> {
    let valid_hash = |hash: &str| {
        hash.strip_prefix("sha256:").is_some_and(|value| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    };
    if scope.schema_version != 1
        || scope
            .mcp_server_hashes
            .iter()
            .chain(scope.knowledge_connection_hashes.iter())
            .any(|(id, hash)| id.trim().is_empty() || !valid_hash(hash))
        || scope
            .knowledge_reference_hashes
            .iter()
            .any(|hash| !valid_hash(hash))
        || scope.office_tools.iter().any(|id| id.trim().is_empty())
        || scope.lifecycle_hooks.len() > 256
        || scope.lifecycle_hooks.iter().any(|hook| {
            hook.id.trim().is_empty()
                || !hook.enabled
                || !matches!(
                    hook.event.as_str(),
                    "before_run" | "before_tool" | "after_tool" | "after_run"
                )
                || !matches!(
                    hook.action.as_str(),
                    "block" | "require_approval" | "annotate"
                )
        })
        || scope
            .lifecycle_hooks
            .iter()
            .map(|hook| &hook.id)
            .collect::<BTreeSet<_>>()
            .len()
            != scope.lifecycle_hooks.len()
        || scope
            .tool_names
            .iter()
            .any(|name| fox_engine_protocol::canonical_runtime_tool_contract(name).is_none())
    {
        return Err("invalid Kernel Host resource scope".into());
    }
    let body = crate::canonical_json::to_string(scope).map_err(|_| "invalid Kernel Host scope")?;
    if body.len() > 1_048_576 {
        return Err("Kernel Host scope exceeds size limit".into());
    }
    Ok(body)
}

fn hash(body: &str) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(body.as_bytes())))
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    #[test]
    fn host_scope_validates_all_resource_hashes() {
        let mut scope = KernelHostScope {
            schema_version: 1,
            tool_names: BTreeSet::new(),
            mcp_server_hashes: BTreeMap::from([("mcp".into(), hash("mcp"))]),
            knowledge_reference_hashes: BTreeSet::from([hash("reference")]),
            knowledge_connection_hashes: BTreeMap::from([("remote".into(), hash("endpoint"))]),
            office_tools: BTreeSet::new(),
            lifecycle_hooks: Vec::new(),
        };
        assert!(scope_body(&scope).is_ok());
        let valid = scope.clone();
        for bad in ["", "sha256:abc", "sha256:not-a-digest", "plain-id"] {
            scope.knowledge_reference_hashes = BTreeSet::from([bad.into()]);
            assert!(scope_body(&scope).is_err());
            scope = valid.clone();
            scope
                .knowledge_connection_hashes
                .insert("remote".into(), bad.into());
            assert!(scope_body(&scope).is_err());
            scope = valid.clone();
            scope.mcp_server_hashes.insert("mcp".into(), bad.into());
            assert!(scope_body(&scope).is_err());
            scope = valid.clone();
        }
        scope
            .knowledge_connection_hashes
            .insert(" ".into(), hash("endpoint"));
        assert!(scope_body(&scope).is_err());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct KernelHostCommand {
    pub seq: i64,
    pub key: String,
    pub kind: String,
    pub tool_call_id: Option<String>,
    pub decision: Option<String>,
    pub policy_version: Option<u64>,
}

/// The run's own conversation, read inside the caller's transaction so the
/// confirmation binds to the same scope the claim will enforce.
fn conversation_of(
    transaction: &rusqlite::Transaction<'_>,
    run_id: &str,
) -> Result<String, rusqlite::Error> {
    transaction
        .query_row(
            "SELECT COALESCE((SELECT conversation_id FROM run_control_bindings WHERE run_id=?1),
                             (SELECT conversation_id FROM runs WHERE id=?1))",
            [run_id],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())
        .map_err(rusqlite::Error::InvalidParameterName)
}

impl Database {

    /// Kernel already owns the execution clock (approval waits do not consume
    /// that clock). Keep the managed cumulative quotas at final resource admission.
    ///
    /// Without a tool name the strict `running` requirement applies; a caller that
    /// knows which call it is admitting must use
    /// [`Self::kernel_validate_resource_acquisition_for`].
    pub(crate) fn kernel_validate_resource_acquisition(&self, run_id: &str) -> Result<(), String> {
        self.kernel_validate_resource_acquisition_for(run_id, None)
    }

    /// Tools whose admission does not depend on some *other* call's approval.
    ///
    /// A pending approval is a decision about ONE call. While it is pending the
    /// Run is still authoritative, its frozen scope is unchanged, and none of
    /// these tools can write anywhere — so refusing them made a model's ordinary
    /// `read`/`ls` fail permanently in the middle of a live Run, with an error
    /// ("requires an active Kernel owner") it could neither act on nor retry
    /// around. A real acceptance Run lost six calls that way and then spent
    /// twenty-odd rounds exploring the filesystem and Fox's own database instead
    /// of reading the workbook it had been asked about.
    ///
    /// Write/command/delegation and generic MCP tools keep the strict requirement.
    /// Some listed readers can access the network or persist Host bookkeeping;
    /// they still require their own frozen policy decision and dispatch lease.
    /// This list permits unrelated admitted reads to proceed, never authorizes
    /// a call or replaces the gateway scope/approval checks.
    const APPROVAL_INDEPENDENT_TOOLS: &[&str] = &[
        "read",
        "ls",
        "find",
        "grep",
        "read_attachment",
        "read_tool_result",
        "sqlite_read",
        "structured_data",
        "tabular_data",
        "git_read",
        "memory_search",
        "list_knowledge_bases",
        "search_knowledge",
        "read_knowledge_document",
        "query_knowledge_graph",
        "list_mcp_tools",
        "web_search",
        "web_read",
        "work_snapshot_get",
        "skill_load",
    ];

    pub(crate) fn approval_independent(tool: Option<&str>) -> bool {
        tool.is_some_and(|tool| Self::APPROVAL_INDEPENDENT_TOOLS.contains(&tool))
    }

    /// Whether one resource call may be admitted for a Run in `state`.
    ///
    /// Pure, so the policy can be asserted without a database fixture: the
    /// database method below is a lookup plus this decision.
    fn resource_admission_allowed(state: Option<&str>, tool: Option<&str>) -> bool {
        match state {
            Some("running") => true,
            // A pending approval decides one call; an independent read is still
            // admitted. Every other state (terminal, cancelled, unknown, no
            // authoritative binding) keeps the strict refusal.
            Some("waiting_approval") => Self::approval_independent(tool),
            _ => false,
        }
    }

    /// Admit one resource call for `run_id`, naming the tool it belongs to.
    pub(crate) fn kernel_validate_resource_acquisition_for(
        &self,
        run_id: &str,
        tool: Option<&str>,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let state: Option<String> = tx
                .query_row(
                    "SELECT k.state FROM run_control_bindings b
                       JOIN kernel_runs k ON k.run_id=b.run_id
                      WHERE b.run_id=?1 AND b.authority='authoritative'",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()?;
            if !Self::resource_admission_allowed(state.as_deref(), tool) {
                return Err(invalid("resource admission requires an active Kernel owner"));
            }
            let exhausted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM child_run_delegations d
                WHERE d.child_run_id=?1 AND (d.total_tokens>=d.max_total_tokens OR d.output_tokens>=d.max_output_tokens
                OR (SELECT COUNT(*) FROM tool_calls t WHERE t.run_id=d.child_run_id)>d.max_tool_calls))",
                [run_id], |row| row.get(0))?;
            if exhausted { return Err(invalid("Kernel child cumulative budget exhausted")); }
            super::validate_managed_tool_acquisition_with_clock(&tx,run_id,now_ms(),false,false)?;
            tx.commit()
        })
    }

    /// Routing only, never sufficient authorization to dispatch an effect.
    pub(crate) fn kernel_host_run_state(&self, run_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            connection
                .query_row(
                    "SELECT state FROM kernel_runs WHERE run_id=?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()
        })
    }

    /// F5: a native approval never belongs in the Kernel queue. Retire any
    /// command written by an older build, but leave the approval itself
    /// pending so the user can decide it again through the native route.
    pub(crate) fn supersede_pending_native_approval_commands(&self, now: i64) -> Result<usize, String> {
        self.with_connection(|connection| connection.execute(
            "UPDATE kernel_host_commands
             SET status='superseded',completed_at=?1,
                 resolution_reason='executor route is pi_native; the decision belongs to the native approval'
             WHERE status='pending' AND kind='approval'
               AND EXISTS(SELECT 1 FROM run_control_bindings b
                          WHERE b.run_id=kernel_host_commands.run_id
                            AND json_extract(b.binding_json,'$.executorRoute')='pi_native')",
            [now],
        ))
    }

    pub(crate) fn approval_run_id(&self, approval_id: &str) -> Result<Option<String>, String> {
        self.with_connection(|connection| connection.query_row(
            "SELECT t.run_id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE a.id=?1",
            [approval_id], |row| row.get(0)).optional())
    }

    pub(crate) fn kernel_host_approval_target(
        &self,
        approval_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        // A native approval is not a Kernel ticket. The route column is the
        // same frozen fact the runtime uses; an absent executorRoute is the
        // only shape the Kernel command queue understands.
        self.with_connection(|connection| connection.query_row(
            "SELECT t.run_id,t.runtime_tool_call_id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
             JOIN run_control_bindings b ON b.run_id=t.run_id WHERE a.id=?1 AND b.authority='authoritative'
               AND json_extract(b.binding_json,'$.executorRoute') IS NULL",
            [approval_id], |row| Ok((row.get(0)?,row.get(1)?))).optional())
    }

    pub(crate) fn freeze_kernel_host_scope(
        &self,
        run_id: &str,
        scope: &KernelHostScope,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let tx =
                connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            freeze_host_scope_in_tx(&tx, run_id, scope)?;
            tx.commit()
        })
    }

    pub(crate) fn kernel_host_scope(&self, run_id: &str) -> Result<KernelHostScope, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let (body, stored_hash): (String, String) = tx.query_row(
                "SELECT scope_json,scope_hash FROM kernel_host_runs WHERE run_id=?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if body.len() > 1_048_576 || hash(&body) != stored_hash {
                return Err(invalid("invalid Kernel Host scope hash"));
            }
            let scope: KernelHostScope = serde_json::from_str(&body)
                .map_err(|_| invalid("invalid stored Kernel Host scope"))?;
            scope_body(&scope).map_err(invalid)?;
            let model = read_model_config(&tx, run_id)?;
            let names: BTreeSet<String> = model
                .proposal_tools
                .iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
                .collect();
            if names != scope.tool_names {
                return Err(invalid("stored Kernel Host catalog mismatch"));
            }
            tx.commit()?;
            Ok(scope)
        })
    }

    pub(crate) fn kernel_host_recoverable_runs(&self) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut query = connection.prepare("SELECT b.run_id FROM run_control_bindings b JOIN runs legacy ON legacy.id=b.run_id
                LEFT JOIN kernel_runs r ON r.run_id=b.run_id WHERE b.authority='authoritative'
                AND legacy.status NOT IN ('completed','failed','cancelled','interrupted')
                AND (r.state IS NULL OR r.state NOT IN ('completed','failed','cancelled','budget_exhausted','approval_expired')) ORDER BY b.created_at,b.run_id")?;
            let rows = query.query_map([], |row| row.get(0))?.collect();
            rows
        })
    }

    /// Stable keys prevent double-click/reconnect from scheduling a second action.
    /// An approval key is derived from its tool id, so conflicting decisions fail.
    pub(crate) fn queue_kernel_host_command(
        &self,
        run_id: &str,
        approval: Option<(&str, &str)>,
    ) -> Result<bool, String> {
        self.queue_kernel_host_command_versioned(run_id, approval, None)
    }

    fn queue_kernel_host_command_versioned(
        &self,
        run_id: &str,
        approval: Option<(&str, &str)>,
        ticket_version: Option<u64>,
    ) -> Result<bool, String> {
        let (mut key, kind, tool, decision) = if let Some((tool, decision)) = approval {
            if tool.trim().is_empty()
                || tool.len() > 512
                || !matches!(decision, "allow_once" | "allow_conversation" | "denied")
            {
                return Err("invalid Kernel Host approval command".into());
            }
            (
                format!("approval:{tool}"),
                "approval",
                Some(tool),
                Some(decision),
            )
        } else {
            ("cancel".into(), "cancel", None, None)
        };
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let policy_version:Option<u64>=if let Some(tool)=tool {
                let stored:Option<u64>=tx.query_row("SELECT policy_version FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2",params![run_id,tool],|r|r.get(0)).optional()?.flatten();
                let current:Option<u64>=tx.query_row("SELECT p.version FROM kernel_execution_policies p JOIN runs r ON r.conversation_id=p.conversation_id WHERE r.id=?1",[run_id],|r|r.get(0)).optional()?;
                if stored.is_none() || stored!=current || ticket_version.is_some_and(|version|Some(version)!=stored) {return Err(invalid("stale_approval: ticket policy version is not current"));}
                let reissued:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_approval_history WHERE run_id=?1 AND tool_call_id=?2)",params![run_id,tool],|r|r.get(0))?;
                if reissued && ticket_version.is_none() {return Err(invalid("stale_approval: reissued ticket requires its approval id"));}
                key=format!("approval:{tool}:v{}",stored.unwrap_or_default());
                stored
            } else {None};
            let existing: Option<(String,Option<String>,Option<String>)> = tx.query_row(
                "SELECT kind,tool_call_id,decision FROM kernel_host_commands WHERE run_id=?1 AND command_key=?2",
                params![run_id,key], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            if let Some((old_kind,old_tool,old_decision)) = existing {
                if old_kind != kind || old_tool.as_deref() != tool || old_decision.as_deref() != decision {
                    return Err(invalid("conflicting Kernel Host command replay"));
                }
                return Ok(false);
            }
            let state: Option<(String,Option<i64>)> = tx.query_row(
                "SELECT COALESCE(r.state,legacy.status),r.approval_deadline_wall_ms FROM run_control_bindings b
                 JOIN runs legacy ON legacy.id=b.run_id LEFT JOIN kernel_runs r ON r.run_id=b.run_id
                 WHERE b.run_id=?1 AND b.authority='authoritative'",
                [run_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            let (state,deadline) = state.ok_or_else(|| invalid("Run is not owned by Kernel Host"))?;
            if matches!(state.as_str(), "completed" | "failed" | "cancelled" | "budget_exhausted" | "approval_expired") {
                return Err(invalid("Kernel Host Run is terminal"));
            }
            let now = now_ms();
            if let Some(tool) = tool {
                let repair_override: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_tool_calls
                    WHERE run_id=?1 AND tool_call_id=?2 AND tool='task_repair_escalate_start')",
                    params![run_id,tool],|row|row.get(0))?;
                if repair_override && decision==Some("allow_conversation") {
                    return Err(invalid("Task Repair budget override requires a one-time human decision"));
                }
                let cancelled: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_host_commands WHERE run_id=?1 AND kind='cancel')",
                    [run_id], |row| row.get(0))?;
                let pending: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_approvals WHERE run_id=?1 AND tool_call_id=?2 AND state='pending')",
                    params![run_id,tool], |row| row.get(0))?;
                // Approvals do not expire: the persisted anchor only has to exist.
                if cancelled || state != "waiting_approval" || !pending || deadline.is_none() {
                    return Err(invalid("Kernel approval is no longer actionable"));
                }
            }
            tx.execute("INSERT INTO kernel_host_commands(run_id,command_key,kind,tool_call_id,decision,created_at,policy_version) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![run_id,key,kind,tool,decision,now,policy_version])?;
            tx.commit()?;
            Ok(true)
        })
    }

    pub(crate) fn pending_kernel_host_commands(
        &self,
        run_id: &str,
    ) -> Result<Vec<KernelHostCommand>, String> {
        self.with_connection(|connection| {
            let mut query = connection.prepare("SELECT command_seq,command_key,kind,tool_call_id,decision,policy_version FROM kernel_host_commands
                WHERE run_id=?1 AND status='pending' ORDER BY CASE kind WHEN 'cancel' THEN 0 ELSE 1 END,command_seq")?;
            let rows = query.query_map([run_id], |row| Ok(KernelHostCommand {
                seq: row.get(0)?, key: row.get(1)?, kind: row.get(2)?, tool_call_id: row.get(3)?, decision: row.get(4)?, policy_version: row.get(5)?,
            }))?.collect();
            rows
        })
    }

}

pub(super) fn freeze_host_scope_in_tx(
    tx: &rusqlite::Transaction<'_>,
    run_id: &str,
    scope: &KernelHostScope,
) -> rusqlite::Result<()> {
    let body = scope_body(scope).map_err(invalid)?;
    let model = read_model_config(&tx, run_id)?;
    let names: BTreeSet<String> = model
        .proposal_tools
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect();
    if names != scope.tool_names {
        return Err(invalid("Kernel scope differs from model tool catalog"));
    }
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT scope_json,scope_hash FROM kernel_host_runs WHERE run_id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((stored, stored_hash)) = existing {
        if stored != body || stored_hash != hash(&body) {
            return Err(invalid("immutable Kernel Host scope conflict"));
        }
    } else {
        tx.execute("INSERT INTO kernel_host_runs(run_id,scope_json,scope_hash,created_at) VALUES(?1,?2,?3,?4)",
                    params![run_id,body,hash(&body),now_ms()])?;
    }
    Ok(())
}

#[cfg(test)]
mod resource_admission_tests {
    use super::Database;

    /// A pending approval decides ONE call. Refusing every other read made a
    /// model's ordinary `read`/`ls` fail permanently mid-Run with an error it
    /// could neither act on nor retry around: a real acceptance Run lost six
    /// calls that way and then spent twenty-odd rounds exploring the filesystem
    /// and Fox's own database instead of reading the workbook it was asked about.
    #[test]
    fn a_pending_approval_admits_independent_reads_but_never_effects() {
        let allowed = |state: Option<&'static str>, tool: Option<&'static str>| {
            Database::resource_admission_allowed(state, tool)
        };
        // Executing: unchanged, everything the frozen scope allows.
        assert!(allowed(Some("running"), None));
        assert!(allowed(Some("running"), Some("read")));
        assert!(allowed(Some("running"), Some("write_file")));
        // Waiting for approval: independent reads only.
        for tool in [
            "read",
            "ls",
            "find",
            "grep",
            "read_attachment",
            "read_tool_result",
            "sqlite_read",
            "tabular_data",
            "structured_data",
            "git_read",
            "memory_search",
            "list_knowledge_bases",
            "search_knowledge",
            "read_knowledge_document",
            "query_knowledge_graph",
            "list_mcp_tools",
            "web_search",
            "web_read",
            "work_snapshot_get",
            "skill_load",
        ] {
            assert!(
                allowed(Some("waiting_approval"), Some(tool)),
                "`{tool}` must be admitted while an unrelated approval is pending"
            );
        }
        // Anything that can write, run, delegate or reach the network keeps the
        // strict requirement: those are what the approval is deciding about.
        for tool in [
            "write_file",
            "edit_file",
            "run_command",
            "call_mcp_tool",
            "attachment_compute",
            "compute_job_start",
            "http_request",
            "memory_propose",
            "test_run",
            "format_code",
            "code_check",
            "child_run_start",
            "workflow_start",
            "office_create",
        ] {
            assert!(
                !allowed(Some("waiting_approval"), Some(tool)),
                "`{tool}` must stay refused while an approval is pending"
            );
        }
        // An unnamed admission is the strict one, and every terminal or unknown
        // state stays refused even for a read.
        assert!(!allowed(Some("waiting_approval"), None));
        for state in [
            None,
            Some("completed"),
            Some("failed"),
            Some("cancelled"),
            Some("budget_exhausted"),
            Some("approval_expired"),
        ] {
            assert!(
                !allowed(state, Some("read")),
                "{state:?} must stay refused even for a read"
            );
        }
    }
}
