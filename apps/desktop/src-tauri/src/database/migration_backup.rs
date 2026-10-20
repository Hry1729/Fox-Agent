//! Automatic backup before a pending schema migration.
//!
//! Before an existing desktop database is migrated to a newer schema, a
//! consistent copy is written with `VACUUM INTO` (it reads through the WAL, so
//! the copy is a single self-contained file; the live `-wal` / `-shm` files are
//! never copied by hand). Copies live in `<Fox data>/backups/` as
//! `fox-pre-migration-<UTC time>-v<old version>.db`; only the newest
//! [`MIGRATION_BACKUP_KEEP`] of them are kept (user-made `.foxbackup` archives in
//! the same folder are never touched).
//!
//! * No pending migration (already current, or newer than this build): no copy.
//! * A brand-new / empty database has nothing to lose: no copy.
//! * The copy fails or does not verify: the caller must NOT migrate.
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::path::{Path, PathBuf};

/// How many pre-migration copies are kept.
pub(crate) const MIGRATION_BACKUP_KEEP: usize = 5;
const PREFIX: &str = "fox-pre-migration-";
const SUFFIX: &str = ".db";

/// The schema version an existing database is at when it still has migrations
/// to apply (`Some(old)`), or `None` when nothing would be migrated or there is
/// nothing to back up (no tables yet).
pub(crate) fn pending_migration_from(connection: &Connection, latest: i64) -> Result<Option<i64>, String> {
    let tables: i64 = connection
        .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table'", [], |row| row.get(0))
        .map_err(|error| format!("无法读取数据库结构：{error}"))?;
    if tables == 0 {
        return Ok(None);
    }
    let has_versions: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| format!("无法读取数据库结构：{error}"))?;
    if !has_versions {
        // Tables but no version record: treat as version 0 (everything pending).
        return Ok(Some(0));
    }
    let current: Option<i64> = connection
        .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0))
        .optional()
        .map_err(|error| format!("无法读取数据库版本：{error}"))?
        .flatten();
    let current = current.unwrap_or(0);
    Ok((current < latest).then_some(current))
}

fn timestamp(now_ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(now_ms)
        .unwrap_or_default()
        .format("%Y%m%dT%H%M%S%3fZ")
        .to_string()
}

/// Back the database at `db_path` up into `backups_dir` when it has a pending
/// migration. Returns the copy's path, or `None` when no copy was needed.
pub(crate) fn backup_before_migration(
    db_path: &Path,
    backups_dir: &Path,
    latest: i64,
    keep: usize,
    now_ms: i64,
) -> Result<Option<PathBuf>, String> {
    if !db_path.exists() {
        return Ok(None);
    }
    // Opened without CREATE so a missing file is never created here, and
    // without changing journal mode or anything else about the live file.
    let connection = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|error| format!("无法打开数据库以备份：{error}"))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    let Some(old_version) = pending_migration_from(&connection, latest)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(backups_dir)
        .map_err(|error| format!("无法创建备份目录 {}：{error}", backups_dir.display()))?;
    let stem = format!("{PREFIX}{}-v{old_version}", timestamp(now_ms));
    let mut target = backups_dir.join(format!("{stem}{SUFFIX}"));
    let mut attempt = 1;
    while target.exists() {
        attempt += 1;
        target = backups_dir.join(format!("{stem}-{attempt}{SUFFIX}"));
    }
    let partial = target.with_extension("db.partial");
    if partial.exists() {
        std::fs::remove_file(&partial).map_err(|error| format!("无法清理残留的未完成备份：{error}"))?;
    }
    let result = (|| {
        connection
            .execute("VACUUM INTO ?1", [partial.to_string_lossy().as_ref()])
            .map_err(|error| format!("VACUUM INTO 失败：{error}"))?;
        drop(connection);
        verify_copy(&partial, old_version)?;
        std::fs::rename(&partial, &target).map_err(|error| format!("无法完成备份文件：{error}"))
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&partial);
        return Err(error);
    }
    prune(backups_dir, keep)?;
    Ok(Some(target))
}

/// The copy opens, passes SQLite's quick check and carries the old version.
fn verify_copy(path: &Path, old_version: i64) -> Result<(), String> {
    let copy = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|error| format!("备份文件无法打开：{error}"))?;
    let check: String = copy
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(|error| format!("备份文件校验失败：{error}"))?;
    if check != "ok" {
        return Err(format!("备份文件校验失败：{check}"));
    }
    if old_version > 0 {
        let version: i64 = copy
            .query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |row| row.get(0))
            .map_err(|error| format!("备份文件缺少版本记录：{error}"))?;
        if version != old_version {
            return Err(format!("备份文件版本 {version} 与原库 {old_version} 不一致"));
        }
    }
    Ok(())
}

/// Keep the newest `keep` pre-migration copies (names sort chronologically).
fn prune(backups_dir: &Path, keep: usize) -> Result<(), String> {
    let mut copies = std::fs::read_dir(backups_dir)
        .map_err(|error| format!("无法读取备份目录：{error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(PREFIX) && name.ends_with(SUFFIX))
        })
        .collect::<Vec<_>>();
    copies.sort();
    let excess = copies.len().saturating_sub(keep.max(1));
    for old in copies.into_iter().take(excess) {
        std::fs::remove_file(&old).map_err(|error| format!("无法删除旧备份 {}：{error}", old.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fox-migration-backup-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn database_at(path: &Path, version: Option<i64>) {
        let connection = Connection::open(path).unwrap();
        connection.pragma_update(None, "journal_mode", "WAL").unwrap();
        connection.execute_batch("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL);").unwrap();
        if let Some(version) = version {
            connection
                .execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);")
                .unwrap();
            connection.execute("INSERT INTO schema_migrations(version, applied_at) VALUES (?1, 0)", [version]).unwrap();
        }
        // Left in the WAL (no checkpoint) on purpose: the copy must include it.
        connection.execute("INSERT INTO notes(body) VALUES ('only in the wal')", []).unwrap();
        std::mem::forget(connection);
    }

    #[test]
    fn a_pending_migration_is_backed_up_consistently_with_its_wal() {
        let dir = temp_dir("pending");
        let db = dir.join("fox.db");
        database_at(&db, Some(94));
        let backups = dir.join("backups");
        let copy = backup_before_migration(&db, &backups, 95, 5, 1_760_000_000_000).unwrap().expect("a copy");
        let name = copy.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(name.starts_with("fox-pre-migration-20251009T") && name.ends_with("-v94.db"), "{name}");
        let restored = Connection::open(&copy).unwrap();
        let body: String = restored.query_row("SELECT body FROM notes", [], |row| row.get(0)).unwrap();
        assert_eq!(body, "only in the wal");
        assert!(!copy.with_extension("db-wal").exists(), "a self-contained copy");
    }

    #[test]
    fn no_pending_migration_or_an_empty_database_makes_no_copy() {
        let dir = temp_dir("none");
        let backups = dir.join("backups");
        let current = dir.join("current.db");
        database_at(&current, Some(95));
        assert_eq!(backup_before_migration(&current, &backups, 95, 5, 1).unwrap(), None);
        let newer = dir.join("newer.db");
        database_at(&newer, Some(96));
        assert_eq!(backup_before_migration(&newer, &backups, 95, 5, 1).unwrap(), None);
        let empty = dir.join("empty.db");
        Connection::open(&empty).unwrap();
        assert_eq!(backup_before_migration(&empty, &backups, 95, 5, 1).unwrap(), None);
        assert_eq!(backup_before_migration(&dir.join("missing.db"), &backups, 95, 5, 1).unwrap(), None);
        assert!(!dir.join("missing.db").exists(), "never created by the backup step");
        assert!(!backups.exists() || std::fs::read_dir(&backups).unwrap().next().is_none());
    }

    #[test]
    fn only_the_newest_copies_are_kept_and_user_archives_are_untouched() {
        let dir = temp_dir("prune");
        let db = dir.join("fox.db");
        database_at(&db, Some(90));
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("fox-backup-1.foxbackup"), b"user archive").unwrap();
        let mut made = Vec::new();
        for step in 0..7_i64 {
            made.push(backup_before_migration(&db, &backups, 95, 5, 1_760_000_000_000 + step * 1_000).unwrap().unwrap());
        }
        let mut left = std::fs::read_dir(&backups)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.starts_with(PREFIX))
            .collect::<Vec<_>>();
        left.sort();
        assert_eq!(left.len(), 5);
        let expected = made[2..].iter().map(|path| path.file_name().unwrap().to_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(left, expected, "the two oldest copies were removed");
        assert!(backups.join("fox-backup-1.foxbackup").exists());
    }

    #[test]
    fn a_failed_copy_is_an_error_and_leaves_no_partial_file() {
        let dir = temp_dir("fail");
        let db = dir.join("fox.db");
        database_at(&db, Some(80));
        // The backups "directory" is a file: the copy cannot be written.
        let blocked = dir.join("backups");
        std::fs::write(&blocked, b"not a directory").unwrap();
        assert!(backup_before_migration(&db, &blocked, 95, 5, 1).is_err());
    }

    #[test]
    fn opening_a_real_old_database_backs_it_up_once_then_migrates() {
        let dir = temp_dir("real");
        let db = dir.join("fox.db");
        {
            let mut connection = Connection::open(&db).unwrap();
            crate::database::migrations::run_to_v74_for_test(&mut connection, 1).unwrap();
        }
        let backups = dir.join("backups");
        let database = crate::database::Database::open_with_migration_backup(db.clone(), &backups).unwrap();
        assert_eq!(database.schema_version().unwrap(), crate::database::DATABASE_SCHEMA_VERSION);
        drop(database);
        let copies = std::fs::read_dir(&backups).unwrap().map(|entry| entry.unwrap().path()).collect::<Vec<_>>();
        assert_eq!(copies.len(), 1);
        assert!(copies[0].to_string_lossy().ends_with("-v74.db"), "{copies:?}");
        let copy = Connection::open(&copies[0]).unwrap();
        let version: i64 = copy.query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 74, "the copy is the pre-migration database");
        // Current now: reopening makes no further copy.
        drop(crate::database::Database::open_with_migration_backup(db, &backups).unwrap());
        assert_eq!(std::fs::read_dir(&backups).unwrap().count(), 1);
    }

    #[test]
    fn a_failed_backup_stops_the_migration() {
        let dir = temp_dir("stop");
        let db = dir.join("fox.db");
        {
            let mut connection = Connection::open(&db).unwrap();
            crate::database::migrations::run_to_v74_for_test(&mut connection, 1).unwrap();
        }
        let blocked = dir.join("backups");
        std::fs::write(&blocked, b"not a directory").unwrap();
        let error = crate::database::Database::open_with_migration_backup(db.clone(), &blocked).err().expect("refused");
        assert!(error.contains("自动备份失败"), "{error}");
        let version: i64 = Connection::open(&db).unwrap()
            .query_row("SELECT MAX(version) FROM schema_migrations", [], |row| row.get(0)).unwrap();
        assert_eq!(version, 74, "not migrated");
    }
}
