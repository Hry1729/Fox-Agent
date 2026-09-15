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
    let body = serde_json::to_string(scope).map_err(|_| "invalid Kernel Host scope")?;
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
}

impl Database {
    pub(crate) fn pending_kernel_host_action_ids(
        &self,
        run_id: &str,
    ) -> Result<Vec<String>, String> {
        self.with_connection(|connection| {
            let mut query = connection.prepare("SELECT a.tool_call_id FROM kernel_host_actions a
                JOIN kernel_tool_calls t ON t.run_id=a.run_id AND t.tool_call_id=a.tool_call_id
                WHERE a.run_id=?1 AND a.status!='completed' AND t.state='completed' ORDER BY a.created_at,a.tool_call_id")?;
            let rows = query.query_map([run_id],|row|row.get(0))?.collect();
            rows
        })
    }

    /// Staging is permitted only inside a leased executor. The owner must not
    /// perform the action until a successful Kernel tool result is committed.
    pub(crate) fn stage_kernel_host_action(
        &self,
        run_id: &str,
        tool_id: &str,
        body: &str,
    ) -> Result<(), String> {
        let _: serde_json::Value =
            serde_json::from_str(body).map_err(|_| "invalid Kernel Host action")?;
        if body.len() > 1_048_576 {
            return Err("Kernel Host action exceeds size limit".into());
        }
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let admitted: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM kernel_tool_calls t
                JOIN kernel_runs r ON r.run_id=t.run_id JOIN run_control_bindings b ON b.run_id=r.run_id
                JOIN kernel_effect_outbox e ON e.run_id=t.run_id AND e.tool_call_id=t.tool_call_id
                WHERE t.run_id=?1 AND t.tool_call_id=?2 AND t.state='running' AND r.state='running'
                  AND b.authority='authoritative' AND e.effect_type='dispatch_tool' AND e.status='leased')",
                params![run_id,tool_id],|row|row.get(0))?;
            if !admitted { return Err(invalid("Host action requires a leased Kernel tool executor")); }
            tx.execute("INSERT INTO kernel_host_actions(run_id,tool_call_id,body_json,body_hash,created_at)
                VALUES(?1,?2,?3,?4,?5) ON CONFLICT(run_id,tool_call_id) DO NOTHING",
                params![run_id,tool_id,body,hash(body),now_ms()])?;
            let stored: String = tx.query_row("SELECT body_json FROM kernel_host_actions WHERE run_id=?1 AND tool_call_id=?2",
                params![run_id,tool_id],|row|row.get(0))?;
            if stored!=body { return Err(invalid("Kernel Host action changed on replay")); }
            tx.commit()
        })
    }

    /// CAS after tool-result commit. A claimed action is uncertain after a crash
    /// and is never automatically dispatched a second time.
    pub(crate) fn claim_kernel_host_action(
        &self,
        run_id: &str,
        tool_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let row: Option<(String,String,String)> = tx.query_row("SELECT body_json,body_hash,status FROM kernel_host_actions
                WHERE run_id=?1 AND tool_call_id=?2",params![run_id,tool_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
            let Some((body,digest,status)) = row else { return Ok(None); };
            if hash(&body)!=digest { return Err(invalid("Kernel Host action hash mismatch")); }
            if status=="completed" { return Ok(None); }
            if status=="claimed" { return Err(invalid("Kernel Host action execution is uncertain; automatic replay is forbidden")); }
            let changed = tx.execute("UPDATE kernel_host_actions SET status='claimed' WHERE run_id=?1 AND tool_call_id=?2
                AND status='pending' AND EXISTS(SELECT 1 FROM kernel_tool_calls t JOIN kernel_runs r ON r.run_id=t.run_id
                    WHERE t.run_id=?1 AND t.tool_call_id=?2 AND t.state='completed' AND r.state='running')
                AND NOT EXISTS(SELECT 1 FROM kernel_host_commands c WHERE c.run_id=?1 AND c.kind='cancel' AND c.status='pending')",
                params![run_id,tool_id])?;
            tx.commit()?;
            Ok((changed==1).then_some(body))
        })
    }

    pub(crate) fn complete_kernel_host_action(
        &self,
        run_id: &str,
        tool_id: &str,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            if connection.execute(
                "UPDATE kernel_host_actions SET status='completed',completed_at=?3
                WHERE run_id=?1 AND tool_call_id=?2 AND status='claimed'",
                params![run_id, tool_id, now_ms()],
            )? != 1
            {
                return Err(invalid("Kernel Host action was not claimed"));
            }
            Ok(())
        })
    }

    /// Kernel already owns the execution clock (approval waits do not consume
    /// that clock). Keep the managed cumulative quotas at final resource admission.
    pub(crate) fn kernel_validate_resource_acquisition(&self, run_id: &str) -> Result<(), String> {
        self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let owned: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM run_control_bindings b
                JOIN kernel_runs k ON k.run_id=b.run_id WHERE b.run_id=?1 AND b.authority='authoritative'
                AND k.state='running')",[run_id],|row|row.get(0))?;
            if !owned { return Err(invalid("resource admission requires an active Kernel owner")); }
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

    pub(crate) fn kernel_prepared_failure_config(
        &self,
        run_id: &str,
    ) -> Result<(crate::kernel::RunFrozenConfig, String), String> {
        self.with_connection(|connection| {
            let (body,turn): (String,Option<String>) = connection.query_row(
                "SELECT r.frozen_config_json,i.turn_id FROM kernel_runs r LEFT JOIN kernel_initial_inputs i ON i.run_id=r.run_id
                 WHERE r.run_id=?1 AND r.state='created' AND r.last_event_seq=0", [run_id], |row| Ok((row.get(0)?,row.get(1)?)))?;
            let config = serde_json::from_str(&body).map_err(|_| invalid("invalid prepared Kernel configuration"))?;
            Ok((config,turn.unwrap_or_else(|| format!("kernel-preparation:{run_id}"))))
        })
    }

    /// Preparation failed before an aggregate existed. This cannot terminate an
    /// executing Kernel or change its authority; active failures use its owner.
    pub(crate) fn kernel_fail_before_aggregate(&self, run_id: &str) -> Result<bool, String> {
        let changed = self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let now = now_ms();
            let changed = tx.execute(
            "UPDATE runs SET status=CASE WHEN EXISTS(SELECT 1 FROM kernel_host_commands c WHERE c.run_id=?1 AND c.kind='cancel')
                THEN 'cancelled' ELSE 'failed' END,finished_at=?2,error_code='kernel.preparation_failed',
                error_message='Kernel preparation did not complete; no model or resource execution was started.'
             WHERE id=?1 AND status IN ('queued','running')
                AND EXISTS(SELECT 1 FROM run_control_bindings b WHERE b.run_id=?1 AND b.authority='authoritative')
                AND NOT EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1)", params![run_id,now])?;
            if changed != 0 {
                let status: String = tx.query_row("SELECT status FROM runs WHERE id=?1",[run_id],|row|row.get(0))?;
                let kind = if status=="cancelled" { "run.cancelled" } else { "run.failed" };
                let payload = serde_json::json!({"code":"kernel.preparation_failed","message":"Kernel preparation did not complete."});
                super::child_runs::project_child_run_event(&tx,run_id,kind,&payload,now)?;
                super::digital_colleagues::project_digital_colleague_event(&tx,run_id,kind,&payload,now)?;
            }
            tx.commit()?;
            Ok(changed)
        })?;
        if changed != 0 {
            self.kernel_changes.committed();
        }
        Ok(changed != 0)
    }

    /// The parent owner holds the child's OS lock. A child created by a work
    /// transaction but never given an execution binding must not survive as
    /// queued after failed staging or interrupted preparation. This cannot
    /// mutate a started child or any Legacy parent's child.
    pub(crate) fn kernel_fail_unstarted_child(
        &self,
        parent_id: &str,
        child_id: &str,
    ) -> Result<bool, String> {
        self.kernel_settle_unstarted_child(parent_id, child_id, false)
    }

    pub(crate) fn kernel_cancel_unstarted_child(
        &self,
        parent_id: &str,
        child_id: &str,
    ) -> Result<bool, String> {
        self.kernel_settle_unstarted_child(parent_id, child_id, true)
    }

    fn kernel_settle_unstarted_child(
        &self,
        parent_id: &str,
        child_id: &str,
        cancelled: bool,
    ) -> Result<bool, String> {
        let changed = self.with_connection(|connection| {
            let tx = connection.transaction()?;
            let now = now_ms();
            let changed = tx.execute("UPDATE runs SET status=CASE WHEN ?4 THEN 'cancelled' ELSE 'failed' END,finished_at=?3,
                error_code='kernel.child_not_dispatched',error_message='Child preparation was interrupted; execution was not replayed.'
                WHERE id=?2 AND parent_run_id=?1 AND status='queued'
                  AND EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?1 AND authority='authoritative')
                  AND NOT EXISTS(SELECT 1 FROM run_control_bindings WHERE run_id=?2)
                  AND NOT EXISTS(SELECT 1 FROM kernel_runs WHERE run_id=?2)",params![parent_id,child_id,now,cancelled])?;
            if changed != 0 {
                let payload = serde_json::json!({"code":"kernel.child_not_dispatched","message":"Child preparation was interrupted; execution was not replayed."});
                super::child_runs::project_child_run_event(&tx,child_id,if cancelled {"run.cancelled"} else {"run.failed"},&payload,now)?;
            }
            tx.commit()?;
            Ok(changed != 0)
        })?;
        if changed {
            self.kernel_changes.committed();
        }
        Ok(changed)
    }

    pub(crate) fn kernel_host_approval_target(
        &self,
        approval_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        self.with_connection(|connection| connection.query_row(
            "SELECT t.run_id,t.runtime_tool_call_id FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id
             JOIN run_control_bindings b ON b.run_id=t.run_id WHERE a.id=?1 AND b.authority='authoritative'",
            [approval_id], |row| Ok((row.get(0)?,row.get(1)?))).optional())
    }

    pub(crate) fn freeze_kernel_host_scope(
        &self,
        run_id: &str,
        scope: &KernelHostScope,
    ) -> Result<(), String> {
        let body = scope_body(scope)?;
        self.with_connection(|connection| {
            let tx = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let model = read_model_config(&tx, run_id)?;
            let names: BTreeSet<String> = model.proposal_tools.iter()
                .filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect();
            if names != scope.tool_names { return Err(invalid("Kernel scope differs from model tool catalog")); }
            let existing: Option<(String,String)> = tx.query_row(
                "SELECT scope_json,scope_hash FROM kernel_host_runs WHERE run_id=?1", [run_id],
                |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
            if let Some((stored, stored_hash)) = existing {
                if stored != body || stored_hash != hash(&body) { return Err(invalid("immutable Kernel Host scope conflict")); }
            } else {
                tx.execute("INSERT INTO kernel_host_runs(run_id,scope_json,scope_hash,created_at) VALUES(?1,?2,?3,?4)",
                    params![run_id,body,hash(&body),now_ms()])?;
            }
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
                AND (r.state IS NULL OR r.state NOT IN ('completed','failed','cancelled','budget_exhausted')) ORDER BY b.created_at,b.run_id")?;
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
        let (key, kind, tool, decision) = if let Some((tool, decision)) = approval {
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
            if matches!(state.as_str(), "completed" | "failed" | "cancelled" | "budget_exhausted") {
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
                if cancelled || state != "waiting_approval" || !pending || !deadline.is_some_and(|value| value > now) {
                    return Err(invalid("Kernel approval is no longer actionable"));
                }
            }
            tx.execute("INSERT INTO kernel_host_commands(run_id,command_key,kind,tool_call_id,decision,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![run_id,key,kind,tool,decision,now])?;
            tx.commit()?;
            Ok(true)
        })
    }

    pub(crate) fn pending_kernel_host_commands(
        &self,
        run_id: &str,
    ) -> Result<Vec<KernelHostCommand>, String> {
        self.with_connection(|connection| {
            let mut query = connection.prepare("SELECT command_seq,command_key,kind,tool_call_id,decision FROM kernel_host_commands
                WHERE run_id=?1 AND status='pending' ORDER BY CASE kind WHEN 'cancel' THEN 0 ELSE 1 END,command_seq")?;
            let rows = query.query_map([run_id], |row| Ok(KernelHostCommand {
                seq: row.get(0)?, key: row.get(1)?, kind: row.get(2)?, tool_call_id: row.get(3)?, decision: row.get(4)?,
            }))?.collect();
            rows
        })
    }

    /// Called after the corresponding durable Kernel transition. On recovery,
    /// the owner first observes the stored decision/terminal before acknowledging.
    pub(crate) fn complete_kernel_host_command(
        &self,
        run_id: &str,
        seq: i64,
    ) -> Result<(), String> {
        self.with_connection(|connection| {
            let changed = connection.execute("UPDATE kernel_host_commands SET status='completed',completed_at=?3
                WHERE run_id=?1 AND command_seq=?2 AND (
                    EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.state IN ('completed','failed','cancelled','budget_exhausted'))
                    OR (kind='cancel' AND EXISTS(SELECT 1 FROM kernel_runs r WHERE r.run_id=?1 AND r.state='cancelling'))
                    OR (kind='approval' AND EXISTS(SELECT 1 FROM kernel_approvals a WHERE a.run_id=?1
                        AND a.tool_call_id=kernel_host_commands.tool_call_id
                        AND (a.state=kernel_host_commands.decision OR a.state IN ('expired','cancelled')))))",
                params![run_id,seq,now_ms()])?;
            if changed != 1 { return Err(invalid("Kernel command has no durable matching outcome")); }
            Ok(())
        })
    }
}
