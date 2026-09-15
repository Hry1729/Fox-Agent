//! Durable per-Run control bindings. Reopening or changing process settings can
//! never silently change a Run's authority, engine, permission or executor.
use super::{now_ms, Database};
use fox_engine_protocol::{ExecutionAuthority, RunControlBinding};
use rusqlite::{params, OptionalExtension};
use sha2::{Digest, Sha256};

fn content_hash(json: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(json.as_bytes()))) }

fn encode(binding: &RunControlBinding) -> Result<String, String> {
    binding.validate()?;
    let permission = serde_json::to_string(&binding.permission).map_err(|e| e.to_string())?;
    if binding.permission_snapshot_id != content_hash(&permission) {
        return Err("frozen permission hash does not match its content".into());
    }
    serde_json::to_string(binding).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fox_engine_protocol::{PermissionMode, ResourceExecutor};

    fn fixture() -> (Database, std::path::PathBuf, String) {
        let path = std::env::temp_dir().join(format!("fox-control-{}.db", uuid::Uuid::new_v4()));
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("control test"), None, None).unwrap();
        db.update_conversation_permission_mode(&conversation.id, "ask").unwrap();
        let run = db.create_run(&conversation.id, "test frozen control", None).unwrap();
        (db, path, run.run.id)
    }

    #[test]
    fn approval_window_survives_reopen_and_rejects_late_grants() {
        use crate::database::ApprovalDecision;
        let (db, path, run_id) = fixture();
        db.apply_runtime_event(&run_id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        let binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        let tool = db.create_host_tool_call(&run_id, "deadline-tool", "write_file", &serde_json::json!({"path":"test.txt"}), "pending", true).unwrap();
        let approval = db.create_approval(&tool.id, "write", &serde_json::json!({"permissionScope":"project"})).unwrap();
        let start = approval.requested_at;
        let budget = binding.budgets.approval_wait_ms;
        assert_eq!(db.run_approval_wait_remaining_ms(&run_id, &approval.id, start + 17).unwrap(), (budget - 17) as u64);
        assert_eq!(db.run_approval_wait_remaining_ms(&run_id, &approval.id, start + budget).unwrap(), 0);
        assert!(db.run_approval_wait_remaining_ms(&run_id, &approval.id, start - 1).is_err());
        assert!(db.run_approval_window("foreign", &approval.id).is_err());
        drop(db);
        let db = Database::open(path).unwrap();
        assert_eq!(db.run_approval_wait_remaining_ms(&run_id, &approval.id, start + 31).unwrap(), (budget - 31) as u64);
        db.with_connection(|connection| connection.execute("UPDATE approvals SET requested_at=1 WHERE id=?1", [&approval.id])).unwrap();
        let expired = db.resolve_approval(&approval.id, ApprovalDecision::AllowConversation).unwrap().unwrap();
        assert_eq!(expired.status, "expired");
        assert_eq!(expired.decision.unwrap()["approved"], false);
        assert!(!db.conversation_tool_permission_granted(&binding.conversation_id, "write_file", "project").unwrap());
        assert!(!db.claim_approved_tool_call(&run_id, "deadline-tool").unwrap());
        assert!(db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap().is_none());
    }

    #[test]
    fn approval_clock_rollback_expires_and_corrupt_binding_still_allows_denial() {
        use crate::database::ApprovalDecision;
        let (db, _path, run_id) = fixture();
        db.apply_runtime_event(&run_id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        for (call_id, corrupt) in [("future", false), ("corrupt", true)] {
            let tool = db.create_host_tool_call(&run_id, call_id, "write_file", &serde_json::json!({}), "pending", true).unwrap();
            let approval = db.create_approval(&tool.id, "write", &serde_json::json!({"permissionScope":"project"})).unwrap();
            if corrupt {
                db.with_connection(|connection| connection.execute("UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1", [&run_id])).unwrap();
                assert!(db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).is_err());
                assert_eq!(db.resolve_approval(&approval.id, ApprovalDecision::Deny).unwrap().unwrap().status, "denied");
            } else {
                db.with_connection(|connection| connection.execute("UPDATE approvals SET requested_at=?2 WHERE id=?1", params![approval.id, now_ms() + 60_000])).unwrap();
                assert_eq!(db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap().unwrap().status, "expired");
            }
        }
    }

    #[test]
    fn run_control_shadow_projection_preserves_grants_and_legacy_compatibility() {
        let (db, _path, run_id) = fixture();
        let conversation: String = db.with_connection(|connection| connection.query_row(
            "SELECT conversation_id FROM runs WHERE id=?1", [&run_id], |row| row.get(0),
        )).unwrap();
        db.with_connection(|connection| connection.execute(
            "INSERT INTO conversation_tool_permissions(conversation_id,tool_name,scope_key,granted_at) VALUES(?1,'read','project',0)",
            [&conversation],
        )).unwrap();
        let legacy = db.run_shadow_permission_snapshot(&run_id, &conversation).unwrap();
        assert!(db.run_shadow_permission_snapshot(&run_id, "foreign").is_err());
        assert!(db.run_shadow_permission_snapshot("missing", &conversation).is_err());
        db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        db.with_connection(|connection| connection.execute(
            "DELETE FROM conversation_tool_permissions WHERE conversation_id=?1", [&conversation],
        )).unwrap();
        assert_eq!(db.run_shadow_permission_snapshot(&run_id, &conversation).unwrap(), legacy);
        assert_eq!(legacy["grants"], serde_json::json!([["read", "project"]]));
    }

    #[test]
    fn run_control_survives_reopen_and_ignores_live_permission_changes() {
        let (db, path, run_id) = fixture();
        let binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        assert_eq!(binding.authority, ExecutionAuthority::Legacy);
        assert_eq!(binding.read_only_executor, ResourceExecutor::Runtime);
        assert_eq!(binding.permission.mode, PermissionMode::Ask);
        db.update_conversation_permission_mode(&binding.conversation_id, "allow").unwrap();
        let shadow = db.run_shadow_permission_snapshot(&run_id, &binding.conversation_id).unwrap();
        assert_eq!(shadow["mode"], "ask");
        assert_eq!(shadow["grants"], serde_json::json!([]));
        assert!(db.run_shadow_permission_snapshot(&run_id, "foreign-conversation").is_err());
        assert_eq!(db.freeze_legacy_run_control(&run_id, "test-profile").unwrap(), binding);
        assert!(db.freeze_legacy_run_control(&run_id, "different-profile").is_err());
        drop(db);
        let reopened = Database::open(path).unwrap();
        assert_eq!(reopened.run_control_binding(&run_id).unwrap(), Some(binding));
    }

    #[test]
    fn run_control_is_immutable_and_rejects_tampering() {
        let (db, _path, run_id) = fixture();
        let binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        db.freeze_run_control(&binding).unwrap();
        let mut changed = binding.clone();
        changed.authority = ExecutionAuthority::Authoritative;
        assert!(db.freeze_run_control(&changed).is_err());
        changed = binding.clone();
        changed.permission.mode = PermissionMode::Allow;
        assert!(db.freeze_run_control(&changed).unwrap_err().contains("permission hash"));
        db.with_connection(|connection| connection.execute("UPDATE run_control_bindings SET engine_id='codex' WHERE run_id=?1", [&run_id])).unwrap();
        assert!(db.run_control_binding(&run_id).unwrap_err().contains("columns disagree"));
        db.with_connection(|connection| connection.execute("UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1", [&run_id])).unwrap();
        assert!(db.run_control_binding(&run_id).unwrap_err().contains("hash mismatch"));
        assert!(db.freeze_legacy_run_control(&run_id, "test-profile").is_err());
        assert!(db.run_shadow_permission_snapshot(&run_id, &binding.conversation_id).is_err());
    }

    #[test]
    fn run_control_rejects_foreign_conversation_and_unknown_run() {
        let (db, _path, run_id) = fixture();
        let mut binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        binding.conversation_id = "another-conversation".into();
        assert!(db.freeze_run_control(&binding).is_err());
        assert!(db.freeze_legacy_run_control("missing-run", "test-profile").is_err());
        assert_eq!(db.run_control_binding("missing-run").unwrap(), None);
    }

    #[test]
    fn kernel_startup_freezes_permissions_budgets_and_refuses_legacy_takeover() {
        let (db, path, run_id) = fixture();
        let mut budgets = fox_engine_protocol::TimeBudgets::default();
        budgets.run_execution_ms = 60_000;
        let binding = db.freeze_kernel_run_control(&run_id, "legacy", budgets.clone()).unwrap();
        assert_eq!(binding.authority, ExecutionAuthority::Authoritative);
        assert_eq!(binding.read_only_executor, ResourceExecutor::Rust);
        assert_eq!(binding.budgets, budgets);
        assert!(db.freeze_legacy_run_control(&run_id, "legacy").is_err());
        db.update_conversation_permission_mode(&binding.conversation_id, "allow").unwrap();
        drop(db);
        let db = Database::open(path).unwrap();
        assert_eq!(db.freeze_kernel_run_control(&run_id, "legacy", fox_engine_protocol::TimeBudgets::default()).unwrap(), binding);
    }

    #[test]
    fn rollback_new_runs_to_legacy_preserves_frozen_authoritative_runs() {
        let (db, path, original_id) = fixture();
        let original = db.freeze_kernel_run_control(&original_id, "legacy", Default::default()).unwrap();
        drop(db);
        // Reopen the same database as a process started after rollback would.
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("after rollback"), None, None).unwrap();
        let new_id = db.create_run(&conversation.id, "new legacy task", None).unwrap().run.id;
        let new_binding = db.freeze_legacy_run_control(&new_id, "legacy").unwrap();
        assert_eq!(new_binding.authority, ExecutionAuthority::Legacy);
        assert_eq!(db.run_control_binding(&original_id).unwrap(), Some(original.clone()));
        assert!(db.freeze_legacy_run_control(&original_id, "legacy").is_err());
        assert!(db.freeze_kernel_run_control(&new_id, "legacy", Default::default()).is_err());
        drop(db);
        let db = Database::open(path).unwrap();
        assert_eq!(db.run_control_binding(&original_id).unwrap(), Some(original));
        assert_eq!(db.run_control_binding(&new_id).unwrap(), Some(new_binding));
    }
}

impl Database {
    /// Absolute approval window survives re-entry and process restarts.
    pub fn run_approval_window(&self, run_id: &str, approval_id: &str) -> Result<(i64, i64), String> {
        let budget = self.run_time_budgets(run_id)?.approval_wait_ms;
        let requested_at: i64 = self.with_connection(|connection| connection.query_row(
            "SELECT a.requested_at FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE a.id=?1 AND t.run_id=?2",
            params![approval_id, run_id], |row| row.get(0),
        ))?;
        if requested_at < 0 || budget <= 0 { return Err("invalid approval window".into()); }
        let deadline = requested_at.checked_add(budget).ok_or("approval deadline overflow")?;
        Ok((requested_at, deadline))
    }

    pub fn run_approval_wait_remaining_ms(&self, run_id: &str, approval_id: &str, now: i64) -> Result<u64, String> {
        let (requested_at, deadline) = self.run_approval_window(run_id, approval_id)?;
        // Clock rollback is not permission to extend the window.
        if now < requested_at { return Err("approval clock moved backwards".into()); }
        Ok(deadline.saturating_sub(now).max(0) as u64)
    }

    /// Project the same frozen policy used by execution into Shadow's legacy
    /// JSON contract. Only pre-binding Runs may use the compatibility reader;
    /// corrupt or mismatched bindings must never fall back to live permissions.
    pub fn run_shadow_permission_snapshot(&self, run_id: &str, conversation_id: &str) -> Result<serde_json::Value, String> {
        match self.run_control_binding(run_id)? {
            Some(binding) => {
                if binding.conversation_id != conversation_id {
                    return Err("frozen Run permission belongs to another conversation".into());
                }
                Ok(serde_json::json!({
                    "mode": binding.permission.mode.as_str(),
                    "projectRoot": binding.permission.project_root,
                    "grants": binding.permission.grants.iter().map(|grant| (&grant.tool, &grant.scope)).collect::<Vec<_>>(),
                }))
            }
            None => {
                let owner: String = self.with_connection(|connection| connection.query_row(
                    "SELECT conversation_id FROM runs WHERE id=?1", [run_id], |row| row.get(0),
                ))?;
                if owner != conversation_id {
                    return Err("legacy Run permission belongs to another conversation".into());
                }
                self.kernel_shadow_permission_snapshot(conversation_id)
            }
        }
    }

    pub fn run_time_budgets(&self, run_id: &str) -> Result<fox_engine_protocol::TimeBudgets, String> {
        match self.run_control_binding(run_id)? {
            Some(binding) => Ok(binding.budgets),
            None => {
                self.with_connection(|connection| connection.query_row(
                    "SELECT id FROM runs WHERE id=?1", [run_id], |row| row.get::<_, String>(0),
                ))?;
                Ok(fox_engine_protocol::TimeBudgets::default())
            }
        }
    }

    /// Capture all mutable permission inputs in one SQLite snapshot.
    pub fn freeze_legacy_run_control(&self, run_id: &str, profile_id: &str) -> Result<RunControlBinding, String> {
        self.freeze_legacy_run_control_with_executor(run_id, profile_id, fox_engine_protocol::ResourceExecutor::Runtime)
    }

    pub fn freeze_legacy_run_control_with_executor(&self, run_id: &str, profile_id: &str, executor: fox_engine_protocol::ResourceExecutor) -> Result<RunControlBinding, String> {
        self.freeze_selected_run_control(run_id, profile_id, ExecutionAuthority::Legacy, executor, fox_engine_protocol::TimeBudgets::default(), "pi")
    }

    pub(crate) fn freeze_kernel_run_control(&self, run_id: &str, profile_id: &str, budgets: fox_engine_protocol::TimeBudgets) -> Result<RunControlBinding, String> {
        budgets.validate()?;
        self.freeze_kernel_run_control_for_engine(run_id, profile_id, budgets, "pi")
    }

    pub(crate) fn freeze_kernel_run_control_for_engine(&self, run_id: &str, profile_id: &str, budgets: fox_engine_protocol::TimeBudgets, engine: &str) -> Result<RunControlBinding, String> {
        budgets.validate()?;
        if !matches!(engine, "pi" | "codex" | "deepseek_harness") { return Err("unsupported Kernel engine".into()); }
        self.freeze_selected_run_control(run_id, profile_id, ExecutionAuthority::Authoritative, fox_engine_protocol::ResourceExecutor::Rust, budgets, engine)
    }

    fn freeze_selected_run_control(&self, run_id: &str, profile_id: &str, authority: ExecutionAuthority,
        executor: fox_engine_protocol::ResourceExecutor, budgets: fox_engine_protocol::TimeBudgets, engine: &str) -> Result<RunControlBinding, String> {
        // A restart must reuse the existing binding, never recapture permissions.
        if let Some(binding) = self.run_control_binding(run_id)? {
            if binding.execution_profile_id != profile_id || binding.authority != authority || binding.engine_id != engine {
                return Err("frozen Run authority/profile cannot be replaced by startup".into());
            }
            return Ok(binding);
        }
        use fox_engine_protocol::{FrozenPermission, PermissionGrant};
        let authority_name = match authority { ExecutionAuthority::Legacy => "legacy", ExecutionAuthority::Authoritative => "authoritative" };
        let binding = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (conversation_id, mode, root): (String, String, Option<String>) = transaction.query_row(
                "SELECT r.conversation_id, COALESCE(p.permission_mode,c.permission_mode,'ask'), COALESCE(p.root_path,c.project_root)
                 FROM runs r JOIN conversations c ON c.id=r.conversation_id LEFT JOIN projects p ON p.id=c.project_id WHERE r.id=?1",
                [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            let grants = {
                let mut query = transaction.prepare("SELECT tool_name,scope_key FROM conversation_tool_permissions WHERE conversation_id=?1 ORDER BY tool_name,scope_key")?;
                let rows = query.query_map([&conversation_id], |row| Ok(PermissionGrant { tool: row.get(0)?, scope: row.get(1)? }))?;
                rows.collect::<Result<Vec<_>,_>>()?
            };
            let permission = FrozenPermission {
                mode: serde_json::from_value(serde_json::json!(mode)).map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
                project_root: root, grants,
            };
            let permission_snapshot_id = Self::run_control_permission_hash(&permission).map_err(rusqlite::Error::InvalidParameterName)?;
            let binding = RunControlBinding {
                schema_version: fox_engine_protocol::CONTROL_SCHEMA_VERSION,
                run_id: run_id.into(), conversation_id, engine_id: engine.into(),
                execution_profile_id: profile_id.into(), authority,
                read_only_executor: executor, permission_snapshot_id, permission,
                budgets,
            };
            let json = encode(&binding).map_err(rusqlite::Error::InvalidParameterName)?;
            transaction.execute(
                "INSERT INTO run_control_bindings(run_id,conversation_id,authority,engine_id,binding_json,binding_hash,created_at) VALUES(?1,?2,?6,?7,?3,?4,?5)",
                params![run_id,binding.conversation_id,json,content_hash(&json),now_ms(),authority_name,engine])?;
            transaction.commit()?;
            Ok(binding)
        })?;
        Ok(binding)
    }

    pub fn run_control_permission_hash(permission: &fox_engine_protocol::FrozenPermission) -> Result<String, String> {
        Ok(content_hash(&serde_json::to_string(permission).map_err(|e| e.to_string())?))
    }

    pub fn freeze_run_control(&self, binding: &RunControlBinding) -> Result<(), String> {
        let json = encode(binding)?;
        let hash = content_hash(&json);
        self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let conversation: String = transaction.query_row("SELECT conversation_id FROM runs WHERE id=?1", [&binding.run_id], |row| row.get(0))?;
            if conversation != binding.conversation_id {
                return Err(rusqlite::Error::InvalidParameterName("control binding belongs to another conversation".into()));
            }
            let existing: Option<(String, String)> = transaction.query_row(
                "SELECT binding_json,binding_hash FROM run_control_bindings WHERE run_id=?1", [&binding.run_id], |row| Ok((row.get(0)?,row.get(1)?))
            ).optional()?;
            if let Some((old_json, old_hash)) = existing {
                if old_json != json || old_hash != hash { return Err(rusqlite::Error::InvalidParameterName("Run control identity is already frozen".into())); }
            } else {
                transaction.execute(
                    "INSERT INTO run_control_bindings(run_id,conversation_id,authority,engine_id,binding_json,binding_hash,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![binding.run_id,binding.conversation_id,match binding.authority { ExecutionAuthority::Legacy => "legacy", ExecutionAuthority::Authoritative => "authoritative" },binding.engine_id,json,hash,now_ms()],
                )?;
            }
            transaction.commit()
        })
    }

    pub fn run_control_binding(&self, run_id: &str) -> Result<Option<RunControlBinding>, String> {
        let row: Option<(String,String,String,String,String)> = self.with_connection(|connection| {
            connection.query_row(
                "SELECT conversation_id,authority,engine_id,binding_json,binding_hash FROM run_control_bindings WHERE run_id=?1", [run_id],
                |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))
            ).optional()
        })?;
        let Some((conversation,authority,engine,json,hash)) = row else { return Ok(None); };
        if content_hash(&json) != hash { return Err("persisted control binding hash mismatch".into()); }
        let binding: RunControlBinding = serde_json::from_str(&json).map_err(|e| format!("corrupt control binding: {e}"))?;
        encode(&binding)?;
        let expected_authority = match binding.authority { ExecutionAuthority::Legacy => "legacy", ExecutionAuthority::Authoritative => "authoritative" };
        if binding.run_id != run_id || binding.conversation_id != conversation || binding.engine_id != engine || expected_authority != authority {
            return Err("persisted control binding columns disagree with the frozen identity".into());
        }
        Ok(Some(binding))
    }

    /// The conversation a Run belongs to, read from Host's own persisted rows.
    ///
    /// Scoped readers use this to derive their authorization instead of trusting
    /// a conversation id supplied by the caller. It reports what Host already
    /// recorded — the frozen binding when one exists, otherwise the Run's own
    /// row — and never rewrites either fact.
    pub fn run_conversation(
        &self,
        run_id: &str,
    ) -> Result<Option<String>, String> {
        self.with_connection(|connection| {
            Ok(connection
                .query_row(
                    "SELECT conversation_id FROM runs WHERE id = ?1",
                    [run_id],
                    |row| row.get(0),
                )
                .optional()?)
        })
    }
}
