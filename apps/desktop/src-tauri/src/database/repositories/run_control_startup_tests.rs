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
