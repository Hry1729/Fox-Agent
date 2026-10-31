//! dbhash: upgrading a 0.1.0 database whose `run_control_bindings` were frozen
//! before canonical JSON (83c16042, 2026-10-21) must not fail startup.
//!
//! Before 83c16042 the Host hashed `serde_json::to_string(&permission)` and
//! stored `serde_json::to_string(&binding)`: struct field order
//! (`mode, projectRoot, grants[tool, scope, kind], approvalEpoch`). Every build
//! since recomputes the hash over canonical (sorted) JSON, so such a binding no
//! longer verifies; the 10-29 startup sweep (`protect_restart_owners`) reads
//! every binding with `?`, so one such row failed the whole startup with
//! `Invalid parameter name: frozen permission hash does not match its content`.
//!
//! The fixture rows below are written byte-for-byte the way those builds wrote
//! them (hand-written; the old serializer is not reachable from this build).
use super::*;
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, FrozenPermission, PermissionMode, ResourceExecutor, RunControlBinding, TimeBudgets};

fn sha(text: &str) -> String { format!("sha256:{}", hex::encode(Sha256::digest(text.as_bytes()))) }

#[derive(Clone, Copy, Debug)]
enum Era {
    /// 2026-09-23 (faa1ad04) .. 2026-10-21 (83c16042): `kind` and `approvalEpoch` present.
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

fn new_run(db: &Database, conversation: &str, status: &str) -> String {
    let run = db.create_run(conversation, "upgrade fixture", None).unwrap().run.id;
    db.with_connection(|connection| connection.execute(
        "UPDATE runs SET status=?2,started_at=1,finished_at=CASE WHEN ?2 IN ('completed','failed','cancelled','interrupted') THEN 2 END WHERE id=?1",
        params![run, status])).unwrap();
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

fn assert_refused(db: &Database, run: &str, code: &str) {
    let error = db.run_control_binding(run).unwrap_err();
    assert!(error.contains(code), "{run}: {error}");
    assert!(db.apply_runtime_event(run, 99, &json!({"type":"run.completed"})).is_err(), "{run} must not continue");
    assert!(db.create_host_tool_call(run, "late", "read", &json!({"path":"x"}), "running", false).is_err(), "{run}");
    assert!(db.mark_run_failed(run, "test", "late").is_err(), "{run}");
}

#[test]
fn dbhash_the_legacy_permission_bytes_never_match_the_canonical_hash() {
    // Root cause, pinned: the old bytes are what was hashed, the canonical
    // re-encoding of the same permission is not.
    for era in [Era::Revocation, Era::PreRevocation] {
        let bytes = legacy_permission(era);
        let decoded: FrozenPermission = serde_json::from_str(&bytes).unwrap();
        assert_ne!(Database::run_control_permission_hash(&decoded).unwrap(), sha(&bytes), "{era:?}");
    }
}

#[test]
fn dbhash_an_upgraded_database_with_legacy_bindings_starts_and_interrupts_those_runs() {
    let path = std::env::temp_dir().join(format!("fox-dbhash-{}.db", uuid::Uuid::new_v4()));
    let (active, waiting, done, kernel_era, fresh) = {
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("upgrade"), None, Some("ask")).unwrap().id;
        let active = new_run(&db, &conversation, "running");
        insert_legacy_binding(&db, &active, &conversation, "legacy", Era::Revocation);
        let waiting = new_run(&db, &conversation, "queued");
        insert_legacy_binding(&db, &waiting, &conversation, "legacy", Era::PreRevocation);
        let done = new_run(&db, &conversation, "completed");
        insert_legacy_binding(&db, &done, &conversation, "legacy", Era::Revocation);
        let kernel_era = new_run(&db, &conversation, "running");
        insert_legacy_binding(&db, &kernel_era, &conversation, "authoritative", Era::Revocation);
        // A Run frozen by this build next to them is unaffected.
        let fresh = new_run(&db, &conversation, "running");
        let permission = FrozenPermission { mode: PermissionMode::Ask, project_root: None, grants: Vec::new(), approval_epoch: None };
        db.freeze_run_control(&RunControlBinding {
            schema_version: 1, run_id: fresh.clone(), conversation_id: conversation.clone(), engine_id: "pi".into(),
            execution_profile_id: "legacy".into(), authority: ExecutionAuthority::Authoritative, read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(), permission,
            budgets: TimeBudgets::default(), executor_route: Some(ExecutorRoute::PiNative),
        }).unwrap();
        (active, waiting, done, kernel_era, fresh)
    };

    // 1. Startup succeeds (it used to fail on the first legacy row).
    let db = startup(&path).expect("an upgraded database must start");
    // 2. Unfinished legacy Runs are interrupted with the legacy-format code.
    for run in [&active, &waiting, &kernel_era] {
        assert_eq!(fact(&db, run), ("interrupted".into(), Some("run_control.binding_legacy_format".into()),
            Some("运行控制绑定为旧版格式，历史 Run 已中断，不能继续".into())), "{run}");
    }
    // A finished historical Run keeps its own terminal facts.
    assert_eq!(fact(&db, &done).0, "completed");
    assert_eq!(fact(&db, &fresh).1.as_deref(), Some("runtime.application_restarted"));
    // 3. None of them can continue: every reader still refuses the binding.
    for run in [&active, &waiting, &done, &kernel_era] {
        assert_refused(&db, run, "run_control.binding_legacy_format");
    }
    assert!(db.run_control_binding(&fresh).unwrap().is_some());
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
    let (tampered_legacy, tampered_canonical, intact) = {
        let db = Database::open(path.clone()).unwrap();
        let conversation = db.create_conversation("fox-general", Some("tamper"), None, Some("ask")).unwrap().id;
        // Legacy row whose permission was widened after freezing; the row hash
        // is recomputed so only the frozen permission hash can catch it.
        let tampered_legacy = new_run(&db, &conversation, "running");
        insert_legacy_binding(&db, &tampered_legacy, &conversation, "legacy", Era::Revocation);
        db.with_connection(|connection| {
            let json: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1", [&tampered_legacy], |row| row.get(0))?;
            let json = json.replace(r#""mode":"ask""#, r#""mode":"allow""#);
            connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1", params![tampered_legacy, json, sha(&json)])
        }).unwrap();
        // Current-format row with the same tampering.
        let tampered_canonical = new_run(&db, &conversation, "running");
        let permission = FrozenPermission { mode: PermissionMode::Ask, project_root: None, grants: Vec::new(), approval_epoch: None };
        db.freeze_run_control(&RunControlBinding {
            schema_version: 1, run_id: tampered_canonical.clone(), conversation_id: conversation.clone(), engine_id: "pi".into(),
            execution_profile_id: "legacy".into(), authority: ExecutionAuthority::Authoritative, read_only_executor: ResourceExecutor::Rust,
            permission_snapshot_id: Database::run_control_permission_hash(&permission).unwrap(), permission,
            budgets: TimeBudgets::default(), executor_route: Some(ExecutorRoute::PiNative),
        }).unwrap();
        db.with_connection(|connection| {
            let json: String = connection.query_row("SELECT binding_json FROM run_control_bindings WHERE run_id=?1", [&tampered_canonical], |row| row.get(0))?;
            let json = json.replace(r#""mode":"ask""#, r#""mode":"allow""#);
            connection.execute("UPDATE run_control_bindings SET binding_json=?2,binding_hash=?3 WHERE run_id=?1", params![tampered_canonical, json, sha(&json)])
        }).unwrap();
        // An intact legacy Run beside them.
        let intact = new_run(&db, &conversation, "running");
        insert_legacy_binding(&db, &intact, &conversation, "legacy", Era::PreRevocation);
        (tampered_legacy, tampered_canonical, intact)
    };
    let db = startup(&path).expect("a tampered row must not block startup");
    for run in [&tampered_legacy, &tampered_canonical] {
        assert_eq!(fact(&db, run).0, "interrupted", "{run}");
        assert_eq!(fact(&db, run).1.as_deref(), Some("run_control.binding_integrity_failed"), "{run}");
        assert_refused(&db, run, "run_control.binding_integrity_failed");
        // Never mistaken for an intact legacy row.
        assert!(!db.run_control_binding(run).unwrap_err().contains("binding_legacy_format"), "{run}");
    }
    assert_eq!(fact(&db, &intact).1.as_deref(), Some("run_control.binding_legacy_format"));
    drop(db);
    let _ = std::fs::remove_file(&path);
}
