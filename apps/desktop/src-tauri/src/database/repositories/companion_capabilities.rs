//! C02/K4: tool-level companion capability grants.
//!
//! A grant is (companion, connector instance, tool). It is necessary but never
//! sufficient: the run catalog and every execution entry intersect it with the
//! installed, enabled reality, and a per-run snapshot freezes what the run
//! started with, so a mid-run grant never widens the current run and a
//! revocation stops calls that were approved but not yet executed.
use super::{now_ms, register_approval_invalidation_in_tx, ApprovalRefusal, Database};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;

/// The built-in web capability pair. Not an mcp_servers row.
pub(crate) const WEB_SERVER_ID: &str = "@web";
pub(crate) const WEB_TOOLS: [&str; 2] = ["web_search", "web_read"];

/// A user-facing grant rejection whose `Display` is exactly the message, so the
/// command surfaces it verbatim through `with_connection`'s error conversion.
fn grant_rejection(message: &str) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
        Some(message.to_owned()),
    )
}

/// Snapshot identifier: web tools use their own name, connector tools use
/// `mcp:<serverId>:<tool>`. OpenAPI operations appear under their stable
/// generated operation name, the same identifier the catalog lists.
pub(crate) fn capability_id(server_id: &str, tool: &str) -> String {
    if server_id == WEB_SERVER_ID {
        tool.to_owned()
    } else {
        format!("mcp:{server_id}:{tool}")
    }
}

/// The capability a concrete tool call requires, or None for base-whitelist
/// tools (file/note), which every companion keeps.
fn required_capability(tool: &str, input: Option<&Value>) -> Result<Option<String>, String> {
    if crate::companion_workspace::allows_tool(tool) {
        return Ok(None);
    }
    match tool {
        "web_search" | "web_read" => Ok(Some(tool.to_owned())),
        "list_mcp_tools" => Ok(Some("mcp:*".to_owned())),
        "call_mcp_tool" => {
            // Event ingress (tool.started/updated/completed) carries no input;
            // the execute ingress already verified the exact tool, so the
            // event only needs the connector capability to exist at all.
            let Some(input) = input else { return Ok(Some("mcp:*".to_owned())) };
            let server = input.get("serverId").and_then(Value::as_str).filter(|value| !value.is_empty())
                .ok_or("缺少连接器 serverId")?;
            let remote = input.get("tool").and_then(Value::as_str).filter(|value| !value.is_empty())
                .ok_or("缺少连接器工具名")?;
            Ok(Some(capability_id(server, remote)))
        }
        _ => Err(format!("私聊不可使用工具 {tool}")),
    }
}

/// Read-only view of [`required_capability`] for the Host notification refresh,
/// which re-derives the capability of an already-persisted approval input.
pub(crate) fn required_capability_for(tool: &str, input: Option<&Value>) -> Result<Option<String>, String> {
    required_capability(tool, input)
}

/// K4 execution gate, shared by the preflight/execute ingress
/// (`companion_workspace::check_run_tool`) and the approved-execution claim
/// (`claim_approved_tool_call`), including restart recovery. Runs on the
/// caller's connection so it can join an existing transaction.
pub(crate) fn ensure_granted_on(
    connection: &Connection,
    run_id: &str,
    tool: &str,
    input: Option<&Value>,
) -> Result<(), String> {
    ensure_granted_inner(connection, run_id, tool, input, false)
}

/// D1-3/D1-4: the startup/recovery variant. A companion run that depends on a
/// capability must present its ORIGINAL frozen snapshot; a missing snapshot is
/// refused with a diagnosable reason instead of falling back to the current
/// grants, which a later regrant could widen. Base-whitelist tools (no required
/// capability) and non-companion runs are unaffected, so historical ordinary
/// Runs keep their compatibility contract.
pub(crate) fn ensure_granted_strict_on(
    connection: &Connection,
    run_id: &str,
    tool: &str,
    input: Option<&Value>,
) -> Result<(), String> {
    ensure_granted_inner(connection, run_id, tool, input, true)
}

fn ensure_granted_inner(
    connection: &Connection,
    run_id: &str,
    tool: &str,
    input: Option<&Value>,
    strict_snapshot: bool,
) -> Result<(), String> {
    let Some(scope) = super::companion_ownership::resolve_run_on(connection, run_id)? else {
        return Ok(());
    };
    let companion = scope.companion_id;
    let Some(required) = required_capability(tool, input)? else { return Ok(()) };
    let current = current_capability_ids_on(connection, &companion)?;
    let snapshot = match run_snapshot_on(connection, run_id)? {
        Some(snapshot) => snapshot,
        None if strict_snapshot => {
            return Err(format!("工具 {tool} 的本轮能力快照缺失，无法按原授权恢复此批准；请重新发起对话"));
        }
        None => current.clone(),
    };
    let granted_now = if required == "mcp:*" {
        current.iter().any(|id| id.starts_with("mcp:"))
    } else {
        current.contains(&required)
    };
    let in_snapshot = if required == "mcp:*" {
        snapshot.iter().any(|id| id.starts_with("mcp:"))
    } else {
        snapshot.contains(&required)
    };
    if !granted_now && !in_snapshot {
        return Err(format!("联系人未启用工具 {tool}，请在资料页的可用能力中开启"));
    }
    if !granted_now {
        return Err(format!("联系人已撤销对工具 {tool} 的授权；重新授权后请重新发起，旧的批准不会恢复"));
    }
    if !in_snapshot {
        return Err(format!("工具 {tool} 在本轮开始后才启用，将从下一轮对话开始可用"));
    }
    Ok(())
}

/// D1-2: register the irreversible invalidation on every unclaimed
/// `tool_execution` approval whose exact capability was revoked, scoped to
/// companion-bound conversations (fixed private chat or bound background task,
/// including archived companions — hence SQL joins, not `resolve_run_on`).
///
/// `companion_id` None means a whole-server change (disable/delete) touching
/// every companion bound to that server; `tool` None matches the server prefix,
/// Some(t) matches the exact `capability_id`. This only RECORDS the barrier —
/// the claim/resolve/startup/waiter gates own the D1-1 settlement, so recording
/// a revocation never depends on an immediate settlement succeeding. Ordinary
/// (non-companion) MCP approvals are deliberately untouched. Returns the
/// affected approval ids for the Host to refresh (best-effort notification).
pub(crate) fn invalidate_companion_capability_in_tx(
    transaction: &Transaction<'_>,
    companion_id: Option<&str>,
    server_id: &str,
    tool: Option<&str>,
    refusal: &ApprovalRefusal,
    now: i64,
) -> rusqlite::Result<Vec<String>> {
    let target = tool.map(|tool| capability_id(server_id, tool));
    let prefix = format!("mcp:{server_id}:");
    let mut query = transaction.prepare(
        "SELECT a.id, t.tool_name, t.input_json
         FROM approvals a
         JOIN tool_calls t ON t.id = a.tool_call_id
         JOIN runs r ON r.id = t.run_id
         WHERE a.category = 'tool_execution' AND a.status IN ('pending','approved') AND a.claimed_at IS NULL
           AND t.status = 'pending' AND t.requires_approval = 1 AND t.execution_location = 'host'
           AND r.status = 'running'
           AND EXISTS (
               SELECT 1 FROM conversations c WHERE c.id = t.conversation_id AND c.companion_id IS NOT NULL
                 AND (?1 IS NULL OR c.companion_id = ?1)
               UNION ALL
               SELECT 1 FROM digital_colleagues dc WHERE dc.conversation_id = t.conversation_id AND dc.companion_bound = 1
                 AND (?1 IS NULL OR dc.companion_id = ?1)
           )",
    )?;
    let rows = query.query_map(params![companion_id], |row| Ok((
        row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
    )))?.collect::<rusqlite::Result<Vec<_>>>()?;
    drop(query);
    let mut affected = Vec::new();
    for (approval_id, tool_name, input_json) in rows {
        let Ok(input) = serde_json::from_str::<Value>(&input_json) else { continue };
        let Ok(Some(required)) = required_capability(&tool_name, Some(&input)) else { continue };
        let matches = match &target {
            Some(target) => required == *target,
            None => required.starts_with(&prefix),
        };
        if !matches { continue; }
        register_approval_invalidation_in_tx(transaction, &approval_id, refusal, now)?;
        affected.push(approval_id);
    }
    Ok(affected)
}

/// Current effective capability ids: grants intersected with the installed,
/// enabled connectors. A grant for a removed or disabled server authorizes
/// nothing until the server is back.
fn current_capability_ids_on(connection: &Connection, companion_id: &str) -> Result<Vec<String>, String> {
    let mut statement = connection.prepare(
        "SELECT g.server_id, g.tool FROM companion_tool_grants g
         LEFT JOIN mcp_servers s ON s.id = g.server_id
         WHERE g.companion_id = ?1 AND (g.server_id = ?2 OR (s.id IS NOT NULL AND s.enabled = 1))",
    ).map_err(|error| error.to_string())?;
    let rows = statement.query_map(params![companion_id, WEB_SERVER_ID], |row| {
        Ok(capability_id(&row.get::<_, String>(0)?, &row.get::<_, String>(1)?))
    }).map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|error| error.to_string())
}

fn run_snapshot_on(connection: &Connection, run_id: &str) -> Result<Option<Vec<String>>, String> {
    let raw: Option<Option<String>> = connection.query_row(
        "SELECT companion_tools_snapshot FROM runs WHERE id = ?1",
        [run_id],
        |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;
    match raw.flatten() {
        Some(json) => serde_json::from_str(&json)
            .map(Some)
            .map_err(|error| format!("运行的能力快照无法读取: {error}")),
        None => Ok(None),
    }
}

impl Database {
    pub fn companion_tool_grants(&self, companion_id: &str) -> Result<Vec<(String, String)>, String> {
        self.with_connection(|connection| {
            let mut statement = connection.prepare(
                "SELECT server_id, tool FROM companion_tool_grants WHERE companion_id = ?1 ORDER BY server_id, tool",
            )?;
            let rows = statement.query_map([companion_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
            rows.collect()
        })
    }

    /// C02/K4 (D1-2): grant or revoke one capability. A revocation deletes the
    /// grant AND records the irreversible invalidation on the matching unclaimed
    /// approvals in the SAME Immediate transaction, so a regrant can never revive
    /// an old approval and a crash can never leave the grant deleted without the
    /// barrier. Returns the affected approval ids (empty for a grant) so the
    /// command can notify the Host best-effort; the persistent facts, not the
    /// notification, are authoritative.
    pub fn set_companion_tool_grant(
        &self,
        companion_id: &str,
        server_id: &str,
        tool: &str,
        granted: bool,
    ) -> Result<Vec<String>, String> {
        let tool = tool.trim();
        if tool.is_empty() || tool.chars().count() > 200 {
            return Err("工具名无效".to_owned());
        }
        let now = now_ms();
        self.with_connection(|connection| {
            let transaction = connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let exists: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM companion_profiles WHERE id = ?1 AND deleting_at IS NULL)",
                [companion_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(grant_rejection("联系人或连接器不存在"));
            }
            if server_id == WEB_SERVER_ID {
                if !WEB_TOOLS.contains(&tool) {
                    return Err(grant_rejection("网页能力只支持 web_search 与 web_read"));
                }
            } else {
                let server: Option<String> = transaction.query_row(
                    "SELECT id FROM mcp_servers WHERE id = ?1",
                    [server_id],
                    |row| row.get(0),
                ).optional()?;
                if server.is_none() {
                    return Err(grant_rejection("联系人或连接器不存在"));
                }
            }
            let mut affected = Vec::new();
            if granted {
                transaction.execute(
                    "INSERT OR IGNORE INTO companion_tool_grants(companion_id, server_id, tool, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![companion_id, server_id, tool, now],
                )?;
            } else {
                transaction.execute(
                    "DELETE FROM companion_tool_grants WHERE companion_id = ?1 AND server_id = ?2 AND tool = ?3",
                    params![companion_id, server_id, tool],
                )?;
                let refusal = ApprovalRefusal::new(
                    "companion.tool_not_available",
                    format!("联系人已撤销对工具 {tool} 的授权；重新授权后请重新发起，旧的批准不会恢复"),
                );
                affected = invalidate_companion_capability_in_tx(
                    &transaction, Some(companion_id), server_id, Some(tool), &refusal, now,
                )?;
            }
            transaction.commit()?;
            Ok(affected)
        })
    }

    pub fn companion_capability_ids(&self, companion_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        current_capability_ids_on(&connection, companion_id)
    }

    /// Freeze the run's capability snapshot. Write-once, like the model
    /// snapshot: queueing, retry and restart never re-resolve it.
    pub fn freeze_run_companion_tools(&self, run_id: &str, ids: &[String]) -> Result<(), String> {
        let json = crate::canonical_json::to_string(ids).map_err(|error| error.to_string())?;
        self.with_connection(|connection| {
            connection.execute(
                "UPDATE runs SET companion_tools_snapshot = ?2 WHERE id = ?1 AND companion_tools_snapshot IS NULL",
                params![run_id, json],
            )?;
            Ok(())
        })
    }

    pub fn run_companion_tools_snapshot(&self, run_id: &str) -> Result<Option<Vec<String>>, String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        run_snapshot_on(&connection, run_id)
    }

    pub fn ensure_companion_tool_granted(&self, run_id: &str, tool: &str, input: Option<&Value>) -> Result<(), String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        ensure_granted_on(&connection, run_id, tool, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::{ApprovalDecision, Database};
    use serde_json::json;

    struct Fixture {
        root: std::path::PathBuf,
        db: Database,
        companion: String,
        chat: String,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("fox-capability-test-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let db = Database::open(root.join("fox.db")).unwrap();
            let view = db.add_companion(&root, "fox-debugger").unwrap();
            Self { root, db, companion: view.profile.id, chat: view.conversation_id }
        }

        fn seed_server(&self, id: &str, enabled: bool) {
            self.db.execute_raw_sql(&format!(
                "INSERT INTO mcp_servers(id,name,command,enabled,created_at,updated_at)
                 VALUES ('{id}','飞书','node',{},1,1)",
                enabled as i64
            )).unwrap();
        }

        fn running_run(&self) -> String {
            let started = self.db.create_run(&self.chat, "查一下日历", None).unwrap();
            self.db.apply_runtime_event(&started.run.id, 1, &json!({"type":"run.started"})).unwrap();
            started.run.id
        }
    }

    #[test]
    fn grants_intersect_enabled_servers_and_the_run_snapshot_is_write_once() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", true).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        f.db.set_companion_tool_grant(&f.companion, WEB_SERVER_ID, "web_search", true).unwrap();
        let ids = f.db.companion_capability_ids(&f.companion).unwrap();
        assert_eq!(ids, vec!["web_search", "mcp:feishu:read_calendar", "mcp:feishu:send_message"]);

        f.db.execute_raw_sql("UPDATE mcp_servers SET enabled=0").unwrap();
        assert_eq!(f.db.companion_capability_ids(&f.companion).unwrap(), vec!["web_search"], "停用的连接器不授权任何工具");
        f.db.execute_raw_sql("UPDATE mcp_servers SET enabled=1").unwrap();

        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &ids).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "create_doc", true).unwrap();
        let widened = f.db.companion_capability_ids(&f.companion).unwrap();
        f.db.freeze_run_companion_tools(&run, &widened).unwrap();
        assert_eq!(f.db.run_companion_tools_snapshot(&run).unwrap().unwrap(), ids, "快照只写一次");

        let call = |tool: &str| json!({"serverId":"feishu","tool":tool,"arguments":{}});
        f.db.ensure_companion_tool_granted(&run, "call_mcp_tool", Some(&call("read_calendar"))).unwrap();
        f.db.ensure_companion_tool_granted(&run, "web_search", None).unwrap();
        f.db.ensure_companion_tool_granted(&run, "list_mcp_tools", None).unwrap();
        assert!(f.db.ensure_companion_tool_granted(&run, "call_mcp_tool", Some(&call("create_doc"))).unwrap_err().contains("下一轮"),
            "本轮开始后新增的授权不能扩大本轮");
        assert!(f.db.ensure_companion_tool_granted(&run, "web_read", None).unwrap_err().contains("未启用"));
        assert!(f.db.ensure_companion_tool_granted(&run, "run_command", None).unwrap_err().contains("私聊不可使用工具"));

        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", false).unwrap();
        assert!(f.db.ensure_companion_tool_granted(&run, "call_mcp_tool", Some(&call("read_calendar"))).unwrap_err().contains("撤销"));
        f.db.ensure_companion_tool_granted(&run, "call_mcp_tool", Some(&call("send_message"))).unwrap();

        let plain = f.db.create_conversation("fox-general", Some("普通对话"), None, None).unwrap();
        let plain_run = f.db.create_run(&plain.id, "普通问题", None).unwrap();
        f.db.ensure_companion_tool_granted(&plain_run.run.id, "run_command", None).unwrap();
        f.db.ensure_companion_tool_granted(&plain_run.run.id, "call_mcp_tool", Some(&call("anything"))).unwrap();

        drop(f.db);
        let db = Database::open(f.root.join("fox.db")).unwrap();
        assert_eq!(db.run_companion_tools_snapshot(&run).unwrap().unwrap(), ids, "重开后快照仍在");
        assert!(db.companion_tool_grants(&f.companion).unwrap().contains(&("feishu".to_owned(), "send_message".to_owned())));
        drop(db);
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[test]
    fn an_approved_call_is_refused_after_revocation_and_never_revives() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &f.db.companion_capability_ids(&f.companion).unwrap()).unwrap();
        let input = json!({"serverId":"feishu","tool":"send_message","arguments":{"text":"你好"}});
        let tool = f.db.create_host_tool_call(&run, "call-1", "call_mcp_tool", &input, "pending", true).unwrap();
        let approval = f.db.create_approval(&tool.id, "发送消息", &input).unwrap();
        f.db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap();

        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", false).unwrap();
        let error = f.db.claim_approved_tool_call(&run, "call-1").unwrap_err();
        assert!(error.contains("撤销"), "{error}");
        let (status, message): (String, String) = f.db.with_connection(|connection| {
            connection.query_row("SELECT status, error_message FROM tool_calls WHERE id=?1", [&tool.id],
                |row| Ok((row.get(0)?, row.get(1)?)))
        }).unwrap();
        assert_eq!(status, "failed", "撤权后的待执行调用落为失败并写明原因");
        assert!(message.contains("撤销"));
        assert!(!f.db.claim_approved_tool_call(&run, "call-1").unwrap(), "恢复路径也不能复活旧批准");

        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        assert!(!f.db.claim_approved_tool_call(&run, "call-1").unwrap(), "重新授权不复活旧批准");
        let again = f.db.create_host_tool_call(&run, "call-2", "call_mcp_tool", &input, "pending", true).unwrap();
        let approval = f.db.create_approval(&again.id, "发送消息", &input).unwrap();
        f.db.resolve_approval(&approval.id, ApprovalDecision::AllowOnce).unwrap();
        assert!(f.db.claim_approved_tool_call(&run, "call-2").unwrap(), "重新发起的新批准可以执行");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn the_workspace_gate_uses_the_same_grant_rules() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &[]).unwrap();
        let input = json!({"serverId":"feishu","tool":"send_message","arguments":{}});
        let (code, _) = crate::companion_workspace::check_run_tool(&f.db, &f.root, &run, "call_mcp_tool", Some(&input), None).unwrap_err();
        assert_eq!(code, "companion.tool_not_available");
        let (code, _) = crate::companion_workspace::check_run_tool(&f.db, &f.root, &run, "web_search", Some(&json!({"query":"报价"})), None).unwrap_err();
        assert_eq!(code, "companion.tool_not_available");
        // A base-whitelist tool passes the grant gate and only then meets the
        // workspace readiness rule (the fixture workspace is still pending).
        let (code, _) = crate::companion_workspace::check_run_tool(&f.db, &f.root, &run, "read", Some(&json!({"path":"a.txt"})), None).unwrap_err();
        assert_eq!(code, "companion.workspace_unavailable");
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    #[test]
    fn grant_validation_rejects_unknown_companions_servers_and_web_tools() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        assert!(f.db.set_companion_tool_grant("cmp_missing", "feishu", "x", true).is_err());
        assert!(f.db.set_companion_tool_grant(&f.companion, "missing", "x", true).is_err());
        assert!(f.db.set_companion_tool_grant(&f.companion, WEB_SERVER_ID, "http_request", true).is_err());
        assert!(f.db.set_companion_tool_grant(&f.companion, "feishu", "", true).is_err());
        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", true).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", true).unwrap();
        assert_eq!(f.db.companion_tool_grants(&f.companion).unwrap().len(), 1, "重复授权幂等");
        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", false).unwrap();
        assert!(f.db.companion_tool_grants(&f.companion).unwrap().is_empty());
        drop(f.db);
        std::fs::remove_dir_all(f.root).unwrap();
    }

    fn invalidated(db: &Database, approval_id: &str) -> bool {
        let record = db.approval_record(approval_id).unwrap();
        crate::database::approval_invalidation(record.decision.as_ref()).is_some()
    }

    #[test]
    fn revocation_registers_a_persistent_barrier_that_survives_regrant_and_restart() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "read_calendar", true).unwrap();
        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &f.db.companion_capability_ids(&f.companion).unwrap()).unwrap();
        let send = json!({"serverId":"feishu","tool":"send_message","arguments":{}});
        let read = json!({"serverId":"feishu","tool":"read_calendar","arguments":{}});
        let send_call = f.db.create_host_tool_call(&run, "call-send", "call_mcp_tool", &send, "pending", true).unwrap();
        let send_approval = f.db.create_approval(&send_call.id, "发送消息", &send).unwrap();
        let read_call = f.db.create_host_tool_call(&run, "call-read", "call_mcp_tool", &read, "pending", true).unwrap();
        let read_approval = f.db.create_approval(&read_call.id, "读日历", &read).unwrap();

        let affected = f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", false).unwrap();
        assert_eq!(affected, vec![send_approval.id.clone()], "只登记精确匹配的工具");
        assert!(invalidated(&f.db, &send_approval.id));
        assert_eq!(f.db.approval_record(&send_approval.id).unwrap().status, "pending", "登记失效不立即结算 pending");
        assert!(!invalidated(&f.db, &read_approval.id), "同 server 的其他工具不受影响");

        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        assert!(invalidated(&f.db, &send_approval.id), "重新授权不删除失效标记");
        let error = f.db.resolve_approval(&send_approval.id, ApprovalDecision::AllowOnce).unwrap_err();
        assert!(error.contains("撤销"), "{error}");
        let settled = f.db.approval_record(&send_approval.id).unwrap();
        assert_ne!(settled.status, "denied", "失效结算不是用户主动拒绝");
        assert_ne!(settled.status, "approved", "失效批准不能被重新授予");
        assert!(crate::database::approval_invalidation(settled.decision.as_ref()).is_some());
        assert!(!f.db.claim_approved_tool_call(&run, "call-send").unwrap(), "结算后不能执行");

        f.db.resolve_approval(&read_approval.id, ApprovalDecision::AllowOnce).unwrap();
        assert!(f.db.claim_approved_tool_call(&run, "call-read").unwrap(), "未撤销的工具照常执行");

        drop(f.db);
        let db = Database::open(f.root.join("fox.db")).unwrap();
        assert!(invalidated(&db, &send_approval.id), "重启后失效仍在");
        drop(db);
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[test]
    fn server_disable_and_delete_invalidate_only_matching_companion_approvals() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        f.seed_server("other", true);
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "other", "ping", true).unwrap();
        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &f.db.companion_capability_ids(&f.companion).unwrap()).unwrap();
        let a = json!({"serverId":"feishu","tool":"send_message","arguments":{}});
        let b = json!({"serverId":"other","tool":"ping","arguments":{}});
        let ca = f.db.create_host_tool_call(&run, "call-a", "call_mcp_tool", &a, "pending", true).unwrap();
        let aa = f.db.create_approval(&ca.id, "A", &a).unwrap();
        let cb = f.db.create_host_tool_call(&run, "call-b", "call_mcp_tool", &b, "pending", true).unwrap();
        let ab = f.db.create_approval(&cb.id, "B", &b).unwrap();

        f.db.set_mcp_server_enabled("feishu", false).unwrap();
        assert!(invalidated(&f.db, &aa.id));
        assert!(!invalidated(&f.db, &ab.id), "其他 server 不受影响");
        let affected = f.db.invalidated_companion_approvals_for_server("feishu").unwrap();
        assert_eq!(affected.len(), 1);
        assert_eq!(affected[0].id, aa.id);

        f.db.set_mcp_server_enabled("feishu", true).unwrap();
        assert!(invalidated(&f.db, &aa.id), "重新启用不复活旧批准");

        f.db.delete_mcp_server("other").unwrap();
        assert!(invalidated(&f.db, &ab.id), "删除在同一事务登记失效");
        drop(f.db);
        std::fs::remove_dir_all(&f.root).unwrap();
    }

    #[test]
    fn create_approval_refuses_to_reuse_an_invalidated_record_after_regrant() {
        let f = Fixture::new();
        f.seed_server("feishu", true);
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        let run = f.running_run();
        f.db.freeze_run_companion_tools(&run, &f.db.companion_capability_ids(&f.companion).unwrap()).unwrap();
        let input = json!({"serverId":"feishu","tool":"send_message","arguments":{}});
        let call = f.db.create_host_tool_call(&run, "call-1", "call_mcp_tool", &input, "pending", true).unwrap();
        f.db.create_approval(&call.id, "发送消息", &input).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", false).unwrap();
        f.db.set_companion_tool_grant(&f.companion, "feishu", "send_message", true).unwrap();
        let error = f.db.create_approval(&call.id, "发送消息", &input).unwrap_err();
        assert!(error.contains("失效"), "{error}");
        drop(f.db);
        std::fs::remove_dir_all(&f.root).unwrap();
    }
}