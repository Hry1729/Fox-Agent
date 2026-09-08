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
}

impl Database {
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

    /// Capture all mutable permission inputs in one SQLite snapshot. This is
    /// deliberately Legacy-only until authoritative dispatch is integrated.
    pub fn freeze_legacy_run_control(&self, run_id: &str, profile_id: &str) -> Result<RunControlBinding, String> {
        self.freeze_legacy_run_control_with_executor(run_id, profile_id, fox_engine_protocol::ResourceExecutor::Runtime)
    }

    pub fn freeze_legacy_run_control_with_executor(&self, run_id: &str, profile_id: &str, executor: fox_engine_protocol::ResourceExecutor) -> Result<RunControlBinding, String> {
        // A restart must reuse the existing binding, never recapture permissions.
        if let Some(binding) = self.run_control_binding(run_id)? {
            if binding.execution_profile_id != profile_id || binding.authority != ExecutionAuthority::Legacy {
                return Err("frozen Run authority/profile cannot be replaced by Legacy startup".into());
            }
            return Ok(binding);
        }
        use fox_engine_protocol::{FrozenPermission, PermissionGrant, TimeBudgets};
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
                run_id: run_id.into(), conversation_id, engine_id: "pi".into(),
                execution_profile_id: profile_id.into(), authority: ExecutionAuthority::Legacy,
                read_only_executor: executor, permission_snapshot_id, permission,
                budgets: TimeBudgets::default(),
            };
            let json = encode(&binding).map_err(rusqlite::Error::InvalidParameterName)?;
            transaction.execute(
                "INSERT INTO run_control_bindings(run_id,conversation_id,authority,engine_id,binding_json,binding_hash,created_at) VALUES(?1,?2,'legacy','pi',?3,?4,?5)",
                params![run_id,binding.conversation_id,json,content_hash(&json),now_ms()])?;
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
}
