//! dbhash: one bad `run_control_bindings` row must not fail startup.
//!
//! The startup sweep (`protect_restart_owners`, 2d494f67) used to read every
//! binding with `?`, so a single row that failed verification aborted the whole
//! application with `Invalid parameter name: frozen permission hash does not
//! match its content`. Now that Run alone is interrupted with
//! `run_control.binding_integrity_failed` and every reader still refuses it.
use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding, TimeBudgets};

fn sha(text: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes()))) }

/// One running PiNative Run in its own conversation (one active Run each).
fn running_run(db: &Database) -> String {
    let conversation = db.create_conversation("fox-general", Some("dbhash"), None, Some("ask")).unwrap().id;
    let run = db.create_run(&conversation, "dbhash fixture", None).unwrap().run.id;
    let permission = FrozenPermission { mode: PermissionMode::Ask, project_root: None, grants: Vec::new(), approval_epoch: None };
    db.freeze_run_control(&RunControlBinding {
        schema_version: 1, run_id: run.clone(), conversation_id: conversation, engine_id: "pi".into(),
        execution_profile_id: "legacy".into(), authority: ExecutionAuthority::Authoritative, read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(), permission,
        budgets: TimeBudgets::default(), executor_route: Some(ExecutorRoute::PiNative),
    }).unwrap();
    db.with_connection(|connection| connection.execute(
        "UPDATE runs SET status='running',started_at=1 WHERE id=?1", [&run])).unwrap();
    run
}

fn fact(db: &Database, run: &str) -> (String, Option<String>, Option<String>) {
    db.with_connection(|connection| connection.query_row(
        "SELECT status,error_code,error_message FROM runs WHERE id=?1", [run],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))).unwrap()
}

/// The same steps `lib.rs` runs at startup, in order.
fn startup(path: &std::path::Path) -> Result<Database, String> {
    let db = Database::open(path.to_path_buf())?;
    db.repair_interrupted_runs()?;
    db.audit_interrupted_tasks()?;
    Ok(db)
}

#[test]
fn dbhash_a_bad_binding_interrupts_only_its_run_and_never_blocks_startup() {
    let path = std::env::temp_dir().join(format!("fox-dbhash-{}.db", uuid::Uuid::new_v4()));
    let (tampered, corrupt_row, healthy) = {
        let db = Database::open(path.clone()).unwrap();
        // Permission widened after freezing, row hash recomputed: only the
        // frozen permission hash can catch it.
        let tampered = running_run(&db);
        db.with_connection(|connection| {
            let json: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1", [&tampered], |row| row.get(0))?;
            let changed = json.replace(r#""mode":"ask""#, r#""mode":"allow""#);
            assert_ne!(changed, json);
            connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1", params![tampered, changed, sha(&changed)])
        }).unwrap();
        // Row hash itself broken.
        let corrupt_row = running_run(&db);
        db.with_connection(|connection| connection.execute(
            "UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id=?1", [&corrupt_row])).unwrap();
        (tampered, corrupt_row, running_run(&db))
    };

    // 1. Startup succeeds.
    let db = startup(&path).expect("one bad binding must not block startup");
    for run in [&tampered, &corrupt_row] {
        // 2. Only that Run is interrupted, with the stable code and reason.
        assert_eq!(fact(&db, run), ("interrupted".into(), Some("run_control.binding_integrity_failed".into()),
            Some("运行控制绑定校验失败，此 Run 已中断，不能继续".into())), "{run}");
        // 3. It can never continue: every reader and writer still refuses it.
        assert!(db.run_control_binding(run).unwrap_err().contains("run_control.binding_integrity_failed"), "{run}");
        assert!(db.apply_runtime_event(run, 99, &json!({"type":"run.completed"})).is_err(), "{run}");
        assert!(db.create_host_tool_call(run, "late", "read", &json!({"path":"x"}), "running", false).is_err(), "{run}");
        assert!(db.mark_run_failed(run, "test", "late").is_err(), "{run}");
    }
    // A healthy Run beside them takes the ordinary restart path.
    assert_eq!(fact(&db, &healthy).1.as_deref(), Some("runtime.application_restarted"));
    assert!(db.run_control_binding(&healthy).unwrap().is_some());
    drop(db);
    // A second start is a no-op and still succeeds.
    let db = startup(&path).expect("restart");
    assert_eq!(fact(&db, &tampered).1.as_deref(), Some("run_control.binding_integrity_failed"));
    drop(db);
    let _ = std::fs::remove_file(&path);
}

/// 绑定损坏且本次确实被改成 interrupted 的 Run，要在同一次启动事务里回写自己的
/// assistant 消息、未结束工具调用和待处理审批。已经是终态的 Run 一行都不改。
/// 通用重启修复只会改 streaming 消息、且不写 tool error_message、审批原因是
/// application_restarted，所以这里用 failed 消息、工具错误文案和 run_interrupted 区分。
#[test]
fn binding_integrity_failure_settles_open_rows_only_when_the_run_is_interrupted() {
    let path = std::env::temp_dir().join(format!("fox-dbhash-rows-{}.db", uuid::Uuid::new_v4()));
    let (active, settled) = {
        let db = Database::open(path.clone()).unwrap();
        let active = running_run(&db);
        let settled = running_run(&db);
        db.with_connection(|connection| {
            let active_conversation: String = connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1", [&active], |row| row.get(0))?;
            let settled_conversation: String = connection.query_row(
                "SELECT conversation_id FROM runs WHERE id=?1", [&settled], |row| row.get(0))?;
            for (id, role, status, ordinal) in [
                ("binding-active-streaming", "assistant", "streaming", 2),
                ("binding-active-failed", "assistant", "failed", 3),
                ("binding-active-error", "assistant", "error", 4),
                ("binding-active-pending", "assistant", "pending", 5),
                ("binding-active-user", "user", "failed", 6),
            ] {
                connection.execute(
                    "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
                     VALUES(?1,?2,?3,?4,'text','row',?5,?6,1,1)",
                    params![id, active_conversation, active, role, status, ordinal])?;
            }
            connection.execute(
                "INSERT INTO tool_calls(id,runtime_tool_call_id,run_id,conversation_id,tool_name,input_json,status,
                    execution_location,requires_approval,started_at,updated_at)
                 VALUES('binding-active-tool','binding-active-call',?1,?2,'read','{}','pending','host',1,1,1)",
                params![active, active_conversation])?;
            connection.execute(
                "INSERT INTO approvals(id,tool_call_id,status,requested_action,request_json,requested_at)
                 VALUES('binding-active-approval','binding-active-tool','pending','read','{}',1)",
                [])?;
            connection.execute(
                "UPDATE runs SET status='completed', finished_at=1, error_code=NULL, error_message=NULL WHERE id=?1",
                [&settled])?;
            connection.execute(
                "INSERT INTO messages(id,conversation_id,run_id,role,kind,content,status,ordinal,created_at,updated_at)
                 VALUES('binding-settled-assistant',?1,?2,'assistant','text','kept','failed',2,1,1)",
                params![settled_conversation, settled])?;
            connection.execute(
                "UPDATE run_control_bindings SET binding_hash='corrupt' WHERE run_id IN (?1,?2)",
                params![active, settled])?;
            Ok(())
        }).unwrap();
        (active, settled)
    };

    let db = startup(&path).expect("one bad binding must not block startup");
    assert_eq!(fact(&db, &active), ("interrupted".into(), Some("run_control.binding_integrity_failed".into()),
        Some("运行控制绑定校验失败，此 Run 已中断，不能继续".into())));
    let messages: Vec<(String, String, i64)> = db.with_connection(|connection| {
        let mut query = connection.prepare(
            "SELECT id,status,updated_at FROM messages
             WHERE id IN ('binding-active-streaming','binding-active-failed','binding-active-error','binding-active-pending','binding-active-user')
             ORDER BY ordinal")?;
        let rows = query.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        Ok(rows)
    }).unwrap();
    assert_eq!(messages.iter().map(|(id, status, _)| (id.as_str(), status.as_str())).collect::<Vec<_>>(), vec![
        ("binding-active-streaming", "interrupted"),
        ("binding-active-failed", "interrupted"),
        ("binding-active-error", "interrupted"),
        ("binding-active-pending", "interrupted"),
        ("binding-active-user", "failed"),
    ]);
    assert!(messages.iter().take(4).all(|(_, _, updated)| *updated > 1));
    assert_eq!(messages[4].2, 1);
    let tool: (String, Option<String>, Option<i64>) = db.with_connection(|connection| connection.query_row(
        "SELECT status,error_message,completed_at FROM tool_calls WHERE id='binding-active-tool'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))).unwrap();
    assert_eq!(tool.0, "interrupted");
    assert_eq!(tool.1.as_deref(), Some("运行控制绑定校验失败，此 Run 已中断，不能继续"));
    assert!(tool.2.is_some_and(|at| at > 1));
    let approval: (String, String, Option<i64>) = db.with_connection(|connection| connection.query_row(
        "SELECT status,decision_json,resolved_at FROM approvals WHERE id='binding-active-approval'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))).unwrap();
    assert_eq!(approval.0, "expired");
    assert_eq!(approval.1, r#"{"reason":"run_interrupted"}"#);
    assert!(approval.2.is_some_and(|at| at > 1));

    let settled_fact: (String, Option<String>, Option<String>, i64) = db.with_connection(|connection| connection.query_row(
        "SELECT status,error_code,error_message,finished_at FROM runs WHERE id=?1", [&settled],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))).unwrap();
    assert_eq!(settled_fact, ("completed".into(), None, None, 1));
    let settled_message: (String, i64) = db.with_connection(|connection| connection.query_row(
        "SELECT status,updated_at FROM messages WHERE id='binding-settled-assistant'",
        [], |row| Ok((row.get(0)?, row.get(1)?)))).unwrap();
    assert_eq!(settled_message, ("failed".into(), 1));
    drop(db);
    let _ = std::fs::remove_file(&path);
}
