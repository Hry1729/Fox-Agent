//! dbhash: upgrading an old 0.1.0 database must not fail startup because of
//! historical `run_control_bindings` rows.
//!
//! Every build hashes the frozen permission exactly as it serializes it (struct
//! field order; canonical JSON since 83c16042 keeps struct order, so that change
//! is byte-neutral here). faa1ad04 (2026-09-23) added `kind` to each grant and
//! `approvalEpoch` to the permission: a binding frozen before it decodes with
//! the defaults, re-encodes with the two extra keys, and no longer matches its
//! stored `permissionSnapshotId`. The startup sweep added in 2d494f67
//! (2026-10-29, `protect_restart_owners`) reads every binding with `?`, so one
//! such row failed the whole startup with
//! `Invalid parameter name: frozen permission hash does not match its content`.
//!
//! The fixture rows below are written byte-for-byte the way those builds wrote
//! them (hand-written: struct field order, no `executorRoute`).
use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding, TimeBudgets};

fn sha(text: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes()))) }

#[derive(Clone, Copy, Debug)]
enum Era {
    /// From 2026-09-23 (faa1ad04) on: `kind` and `approvalEpoch` present.
    Revocation,
    /// Before 2026-09-23: neither `kind` nor `approvalEpoch` existed.
    PreRevocation,
}

/// The permission bytes an old build hashed, in struct field order.
fn legacy_permission(era: Era) -> String {
    match era {
        Era::Revocation => r#"{"mode":"ask","projectRoot":"/work/legacy","grants":[{"tool":"read","scope":"/work/legacy","kind":"resource"}],"approvalEpoch":3}"#.into(),
        Era::PreRevocation => r#"{"mode":"ask","projectRoot":"/work/legacy","grants":[{"tool":"read","scope":"/work/legacy"}]}"#.into(),
    }
}

/// A whole binding row as an old build stored it (struct field order, no
/// `executorRoute`, which did not exist before 2026-10-29).
fn insert_legacy_binding(db: &Database, run: &str, conversation: &str, authority: &str, era: Era) {
    let permission = legacy_permission(era);
    let budgets = r#"{"modelRequestMs":300000,"toolExecutionMs":600000,"runExecutionMs":1800000,"approvalWaitMs":300000}"#;
    let json = format!(
        r#"{{"schemaVersion":1,"runId":"{run}","conversationId":"{conversation}","engineId":"pi","executionProfileId":"legacy","authority":"{authority}","readOnlyExecutor":"rust","permissionSnapshotId":"{}","permission":{permission},"budgets":{budgets}}}"#,
        sha(&permission));
    db.with_connection(|connection| connection.execute(
        "INSERT INTO run_control_bindings(run_id,conversation_id,authority,engine_id,binding_json,binding_hash,created_at)
         VALUES(?1,?2,?3,'pi',?4,?5,1)",
        params![run, conversation, authority, json, sha(&json)])).unwrap();
}

/// One Run in its own conversation (a conversation holds one active Run).
fn new_run(db: &Database, status: &str) -> (String, String) {
    let conversation = db.create_conversation("fox-general", Some("upgrade"), None, Some("ask")).unwrap().id;
    let run = db.create_run(&conversation, "upgrade fixture", None).unwrap().run.id;
    db.with_connection(|connection| connection.execute(
        "UPDATE runs SET status=?2,started_at=1,finished_at=CASE WHEN ?2 IN ('completed','failed','cancelled','interrupted') THEN 2 END WHERE id=?1",
        params![run, status])).unwrap();
    (run, conversation)
}

fn legacy_run(db: &Database, status: &str, authority: &str, era: Era) -> String {
    let (run, conversation) = new_run(db, status);
    insert_legacy_binding(db, &run, &conversation, authority, era);
    run
}

fn current_pi_native_run(db: &Database) -> String {
    let (run, conversation) = new_run(db, "running");
    let permission = FrozenPermission { mode: PermissionMode::Ask, project_root: None, grants: Vec::new(), approval_epoch: None };
    db.freeze_run_control(&RunControlBinding {
        schema_version: 1, run_id: run.clone(), conversation_id: conversation, engine_id: "pi".into(),
        execution_profile_id: "legacy".into(), authority: ExecutionAuthority::Authoritative, read_only_executor: ResourceExecutor::Rust,
        permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(), permission,
        budgets: TimeBudgets::default(), executor_route: Some(ExecutorRoute::PiNative),
    }).unwrap();
    run
}

fn tamper_mode(db: &Database, run: &str) {
    db.with_connection(|connection| {
        let json: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1", [run], |row| row.get(0))?;
        let tampered = json.replace(r#""mode":"ask""#, r#""mode":"allow""#);
        assert_ne!(tampered, json);
        connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1", params![run, tampered, sha(&tampered)])
    }).unwrap();
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

fn assert_refused(db: &Database, run: &str, code: &str) {
    let error = db.run_control_binding(run).unwrap_err();
    assert!(error.contains(code), "{run}: {error}");
    assert!(db.apply_runtime_event(run, 99, &json!({"type":"run.completed"})).is_err(), "{run} must not continue");
    assert!(db.create_host_tool_call(run, "late", "read", &json!({"path":"x"}), "running", false).is_err(), "{run}");
    assert!(db.mark_run_failed(run, "test", "late").is_err(), "{run}");
}

#[test]
fn dbhash_only_the_pre_revocation_permission_format_misses_the_current_hash() {
    // Root cause, pinned: rows frozen since faa1ad04 still verify (canonical
    // JSON keeps struct order); rows frozen before it cannot.
    let current = legacy_permission(Era::Revocation);
    let decoded: FrozenPermission = serde_json::from_str(&current).unwrap();
    assert_eq!(Database::run_control_permission_hash(&decoded).unwrap(), sha(&current));
    let old = legacy_permission(Era::PreRevocation);
    let decoded: FrozenPermission = serde_json::from_str(&old).unwrap();
    assert_ne!(Database::run_control_permission_hash(&decoded).unwrap(), sha(&old));
}

#[test]
fn dbhash_an_upgraded_database_with_legacy_bindings_starts_and_interrupts_those_runs() {
    let path = std::env::temp_dir().join(format!("fox-dbhash-{}.db", uuid::Uuid::new_v4()));
    let (active, waiting, kernel_era, done, newer_legacy, newer_kernel, fresh) = {
        let db = Database::open(path.clone()).unwrap();
        (
            legacy_run(&db, "running", "legacy", Era::PreRevocation),
            legacy_run(&db, "queued", "legacy", Era::PreRevocation),
            legacy_run(&db, "running", "authoritative", Era::PreRevocation),
            legacy_run(&db, "completed", "legacy", Era::PreRevocation),
            // Rows frozen after faa1ad04 verify and follow the ordinary paths.
            legacy_run(&db, "running", "legacy", Era::Revocation),
            legacy_run(&db, "running", "authoritative", Era::Revocation),
            current_pi_native_run(&db),
        )
    };

    // 1. Startup succeeds (it used to fail on the first old-format row).
    let db = startup(&path).expect("an upgraded database must start");
    // 2. Unfinished old-format Runs are interrupted with the legacy-format code.
    for run in [&active, &waiting, &kernel_era] {
        assert_eq!(fact(&db, run), ("interrupted".into(), Some("run_control.binding_legacy_format".into()),
            Some("运行控制绑定为旧版格式，历史 Run 已中断，不能继续".into())), "{run}");
    }
    // A finished historical Run keeps its own terminal facts.
    assert_eq!(fact(&db, &done).0, "completed");
    // 3. None of them can continue: every reader still refuses the binding.
    for run in [&active, &waiting, &kernel_era, &done] {
        assert_refused(&db, run, "run_control.binding_legacy_format");
    }
    // Verifiable rows beside them are untouched by the new sweep.
    assert_eq!(fact(&db, &newer_legacy).1.as_deref(), Some("runtime.application_restarted"));
    assert_eq!(fact(&db, &newer_kernel).1.as_deref(), Some("executor_route_retired"));
    assert_eq!(fact(&db, &fresh).1.as_deref(), Some("runtime.application_restarted"));
    for run in [&newer_legacy, &newer_kernel, &fresh] {
        assert!(db.run_control_binding(run).unwrap().is_some(), "{run}");
    }
    drop(db);
    // A second start is a no-op and still succeeds.
    let db = startup(&path).expect("restart");
    assert_eq!(fact(&db, &active).1.as_deref(), Some("run_control.binding_legacy_format"));
    drop(db);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dbhash_a_tampered_binding_is_refused_but_does_not_block_startup() {
    let path = std::env::temp_dir().join(format!("fox-dbhash-tamper-{}.db", uuid::Uuid::new_v4()));
    let (tampered_old, tampered_current, intact_old) = {
        let db = Database::open(path.clone()).unwrap();
        // Permission widened after freezing; the row hash is recomputed so
        // only the frozen permission hash can catch it.
        let tampered_old = legacy_run(&db, "running", "legacy", Era::PreRevocation);
        tamper_mode(&db, &tampered_old);
        let tampered_current = current_pi_native_run(&db);
        tamper_mode(&db, &tampered_current);
        let intact_old = legacy_run(&db, "running", "legacy", Era::PreRevocation);
        (tampered_old, tampered_current, intact_old)
    };
    let db = startup(&path).expect("a tampered row must not block startup");
    for run in [&tampered_old, &tampered_current] {
        assert_eq!(fact(&db, run).0, "interrupted", "{run}");
        assert_eq!(fact(&db, run).1.as_deref(), Some("run_control.binding_integrity_failed"), "{run}");
        assert_refused(&db, run, "run_control.binding_integrity_failed");
        // Never mistaken for an intact old-format row.
        assert!(!db.run_control_binding(run).unwrap_err().contains("binding_legacy_format"), "{run}");
    }
    assert_eq!(fact(&db, &intact_old).1.as_deref(), Some("run_control.binding_legacy_format"));
    drop(db);
    let _ = std::fs::remove_file(&path);
}
