//! Durable per-Run control bindings. Reopening or changing process settings can
//! never silently change a Run's authority, engine, permission or executor.
use super::{now_ms, Database};
use fox_engine_protocol::{ExecutionAuthority, RunControlBinding};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

fn content_hash(json: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(json.as_bytes()))) }

fn encode(binding: &RunControlBinding) -> Result<String, String> {
    binding.validate()?;
    let permission = crate::canonical_json::to_string(&binding.permission).map_err(|e| e.to_string())?;
    if binding.permission_snapshot_id != content_hash(&permission) {
        return Err(PERMISSION_HASH_MISMATCH.into());
    }
    crate::canonical_json::to_string(binding).map_err(|e| e.to_string())
}

pub(super) fn binding_in_connection(connection: &Connection, run_id: &str) -> rusqlite::Result<Option<RunControlBinding>> {
    // Every refusal below names `run_control.binding_integrity_failed` (or the
    // more specific legacy-format / unknown-route code) so the startup sweep can
    // tell an unverifiable historical binding from a database fault and
    // interrupt only that Run instead of failing the whole startup.
    let invalid = |message: String| rusqlite::Error::InvalidParameterName(format!("{BINDING_INTEGRITY_FAILED}: {message}"));
    let row: Option<(String,String,String,String,String)> = connection.query_row(
        "SELECT conversation_id,authority,engine_id,binding_json,binding_hash FROM run_control_bindings WHERE run_id=?1", [run_id],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
    ).optional()?;
    let Some((conversation,authority,engine,json,hash)) = row else { return Ok(None); };
    if content_hash(&json) != hash { return Err(invalid("persisted control binding hash mismatch".into())); }
    require_known_route(&json).map_err(invalid)?;
    let binding: RunControlBinding = serde_json::from_str(&json).map_err(|error| invalid(format!("corrupt control binding: {error}")))?;
    if let Err(error) = encode(&binding) {
        if error == PERMISSION_HASH_MISMATCH && legacy_permission_bytes_match(&json, &binding.permission_snapshot_id) {
            // Intact, but frozen by a build that hashed the permission before
            // canonical JSON (83c16042). Never continued: the user decided
            // historical Runs are interrupted, not migrated.
            return Err(rusqlite::Error::InvalidParameterName(format!(
                "{BINDING_LEGACY_FORMAT}: frozen permission was hashed in the pre-canonical format; this historical Run cannot continue")));
        }
        return Err(invalid(error));
    }
    let expected_authority = match binding.authority { ExecutionAuthority::Legacy => "legacy", ExecutionAuthority::Authoritative => "authoritative" };
    let owner: String = connection.query_row("SELECT conversation_id FROM runs WHERE id=?1", [run_id], |row| row.get(0))?;
    if binding.run_id != run_id || binding.conversation_id != conversation || conversation != owner
        || binding.engine_id != engine || expected_authority != authority {
        return Err(invalid("persisted control binding columns disagree with the frozen identity".into()));
    }
    Ok(Some(binding))
}

/// Stable code for a frozen control binding that fails its integrity check
/// (stored hash, permission hash, identity columns, decode). Such a Run is
/// refused by every reader; at startup it is interrupted with this code instead
/// of failing the whole application.
pub(crate) const BINDING_INTEGRITY_FAILED: &str = "run_control.binding_integrity_failed";
pub(crate) const BINDING_INTEGRITY_FAILED_MESSAGE: &str = "运行控制绑定校验失败，此 Run 已中断，不能继续";

/// Stable code for an intact binding frozen before canonical JSON (83c16042,
/// 2026-10-21): its permission hash matches the stored bytes in the old
/// field order, not the current canonical form. It is never continued.
pub(crate) const BINDING_LEGACY_FORMAT: &str = "run_control.binding_legacy_format";
pub(crate) const BINDING_LEGACY_FORMAT_MESSAGE: &str = "运行控制绑定为旧版格式，历史 Run 已中断，不能继续";

const PERMISSION_HASH_MISMATCH: &str = "frozen permission hash does not match its content";

/// Versioned check for bindings frozen before canonical JSON: those hashed
/// `serde_json::to_string(&permission)` (struct field order) and stored the
/// binding the same way, so the stored `permission` bytes are exactly what was
/// hashed. Only the bytes are hashed; nothing is re-serialized or trusted.
fn legacy_permission_bytes_match(json: &str, snapshot_id: &str) -> bool {
    #[derive(serde::Deserialize)]
    struct Stored<'a> { #[serde(borrow)] permission: &'a serde_json::value::RawValue }
    serde_json::from_str::<Stored<'_>>(json)
        .is_ok_and(|stored| content_hash(stored.permission.get()) == snapshot_id)
}

/// The stable code and user-facing reason for a Run whose frozen binding this
/// build cannot verify, or `None` when `error` is not a binding refusal (a
/// database fault, which must still fail loudly).
pub(crate) fn binding_refusal(error: &rusqlite::Error) -> Option<(&'static str, &'static str)> {
    let rusqlite::Error::InvalidParameterName(message) = error else { return None };
    if message.starts_with(BINDING_LEGACY_FORMAT) {
        Some((BINDING_LEGACY_FORMAT, BINDING_LEGACY_FORMAT_MESSAGE))
    } else if message.starts_with(BINDING_INTEGRITY_FAILED) && message.contains(EXECUTOR_ROUTE_UNKNOWN) {
        Some((EXECUTOR_ROUTE_UNKNOWN, EXECUTOR_ROUTE_UNKNOWN_MESSAGE))
    } else if message.starts_with(BINDING_INTEGRITY_FAILED) {
        Some((BINDING_INTEGRITY_FAILED, BINDING_INTEGRITY_FAILED_MESSAGE))
    } else {
        None
    }
}

/// Stable refusal code for a frozen executor route this build does not know.
pub(crate) const EXECUTOR_ROUTE_UNKNOWN: &str = "executor_route_unknown";
pub(crate) const EXECUTOR_ROUTE_UNKNOWN_MESSAGE: &str = "执行路径无法识别，此 Run 已中断，不能继续";

/// Terminal code and reason for a historical Kernel Run (Authoritative, no
/// frozen route) found unfinished at startup. The Kernel loop is frozen: such
/// a Run is interrupted, never resumed, and the user continues in a new Run.
pub(crate) const EXECUTOR_ROUTE_RETIRED: &str = "executor_route_retired";
pub(crate) const EXECUTOR_ROUTE_RETIRED_MESSAGE: &str = "执行路径已退役";

/// The route every new Run freezes.
///
/// There is no process-level routing switch any more (A0 retired the
/// executor-route environment switch): a new Authoritative Run and every child Run freeze
/// `PiNative`. A Legacy-authority binding never carries a route, because the
/// Pi-native path reuses the Authoritative execution contract instead of adding
/// a third authority.
pub(crate) fn new_run_executor_route(authority: ExecutionAuthority) -> Option<fox_engine_protocol::ExecutorRoute> {
    match authority {
        ExecutionAuthority::Authoritative => Some(fox_engine_protocol::ExecutorRoute::PiNative),
        ExecutionAuthority::Legacy => None,
    }
}

/// Refuse a persisted `executorRoute` this build does not recognize with the
/// stable `executor_route_unknown` code, instead of a generic decode error or a
/// guessed fallback. An absent or null route is the historical unrouted shape.
fn require_known_route(json: &str) -> Result<(), String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| format!("corrupt control binding: {error}"))?;
    match value.get("executorRoute") {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(route) if serde_json::from_value::<fox_engine_protocol::ExecutorRoute>(route.clone()).is_ok() => Ok(()),
        Some(route) => Err(format!("{EXECUTOR_ROUTE_UNKNOWN}: frozen executor route {route} is not recognized by this build")),
    }
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

    /// A binding frozen before the route field existed must re-encode to the
    /// exact stored bytes, because `freeze_run_control` compares both the JSON
    /// and its hash before it will accept a repeat freeze.
    #[test]
    fn an_unrouted_binding_reencodes_to_its_stored_bytes_and_hash() {
        let (db, _path, run_id) = fixture();
        db.apply_runtime_event(&run_id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        let binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        assert_eq!(binding.executor_route, None);
        let (stored_json, stored_hash): (String, String) = db.with_connection(|connection| {
            connection.query_row(
                "SELECT binding_json,binding_hash FROM run_control_bindings WHERE run_id=?1",
                [&run_id], |row| Ok((row.get(0)?, row.get(1)?)),
            )
        }).unwrap();
        assert!(!stored_json.contains("executorRoute"), "an unrouted binding must not emit the field at all");
        assert_eq!(encode(&binding).unwrap(), stored_json);
        assert_eq!(content_hash(&stored_json), stored_hash);
        // So a restart re-freeze is accepted instead of refused as tampered,
        // and the Run still carries no route afterwards.
        db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        assert_eq!(db.run_control_binding(&run_id).unwrap().unwrap().executor_route, None);
    }

    #[test]
    fn a0_route_new_runs_always_freeze_pi_native_without_a_process_switch() {
        // No environment is consulted any more: the Authoritative contract
        // always freezes PiNative and Legacy authority never carries a route.
        assert_eq!(new_run_executor_route(ExecutionAuthority::Authoritative),
            Some(fox_engine_protocol::ExecutorRoute::PiNative));
        assert_eq!(new_run_executor_route(ExecutionAuthority::Legacy), None);
        assert_eq!(Database::new_run_executor_route(ExecutionAuthority::Authoritative),
            Some(fox_engine_protocol::ExecutorRoute::PiNative));
        // The retired switch must not exist anywhere in the routing code.
        let source = include_str!("run_control.rs");
        let retired = ["FOX_EXECUTOR", "_ROUTE\")"].concat();
        assert!(!source.contains(&retired), "the retired routing switch is still read");
    }

    #[test]
    fn a_frozen_route_never_reads_the_current_candidate_switch_again() {
        for route in [None,Some(fox_engine_protocol::ExecutorRoute::PiNative)] {
            let (db,_path,run_id) = fixture();
            let frozen = db.freeze_selected_run_control_with_route(&run_id,"legacy",ExecutionAuthority::Authoritative,
                ResourceExecutor::Rust,Default::default(),"pi",||Ok(route)).unwrap();
            let reopened = db.freeze_selected_run_control_with_route(&run_id,"legacy",ExecutionAuthority::Authoritative,
                ResourceExecutor::Rust,Default::default(),"pi",||panic!("an existing Run must not read a changed or invalid environment")).unwrap();
            assert_eq!(frozen,reopened);
            assert_eq!(reopened.executor_route,route);
        }
    }

    #[test]
    fn a0_route_default_freeze_persists_pi_native_and_refuses_a_non_pi_engine() {
        let (db, path, routed_run) = fixture();
        let routed = db.freeze_default_run_control_for_engine(&routed_run, "legacy", Default::default(), "pi").unwrap();
        assert_eq!(routed.authority, ExecutionAuthority::Authoritative);
        assert_eq!(routed.executor_route, Some(fox_engine_protocol::ExecutorRoute::PiNative));
        drop(db);
        let db = Database::open(path).unwrap();
        assert_eq!(db.run_control_binding(&routed_run).unwrap().unwrap().executor_route,
            Some(fox_engine_protocol::ExecutorRoute::PiNative));
        // PiNative is only driven by the pi engine; any other engine is refused
        // before a binding is written instead of freezing an unexecutable Run.
        for engine in ["codex", "deepseek_harness", "kernel"] {
            let conversation = db.create_conversation("fox-general", Some("engine"), None, None).unwrap();
            let run = db.create_run(&conversation.id, "engine", None).unwrap().run.id;
            assert!(db.freeze_default_run_control_for_engine(&run, "legacy", Default::default(), engine).is_err(), "{engine}");
            assert_eq!(db.run_control_binding(&run).unwrap(), None, "{engine}");
        }
    }

    #[test]
    fn a0_route_an_unknown_frozen_route_is_refused_with_executor_route_unknown() {
        let (db, _path, run_id) = fixture();
        db.freeze_default_run_control_for_engine(&run_id, "legacy", Default::default(), "pi").unwrap();
        // A binding written by another build with a route this build does not
        // know: the hash is consistent, so only the route itself is foreign.
        let (json,): (String,) = db.with_connection(|connection| connection.query_row(
            "SELECT binding_json FROM run_control_bindings WHERE run_id=?1", [&run_id], |row| Ok((row.get(0)?,)))).unwrap();
        assert!(json.contains("\"executorRoute\":\"pi_native\""));
        for foreign in ["\"kernel\"", "\"pi_native_v2\"", "7"] {
            let tampered = json.replace("\"executorRoute\":\"pi_native\"", &format!("\"executorRoute\":{foreign}"));
            db.with_connection(|connection| connection.execute(
                "UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1",
                params![run_id, tampered, content_hash(&tampered)])).unwrap();
            let error = db.run_control_binding(&run_id).unwrap_err();
            assert!(error.contains(EXECUTOR_ROUTE_UNKNOWN), "{foreign}: {error}");
            // A restart re-freeze refuses it too; it never falls back to a route.
            assert!(db.freeze_default_run_control_for_engine(&run_id, "legacy", Default::default(), "pi")
                .unwrap_err().contains(EXECUTOR_ROUTE_UNKNOWN), "{foreign}");
        }
        assert!(require_known_route("{\"executorRoute\":null}").is_ok());
        assert!(require_known_route("{}").is_ok());
        assert!(require_known_route("{\"executorRoute\":\"pi_native\"}").is_ok());
    }

    #[test]
    fn an_invalid_candidate_switch_cannot_leave_a_partially_frozen_new_run() {
        let (db,_path,run_id) = fixture();
        assert!(db.freeze_selected_run_control_with_route(&run_id,"legacy",ExecutionAuthority::Authoritative,
            ResourceExecutor::Rust,Default::default(),"pi",||Err("invalid candidate switch".into())).is_err());
        assert_eq!(db.run_control_binding(&run_id).unwrap(),None);
    }

    #[test]
    fn approval_has_no_deadline_and_survives_reopen() {
        use crate::database::ApprovalDecision;
        let (db, path, run_id) = fixture();
        db.apply_runtime_event(&run_id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        let binding = db.freeze_legacy_run_control(&run_id, "test-profile").unwrap();
        let tool = db.create_host_tool_call(&run_id, "deadline-tool", "write_file", &serde_json::json!({"path":"test.txt"}), "pending", true).unwrap();
        let approval = db.create_approval(&tool.id, "write", &serde_json::json!({"permissionScope":"project"})).unwrap();
        assert_eq!(db.run_approval_requested_at(&run_id, &approval.id).unwrap(), approval.requested_at);
        assert!(db.run_approval_requested_at("foreign", &approval.id).is_err());
        drop(db);
        let db = Database::open(path).unwrap();
        // Requested long before the frozen nominal window: still pending and
        // still grantable (approvals wait for the user, they never expire).
        db.with_connection(|connection| connection.execute("UPDATE approvals SET requested_at=1 WHERE id=?1", [&approval.id])).unwrap();
        let pending: String = db.with_connection(|connection| connection.query_row(
            "SELECT status FROM approvals WHERE id=?1", [&approval.id], |row| row.get(0))).unwrap();
        assert_eq!(pending, "pending");
        let granted = db.resolve_approval(&approval.id, ApprovalDecision::AllowConversation).unwrap().unwrap();
        assert_eq!(granted.status, "approved");
        assert_eq!(granted.decision.unwrap()["approved"], true);
        assert!(db.conversation_tool_permission_granted(&binding.conversation_id, "write_file", "project").unwrap());
        assert!(db.claim_approved_tool_call(&run_id, "deadline-tool").unwrap());
        assert!(db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap().is_none());
    }

    #[test]
    fn approval_wait_is_the_union_of_waiting_intervals() {
        let (db, _path, run_id) = fixture();
        db.apply_runtime_event(&run_id, 1, &serde_json::json!({"type":"run.started"})).unwrap();
        assert_eq!(db.run_approval_wait_ms(&run_id, 10_000).unwrap(), 0);
        let mut ids = Vec::new();
        for call in ["w1", "w2", "w3"] {
            let tool = db.create_host_tool_call(&run_id, call, "write_file", &serde_json::json!({}), "pending", true).unwrap();
            ids.push(db.create_approval(&tool.id, "write", &serde_json::json!({})).unwrap().id);
        }
        db.with_connection(|connection| {
            // [1000,3000] and [2000,4000] overlap -> 3000; [6000, open] until now=7000 -> 1000.
            connection.execute("UPDATE approvals SET requested_at=1000, resolved_at=3000, status='approved' WHERE id=?1", [&ids[0]])?;
            connection.execute("UPDATE approvals SET requested_at=2000, resolved_at=4000, status='denied' WHERE id=?1", [&ids[1]])?;
            connection.execute("UPDATE approvals SET requested_at=6000, resolved_at=NULL WHERE id=?1", [&ids[2]])
        }).unwrap();
        assert_eq!(db.run_approval_wait_ms(&run_id, 7_000).unwrap(), 4_000);
    }

    #[test]
    fn approval_clock_skew_does_not_expire_and_corrupt_binding_still_allows_denial() {
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
                assert_eq!(db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap().unwrap().status, "approved");
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
    pub(crate) fn new_run_executor_route(authority: ExecutionAuthority) -> Option<fox_engine_protocol::ExecutorRoute> {
        new_run_executor_route(authority)
    }

    /// When an approval was requested, after checking that the Run's frozen
    /// control is intact (a corrupt or mismatched binding still fails closed).
    /// Approvals have no deadline: the frozen `approval_wait_ms` is kept for
    /// record compatibility but no longer bounds the wait.
    pub fn run_approval_requested_at(&self, run_id: &str, approval_id: &str) -> Result<i64, String> {
        self.run_time_budgets(run_id)?;
        let requested_at: i64 = self.with_connection(|connection| connection.query_row(
            "SELECT a.requested_at FROM approvals a JOIN tool_calls t ON t.id=a.tool_call_id WHERE a.id=?1 AND t.run_id=?2",
            params![approval_id, run_id], |row| row.get(0),
        ))?;
        if requested_at < 0 { return Err("invalid approval request time".into()); }
        Ok(requested_at)
    }

    /// Wall time this Run has spent waiting for human approvals so far. Host
    /// wall-clock duration timers add it to their deadline: waiting for the
    /// user is not execution time.
    pub fn run_approval_wait_ms(&self, run_id: &str, now: i64) -> Result<i64, String> {
        self.with_connection(|connection| super::approval_wait_ms_in_tx(connection, run_id, now))
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
        self.freeze_selected_run_control(run_id, profile_id, ExecutionAuthority::Legacy, executor, fox_engine_protocol::TimeBudgets::continuous(), "pi")
    }

    /// Test fixture for the frozen Kernel loop: freezes an Authoritative Run
    /// with no route, the shape historical Kernel Runs were frozen with.
    /// Production never freezes this shape any more (A0).
    #[cfg(test)]
    pub(crate) fn freeze_kernel_run_control(&self, run_id: &str, profile_id: &str, budgets: fox_engine_protocol::TimeBudgets) -> Result<RunControlBinding, String> {
        budgets.validate()?;
        self.freeze_kernel_run_control_for_engine(run_id, profile_id, budgets, "pi")
    }

    #[cfg(test)]
    pub(crate) fn freeze_kernel_run_control_for_engine(&self, run_id: &str, profile_id: &str, budgets: fox_engine_protocol::TimeBudgets, engine: &str) -> Result<RunControlBinding, String> {
        budgets.validate()?;
        if !matches!(engine, "pi" | "codex" | "deepseek_harness") { return Err("unsupported Kernel engine".into()); }
        self.freeze_selected_run_control_with_route(run_id, profile_id, ExecutionAuthority::Authoritative,
            fox_engine_protocol::ResourceExecutor::Rust, budgets, engine, || Ok(None))
    }

    /// Freeze a new local Run. Every new Run is Authoritative + PiNative,
    /// driven by the pi engine; there is no process switch and no Kernel
    /// fallback.
    pub(crate) fn freeze_default_run_control_for_engine(&self, run_id: &str, profile_id: &str, budgets: fox_engine_protocol::TimeBudgets, engine: &str) -> Result<RunControlBinding, String> {
        budgets.validate()?;
        if engine != "pi" { return Err(format!("unsupported PiNative engine `{engine}`")); }
        self.freeze_selected_run_control(run_id, profile_id, ExecutionAuthority::Authoritative,
            fox_engine_protocol::ResourceExecutor::Rust, budgets, engine)
    }

    fn freeze_selected_run_control(&self, run_id: &str, profile_id: &str, authority: ExecutionAuthority,
        executor: fox_engine_protocol::ResourceExecutor, budgets: fox_engine_protocol::TimeBudgets, engine: &str) -> Result<RunControlBinding, String> {
        self.freeze_selected_run_control_with_route(run_id, profile_id, authority, executor, budgets, engine,
            || Ok(new_run_executor_route(authority)))
    }

    fn freeze_selected_run_control_with_route(&self, run_id: &str, profile_id: &str, authority: ExecutionAuthority,
        executor: fox_engine_protocol::ResourceExecutor, budgets: fox_engine_protocol::TimeBudgets, engine: &str,
        select_route: impl FnOnce() -> Result<Option<fox_engine_protocol::ExecutorRoute>, String>) -> Result<RunControlBinding, String> {
        // A restart must reuse the existing binding, never recapture permissions.
        if let Some(binding) = self.run_control_binding(run_id)? {
            if binding.execution_profile_id != profile_id || binding.authority != authority
                || binding.engine_id != engine {
                return Err("frozen Run authority/profile cannot be replaced by startup".into());
            }
            return Ok(binding);
        }
        let executor_route = select_route()?;
        use fox_engine_protocol::{FrozenPermission, PermissionGrant};
        let authority_name = match authority { ExecutionAuthority::Legacy => "legacy", ExecutionAuthority::Authoritative => "authoritative" };
        let binding = self.with_connection(|connection| {
            let transaction = connection.transaction()?;
            let (conversation_id, mode, root): (String, String, Option<String>) = transaction.query_row(
                "SELECT r.conversation_id, COALESCE(kep.mode, c.permission_mode, 'ask'), COALESCE(p.root_path,c.project_root)
                 FROM runs r JOIN conversations c ON c.id=r.conversation_id
                 LEFT JOIN projects p ON p.id=c.project_id
                 LEFT JOIN kernel_execution_policies kep ON kep.conversation_id=c.id
                 WHERE r.id=?1",
                [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
            let grants = {
                // Every row in this table is a reusable approval, so it is frozen
                // as `ApprovalReuse` and re-checked live at each use: a Run that
                // is already frozen must not outlive a withdrawn permission
                // (REV-04). Revoked rows are not frozen at all.
                let mut query = transaction.prepare("SELECT tool_name,scope_key FROM conversation_tool_permissions WHERE conversation_id=?1 AND revoked_at IS NULL ORDER BY tool_name,scope_key")?;
                let rows = query.query_map([&conversation_id], |row| Ok(PermissionGrant {
                    tool: row.get(0)?, scope: row.get(1)?,
                    kind: fox_engine_protocol::GrantKind::ApprovalReuse,
                }))?;
                rows.collect::<Result<Vec<_>,_>>()?
            };
            // The approval generation at freeze time. Every revocation advances
            // it, so a reusable approval cannot outlive the generation that
            // issued it — a later re-grant of the same scope is a DIFFERENT
            // authorization and must not revive this Run's frozen one.
            let approval_epoch: Option<u64> = transaction
                .query_row(
                    "SELECT version FROM kernel_execution_policies WHERE conversation_id=?1",
                    [&conversation_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())
                .map_err(rusqlite::Error::InvalidParameterName)?;
            let permission = FrozenPermission {
                mode: serde_json::from_value(serde_json::json!(mode)).map_err(|error| rusqlite::Error::InvalidParameterName(error.to_string()))?,
                project_root: root, grants, approval_epoch,
            };
            let permission_snapshot_id = Self::run_control_permission_hash(&permission).map_err(rusqlite::Error::InvalidParameterName)?;
            let binding = RunControlBinding {
                schema_version: fox_engine_protocol::CONTROL_SCHEMA_VERSION,
                run_id: run_id.into(), conversation_id, engine_id: engine.into(),
                execution_profile_id: profile_id.into(), authority,
                read_only_executor: executor, permission_snapshot_id, permission,
                budgets,
                executor_route,
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
        Ok(content_hash(&crate::canonical_json::to_string(permission).map_err(|e| e.to_string())?))
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
        self.with_connection(|connection| binding_in_connection(connection, run_id))
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
