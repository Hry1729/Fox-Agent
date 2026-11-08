//! C02/K4: tool-level companion capability grants.
//!
//! A grant is (companion, connector instance, tool). It is necessary but never
//! sufficient: the run catalog and every execution entry intersect it with the
//! installed, enabled reality, and a per-run snapshot freezes what the run
//! started with, so a mid-run grant never widens the current run and a
//! revocation stops calls that were approved but not yet executed.
use super::{now_ms, Database};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// The built-in web capability pair. Not an mcp_servers row.
pub(crate) const WEB_SERVER_ID: &str = "@web";
pub(crate) const WEB_TOOLS: [&str; 2] = ["web_search", "web_read"];

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
    let companion: Option<String> = connection.query_row(
        "SELECT c.companion_id FROM runs r JOIN conversations c ON c.id = r.conversation_id
         WHERE r.id = ?1 AND c.companion_id IS NOT NULL",
        [run_id],
        |row| row.get(0),
    ).optional().map_err(|error| error.to_string())?;
    let Some(companion) = companion else { return Ok(()) };
    let Some(required) = required_capability(tool, input)? else { return Ok(()) };
    let current = current_capability_ids_on(connection, &companion)?;
    let snapshot = run_snapshot_on(connection, run_id)?.unwrap_or_else(|| current.clone());
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

    pub fn set_companion_tool_grant(
        &self,
        companion_id: &str,
        server_id: &str,
        tool: &str,
        granted: bool,
    ) -> Result<(), String> {
        let tool = tool.trim();
        if tool.is_empty() || tool.chars().count() > 200 {
            return Err("工具名无效".to_owned());
        }
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        let result = (|| -> rusqlite::Result<()> {
            let exists: bool = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM companion_profiles WHERE id = ?1 AND deleting_at IS NULL)",
                [companion_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(rusqlite::Error::QueryReturnedNoRows);
            }
            if server_id == WEB_SERVER_ID {
                if !WEB_TOOLS.contains(&tool) {
                    return Err(rusqlite::Error::InvalidQuery);
                }
            } else {
                let server: Option<String> = connection.query_row(
                    "SELECT id FROM mcp_servers WHERE id = ?1",
                    [server_id],
                    |row| row.get(0),
                ).optional()?;
                if server.is_none() {
                    return Err(rusqlite::Error::QueryReturnedNoRows);
                }
            }
            if granted {
                connection.execute(
                    "INSERT OR IGNORE INTO companion_tool_grants(companion_id, server_id, tool, created_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![companion_id, server_id, tool, now_ms()],
                )?;
            } else {
                connection.execute(
                    "DELETE FROM companion_tool_grants WHERE companion_id = ?1 AND server_id = ?2 AND tool = ?3",
                    params![companion_id, server_id, tool],
                )?;
            }
            Ok(())
        })();
        result.map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => "联系人或连接器不存在".to_owned(),
            rusqlite::Error::InvalidQuery => "网页能力只支持 web_search 与 web_read".to_owned(),
            other => other.to_string(),
        })
    }

    pub fn companion_capability_ids(&self, companion_id: &str) -> Result<Vec<String>, String> {
        let connection = self.connection.lock().map_err(|_| "database lock is poisoned".to_owned())?;
        current_capability_ids_on(&connection, companion_id)
    }

    /// Freeze the run's capability snapshot. Write-once, like the model
    /// snapshot: queueing, retry and restart never re-resolve it.
    pub fn freeze_run_companion_tools(&self, run_id: &str, ids: &[String]) -> Result<(), String> {
        let json = serde_json::to_string(ids).map_err(|error| error.to_string())?;
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
}