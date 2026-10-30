use super::{Database, DATABASE_SCHEMA_VERSION};
use fox_engine_protocol::{ExecutionAuthority, ExecutorRoute, TimeBudgets};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path, process::{Command, Stdio}};

const TEST: &str = "database::migration_copy_tests::user_data_copy_upgrade_and_backup_rollback_preserve_core_rows";
#[derive(Debug, PartialEq, Eq, Serialize)]
struct RowsDigest { count: usize, sha256: String }

fn checked<T, E>(result: Result<T, E>, phase: &str) -> T {
    result.unwrap_or_else(|_| panic!("U11 failed: {phase}"))
}
fn read_only(path: &Path) -> Connection {
    checked(Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX), "open isolated copy")
}
fn verified_version(db: &Connection) -> i64 {
    let check: String = checked(db.query_row("PRAGMA quick_check", [], |r| r.get(0)), "quick_check");
    assert!(check == "ok", "U11 quick_check failed");
    checked(db.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0)), "schema version")
}
fn quoted(name: &str) -> String { format!("\"{}\"", name.replace('"', "\"\"")) }
fn columns(db: &Connection, table: &str) -> Vec<String> {
    let mut statement = checked(db.prepare(&format!("PRAGMA table_info({})", quoted(table))), "original columns");
    let rows = checked(statement.query_map([], |row| row.get(1)), "original columns");
    checked(rows.collect::<rusqlite::Result<Vec<String>>>(), "original columns")
}
fn rows_digest(db: &Connection, table: &str, columns: &[String], exclude_run: Option<&str>) -> rusqlite::Result<RowsDigest> {
    let fields = columns.iter().map(|name| quoted(name)).collect::<Vec<_>>().join(",");
    let filter = if exclude_run.is_some() { " WHERE run_id <> ?1" } else { "" };
    let mut statement = db.prepare(&format!("SELECT {fields} FROM {}{filter}", quoted(table)))?;
    let mut rows = statement.query(rusqlite::params_from_iter(exclude_run))?;
    let mut digests: Vec<[u8; 32]> = Vec::new();
    while let Some(row) = rows.next()? {
        let mut digest = Sha256::new();
        for index in 0..columns.len() {
            let (kind, bytes): (u8, Vec<u8>) = match row.get_ref(index)? {
                ValueRef::Null => (0, Vec::new()),
                ValueRef::Integer(value) => (1, value.to_le_bytes().to_vec()),
                ValueRef::Real(value) => (2, value.to_bits().to_le_bytes().to_vec()),
                ValueRef::Text(value) => (3, value.to_vec()),
                ValueRef::Blob(value) => (4, value.to_vec()),
            };
            digest.update([kind]); digest.update((bytes.len() as u64).to_le_bytes()); digest.update(bytes);
        }
        digests.push(digest.finalize().into());
    }
    digests.sort_unstable();
    let mut digest = Sha256::new();
    for row in &digests { digest.update(row); }
    Ok(RowsDigest { count: digests.len(), sha256: hex::encode(digest.finalize()) })
}
fn file_digest(path: &Path) -> String { hex::encode(Sha256::digest(checked(fs::read(path), "file digest"))) }
fn write_report(path: &Path, report: &serde_json::Value) {
    checked(fs::write(path, checked(crate::canonical_json::to_vec_pretty(report), "encode report")), "write report");
}

fn binding_child(root: &Path, output: &Path, route: &str) {
    assert!(route == "default", "U11 invalid binding phase");
    let directory = output.join(format!("binding-{route}"));
    checked(fs::create_dir(&directory), "create binding copy");
    checked(fs::copy(root.join("baseline.db"), directory.join("fox.db")), "copy binding baseline");
    let db = checked(Database::open_with_migration_backup(directory.join("fox.db"), &directory.join("backups")), "migrate binding copy");
    let fields = checked(db.with_connection(|connection| Ok(columns(connection, "run_control_bindings"))), "binding columns");
    let before = checked(db.with_connection(|connection| rows_digest(connection, "run_control_bindings", &fields, None)), "old binding digest");
    let conversation = checked(db.create_conversation(db.default_agent_id(), Some("U11 isolated binding"), None, Some("read_only")), "new isolated conversation");
    let run = checked(db.create_run(&conversation.id, "U11 binding only; no model execution", None), "new isolated Run");
    let binding = checked(db.freeze_default_run_control_for_engine(&run.run.id, "legacy", TimeBudgets::continuous(), "pi"), "freeze new binding");
    // A0: no process switch exists; every new Run freezes PiNative.
    let expected = Some(ExecutorRoute::PiNative);
    let route_matches = binding.executor_route == expected && binding.authority == ExecutionAuthority::Authoritative && binding.engine_id == "pi";
    let stored = checked(db.run_control_binding(&run.run.id), "read new binding");
    let persisted = stored.is_some_and(|value| value.executor_route == expected && value.authority == binding.authority && value.engine_id == binding.engine_id);
    let after = checked(db.with_connection(|connection| rows_digest(connection, "run_control_bindings", &fields, Some(&run.run.id))), "old binding digest");
    let unchanged = before == after;
    write_report(&output.join(format!("binding-{route}.json")), &serde_json::json!({"route":route,"route_matches":route_matches,"persisted":persisted,"old_bindings_unchanged":unchanged,"before":before,"after":after}));
    assert!(route_matches && persisted && unchanged, "U11 isolated binding check failed");
}

#[test]
#[ignore = "requires FOX_TEST_USER_DATA_COPY_ROOT with an isolated marked baseline snapshot"]
fn user_data_copy_upgrade_and_backup_rollback_preserve_core_rows() {
    let root = checked(fs::canonicalize(checked(std::env::var_os("FOX_TEST_USER_DATA_COPY_ROOT").ok_or(()), "snapshot env gate")), "snapshot directory");
    let migration_work = checked(fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migration-work")), "repository migration-work");
    assert!(root.parent() == Some(migration_work.as_path()) && root.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("user-data-copy-")), "U11 snapshot must be an isolated repository copy");
    assert!(root.join(".fox-user-data-copy").is_file() && root.join("baseline.db").is_file(), "U11 snapshot marker and baseline required");
    // Read-only SQLite checks can leave an empty WAL and a SHM coordination file.
    let wal = root.join("baseline.db-wal");
    assert!(!wal.exists() || checked(fs::metadata(&wal), "baseline WAL metadata").len() == 0, "U11 baseline must have no nonempty WAL");
    if let Ok(route) = std::env::var("FOX_TEST_USER_DATA_COPY_BINDING_ROUTE") {
        let output = root.join(checked(std::env::var("FOX_TEST_USER_DATA_COPY_OUTPUT"), "binding output"));
        assert!(output.parent() == Some(root.as_path()) && output.is_dir(), "U11 isolated output required");
        binding_child(&root, &output, &route); return;
    }
    let output = root.join(format!("u11-{}", uuid::Uuid::new_v4()));
    checked(fs::create_dir(&output), "create evidence directory");
    let baseline = root.join("baseline.db"); let original_sha = file_digest(&baseline);
    let original = read_only(&baseline); let old_version = verified_version(&original);
    assert!(old_version < DATABASE_SCHEMA_VERSION, "U11 baseline must require migration");
    let mut statement = checked(original.prepare("SELECT name FROM sqlite_master WHERE type='table' AND (name IN ('conversations','projects','messages','runs','run_control_bindings','runtime_sessions','run_events','tool_calls','approvals','execution_receipts') OR name LIKE 'work_%' OR name LIKE 'kernel_%') ORDER BY name"), "core tables");
    let names = checked(statement.query_map([], |row| row.get(0)), "core tables");
    let names = checked(names.collect::<rusqlite::Result<Vec<String>>>(), "core tables");
    assert!(["messages", "runs", "run_control_bindings", "runtime_sessions"].iter().all(|name| names.iter().any(|table| table == name)), "U11 required core tables missing");
    let mut snapshots = BTreeMap::new();
    for table in names { let fields = columns(&original, &table); let digest = checked(rows_digest(&original, &table, &fields, None), &format!("digest {table}")); snapshots.insert(table, (fields, digest)); }
    let upgrade = output.join("upgrade"); let rollback = output.join("rollback-copy");
    checked(fs::create_dir(&upgrade), "create upgrade directory"); checked(fs::create_dir(&rollback), "create rollback directory");
    checked(fs::copy(&baseline, upgrade.join("fox.db")), "copy baseline");
    drop(checked(Database::open_with_migration_backup(upgrade.join("fox.db"), &upgrade.join("backups")), "real copy migration and automatic backup"));
    let backups = checked(fs::read_dir(upgrade.join("backups")), "automatic backups").map(|entry| checked(entry, "automatic backup entry").path()).filter(|path| path.extension().is_some_and(|extension| extension == "db")).collect::<Vec<_>>();
    assert!(backups.len() == 1, "U11 expected one automatic migration backup");
    checked(fs::copy(&backups[0], rollback.join("fox.db")), "restore automatic backup into rollback copy");
    let upgraded = read_only(&upgrade.join("fox.db")); let backup = read_only(&backups[0]); let restored = read_only(&rollback.join("fox.db"));
    let versions = [verified_version(&upgraded), verified_version(&backup), verified_version(&restored)];
    let schema_matches = versions == [DATABASE_SCHEMA_VERSION, old_version, old_version];
    let mut tables = BTreeMap::new(); let mut unchanged = true;
    for (table, (fields, expected)) in snapshots {
        let actual = [&upgraded, &backup, &restored].map(|db| checked(rows_digest(db, &table, &fields, None), &format!("digest {table}")));
        let matches = actual.iter().all(|value| value == &expected); unchanged &= matches;
        tables.insert(table, serde_json::json!({"baseline":expected,"upgrade":actual[0],"automatic_backup":actual[1],"rollback_copy":actual[2],"matches":matches}));
    }
    let mut bindings = BTreeMap::new();
    for route in ["default"] {
        let mut command = Command::new(checked(std::env::current_exe(), "current candidate test binary"));
        command.args(["--ignored", "--exact", TEST]).env("FOX_TEST_USER_DATA_COPY_ROOT", &root).env("FOX_TEST_USER_DATA_COPY_OUTPUT", checked(output.file_name().ok_or(()), "evidence name")).env("FOX_TEST_USER_DATA_COPY_BINDING_ROUTE", route).env("FOX_KERNEL_MODE", "authoritative").stdout(Stdio::null()).stderr(Stdio::null());
        bindings.insert(route, checked(command.status(), "start isolated binding process").success());
    }
    let baseline_sha_after = file_digest(&baseline); let baseline_unchanged = original_sha == baseline_sha_after;
    let valid = schema_matches && unchanged && baseline_unchanged && bindings.values().all(|value| *value);
    write_report(&output.join("report.json"), &serde_json::json!({"scope":"copy DB migration and automatic-backup rollback; no desktop, old binary, model execution or runtime-file recovery","valid":valid,"baseline_schema":old_version,"candidate_schema":DATABASE_SCHEMA_VERSION,"observed_versions":versions,"quick_check":"ok","baseline_sha256_before":original_sha,"baseline_sha256_after":baseline_sha_after,"baseline_unchanged":baseline_unchanged,"core_tables":tables,"binding_processes":bindings}));
    assert!(valid, "U11 copy migration evidence failed; inspect count/hash report");
}
